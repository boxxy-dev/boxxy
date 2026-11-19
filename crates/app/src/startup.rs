//! Primary GUI instance initialization (`connect_startup`), `GResource` registration,
//! environment API-key hydration, and non-blocking background telemetry/audio prewarm.

use gtk4::gio;
use std::path::Path;
use std::time::Duration;
use tokio::task::JoinSet;

/// Deduplicated environment variable lookup target mapping a single `ENV_VAR` name
/// (e.g. `OPENCODE_API_KEY` or a custom endpoint's `api_key_env`) to one or more
/// logical provider names in `Settings`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EnvKeySpec {
    env_var: String,
    providers: Vec<String>,
}

/// Runs strictly once on the primary GUI instance (never on secondary `--tab` / `--present`
/// CLI forwarders), avoiding duplicate config seeding, daemon claims, `GStreamer` init,
/// and `app.launch` telemetry.
pub(crate) fn on_startup() {
    boxxy_sys_utils::prepare_dev_environment();

    register_resources();
    gtk4::Window::set_default_icon_name(boxxy_sys_utils::STABLE_GTK_APP_ID);
    crate::wayland_guard::install_for_default_display();

    boxxy_preferences::Settings::init();
    boxxy_preferences::AppState::init();

    // Hydrate local environment API keys synchronously on Native builds (<50µs, zero disk/IPC)
    // *before* `AppWindow::new()` runs so Frame-1 model validation never fires a false
    // "missing API key" toast when keys are supplied via shell environment variables.
    let is_flatpak = boxxy_sys_utils::is_flatpak();
    let missing_host_specs = hydrate_local_env_keys(is_flatpak);

    // Offload synchronous filesystem bookmark & skill seeding from Tokio async workers.
    tokio::task::spawn_blocking(|| {
        boxxy_bookmarks::manager::BookmarksManager::init();
        boxxy_preferences::Settings::ensure_claw_skills();
    });

    boxxy_terminal::register_agent_ensure_hook(|| async {
        // Heap-pin `ensure_agent_running` (~17 KiB state machine due to chunked binary diffing)
        // to keep per-task stack frames compact.
        if let Err(e) = Box::pin(boxxy_window::agent_deployer::ensure_agent_running()).await {
            log::error!("Failed to ensure agent is running: {e:#}");
        }
    });

    tokio::spawn(async move {
        run_post_startup_tasks(is_flatpak, missing_host_specs).await;
    });
}

pub(crate) fn register_resources() {
    gio::resources_register_include!("compiled.gresource")
        .expect("Failed to register compiled GResources.");
}

fn collect_env_key_specs(custom_endpoints: &[(Option<&str>, &str)]) -> Vec<EnvKeySpec> {
    let mut specs: Vec<EnvKeySpec> = Vec::with_capacity(
        boxxy_sys_utils::env::PROVIDER_ENV_KEY_MAP.len() + custom_endpoints.len(),
    );

    let mut push_mapping = |raw_env: &str, raw_provider: &str| {
        let env_var = raw_env.trim();
        let provider = raw_provider.trim();
        if env_var.is_empty() || provider.is_empty() {
            return;
        }
        if let Some(existing) = specs.iter_mut().find(|s| s.env_var == env_var) {
            if !existing.providers.iter().any(|p| p == provider) {
                existing.providers.push(provider.to_string());
            }
        } else {
            specs.push(EnvKeySpec {
                env_var: env_var.to_string(),
                providers: vec![provider.to_string()],
            });
        }
    };

    for &(env_var, provider) in boxxy_sys_utils::env::PROVIDER_ENV_KEY_MAP {
        push_mapping(env_var, provider);
    }

    for &(api_key_env, name) in custom_endpoints {
        if let Some(env_var) = api_key_env {
            push_mapping(env_var, name);
        }
    }

    specs
}

/// Records a resolved environment API key under both each mapped provider name *and*
/// the raw `env_var` identifier (so custom endpoint `/models` discovery in Preferences
/// can look up `Settings::get_env_api_key(&env_var)` inside Flatpak sandboxes).
fn store_resolved_env_key(spec: &EnvKeySpec, key: &str) {
    boxxy_preferences::Settings::set_env_api_key(&spec.env_var, key.to_string());
    for provider in &spec.providers {
        boxxy_preferences::Settings::set_env_api_key(provider, key.to_string());
    }
}

fn hydrate_local_env_keys(is_flatpak: bool) -> Vec<EnvKeySpec> {
    let settings = boxxy_preferences::Settings::load();
    let custom_pairs: Vec<(Option<&str>, &str)> = settings
        .openai_compatible_endpoints
        .iter()
        .map(|ep| (ep.api_key_env.as_deref(), ep.name.as_str()))
        .collect();
    let all_specs = collect_env_key_specs(&custom_pairs);

    if is_flatpak {
        // Inside Flatpak, host environment variables live on `boxxy-agent` rather than
        // the sandboxed UI process.
        return all_specs;
    }

    let mut missing = Vec::new();
    for spec in all_specs {
        let local_val = std::env::var(&spec.env_var)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        if let Some(key) = local_val {
            store_resolved_env_key(&spec, &key);
        } else {
            missing.push(spec);
        }
    }
    missing
}

async fn run_post_startup_tasks(is_flatpak: bool, missing_host_specs: Vec<EnvKeySpec>) {
    let agent = boxxy_terminal::get_agent().await;

    // Query any remaining unset environment keys from the host daemon concurrently
    // instead of issuing 14+ sequential D-Bus round-trips.
    if !missing_host_specs.is_empty() {
        let mut join_set = JoinSet::new();
        for spec in missing_host_specs {
            let proxy = agent.proxy().clone();
            join_set.spawn(async move {
                let val = proxy
                    .get_environment_variable(spec.env_var.clone())
                    .await
                    .ok()
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty());
                (spec, val)
            });
        }

        let mut found_host_key = false;
        while let Some(res) = join_set.join_next().await {
            if let Ok((spec, Some(key))) = res {
                store_resolved_env_key(&spec, &key);
                found_host_key = true;
            }
        }

        if found_host_key {
            boxxy_preferences::Settings::notify_changed();
        }
    }

    let settings = boxxy_preferences::Settings::load();
    if let Err(e) = agent
        .update_credentials(
            settings.get_effective_api_keys(),
            settings.ollama_base_url.clone(),
        )
        .await
    {
        log::warn!("Failed to sync initial credentials to boxxy-agent: {e}");
    }

    // Defer GStreamer plugin initialization until after Frame 1 settles, and run it
    // on `spawn_blocking` so cold-cache shared-library scanning never stalls Tokio I/O.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = tokio::task::spawn_blocking(boxxy_window::sound::ensure_gstreamer_init).await;

    if !should_record_launch_telemetry(&settings) {
        return;
    }

    let ((), host_meta, preferred_shell) = tokio::join!(
        boxxy_telemetry::init_db(),
        tokio::task::spawn_blocking(move || {
            let os = detect_os_name(is_flatpak);
            let pkg_type = boxxy_sys_utils::detect_package_type();
            (os, pkg_type)
        }),
        agent.get_preferred_shell(),
    );

    let (os, pkg_type) = host_meta.unwrap_or_else(|_| ("Unknown".to_string(), "native"));
    let shell = preferred_shell
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string()));
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".to_string());
    let theme = {
        let s = boxxy_preferences::Settings::load();
        boxxy_telemetry::resolve_theme_telemetry(s.app_theme_mode, &s.app_theme_id)
    };

    boxxy_telemetry::track_launch(
        &os,
        std::env::consts::ARCH,
        pkg_type,
        env!("CARGO_PKG_VERSION"),
        &shell,
        &desktop,
        theme,
    )
    .await;
}

fn should_record_launch_telemetry(settings: &boxxy_preferences::Settings) -> bool {
    if !settings.enable_telemetry {
        return false;
    }
    if boxxy_sys_utils::is_dev_instance() && boxxy_sys_utils::env::telemetry_endpoint().is_none() {
        return false;
    }
    true
}

fn detect_os_name(is_flatpak: bool) -> String {
    let candidates: &[&str] = if is_flatpak {
        &[
            "/run/host/os-release",
            "/run/host/etc/os-release",
            "/run/host/usr/lib/os-release",
            "/etc/os-release",
            "/usr/lib/os-release",
        ]
    } else {
        &["/etc/os-release", "/usr/lib/os-release"]
    };

    for path in candidates {
        if let Ok(content) = std::fs::read_to_string(Path::new(path))
            && let Some(name) = parse_os_release_name(&content)
        {
            return name;
        }
    }

    std::env::consts::OS.to_string()
}

fn parse_os_release_name(content: &str) -> Option<String> {
    let mut fallback_id = None;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(val) = line.strip_prefix("NAME=") {
            let cleaned = unquote_os_release_value(val);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        } else if fallback_id.is_none()
            && let Some(val) = line.strip_prefix("ID=")
        {
            let cleaned = unquote_os_release_value(val);
            if !cleaned.is_empty() {
                fallback_id = Some(cleaned);
            }
        }
    }

    fallback_id
}

fn unquote_os_release_value(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    unquoted
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_key_specs_deduplicate_shared_vars_and_append_custom_endpoints() {
        let custom = [
            (Some("LITELLM_API_KEY"), "Gateway Alpha"),
            (Some("  LITELLM_API_KEY "), "Gateway Beta"),
            (Some("   "), "Ignored Blank"),
            (None, "Local Unauthed"),
        ];
        let specs = collect_env_key_specs(&custom);

        let opencode = specs
            .iter()
            .find(|s| s.env_var == "OPENCODE_API_KEY")
            .expect("OPENCODE_API_KEY must exist once");
        assert_eq!(
            opencode.providers,
            vec!["OpenCode Zen".to_string(), "OpenCode Go".to_string()]
        );
        assert_eq!(
            specs
                .iter()
                .filter(|s| s.env_var == "OPENCODE_API_KEY")
                .count(),
            1
        );

        let litellm = specs
            .iter()
            .find(|s| s.env_var == "LITELLM_API_KEY")
            .expect("custom env var must be coalesced");
        assert_eq!(
            litellm.providers,
            vec!["Gateway Alpha".to_string(), "Gateway Beta".to_string()]
        );
    }

    #[test]
    fn os_release_parser_handles_quotes_comments_and_pretty_name_ordering() {
        let sample = "\
# Synthetic OS release fixture
PRETTY_NAME=\"Synthetic Linux 42 (Workstation)\"
NAME=\"Synthetic Linux\"
ID=synthetic
";
        assert_eq!(
            parse_os_release_name(sample).as_deref(),
            Some("Synthetic Linux")
        );

        let single_quoted = "NAME='Minimal Distro'\nID=minimal\n";
        assert_eq!(
            parse_os_release_name(single_quoted).as_deref(),
            Some("Minimal Distro")
        );

        let id_only = "PRETTY_NAME=\"Unnamed\"\nID=arch\n";
        assert_eq!(parse_os_release_name(id_only).as_deref(), Some("arch"));

        assert_eq!(parse_os_release_name("# comment only\n\n"), None);
    }

    #[test]
    fn compiled_gresources_register_and_contain_core_assets() {
        register_resources();

        for required_path in [
            "/dev/boxxy/BoxxyTerminal/style.css",
            "/dev/boxxy/BoxxyTerminal/themes/boxxy_skeleton.css",
            "/dev/boxxy/BoxxyTerminal/icons/dev.boxxy.BoxxyTerminal.png",
            "/dev/boxxy/BoxxyTerminal/icons/boxxy-tab-new-symbolic.svg",
            "/dev/boxxy/BoxxyTerminal/icons/boxxy-boxxyclaw-symbolic.svg",
            "/dev/boxxy/BoxxyTerminal/icons/boxxy-window-new-symbolic.svg",
            "/dev/boxxy/BoxxyTerminal/prompts/claw.md",
            "/dev/boxxy/BoxxyTerminal/ui/preferences.ui",
            "/dev/boxxy/BoxxyTerminal/ui/widgets/avatar_crop_dialog.ui",
            "/dev/boxxy/BoxxyTerminal/sounds/task.wav",
            "/dev/boxxy/BoxxyTerminal/sounds/timer.wav",
        ] {
            let bytes = gio::resources_lookup_data(required_path, gio::ResourceLookupFlags::NONE)
                .unwrap_or_else(|e| panic!("missing GResource '{required_path}': {e}"));
            assert!(
                !bytes.is_empty(),
                "GResource '{required_path}' must not be empty"
            );
        }
    }
}
