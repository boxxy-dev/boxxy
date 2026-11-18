//! Canonical Claw slash-command definitions and submission-state classification
//! shared between the GTK frontend (`boxxy-claw-widget` / `boxxy-terminal`) and
//! the reasoning engine (`boxxy-claw`).

/// Built-in slash commands recognized in the Claw overlay input bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommand {
    Incognito,
    Resume,
    Debate,
    Drama,
    Stop,
    Undo,
    Models,
    Vibe,
    Sleep,
    Pin,
    Web,
    Code,
    Clear,
    Exit,
}

/// Immediate UI state effect when a message or slash command is submitted in the Claw overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashSubmitEffect {
    /// Handled strictly inside `MsgBarComponent` before `on_submit` (e.g. `/models`, `/vibe`, `/code`, `/clear`, `/exit`, bare `/resume`).
    FrontendOnly,
    /// Sent to session/engine and resolved immediately without starting an LLM turn
    /// (e.g. bare `/incognito`, `/resume <id>`, bare `/drama`). Keeps `OverlayState::Idle`.
    InstantControl,
    /// Cancels any active turn or proposal and settles immediately into `OverlayState::Idle`
    /// without starting an LLM turn (e.g. `/stop`, `/undo`).
    CancelAndSettle,
    /// Starts an asynchronous LLM turn (`OverlayState::Thinking`) — e.g. `/incognito <msg>`,
    /// `/drama <args>`, `/debate [args]`, or normal user conversation.
    StartsTurn,
}

impl SlashCommand {
    /// Parses a leading slash command and its trimmed argument remainder.
    ///
    /// Returns `None` for non-command text, unrecognized `/tokens`, Unix filesystem
    /// paths (such as `/etc/hosts` or `/var/log/syslog`), or zero-argument commands
    /// given unexpected trailing tokens.
    #[must_use]
    pub fn parse(input: &str) -> Option<(Self, &str)> {
        let trimmed = input.trim();
        let without_slash = trimmed.strip_prefix('/')?;
        let (head, rest) = without_slash
            .find(char::is_whitespace)
            .map_or((without_slash, ""), |idx| {
                (&without_slash[..idx], without_slash[idx..].trim())
            });

        if head.is_empty() || head.contains('/') {
            return None;
        }

        let cmd = if head.eq_ignore_ascii_case("incognito") {
            Self::Incognito
        } else if head.eq_ignore_ascii_case("resume") {
            Self::Resume
        } else if head.eq_ignore_ascii_case("debate") {
            Self::Debate
        } else if head.eq_ignore_ascii_case("drama") {
            Self::Drama
        } else if head.eq_ignore_ascii_case("stop") {
            Self::Stop
        } else if head.eq_ignore_ascii_case("undo") {
            Self::Undo
        } else if head.eq_ignore_ascii_case("models") {
            Self::Models
        } else if head.eq_ignore_ascii_case("vibe") {
            Self::Vibe
        } else if head.eq_ignore_ascii_case("sleep") {
            Self::Sleep
        } else if head.eq_ignore_ascii_case("pin") {
            Self::Pin
        } else if head.eq_ignore_ascii_case("web") {
            Self::Web
        } else if head.eq_ignore_ascii_case("code") {
            Self::Code
        } else if head.eq_ignore_ascii_case("clear") {
            Self::Clear
        } else if head.eq_ignore_ascii_case("exit") {
            Self::Exit
        } else {
            return None;
        };

        if !cmd.accepts_args() && !rest.is_empty() {
            return None;
        }

        Some((cmd, rest))
    }

    /// Whether this command accepts trailing arguments after `/command `.
    #[must_use]
    pub const fn accepts_args(self) -> bool {
        matches!(self, Self::Incognito | Self::Resume | Self::Debate | Self::Drama)
    }

    /// Returns `true` when selecting `replacement` from the autocomplete popup should
    /// immediately submit the command rather than leaving the cursor in the input buffer.
    #[must_use]
    pub fn auto_submits_on_complete(replacement: &str) -> bool {
        if replacement.ends_with(' ') {
            return false;
        }
        let Some((cmd, rest)) = Self::parse(replacement) else {
            return false;
        };
        match cmd {
            Self::Resume => !rest.is_empty(),
            Self::Debate => false,
            Self::Incognito
            | Self::Drama
            | Self::Stop
            | Self::Undo
            | Self::Models
            | Self::Vibe
            | Self::Sleep
            | Self::Pin
            | Self::Web
            | Self::Code
            | Self::Clear
            | Self::Exit => rest.is_empty(),
        }
    }

    /// Computes how the Claw overlay state machine should react immediately upon submission.
    #[must_use]
    pub const fn submit_effect(self, args: &str) -> SlashSubmitEffect {
        match self {
            Self::Models
            | Self::Vibe
            | Self::Sleep
            | Self::Pin
            | Self::Web
            | Self::Code
            | Self::Clear
            | Self::Exit => SlashSubmitEffect::FrontendOnly,
            Self::Resume => {
                if args.is_empty() {
                    SlashSubmitEffect::FrontendOnly
                } else {
                    SlashSubmitEffect::InstantControl
                }
            }
            Self::Incognito | Self::Drama => {
                if args.is_empty() {
                    SlashSubmitEffect::InstantControl
                } else {
                    SlashSubmitEffect::StartsTurn
                }
            }
            Self::Stop | Self::Undo => SlashSubmitEffect::CancelAndSettle,
            Self::Debate => SlashSubmitEffect::StartsTurn,
        }
    }
}

/// Classifies the immediate UI state transition for a raw input string submitted in the Claw overlay.
#[must_use]
pub fn classify_submit_effect(input: &str) -> SlashSubmitEffect {
    let trimmed = input.trim();
    if trimmed
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("/btw "))
    {
        return SlashSubmitEffect::InstantControl;
    }
    match SlashCommand::parse(trimmed) {
        Some((cmd, args)) => cmd.submit_effect(args),
        None => SlashSubmitEffect::StartsTurn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_commands_and_preserves_unix_paths() {
        assert_eq!(
            SlashCommand::parse("/drama"),
            Some((SlashCommand::Drama, ""))
        );
        assert_eq!(
            SlashCommand::parse("  /DRAMA   :levi-kujo kernel panic  "),
            Some((SlashCommand::Drama, ":levi-kujo kernel panic"))
        );
        assert_eq!(
            SlashCommand::parse("/debate"),
            Some((SlashCommand::Debate, ""))
        );
        assert_eq!(
            SlashCommand::parse("/debate :kuro Rust vs C"),
            Some((SlashCommand::Debate, ":kuro Rust vs C"))
        );
        assert_eq!(SlashCommand::parse("/stop"), Some((SlashCommand::Stop, "")));
        assert_eq!(SlashCommand::parse("/undo"), Some((SlashCommand::Undo, "")));

        // Zero-arg commands with trailing prose are not parsed as slash commands
        assert_eq!(SlashCommand::parse("/stop extra"), None);
        assert_eq!(SlashCommand::parse("/undo file.rs"), None);

        // Unix filesystem paths must never be misclassified as slash commands
        assert_eq!(SlashCommand::parse("/etc/hosts is missing"), None);
        assert_eq!(SlashCommand::parse("/var/log/syslog"), None);
        assert_eq!(SlashCommand::parse("/tmp"), None);
    }

    #[test]
    fn submit_effects_distinguish_instant_commands_from_llm_turns() {
        assert_eq!(
            classify_submit_effect("/drama"),
            SlashSubmitEffect::InstantControl
        );
        assert_eq!(
            classify_submit_effect("/drama :levi-kujo"),
            SlashSubmitEffect::StartsTurn
        );
        assert_eq!(
            classify_submit_effect("/debate"),
            SlashSubmitEffect::StartsTurn
        );
        assert_eq!(
            classify_submit_effect("/stop"),
            SlashSubmitEffect::CancelAndSettle
        );
        assert_eq!(
            classify_submit_effect("/undo"),
            SlashSubmitEffect::CancelAndSettle
        );
        assert_eq!(
            classify_submit_effect("/incognito"),
            SlashSubmitEffect::InstantControl
        );
        assert_eq!(
            classify_submit_effect("/incognito hello"),
            SlashSubmitEffect::StartsTurn
        );
        assert_eq!(
            classify_submit_effect("/resume abc-123"),
            SlashSubmitEffect::InstantControl
        );
        assert_eq!(
            classify_submit_effect("/code"),
            SlashSubmitEffect::FrontendOnly
        );
        assert_eq!(
            classify_submit_effect("Check /etc/fstab please"),
            SlashSubmitEffect::StartsTurn
        );
    }

    #[test]
    fn autocomplete_auto_submit_respects_trailing_space_prompts() {
        assert!(SlashCommand::auto_submits_on_complete("/drama"));
        assert!(SlashCommand::auto_submits_on_complete("/stop"));
        assert!(SlashCommand::auto_submits_on_complete("/undo"));
        assert!(SlashCommand::auto_submits_on_complete("/resume sess-01"));
        assert!(!SlashCommand::auto_submits_on_complete("/resume "));
        assert!(!SlashCommand::auto_submits_on_complete("/debate "));
        assert!(!SlashCommand::auto_submits_on_complete("/drama :levi-kujo "));
    }
}
