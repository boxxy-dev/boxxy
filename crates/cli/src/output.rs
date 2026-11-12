//! Terminal-aware output styling and JSON formatting for `+action` commands.

use serde::Serialize;
use std::io::IsTerminal;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputFormat {
    #[default]
    Human,
    Json,
}

impl OutputFormat {
    #[must_use]
    pub const fn from_json_flag(json: bool) -> Self {
        if json { Self::Json } else { Self::Human }
    }
}

/// Minimal zero-dependency ANSI styler that honors TTY detection and `NO_COLOR`.
#[derive(Clone, Copy, Debug)]
pub struct TermStyle {
    color: bool,
}

impl TermStyle {
    #[must_use]
    pub fn stdout() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some()
            || std::env::var("TERM").is_ok_and(|t| t == "dumb");
        Self {
            color: !no_color && std::io::stdout().is_terminal(),
        }
    }

    #[must_use]
    pub const fn plain() -> Self {
        Self { color: false }
    }

    #[must_use]
    pub fn bold(&self, text: &str) -> String {
        if self.color {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    #[must_use]
    pub fn dim(&self, text: &str) -> String {
        if self.color {
            format!("\x1b[2m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    #[must_use]
    pub fn green(&self, text: &str) -> String {
        if self.color {
            format!("\x1b[32m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    #[must_use]
    pub fn yellow(&self, text: &str) -> String {
        if self.color {
            format!("\x1b[33m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    #[must_use]
    pub fn red(&self, text: &str) -> String {
        if self.color {
            format!("\x1b[31m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    #[must_use]
    pub fn cyan(&self, text: &str) -> String {
        if self.color {
            format!("\x1b[36m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

pub fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    let rendered = serde_json::to_string_pretty(value)?;
    println!("{rendered}");
    Ok(())
}
