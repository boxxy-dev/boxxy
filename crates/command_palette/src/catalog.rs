use gtk4::prelude::ToVariant;

/// Action name registered on `AppWindow` to open `PreferencesComponent` at a specific category page.
pub const OPEN_PREFERENCES_PAGE_ACTION: &str = "win.open-preferences-page";

/// Metadata for one of the 12 top-level categories in `Preferences`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreferencesCategorySpec {
    pub page_name: &'static str,
    pub subtitle_id: &'static str,
    pub title: &'static str,
    pub keywords: &'static [&'static str],
}

/// Canonical 12 main categories of `Preferences`, ordered identically to the Preferences sidebar.
pub const PREFERENCES_CATEGORIES: [PreferencesCategorySpec; 12] = [
    PreferencesCategorySpec {
        page_name: "appearance",
        subtitle_id: "pref.terminal",
        title: "Terminal",
        keywords: &[
            "preferences",
            "settings",
            "appearance",
            "font",
            "cursor",
            "opacity",
            "frosted glass",
            "padding",
            "layout",
            "previews",
            "smart tab naming",
        ],
    },
    PreferencesCategorySpec {
        page_name: "themes",
        subtitle_id: "pref.themes",
        title: "Themes",
        keywords: &[
            "preferences",
            "settings",
            "app themes",
            "dark mode",
            "light mode",
            "accent color",
            "community themes",
            "color scheme",
        ],
    },
    PreferencesCategorySpec {
        page_name: "apis",
        subtitle_id: "pref.apis",
        title: "APIs",
        keywords: &[
            "preferences",
            "settings",
            "api keys",
            "llm",
            "providers",
            "openai",
            "anthropic",
            "claude",
            "gemini",
            "deepseek",
            "openrouter",
            "ollama",
            "tavily",
            "endpoints",
            "web search",
        ],
    },
    PreferencesCategorySpec {
        page_name: "characters",
        subtitle_id: "pref.characters",
        title: "Characters",
        keywords: &[
            "preferences",
            "settings",
            "agents",
            "personas",
            "avatar",
            "profile",
            "user name",
        ],
    },
    PreferencesCategorySpec {
        page_name: "integrations",
        subtitle_id: "pref.desktop-integrations",
        title: "Desktop Integrations",
        keywords: &[
            "preferences",
            "settings",
            "gnome",
            "cosmic",
            "nautilus",
            "files",
            "extension",
        ],
    },
    PreferencesCategorySpec {
        page_name: "mcp",
        subtitle_id: "pref.mcp",
        title: "MCP",
        keywords: &[
            "preferences",
            "settings",
            "model context protocol",
            "mcp servers",
            "external tools",
        ],
    },
    PreferencesCategorySpec {
        page_name: "skills",
        subtitle_id: "pref.skills",
        title: "Skills",
        keywords: &[
            "preferences",
            "settings",
            "agent skills",
            "self dev",
            "prompts",
        ],
    },
    PreferencesCategorySpec {
        page_name: "claw_ui",
        subtitle_id: "pref.claw-ui",
        title: "Claw UI & Sounds",
        keywords: &[
            "preferences",
            "settings",
            "overlay",
            "drawer",
            "sounds",
            "audio",
            "notifications",
            "tips",
            "currency",
            "font scale",
        ],
    },
    PreferencesCategorySpec {
        page_name: "toolbox",
        subtitle_id: "pref.toolbox",
        title: "Toolbox & Permissions",
        keywords: &[
            "preferences",
            "settings",
            "vibe mode",
            "trusted locations",
            "permissions",
            "approvals",
            "roundtable",
        ],
    },
    PreferencesCategorySpec {
        page_name: "shortcuts",
        subtitle_id: "pref.shortcuts",
        title: "Shortcuts",
        keywords: &[
            "preferences",
            "settings",
            "keyboard shortcuts",
            "keybindings",
            "hotkeys",
            "accelerators",
        ],
    },
    PreferencesCategorySpec {
        page_name: "advanced",
        subtitle_id: "pref.advanced",
        title: "Advanced",
        keywords: &[
            "preferences",
            "settings",
            "shell",
            "background processes",
            "persistence",
            "dreams",
            "memory",
            "telemetry",
        ],
    },
    PreferencesCategorySpec {
        page_name: "about",
        subtitle_id: "pref.about",
        title: "About",
        keywords: &[
            "preferences",
            "settings",
            "version",
            "commit",
            "license",
            "website",
            "credits",
        ],
    },
];

/// Semantic category of an item in [`crate::CommandPaletteComponent`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommandKind {
    /// Standard window, terminal, or application command (`win.*` / `app.*`).
    #[default]
    Action,
    /// Top-level Preferences category (`pref.*` subtitle).
    PreferencesCategory,
    /// User-defined script bookmark (`bookmark.*` subtitle).
    Bookmark,
}

/// A single actionable item registered in [`crate::CommandPaletteComponent`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandItem {
    pub title: String,
    pub subtitle: Option<String>,
    pub action: String,
    pub parameter: Option<gtk4::glib::Variant>,
    pub shortcut: Option<String>,
    pub keywords: Vec<String>,
    pub kind: CommandKind,
}

impl CommandItem {
    /// Creates a standard command entry targeting `action` (e.g. `"win.new-tab"`).
    #[must_use]
    pub fn action(title: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            action: action.into(),
            parameter: None,
            shortcut: None,
            keywords: Vec::new(),
            kind: CommandKind::Action,
        }
    }

    /// Creates an entry that opens `Preferences` directly on `spec.page_name`.
    #[must_use]
    pub fn preferences_category(spec: PreferencesCategorySpec) -> Self {
        Self {
            title: spec.title.to_string(),
            subtitle: Some(spec.subtitle_id.to_string()),
            action: OPEN_PREFERENCES_PAGE_ACTION.to_string(),
            parameter: Some(spec.page_name.to_variant()),
            shortcut: None,
            keywords: spec.keywords.iter().map(|&k| k.to_string()).collect(),
            kind: CommandKind::PreferencesCategory,
        }
    }

    /// Creates a dynamic bookmark entry targeting `"win.execute-bookmark"`.
    #[must_use]
    pub fn bookmark(
        name: impl AsRef<str>,
        filename: impl Into<String>,
        parameter: gtk4::glib::Variant,
    ) -> Self {
        let name_ref = name.as_ref();
        let filename_owned = filename.into();
        let slug = slugify_id(name_ref)
            .or_else(|| slugify_id(&filename_owned))
            .unwrap_or_else(|| "script".to_string());
        Self {
            title: format!("Bookmark: {name_ref}"),
            subtitle: Some(format!("bookmark.{slug}")),
            action: "win.execute-bookmark".to_string(),
            parameter: Some(parameter),
            shortcut: None,
            keywords: vec!["bookmark".to_string(), "script".to_string(), filename_owned],
            kind: CommandKind::Bookmark,
        }
    }

    #[must_use]
    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    #[must_use]
    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    #[must_use]
    pub fn with_parameter(mut self, parameter: gtk4::glib::Variant) -> Self {
        self.parameter = Some(parameter);
        self
    }

    #[must_use]
    pub fn with_keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    /// Subtitle displayed beneath the row title.
    #[must_use]
    pub fn display_subtitle(&self) -> &str {
        self.subtitle.as_deref().unwrap_or(&self.action)
    }
}

fn slugify_id(input: &str) -> Option<String> {
    let stem = input
        .strip_suffix(".sh")
        .or_else(|| input.strip_suffix(".py"))
        .unwrap_or(input);
    let mut slug = String::with_capacity(stem.len());
    let mut prev_dash = false;
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !slug.is_empty() && !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    if slug.ends_with('-') {
        slug.pop();
    }
    (!slug.is_empty()).then_some(slug)
}

/// Builds the complete static command catalog (core actions + 12 Preferences categories).
#[must_use]
#[allow(clippy::too_many_lines)]
pub(super) fn builtin_commands() -> Vec<CommandItem> {
    use boxxy_keybindings as kb;

    let mut items = Vec::with_capacity(44);

    // ── Window & Tabs ────────────────────────────────────────────────────────
    items.push(
        CommandItem::action("New Window", kb::NEW_WINDOW.action_name)
            .with_shortcut(kb::NEW_WINDOW.trigger),
    );
    items.push(
        CommandItem::action("New Tab", kb::NEW_TAB.action_name).with_shortcut(kb::NEW_TAB.trigger),
    );
    items.push(
        CommandItem::action("Close Tab", kb::CLOSE_TAB.action_name)
            .with_shortcut(kb::CLOSE_TAB.trigger),
    );
    items.push(
        CommandItem::action("Detached Shells", "win.detached-sessions").with_keywords([
            "background shells",
            "reattach",
            "sessions",
        ]),
    );
    items.push(
        CommandItem::action("Open in Files", "win.open-in-files").with_keywords([
            "file manager",
            "folder",
            "directory",
            "nautilus",
        ]),
    );

    // ── Terminal, Clipboard & Zoom ───────────────────────────────────────────
    items.push(CommandItem::action("Copy", kb::COPY.action_name).with_shortcut(kb::COPY.trigger));
    items
        .push(CommandItem::action("Paste", kb::PASTE.action_name).with_shortcut(kb::PASTE.trigger));
    items.push(
        CommandItem::action("Select All", kb::SELECT_ALL.action_name)
            .with_shortcut(kb::SELECT_ALL.trigger),
    );
    items.push(
        CommandItem::action("Copy Last Command Output", "win.copy-last-output")
            .with_keywords(["clipboard", "output"]),
    );
    items.push(
        CommandItem::action("Search in Terminal", kb::SEARCH.action_name)
            .with_shortcut(kb::SEARCH.trigger)
            .with_keywords(["find", "search"]),
    );
    items.push(
        CommandItem::action("Open Scrollback in Editor", kb::OPEN_SCROLLBACK.action_name)
            .with_shortcut(kb::OPEN_SCROLLBACK.trigger)
            .with_keywords(["buffer", "history", "editor"]),
    );
    items.push(CommandItem::action(
        "Clear Scrollback",
        "win.clear-scrollback",
    ));
    items.push(CommandItem::action("Reset Terminal", "win.reset-terminal"));
    items.push(
        CommandItem::action("Zoom In", kb::ZOOM_IN.action_name).with_shortcut(kb::ZOOM_IN.trigger),
    );
    items.push(
        CommandItem::action("Zoom Out", kb::ZOOM_OUT.action_name)
            .with_shortcut(kb::ZOOM_OUT.trigger),
    );
    items.push(
        CommandItem::action("Reset Zoom", kb::RESET_ZOOM.action_name)
            .with_shortcut(kb::RESET_ZOOM.trigger),
    );

    // ── Split Panes ──────────────────────────────────────────────────────────
    items.push(
        CommandItem::action("Split Right", kb::SPLIT_RIGHT.action_name)
            .with_shortcut(kb::SPLIT_RIGHT.trigger)
            .with_keywords(["split vertical", "pane"]),
    );
    items.push(
        CommandItem::action("Split Down", kb::SPLIT_DOWN.action_name)
            .with_shortcut(kb::SPLIT_DOWN.trigger)
            .with_keywords(["split horizontal", "pane"]),
    );
    items.push(
        CommandItem::action("Close Pane", kb::CLOSE_PANE.action_name)
            .with_shortcut(kb::CLOSE_PANE.trigger)
            .with_keywords(["close split", "pane"]),
    );
    items.push(
        CommandItem::action("Toggle Maximize Pane", "win.toggle-maximize").with_keywords([
            "zoom pane",
            "unmaximize",
            "fullscreen pane",
        ]),
    );
    items.push(
        CommandItem::action("Focus Pane Left", kb::FOCUS_LEFT.action_name)
            .with_shortcut(kb::FOCUS_LEFT.trigger),
    );
    items.push(
        CommandItem::action("Focus Pane Right", kb::FOCUS_RIGHT.action_name)
            .with_shortcut(kb::FOCUS_RIGHT.trigger),
    );
    items.push(
        CommandItem::action("Focus Pane Up", kb::FOCUS_UP.action_name)
            .with_shortcut(kb::FOCUS_UP.trigger),
    );
    items.push(
        CommandItem::action("Focus Pane Down", kb::FOCUS_DOWN.action_name)
            .with_shortcut(kb::FOCUS_DOWN.trigger),
    );
    items.push(
        CommandItem::action("Swap Pane Left", kb::SWAP_LEFT.action_name)
            .with_shortcut(kb::SWAP_LEFT.trigger),
    );
    items.push(
        CommandItem::action("Swap Pane Right", kb::SWAP_RIGHT.action_name)
            .with_shortcut(kb::SWAP_RIGHT.trigger),
    );
    items.push(
        CommandItem::action("Swap Pane Up", kb::SWAP_UP.action_name)
            .with_shortcut(kb::SWAP_UP.trigger),
    );
    items.push(
        CommandItem::action("Swap Pane Down", kb::SWAP_DOWN.action_name)
            .with_shortcut(kb::SWAP_DOWN.trigger),
    );

    // ── Sidebar, Claw, Bookmarks & Models ────────────────────────────────────
    items.push(
        CommandItem::action("Toggle Sidebar", kb::TOGGLE_SIDEBAR.action_name)
            .with_shortcut(kb::TOGGLE_SIDEBAR.trigger),
    );
    items.push(CommandItem::action("Claw Sidebar", "win.claw").with_keywords(["agent", "logs"]));
    items.push(
        CommandItem::action(
            "Toggle Claw Code & Diff Blocks",
            kb::TOGGLE_CLAW_CODE_BLOCKS.action_name,
        )
        .with_shortcut(kb::TOGGLE_CLAW_CODE_BLOCKS.trigger)
        .with_keywords(["xray", "diffs", "code blocks"]),
    );
    items.push(CommandItem::action("Bookmarks", "win.bookmarks").with_keywords(["scripts"]));
    items.push(
        CommandItem::action("Models Selection", "win.model-selection").with_keywords([
            "llm",
            "ai model",
            "provider",
            "dreams model",
            "character model",
        ]),
    );
    items.push(
        CommandItem::action("Terminal Colors", "win.terminal-colors").with_keywords([
            "palette",
            "ansi",
            "color scheme",
        ]),
    );

    // ── Preferences & All 12 Main Preferences Categories ─────────────────────
    items.push(
        CommandItem::action("Preferences", kb::PREFERENCES.action_name)
            .with_shortcut(kb::PREFERENCES.trigger)
            .with_keywords(["settings", "options", "configuration"]),
    );
    for spec in PREFERENCES_CATEGORIES {
        items.push(CommandItem::preferences_category(spec));
    }

    // ── Meta & Diagnostics ───────────────────────────────────────────────────
    items.push(
        CommandItem::action("What's New", "win.whats-new").with_keywords([
            "changelog",
            "release notes",
            "updates",
        ]),
    );
    items.push(
        CommandItem::action("GTK Inspector", "app.inspector")
            .with_keywords(["debug", "css", "devtools"]),
    );

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn preferences_categories_have_12_unique_pages_and_titles() {
        assert_eq!(PREFERENCES_CATEGORIES.len(), 12);
        let mut pages = HashSet::new();
        let mut subtitles = HashSet::new();
        let mut titles = HashSet::new();
        for cat in PREFERENCES_CATEGORIES {
            assert_ne!(cat.page_name, "");
            assert!(
                cat.subtitle_id.starts_with("pref."),
                "expected pref.* subtitle_id, got: {}",
                cat.subtitle_id
            );
            assert_ne!(cat.title, "");
            assert!(
                pages.insert(cat.page_name),
                "duplicate page_name: {}",
                cat.page_name
            );
            assert!(
                subtitles.insert(cat.subtitle_id),
                "duplicate subtitle_id: {}",
                cat.subtitle_id
            );
            assert!(titles.insert(cat.title), "duplicate title: {}", cat.title);
        }
    }

    #[test]
    fn builtin_commands_have_unique_targets_and_registered_shortcuts() {
        let mut known_triggers: HashSet<&str> = boxxy_keybindings::get_shortcuts_by_category()
            .into_iter()
            .flat_map(|cat| cat.items.into_iter().map(|(_, kb)| kb.trigger))
            .collect();
        known_triggers.insert(boxxy_keybindings::RESET_ZOOM.trigger);

        let cmds = builtin_commands();
        let mut seen_targets = HashSet::new();
        let mut pref_cat_count = 0usize;

        for cmd in &cmds {
            assert_ne!(cmd.title.trim(), "");
            assert!(
                cmd.action.starts_with("win.") || cmd.action.starts_with("app."),
                "unexpected action namespace: {}",
                cmd.action
            );

            let param_repr = cmd
                .parameter
                .as_ref()
                .map_or_else(String::new, ToString::to_string);
            let key = format!("{}::{param_repr}", cmd.action);
            assert!(seen_targets.insert(key.clone()), "duplicate target: {key}");

            if cmd.kind == CommandKind::PreferencesCategory {
                pref_cat_count += 1;
                assert!(cmd.display_subtitle().starts_with("pref."));
                assert_eq!(cmd.action, OPEN_PREFERENCES_PAGE_ACTION);
                assert!(cmd.parameter.is_some());
            }

            if let Some(ref sc) = cmd.shortcut {
                assert!(
                    known_triggers.contains(sc.as_str()),
                    "unregistered shortcut '{sc}' on '{}' (not in boxxy-keybindings)",
                    cmd.title
                );
            }
        }

        assert_eq!(pref_cat_count, 12);
    }
}
