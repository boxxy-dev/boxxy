/// Exhaustive allowlist of default built-in Boxxy tools (strictly sorted alphabetically).
///
/// Any tool not in this list (e.g. user-configured MCP tools) is grouped under
/// [`EXTERNAL_MCP_TOOL_NAME`] so private MCP server names never leak into telemetry.
pub const BUILTIN_BOXXY_TOOLS: &[&str] = &[
    "abort_agent_task",
    "acquire_lock",
    "activate_skill",
    "await_tasks",
    "cancel_task",
    "close_agent",
    "delegate_task",
    "delegate_task_async",
    "file_delete",
    "file_read",
    "file_write",
    "generate_image",
    "get_clipboard",
    "get_system_info",
    "glob_search",
    "grep_search",
    "http_fetch",
    "invite_character",
    "kill_process",
    "list_active_agents",
    "list_directory",
    "list_my_tasks",
    "list_processes",
    "memory_delete",
    "memory_store",
    "native_web_search",
    "orchestrate_agent",
    "publish_custom_event",
    "read_pane_buffer",
    "read_scrollback_page",
    "release_lock",
    "replace_in_file",
    "request_assistance",
    "restore_background_shell",
    "run_background_command",
    "schedule_task",
    "send_command_to_pane",
    "send_keystrokes_to_pane",
    "set_agent_state",
    "set_clipboard",
    "set_global_intent",
    "spawn_agent",
    "subscribe_to_pane",
    "summon_headless_worker",
    "sys_shell_exec",
    "terminal_exec",
    "web_search",
];

/// Canonical display names of all 15 built-in Boxxy model providers.
pub const BUILTIN_PROVIDERS: &[&str] = &[
    "ChatGPT",
    "Gemini",
    "Ollama",
    "Anthropic",
    "OpenAI",
    "OpenRouter",
    "DeepSeek",
    "Moonshot",
    "xAI",
    "Z.ai",
    "Mistral",
    "MiniMax",
    "Groq",
    "OpenCode Zen",
    "OpenCode Go",
];

pub const EXTERNAL_MCP_TOOL_NAME: &str = "external_mcp_tool";
pub const CUSTOM_ENDPOINT_PROVIDER_NAME: &str = "Custom Endpoints";
/// Privacy placeholder used in place of local `Ollama` or user-defined `Custom Endpoints` model IDs.
pub const REDACTED_MODEL_NAME: &str = "redacted";

const MAX_ATTR_LEN: usize = 128;

pub(crate) fn sanitize_attr_text(raw: &str) -> String {
    raw.chars()
        .filter(|c| !matches!(c, '<' | '>' | '\\' | '\0' | '\r' | '\n'))
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_ATTR_LEN)
        .collect()
}

/// Normalizes a tool name to either a known built-in Boxxy tool or `"external_mcp_tool"`.
#[must_use]
pub fn normalize_tool_name(tool_name: &str) -> &'static str {
    let trimmed = tool_name.trim();
    BUILTIN_BOXXY_TOOLS
        .binary_search(&trimmed)
        .map_or(EXTERNAL_MCP_TOOL_NAME, |idx| BUILTIN_BOXXY_TOOLS[idx])
}

/// Normalizes a model provider name to one of the 15 built-in providers or `"Custom Endpoints"`,
/// ensuring user-defined custom endpoint names never leak into telemetry.
#[must_use]
pub fn normalize_provider_name(provider: &str) -> &'static str {
    match provider.trim().to_ascii_lowercase().as_str() {
        "chatgpt" | "chatgpt (codex)" | "codex" => "ChatGPT",
        "gemini" | "google" => "Gemini",
        "ollama" => "Ollama",
        "anthropic" => "Anthropic",
        "openai" => "OpenAI",
        "openrouter" => "OpenRouter",
        "deepseek" => "DeepSeek",
        "moonshot" => "Moonshot",
        "xai" => "xAI",
        "z.ai" | "zai" => "Z.ai",
        "mistral" => "Mistral",
        "minimax" => "MiniMax",
        "groq" => "Groq",
        "opencode zen" | "opencode-zen" | "opencode_zen" | "opencode" => "OpenCode Zen",
        "opencode go" | "opencode-go" | "opencode_go" => "OpenCode Go",
        _ => CUSTOM_ENDPOINT_PROVIDER_NAME,
    }
}

/// Enforces client-side model-name privacy:
/// - Redacts model names for `"Ollama"` and `"Custom Endpoints"` so private local tags or
///   fine-tuned model paths never leave the user's machine.
/// - Sanitizes and length-bounds public catalog model IDs.
#[must_use]
pub fn normalize_model_name(normalized_provider: &str, raw_model: &str) -> String {
    if normalized_provider == "Ollama" || normalized_provider == CUSTOM_ENDPOINT_PROVIDER_NAME {
        return REDACTED_MODEL_NAME.to_string();
    }
    let clean = sanitize_attr_text(raw_model);
    if clean.is_empty() {
        "unknown".to_string()
    } else {
        clean
    }
}

/// Normalizes an OS name string from `/etc/os-release` or `sysinfo`, stripping quotes
/// and unifying known distro aliases.
#[must_use]
pub fn normalize_os(raw: &str) -> String {
    let cleaned = sanitize_attr_text(raw.trim().trim_matches(['"', '\'']));
    if cleaned.is_empty() || cleaned.eq_ignore_ascii_case("gnome") {
        "Unknown".to_string()
    } else if cleaned.eq_ignore_ascii_case("cachyos") {
        "CachyOS Linux".to_string()
    } else {
        cleaned
    }
}

/// Normalizes a shell path (e.g. `/usr/bin/zsh`, `/run/current-system/sw/bin/fish`) to its binary basename.
#[must_use]
pub fn normalize_shell(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(['"', '\'']).trim();
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed).trim();
    let clean = sanitize_attr_text(base);
    if clean.is_empty() {
        "unknown".to_string()
    } else {
        clean
    }
}

/// Normalizes `XDG_CURRENT_DESKTOP` (e.g. `ubuntu:GNOME`, `zorin:GNOME`, `X-Cinnamon`, `sway:wlroots`).
#[must_use]
pub fn normalize_desktop(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches(['"', '\'']).trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") {
        return "unknown".to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("gnome") {
        "GNOME".to_string()
    } else if lower.contains("kde") || lower.contains("plasma") {
        "KDE".to_string()
    } else if lower.contains("cosmic") {
        "COSMIC".to_string()
    } else if lower.contains("hyprland") {
        "Hyprland".to_string()
    } else if lower.contains("cinnamon") {
        "Cinnamon".to_string()
    } else if lower.contains("sway") {
        "Sway".to_string()
    } else if lower.contains("xfce") {
        "XFCE".to_string()
    } else if lower.contains("mate") {
        "MATE".to_string()
    } else if lower.contains("lxqt") {
        "LXQt".to_string()
    } else if lower.contains("budgie") {
        "Budgie".to_string()
    } else if lower.contains("niri") {
        "Niri".to_string()
    } else if lower.contains("labwc") {
        "Labwc".to_string()
    } else if lower.contains("river") {
        "River".to_string()
    } else if lower == "i3" || lower.starts_with("i3:") || lower.ends_with(":i3") {
        "i3".to_string()
    } else {
        let primary = trimmed
            .split(':')
            .find(|s| !s.trim().is_empty())
            .unwrap_or(trimmed);
        let clean = sanitize_attr_text(primary);
        if clean.is_empty() {
            "unknown".to_string()
        } else {
            clean
        }
    }
}

/// Normalizes installation package type strictly into `"flatpak"`, `"distro"`, or `"native"`.
#[must_use]
pub fn normalize_pkg_type(raw: &str) -> &'static str {
    match raw
        .trim()
        .trim_matches(['"', '\''])
        .to_ascii_lowercase()
        .as_str()
    {
        "flatpak" => "flatpak",
        "distro" | "distro packaged" | "distro-packaged" | "distro_packaged" | "system"
        | "system-package" | "nix" | "aur" | "rpm" | "deb" => "distro",
        _ => "native",
    }
}

/// Resolves the active app theme mode and ID into one of the 4 privacy-safe telemetry buckets:
/// `"Boxxy Dark"`, `"Boxxy Light"`, `"Boxxy Community"`, or `"Adwaita"`.
#[must_use]
pub fn resolve_theme_telemetry(
    mode: boxxy_preferences::AppThemeMode,
    theme_id: &str,
) -> &'static str {
    match mode {
        boxxy_preferences::AppThemeMode::DefaultGtk => "Adwaita",
        boxxy_preferences::AppThemeMode::Boxxy => {
            let trimmed = theme_id.trim();
            if trimmed.is_empty()
                || trimmed.eq_ignore_ascii_case("boxxy-dark")
                || trimmed.eq_ignore_ascii_case("boxxy dark")
            {
                "Boxxy Dark"
            } else if trimmed.eq_ignore_ascii_case("boxxy-light")
                || trimmed.eq_ignore_ascii_case("boxxy light")
            {
                "Boxxy Light"
            } else {
                "Boxxy Community"
            }
        }
    }
}

/// Normalizes a raw theme attribute string into one of the 4 allowed telemetry buckets.
///
/// Buckets are `"Boxxy Dark"`, `"Boxxy Light"`, `"Boxxy Community"`, and `"Adwaita"`,
/// ensuring user-created `.toml` theme names never leak into telemetry.
#[must_use]
pub fn normalize_theme(raw: &str) -> &'static str {
    let cleaned = raw.trim().trim_matches(['"', '\'']).trim();
    if cleaned.is_empty() {
        return "Boxxy Dark";
    }
    match cleaned.to_ascii_lowercase().as_str() {
        "boxxy dark" | "boxxy-dark" | "boxxy_dark" => "Boxxy Dark",
        "boxxy light" | "boxxy-light" | "boxxy_light" => "Boxxy Light",
        "adwaita" | "legacy adwaita" | "default_gtk" | "default-gtk" | "system follow"
        | "system dark" | "system light" => "Adwaita",
        _ => "Boxxy Community",
    }
}
