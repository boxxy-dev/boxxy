use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Visual grouping of shortcuts in `Preferences -> Shortcuts` and `+keybinds`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShortcutCategory {
    General,
    Terminal,
    SplitPanes,
    Tabs,
}

impl ShortcutCategory {
    pub const ALL: [Self; 4] = [Self::General, Self::Terminal, Self::SplitPanes, Self::Tabs];

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Terminal => "Terminal",
            Self::SplitPanes => "Split Panes",
            Self::Tabs => "Tabs",
        }
    }
}

/// Widget focus scope required for a shortcut to activate.
///
/// Ordered from broadest (`GlobalWindow`) to most specific (`TerminalFocused`)
/// so conflict analysis knows which layer intercepts an event first during
/// GTK's `PropagationPhase::Capture` traversal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActionScope {
    /// Active anywhere inside the main window workspace (suspended when a modal dialog is open).
    GlobalWindow = 0,
    /// Active when focus resides inside a terminal pane (`TerminalWidget`, Claw drawer, or `SearchBar`).
    PaneOrOverlay = 1,
    /// Active strictly when `TerminalWidget` (or `SearchBar` for `Search`) owns focus, ensuring
    /// custom bindings like `Ctrl+C`/`Ctrl+V`/`Ctrl+A` never hijack normal GTK text inputs.
    TerminalFocused = 2,
}

/// Active input mode of the focused terminal pane.
///
/// Includes `ViNormal` and `ViVisual` hooks so Stage 2 Vi/Copy Mode plugs into
/// the capture dispatcher without altering keymap or window plumbing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TerminalInputMode {
    #[default]
    Normal,
    Search,
    ViNormal,
    ViVisual,
}

/// Strongly typed identifier for every user-bindable shortcut action in Boxxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShortcutId {
    // General
    NewWindow,
    CommandPalette,
    ToggleSidebar,
    AskClaw,
    ToggleClawCodeBlocks,
    Preferences,
    DetachedSessions,
    ModelSelection,
    Bookmarks,

    // Terminal
    Copy,
    Paste,
    SelectAll,
    Search,
    PrevPrompt,
    NextPrompt,
    CopyLastOutput,
    OpenScrollback,
    ClearScrollback,
    ResetTerminal,
    ZoomIn,
    ZoomOut,
    ResetZoom,

    // Split Panes
    SplitDown,
    SplitRight,
    ClosePane,
    ToggleMaximizePane,
    EqualizeSplits,
    FocusUp,
    FocusDown,
    FocusLeft,
    FocusRight,
    SwapUp,
    SwapDown,
    SwapLeft,
    SwapRight,

    // Tabs
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    SwitchTab1,
    SwitchTab2,
    SwitchTab3,
    SwitchTab4,
    SwitchTab5,
    SwitchTab6,
    SwitchTab7,
    SwitchTab8,
    SwitchTab9,
    SwitchTab10,
}

impl ShortcutId {
    pub const ALL: [Self; 49] = [
        Self::NewWindow,
        Self::CommandPalette,
        Self::ToggleSidebar,
        Self::AskClaw,
        Self::ToggleClawCodeBlocks,
        Self::Preferences,
        Self::DetachedSessions,
        Self::ModelSelection,
        Self::Bookmarks,
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
        Self::Search,
        Self::PrevPrompt,
        Self::NextPrompt,
        Self::CopyLastOutput,
        Self::OpenScrollback,
        Self::ClearScrollback,
        Self::ResetTerminal,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ResetZoom,
        Self::SplitDown,
        Self::SplitRight,
        Self::ClosePane,
        Self::ToggleMaximizePane,
        Self::EqualizeSplits,
        Self::FocusUp,
        Self::FocusDown,
        Self::FocusLeft,
        Self::FocusRight,
        Self::SwapUp,
        Self::SwapDown,
        Self::SwapLeft,
        Self::SwapRight,
        Self::NewTab,
        Self::CloseTab,
        Self::NextTab,
        Self::PrevTab,
        Self::SwitchTab1,
        Self::SwitchTab2,
        Self::SwitchTab3,
        Self::SwitchTab4,
        Self::SwitchTab5,
        Self::SwitchTab6,
        Self::SwitchTab7,
        Self::SwitchTab8,
        Self::SwitchTab9,
        Self::SwitchTab10,
    ];

    /// Stable kebab-case key used in `keybindings.toml`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NewWindow => "new-window",
            Self::CommandPalette => "command-palette",
            Self::ToggleSidebar => "toggle-sidebar",
            Self::AskClaw => "ask-claw",
            Self::ToggleClawCodeBlocks => "toggle-claw-code-blocks",
            Self::Preferences => "preferences",
            Self::DetachedSessions => "detached-sessions",
            Self::ModelSelection => "model-selection",
            Self::Bookmarks => "bookmarks",
            Self::Copy => "copy",
            Self::Paste => "paste",
            Self::SelectAll => "select-all",
            Self::Search => "search",
            Self::PrevPrompt => "prev-prompt",
            Self::NextPrompt => "next-prompt",
            Self::CopyLastOutput => "copy-last-output",
            Self::OpenScrollback => "open-scrollback",
            Self::ClearScrollback => "clear-scrollback",
            Self::ResetTerminal => "reset-terminal",
            Self::ZoomIn => "zoom-in",
            Self::ZoomOut => "zoom-out",
            Self::ResetZoom => "reset-zoom",
            Self::SplitDown => "split-down",
            Self::SplitRight => "split-right",
            Self::ClosePane => "close-pane",
            Self::ToggleMaximizePane => "toggle-maximize",
            Self::EqualizeSplits => "equalize-splits",
            Self::FocusUp => "focus-up",
            Self::FocusDown => "focus-down",
            Self::FocusLeft => "focus-left",
            Self::FocusRight => "focus-right",
            Self::SwapUp => "swap-up",
            Self::SwapDown => "swap-down",
            Self::SwapLeft => "swap-left",
            Self::SwapRight => "swap-right",
            Self::NewTab => "new-tab",
            Self::CloseTab => "close-tab",
            Self::NextTab => "next-tab",
            Self::PrevTab => "prev-tab",
            Self::SwitchTab1 => "tab-1",
            Self::SwitchTab2 => "tab-2",
            Self::SwitchTab3 => "tab-3",
            Self::SwitchTab4 => "tab-4",
            Self::SwitchTab5 => "tab-5",
            Self::SwitchTab6 => "tab-6",
            Self::SwitchTab7 => "tab-7",
            Self::SwitchTab8 => "tab-8",
            Self::SwitchTab9 => "tab-9",
            Self::SwitchTab10 => "tab-10",
        }
    }

    /// Returns `true` when this action supports conditional `performable:` pass-through.
    #[must_use]
    pub const fn supports_performable(self) -> bool {
        matches!(
            self,
            Self::Copy | Self::ClosePane | Self::ToggleMaximizePane | Self::EqualizeSplits
        )
    }
}

impl fmt::Display for ShortcutId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownShortcutIdError(pub String);

impl fmt::Display for UnknownShortcutIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown shortcut action '{}'", self.0)
    }
}

impl std::error::Error for UnknownShortcutIdError {}

impl FromStr for ShortcutId {
    type Err = UnknownShortcutIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim();
        for id in Self::ALL {
            if id.as_str().eq_ignore_ascii_case(normalized) {
                return Ok(id);
            }
        }
        // Accept friendly aliases when users hand-edit `keybindings.toml`.
        match normalized.to_ascii_lowercase().as_str() {
            "message-bar" | "claw-msgbar" => Ok(Self::AskClaw),
            "split-horizontal" => Ok(Self::SplitDown),
            "split-vertical" => Ok(Self::SplitRight),
            "close-split" => Ok(Self::ClosePane),
            "previous-tab" => Ok(Self::PrevTab),
            "previous-prompt" => Ok(Self::PrevPrompt),
            _ => Err(UnknownShortcutIdError(normalized.to_string())),
        }
    }
}

impl Serialize for ShortcutId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ShortcutId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
