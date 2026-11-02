/// Terminal application category classification.
///
/// Decoupled from GTK and tab UI so application classification rules can be
/// tested and extended independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApplicationCategory {
    #[default]
    Idle,
    /// Text editors: `vim`, `vi`, `nvim`, `micro`, `nano`, `hx`, `helix`, `emacs`, `kak`, `msedit`
    Editor,
    /// System and hardware monitors: `btop`, `htop`, `top`, `nvtop`, `iotop`, `btm`, `glances`, `watch`
    Monitor,
    /// Terminal AI coding agents: `opencode`, `claude`, `aider`, `codex`, `goose`
    AiAgent,
    /// Git TUI tools: `lazygit`, `tig`, `gitui`, `gh-dash`
    Git,
    /// Terminal file managers: `yazi`, `ranger`, `nnn`, `lf`, `mc`, `broot`, `vifm`
    FileManager,
    /// Pagers and documentation viewers: `less`, `more`, `man`, `bat`, `glow`, `most`, `info`
    Pager,
    /// Terminal multiplexers: `tmux`, `zellij`, `screen`, `byobu`
    Multiplexer,
    /// Remote shell sessions: `ssh`, `mosh`, `et`, `telnet`
    Remote,
    /// Unrecognized application actively using the alternate screen buffer (`?1049h`)
    GenericTui,
    /// Other running foreground CLI command / build job (`cargo`, `make`, `npm`, `pytest`, etc.)
    RunningCommand,
}

impl ApplicationCategory {
    /// Returns `true` when any foreground application or command is active.
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// Returns the GResource path of the animated `.gpa` icon for this category.
    pub fn gpa_resource_path(self) -> Option<&'static str> {
        match self {
            Self::Idle => None,
            Self::Editor => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-pencil.gpa"),
            Self::AiAgent => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-ai-slop.gpa"),
            Self::FileManager => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-folder-silhouette.gpa"),
            Self::Git => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-git.gpa"),
            Self::Pager => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-text-left.gpa"),
            Self::Monitor => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-monitor.gpa"),
            Self::RunningCommand => {
                Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-celestial-loader.gpa")
            }
            Self::Multiplexer => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-zoom-fit.gpa"),
            Self::Remote => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-folder-remote.gpa"),
            Self::GenericTui => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-3d-box.gpa"),
        }
    }

    /// Human-readable category label for tab hover tooltips.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Self::Idle => None,
            Self::Editor => Some("Text editor"),
            Self::Monitor => Some("System monitor"),
            Self::AiAgent => Some("AI agent"),
            Self::Git => Some("Git interface"),
            Self::FileManager => Some("File manager"),
            Self::Pager => Some("Pager"),
            Self::Multiplexer => Some("Terminal multiplexer"),
            Self::Remote => Some("Remote session"),
            Self::GenericTui => Some("TUI application"),
            Self::RunningCommand => Some("Running command"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TerminalAppState {
    pub foreground_process: Option<String>,
    pub application_category: ApplicationCategory,
    pub alternate_screen_active: bool,
}

/// Tab indicator badge states (`AdwTab`'s `.tab-indicator` slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabIndicatorKind {
    #[default]
    None,
    /// Pending reminder or scheduled task (`icons/boxxy-clock.gpa`)
    Timer,
    /// Active Claw session (`icons/boxxy-boxxyclaw.gpa`)
    Claw,
    /// Unread notification / visual bell (`icons/boxxy-bell.gpa`)
    Bell,
}

impl TabIndicatorKind {
    pub fn is_active(self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn gpa_resource_path(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Timer => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-clock.gpa"),
            Self::Claw => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-boxxyclaw.gpa"),
            Self::Bell => Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-bell.gpa"),
        }
    }

    pub fn fallback_icon_name(self) -> &'static str {
        match self {
            Self::None | Self::Claw => "boxxy-boxxyclaw-symbolic",
            Self::Timer => "boxxy-timer-symbolic",
            Self::Bell => "boxxy-visual-bell-symbolic",
        }
    }
}

/// Classifies the current terminal application state from the foreground process
/// name, remote host detection, and VT alternate-screen mode (`?1049h` / `?1049l`).
///
/// Note: OSC window titles are intentionally NOT used as proof of application
/// identity here, as exited applications may leave stale titles behind.
pub fn detect_application_state(
    foreground_process: Option<&str>,
    remote_host: Option<&str>,
    alternate_screen_active: bool,
) -> TerminalAppState {
    let fg_clean = foreground_process
        .and_then(normalize_executable_name)
        .filter(|name| !is_shell_executable(name) && !is_prompt_helper_executable(name));

    if remote_host.is_some() {
        return TerminalAppState {
            foreground_process: fg_clean,
            application_category: ApplicationCategory::Remote,
            alternate_screen_active,
        };
    }

    if let Some(ref proc_name) = fg_clean {
        let category = classify_executable(proc_name, alternate_screen_active);
        return TerminalAppState {
            foreground_process: fg_clean,
            application_category: category,
            alternate_screen_active,
        };
    }

    TerminalAppState {
        foreground_process: None,
        application_category: ApplicationCategory::Idle,
        alternate_screen_active,
    }
}

/// Matches a normalized executable basename against the known application registry.
pub fn classify_executable(name: &str, alternate_screen_active: bool) -> ApplicationCategory {
    match name.to_ascii_lowercase().as_str() {
        "vim" | "vi" | "nvim" | "vimdiff" | "rview" | "rvim" | "micro" | "nano" | "hx"
        | "helix" | "emacs" | "emacsclient" | "kak" | "msedit" | "edit" | "ne" | "joe" | "jed"
        | "mcedit" | "amp" | "lapce" => ApplicationCategory::Editor,

        "btop" | "htop" | "top" | "atop" | "nvtop" | "iotop" | "iftop" | "nethogs" | "btm"
        | "bottom" | "glances" | "nmon" | "s-tui" | "powertop" | "radeontop" | "intel_gpu_top"
        | "watch" => ApplicationCategory::Monitor,

        "opencode" | "claude" | "aider" | "codex" | "goose" => ApplicationCategory::AiAgent,

        "lazygit" | "tig" | "gitui" | "git-absorb" | "gh-dash" => ApplicationCategory::Git,

        "yazi" | "ranger" | "nnn" | "lf" | "mc" | "broot" | "vifm" | "superfile" | "spf"
        | "joshuto" | "xplr" | "tere" => ApplicationCategory::FileManager,

        "less" | "more" | "man" | "most" | "info" | "pinfo" | "bat" | "glow" | "ov" | "moar" => {
            ApplicationCategory::Pager
        }

        "tmux" | "zellij" | "screen" | "byobu" | "abduco" | "dtach" => {
            ApplicationCategory::Multiplexer
        }

        "ssh" | "mosh" | "mosh-client" | "et" | "telnet" => ApplicationCategory::Remote,

        _ if alternate_screen_active => ApplicationCategory::GenericTui,
        _ => ApplicationCategory::RunningCommand,
    }
}

pub fn normalize_executable_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let token = trimmed.split_whitespace().next()?;
    let base = token.rsplit('/').next().unwrap_or(token);
    let stripped = base.trim_start_matches('-');
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

pub fn is_shell_executable(name: &str) -> bool {
    let Some(norm) = normalize_executable_name(name) else {
        return false;
    };
    matches!(
        norm.to_ascii_lowercase().as_str(),
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "nu"
            | "nushell"
            | "dash"
            | "ksh"
            | "mksh"
            | "tcsh"
            | "csh"
            | "elvish"
            | "xonsh"
            | "oil"
            | "osh"
            | "pwsh"
    )
}

pub fn is_prompt_helper_executable(name: &str) -> bool {
    let Some(norm) = normalize_executable_name(name) else {
        return false;
    };
    matches!(
        norm.to_ascii_lowercase().as_str(),
        "starship" | "gitstatusd" | "gitstatusd-linu" | "direnv"
    )
}

pub fn is_ssh_executable(name: &str) -> bool {
    let Some(norm) = normalize_executable_name(name) else {
        return false;
    };
    matches!(
        norm.to_ascii_lowercase().as_str(),
        "ssh" | "mosh" | "mosh-client" | "et"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_editors_monitors_git_and_file_managers() {
        let vim = detect_application_state(Some("nvim"), None, true);
        assert_eq!(vim.application_category, ApplicationCategory::Editor);
        assert_eq!(
            vim.application_category.gpa_resource_path(),
            Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-pencil.gpa")
        );

        let micro = detect_application_state(Some("micro"), None, true);
        assert_eq!(micro.application_category, ApplicationCategory::Editor);

        let btop = detect_application_state(Some("btop"), None, true);
        assert_eq!(btop.application_category, ApplicationCategory::Monitor);
        assert_eq!(
            btop.application_category.gpa_resource_path(),
            Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-monitor.gpa")
        );

        let lazygit = detect_application_state(Some("lazygit"), None, true);
        assert_eq!(lazygit.application_category, ApplicationCategory::Git);
        assert_eq!(
            lazygit.application_category.gpa_resource_path(),
            Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-git.gpa")
        );

        let yazi = detect_application_state(Some("yazi"), None, true);
        assert_eq!(yazi.application_category, ApplicationCategory::FileManager);
        assert_eq!(
            yazi.application_category.gpa_resource_path(),
            Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-folder-silhouette.gpa")
        );

        let less = detect_application_state(Some("less"), None, true);
        assert_eq!(less.application_category, ApplicationCategory::Pager);
        assert_eq!(
            less.application_category.gpa_resource_path(),
            Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-text-left.gpa")
        );

        let tmux = detect_application_state(Some("tmux"), None, true);
        assert_eq!(tmux.application_category, ApplicationCategory::Multiplexer);
        assert_eq!(
            tmux.application_category.gpa_resource_path(),
            Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-zoom-fit.gpa")
        );
    }

    #[test]
    fn test_unknown_alt_screen_vs_running_command_vs_idle() {
        let unknown_tui = detect_application_state(Some("ncdu"), None, true);
        assert_eq!(
            unknown_tui.application_category,
            ApplicationCategory::GenericTui
        );
        assert_eq!(
            unknown_tui.application_category.gpa_resource_path(),
            Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-3d-box.gpa")
        );

        let build = detect_application_state(Some("cargo"), None, false);
        assert_eq!(
            build.application_category,
            ApplicationCategory::RunningCommand
        );
        assert_eq!(
            build.application_category.gpa_resource_path(),
            Some("/dev/boxxy/BoxxyTerminal/icons/boxxy-celestial-loader.gpa")
        );

        let idle_shell = detect_application_state(Some("fish"), None, false);
        assert_eq!(idle_shell.application_category, ApplicationCategory::Idle);
        assert!(!idle_shell.application_category.is_active());
    }
}
