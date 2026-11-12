use crate::actions::ActionContext;
use crate::actions::version::full_version_str;
use crate::output::{OutputFormat, print_json};
use anyhow::Result;
use boxxy_agent::cli_ops;
use boxxy_claw_protocol::characters::CharacterConfig;
use boxxy_mcp::config::{McpServerConfig, McpTransport};
use boxxy_preferences::Settings;
use boxxy_preferences::config::ModelConfigState;
use boxxy_sys_utils::env::PROVIDER_ENV_KEY_MAP;
use boxxy_themes::app_theme::community::parse_community_theme_toml;
use clap::Args;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Args, Debug, Clone, Default)]
#[command(about = "Validate config files, themes, characters, MCP servers, and daemon health")]
pub struct DoctorArgs {
    /// Emit diagnostic report as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckLevel {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticCheck {
    pub id: &'static str,
    pub label: &'static str,
    pub level: CheckLevel,
    pub summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub version: &'static str,
    pub instance_mode: String,
    pub gtk_app_id: &'static str,
    pub agent_bus_name: &'static str,
    pub config_dir: PathBuf,
    pub database_path: PathBuf,
    pub checks: Vec<DiagnosticCheck>,
    pub ok: bool,
}

pub fn run(ctx: &ActionContext, args: &DoctorArgs) -> Result<ExitCode> {
    ctx.apply_dev_overrides();

    let config_dir = boxxy_sys_utils::get_config_dir();
    let db_path = config_dir.join("boxxyclaw").join("boxxy.db");
    let instance_mode = match (
        boxxy_sys_utils::instance_id(),
        boxxy_sys_utils::is_ephemeral_mode(),
    ) {
        (None, _) => "stable".to_string(),
        (Some(slug), true) => format!("ephemeral ({slug})"),
        (Some(slug), false) => format!("dev ({slug})"),
    };

    let mut checks = Vec::with_capacity(8);

    // 1. Config directory & SQLite database check
    checks.push(check_paths(&config_dir, &db_path));

    // 2. settings.json strict JSON syntax + schema check
    let (settings_check, loaded_settings) = check_settings_file(&config_dir);
    checks.push(settings_check);
    let mut settings = loaded_settings.unwrap_or_default();

    // Merge host environment API keys into our local copy so model readiness matches GUI launch
    apply_local_env_keys(&mut settings);

    // 3. Community themes (.toml) validation
    checks.push(check_community_themes(&config_dir.join("themes")));

    // 4. Characters catalog (CHARACTER.toml) validation
    checks.push(check_characters(
        &config_dir.join("boxxyclaw").join("characters"),
    ));

    // 5. MCP servers configuration & command/URL sanity check
    checks.push(check_mcp_servers(&settings.mcp_servers));

    // 6. AI Models & API Key readiness
    checks.push(check_models(&settings));

    // 7. Shell integration check
    checks.push(check_shell_integration());

    // 8. Host daemon (`boxxy-agent`) D-Bus & binary check
    checks.push(check_agent_daemon(ctx, &config_dir));

    let has_failures = checks.iter().any(|c| c.level == CheckLevel::Fail);
    let report = DoctorReport {
        version: full_version_str(),
        instance_mode,
        gtk_app_id: boxxy_sys_utils::gtk_app_id(),
        agent_bus_name: boxxy_sys_utils::agent_bus_name(),
        config_dir,
        database_path: db_path,
        checks,
        ok: !has_failures,
    };

    match ctx.format(args.json) {
        OutputFormat::Json => print_json(&report)?,
        OutputFormat::Human => render_human_report(ctx, &report),
    }

    Ok(if report.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn check_paths(config_dir: &Path, db_path: &Path) -> DiagnosticCheck {
    let mut details = Vec::new();
    if config_dir.exists() {
        if !config_dir.is_dir() {
            return DiagnosticCheck {
                id: "paths",
                label: "Paths & Storage",
                level: CheckLevel::Fail,
                summary: format!(
                    "Config path '{}' exists but is not a directory.",
                    config_dir.display()
                ),
                details,
            };
        }
        let db_state = if db_path.is_file() {
            "SQLite database present"
        } else {
            "SQLite database not created yet (created on first Claw session)"
        };
        DiagnosticCheck {
            id: "paths",
            label: "Paths & Storage",
            level: CheckLevel::Ok,
            summary: format!("{} ({db_state})", config_dir.display()),
            details,
        }
    } else {
        details.push(format!(
            "Directory '{}' will be initialized on first launch.",
            config_dir.display()
        ));
        DiagnosticCheck {
            id: "paths",
            label: "Paths & Storage",
            level: CheckLevel::Ok,
            summary: format!("{} (clean / not yet created)", config_dir.display()),
            details,
        }
    }
}

fn check_settings_file(config_dir: &Path) -> (DiagnosticCheck, Option<Settings>) {
    let path = config_dir.join("settings.json");
    if !path.exists() {
        return (
            DiagnosticCheck {
                id: "settings",
                label: "Settings File",
                level: CheckLevel::Ok,
                summary: "Using defaults (settings.json not yet written)".to_string(),
                details: Vec::new(),
            },
            None,
        );
    }

    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            return (
                DiagnosticCheck {
                    id: "settings",
                    label: "Settings File",
                    level: CheckLevel::Fail,
                    summary: format!("Failed to read {}: {e}", path.display()),
                    details: Vec::new(),
                },
                None,
            );
        }
    };

    match serde_json::from_str::<Settings>(&raw) {
        Ok(s) => (
            DiagnosticCheck {
                id: "settings",
                label: "Settings File",
                level: CheckLevel::Ok,
                summary: format!(
                    "Valid JSON (app theme: '{}', terminal palette: '{}')",
                    s.app_theme_id, s.theme
                ),
                details: Vec::new(),
            },
            Some(s),
        ),
        Err(e) => (
            DiagnosticCheck {
                id: "settings",
                label: "Settings File",
                level: CheckLevel::Fail,
                summary: format!("Malformed JSON in {}: {e}", path.display()),
                details: Vec::new(),
            },
            None,
        ),
    }
}

fn check_community_themes(themes_dir: &Path) -> DiagnosticCheck {
    if !themes_dir.is_dir() {
        return DiagnosticCheck {
            id: "themes",
            label: "Community Themes",
            level: CheckLevel::Ok,
            summary: "No custom themes directory yet (seeded on launch)".to_string(),
            details: Vec::new(),
        };
    }

    let mut valid_count = 0usize;
    let mut errors = Vec::new();

    if let Ok(entries) = std::fs::read_dir(themes_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let slug = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            match std::fs::read_to_string(&p) {
                Ok(content) => {
                    if let Err(toml_err) = toml::from_str::<toml::Value>(&content) {
                        errors.push(format!("{}: TOML syntax error ({toml_err})", p.display()));
                    } else if parse_community_theme_toml(&slug, &content).is_some() {
                        valid_count += 1;
                    } else {
                        errors.push(format!(
                            "{}: missing required hex color fields or invalid #RRGGBB values",
                            p.display()
                        ));
                    }
                }
                Err(e) => errors.push(format!("{}: read error ({e})", p.display())),
            }
        }
    }

    if errors.is_empty() {
        DiagnosticCheck {
            id: "themes",
            label: "Community Themes",
            level: CheckLevel::Ok,
            summary: format!("{valid_count} valid .toml theme(s)"),
            details: Vec::new(),
        }
    } else {
        DiagnosticCheck {
            id: "themes",
            label: "Community Themes",
            level: CheckLevel::Fail,
            summary: format!(
                "{} invalid theme file(s), {valid_count} valid",
                errors.len()
            ),
            details: errors,
        }
    }
}

fn check_characters(chars_dir: &Path) -> DiagnosticCheck {
    if !chars_dir.is_dir() {
        return DiagnosticCheck {
            id: "characters",
            label: "Claw Characters",
            level: CheckLevel::Ok,
            summary: "Bundled defaults will be extracted on first session".to_string(),
            details: Vec::new(),
        };
    }

    let mut valid_count = 0usize;
    let mut errors = Vec::new();

    if let Ok(entries) = std::fs::read_dir(chars_dir) {
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let toml_path = dir.join("CHARACTER.toml");
            if !toml_path.exists() {
                continue;
            }
            match std::fs::read_to_string(&toml_path) {
                Ok(raw) => match toml::from_str::<CharacterConfig>(&raw) {
                    Ok(_) => valid_count += 1,
                    Err(e) => errors.push(format!("{}: {e}", toml_path.display())),
                },
                Err(e) => errors.push(format!("{}: read error ({e})", toml_path.display())),
            }
        }
    }

    if errors.is_empty() {
        DiagnosticCheck {
            id: "characters",
            label: "Claw Characters",
            level: CheckLevel::Ok,
            summary: format!("{valid_count} valid CHARACTER.toml profile(s)"),
            details: Vec::new(),
        }
    } else {
        DiagnosticCheck {
            id: "characters",
            label: "Claw Characters",
            level: CheckLevel::Fail,
            summary: format!(
                "{} broken CHARACTER.toml file(s), {valid_count} valid",
                errors.len()
            ),
            details: errors,
        }
    }
}

pub(crate) fn check_mcp_servers(servers: &[McpServerConfig]) -> DiagnosticCheck {
    if servers.is_empty() {
        return DiagnosticCheck {
            id: "mcp",
            label: "MCP Servers",
            level: CheckLevel::Ok,
            summary: "None configured".to_string(),
            details: Vec::new(),
        };
    }

    let enabled_count = servers.iter().filter(|s| s.enabled).count();
    let disabled_count = servers.len() - enabled_count;
    let mut details = Vec::new();
    let mut has_error = false;
    let mut has_warning = false;

    for srv in servers {
        let name = if srv.name.trim().is_empty() {
            "<unnamed>"
        } else {
            srv.name.trim()
        };
        let state_tag = if srv.enabled { "enabled" } else { "disabled" };

        if srv.name.trim().is_empty() {
            has_error = true;
            details.push("MCP entry has an empty server name.".to_string());
        }

        match &srv.transport {
            McpTransport::Stdio { command, args, .. } => {
                let cmd_trimmed = command.trim();
                if cmd_trimmed.is_empty() {
                    if srv.enabled {
                        has_error = true;
                    } else {
                        has_warning = true;
                    }
                    details.push(format!(
                        "[{state_tag}] '{name}' (stdio): command is empty"
                    ));
                } else if resolve_executable_in_path(cmd_trimmed).is_none() {
                    if srv.enabled {
                        has_error = true;
                    } else {
                        has_warning = true;
                    }
                    details.push(format!(
                        "[{state_tag}] '{name}' (stdio): executable '{cmd_trimmed}' not found in PATH"
                    ));
                } else {
                    details.push(format!(
                        "[{state_tag}] '{name}' (stdio): {} ({} arg(s))",
                        cmd_trimmed,
                        args.len()
                    ));
                }
            }
            McpTransport::Http {
                url, streamable, ..
            } => {
                let mode = if *streamable { "streamable-http" } else { "http" };
                let trimmed = url.trim();
                match reqwest::Url::parse(trimmed) {
                    Ok(parsed)
                        if (parsed.scheme() == "http" || parsed.scheme() == "https")
                            && parsed.host_str().is_some() =>
                    {
                        details.push(format!(
                            "[{state_tag}] '{name}' ({mode}): {}://{}",
                            parsed.scheme(),
                            parsed.host_str().unwrap_or_default()
                        ));
                    }
                    _ => {
                        if srv.enabled {
                            has_error = true;
                        } else {
                            has_warning = true;
                        }
                        details.push(format!(
                            "[{state_tag}] '{name}' ({mode}): invalid HTTP(S) URL '{trimmed}'"
                        ));
                    }
                }
            }
        }
    }

    let level = if has_error {
        CheckLevel::Fail
    } else if has_warning {
        CheckLevel::Warn
    } else {
        CheckLevel::Ok
    };

    DiagnosticCheck {
        id: "mcp",
        label: "MCP Servers",
        level,
        summary: format!(
            "{} configured ({enabled_count} enabled, {disabled_count} disabled)",
            servers.len()
        ),
        details,
    }
}

fn check_models(settings: &Settings) -> DiagnosticCheck {
    let claw_state = settings.resolve_model_state(&settings.claw_model);
    let mem_state = settings.resolve_model_state(&settings.memory_model);

    let describe = |slot: &str, state: &ModelConfigState| -> (CheckLevel, String) {
        match state {
            ModelConfigState::Ready(mp) => {
                (CheckLevel::Ok, format!("{slot}: Ready ({mp:?})"))
            }
            ModelConfigState::Unconfigured => {
                (CheckLevel::Ok, format!("{slot}: Not configured"))
            }
            ModelConfigState::MissingApiKey { provider_name } => (
                CheckLevel::Warn,
                format!("{slot}: {provider_name} selected, but API key is missing"),
            ),
            ModelConfigState::OllamaOffline => (
                CheckLevel::Warn,
                format!("{slot}: Ollama selected, service currently offline"),
            ),
        }
    };

    let (claw_lvl, claw_msg) = describe("Claw", &claw_state);
    let (mem_lvl, mem_msg) = describe("Dreams", &mem_state);
    let level = if claw_lvl == CheckLevel::Warn || mem_lvl == CheckLevel::Warn {
        CheckLevel::Warn
    } else {
        CheckLevel::Ok
    };

    DiagnosticCheck {
        id: "models",
        label: "AI Models",
        level,
        summary: format!("{claw_msg}; {mem_msg}"),
        details: Vec::new(),
    }
}

fn check_shell_integration() -> DiagnosticCheck {
    let shell_raw = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let shell_name = Path::new(&shell_raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let supported = matches!(shell_name, "bash" | "zsh" | "fish" | "nu");
    let enabled = boxxy_sys_utils::env::shell_integration_enabled();

    if !enabled {
        DiagnosticCheck {
            id: "shell",
            label: "Shell Integration",
            level: CheckLevel::Ok,
            summary: format!("Opted out via BOXXY_SHELL_INTEGRATION=0 (SHELL={shell_raw})"),
            details: Vec::new(),
        }
    } else if supported {
        DiagnosticCheck {
            id: "shell",
            label: "Shell Integration",
            level: CheckLevel::Ok,
            summary: format!("Enabled for '{shell_name}' ({shell_raw})"),
            details: Vec::new(),
        }
    } else {
        DiagnosticCheck {
            id: "shell",
            label: "Shell Integration",
            level: CheckLevel::Warn,
            summary: format!(
                "SHELL='{shell_raw}' does not have built-in OSC 133 auto-shims (supported: bash, zsh, fish, nu)"
            ),
            details: Vec::new(),
        }
    }
}

fn check_agent_daemon(ctx: &ActionContext, expected_config_dir: &Path) -> DiagnosticCheck {
    let bus_name = boxxy_sys_utils::agent_bus_name();
    match ctx.run_async(cli_ops::query_status()) {
        Ok(status) => {
            let expected_ver = full_version_str();
            let mut details = vec![format!(
                "Power: {}, Dream Cycle: {}, Active Claw Panes: {}, Detached Shells: {}",
                status.power_source,
                status.dream_cycle,
                status.active_claw_panes,
                status.detached_shells
            )];

            let mut level = CheckLevel::Ok;
            if status.version != expected_ver {
                level = CheckLevel::Warn;
                details.push(format!(
                    "Version mismatch: daemon is '{}', CLI is '{expected_ver}' (will auto-upgrade on next window launch or `+agent restart`).",
                    status.version
                ));
            }
            if Path::new(&status.config_dir) != expected_config_dir {
                level = CheckLevel::Warn;
                details.push(format!(
                    "Config dir mismatch: daemon uses '{}', expected '{}'.",
                    status.config_dir,
                    expected_config_dir.display()
                ));
            }

            DiagnosticCheck {
                id: "daemon",
                label: "Host Daemon",
                level,
                summary: format!("Running v{} on {bus_name}", status.version),
                details,
            }
        }
        Err(_) => {
            let expected_bin = expected_agent_binary_path();
            if let Some(ref bin_path) = expected_bin
                && bin_path.is_file()
            {
                DiagnosticCheck {
                    id: "daemon",
                    label: "Host Daemon",
                    level: CheckLevel::Ok,
                    summary: format!(
                        "Idle / not running on {bus_name} (binary ready at {})",
                        bin_path.display()
                    ),
                    details: Vec::new(),
                }
            } else if boxxy_sys_utils::is_flatpak() {
                DiagnosticCheck {
                    id: "daemon",
                    label: "Host Daemon",
                    level: CheckLevel::Ok,
                    summary: format!(
                        "Not yet deployed on host ({bus_name}; deployed automatically on first window launch)"
                    ),
                    details: Vec::new(),
                }
            } else {
                DiagnosticCheck {
                    id: "daemon",
                    label: "Host Daemon",
                    level: CheckLevel::Warn,
                    summary: format!(
                        "Not running on {bus_name} and sibling `boxxy-agent` binary was not found alongside current executable"
                    ),
                    details: Vec::new(),
                }
            }
        }
    }
}

fn expected_agent_binary_path() -> Option<PathBuf> {
    if boxxy_sys_utils::is_flatpak() {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join(".local/boxxy-terminal/boxxy-agent"))
    } else {
        let mut p = std::env::current_exe().ok()?;
        p.pop();
        p.push("boxxy-agent");
        Some(p)
    }
}

fn resolve_executable_in_path(cmd: &str) -> Option<PathBuf> {
    let candidate = Path::new(cmd);
    if candidate.components().count() > 1 {
        return is_executable_file(candidate).then(|| candidate.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let full = dir.join(cmd);
        if is_executable_file(&full) {
            return Some(full);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn apply_local_env_keys(settings: &mut Settings) {
    for &(env_var, provider) in PROVIDER_ENV_KEY_MAP {
        if let Ok(val) = std::env::var(env_var)
            && !val.trim().is_empty()
        {
            settings
                .api_keys
                .insert(provider.to_string(), val.trim().to_string());
        }
    }
}

fn render_human_report(ctx: &ActionContext, report: &DoctorReport) {
    println!(
        "{} {}  {}",
        ctx.style.bold("Boxxy Doctor"),
        ctx.style.dim(&format!("({})", report.version)),
        ctx.style
            .cyan(&format!("[instance: {}]", report.instance_mode))
    );
    println!(
        "  {} {}   {} {}",
        ctx.style.dim("GTK App ID:"),
        report.gtk_app_id,
        ctx.style.dim("D-Bus:"),
        report.agent_bus_name
    );
    println!();

    for check in &report.checks {
        let badge = match check.level {
            CheckLevel::Ok => ctx.style.green("[OK]  "),
            CheckLevel::Warn => ctx.style.yellow("[WARN]"),
            CheckLevel::Fail => ctx.style.red("[FAIL]"),
        };
        println!(
            "  {} {:<19} {}",
            badge,
            ctx.style.bold(check.label),
            check.summary
        );
        for detail in &check.details {
            println!("         {}", ctx.style.dim(&format!("• {detail}")));
        }
    }

    println!();
    if report.ok {
        println!("{}", ctx.style.green("All critical checks passed."));
    } else {
        println!(
            "{}",
            ctx.style
                .red("One or more checks failed. Fix the reported issues above.")
        );
    }
}
