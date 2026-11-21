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
        title: "APIs & Subscriptions",
        keywords: &[
            "preferences",
            "settings",
            "apis",
            "api keys",
            "subscription",
            "subscriptions",
            "oauth",
            "chatgpt",
            "codex",
            "sign in",
            "account",
            "llm",
            "providers",
            "openai",
            "anthropic",
            "claude",
            "gemini",
            "deepseek",
            "openrouter",
            "opencode",
            "moonshot",
            "kimi",
            "xai",
            "grok",
            "mistral",
            "groq",
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

fn cmd_for_id(
    km: &boxxy_keybindings::CompiledKeymap,
    id: boxxy_keybindings::ShortcutId,
    title_override: Option<&str>,
) -> CommandItem {
    let spec = boxxy_keybindings::spec_for(id);
    let mut item = CommandItem::action(title_override.unwrap_or(spec.title), spec.action_name)
        .with_keywords(spec.keywords.iter().copied());
    if let Some(accel) = km.primary_gtk_accel(id) {
        item = item.with_shortcut(accel);
    }
    item
}

/// Builds the complete static command catalog (core actions + 12 Preferences categories).
#[must_use]
#[allow(clippy::too_many_lines)]
pub(super) fn builtin_commands() -> Vec<CommandItem> {
    use boxxy_keybindings::ShortcutId as Id;

    let km = boxxy_keybindings::keymap();
    let mut items = Vec::with_capacity(50);

    // ── Window & Tabs ────────────────────────────────────────────────────────
    items.push(cmd_for_id(&km, Id::NewWindow, None));
    items.push(cmd_for_id(&km, Id::NewTab, None));
    items.push(cmd_for_id(&km, Id::CloseTab, None));
    items.push(cmd_for_id(&km, Id::NextTab, None));
    items.push(cmd_for_id(&km, Id::PrevTab, None));
    items.push(cmd_for_id(&km, Id::DetachedSessions, Some("Detached Shells")));
    items.push(
        CommandItem::action("Open in Files", "win.open-in-files").with_keywords([
            "file manager",
            "folder",
            "directory",
            "nautilus",
        ]),
    );

    // ── Terminal, Clipboard & Zoom ───────────────────────────────────────────
    items.push(cmd_for_id(&km, Id::Copy, None));
    items.push(cmd_for_id(&km, Id::Paste, None));
    items.push(cmd_for_id(&km, Id::SelectAll, None));
    items.push(cmd_for_id(&km, Id::CopyLastOutput, None));
    items.push(cmd_for_id(&km, Id::Search, Some("Search in Terminal")));
    items.push(cmd_for_id(&km, Id::PrevPrompt, None));
    items.push(cmd_for_id(&km, Id::NextPrompt, None));
    items.push(cmd_for_id(&km, Id::OpenScrollback, None));
    items.push(cmd_for_id(&km, Id::ClearScrollback, None));
    items.push(cmd_for_id(&km, Id::ResetTerminal, None));
    items.push(cmd_for_id(&km, Id::ZoomIn, None));
    items.push(cmd_for_id(&km, Id::ZoomOut, None));
    items.push(cmd_for_id(&km, Id::ResetZoom, None));

    // ── Split Panes ──────────────────────────────────────────────────────────
    items.push(cmd_for_id(&km, Id::SplitRight, None));
    items.push(cmd_for_id(&km, Id::SplitDown, None));
    items.push(cmd_for_id(&km, Id::ClosePane, None));
    items.push(cmd_for_id(&km, Id::ToggleMaximizePane, None));
    items.push(cmd_for_id(&km, Id::EqualizeSplits, None));
    items.push(cmd_for_id(&km, Id::FocusLeft, Some("Focus Pane Left")));
    items.push(cmd_for_id(&km, Id::FocusRight, Some("Focus Pane Right")));
    items.push(cmd_for_id(&km, Id::FocusUp, Some("Focus Pane Up")));
    items.push(cmd_for_id(&km, Id::FocusDown, Some("Focus Pane Down")));
    items.push(cmd_for_id(&km, Id::SwapLeft, Some("Swap Pane Left")));
    items.push(cmd_for_id(&km, Id::SwapRight, Some("Swap Pane Right")));
    items.push(cmd_for_id(&km, Id::SwapUp, Some("Swap Pane Up")));
    items.push(cmd_for_id(&km, Id::SwapDown, Some("Swap Pane Down")));

    // ── Sidebar, Claw, Bookmarks & Models ────────────────────────────────────
    items.push(cmd_for_id(&km, Id::ToggleSidebar, None));
    items.push(cmd_for_id(&km, Id::AskClaw, None));
    items.push(CommandItem::action("Claw Sidebar", "win.claw").with_keywords(["agent", "logs"]));
    items.push(cmd_for_id(
        &km,
        Id::ToggleClawCodeBlocks,
        Some("Toggle Claw Code & Diff Blocks"),
    ));
    items.push(cmd_for_id(&km, Id::Bookmarks, None));
    items.push(cmd_for_id(&km, Id::ModelSelection, None));
    items.push(
        CommandItem::action("Terminal Colors", "win.terminal-colors").with_keywords([
            "palette",
            "ansi",
            "color scheme",
        ]),
    );

    // ── Preferences & All 12 Main Preferences Categories ─────────────────────
    items.push(cmd_for_id(&km, Id::Preferences, None));
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
        let km = boxxy_keybindings::keymap();
        let known_triggers: HashSet<String> = boxxy_keybindings::ShortcutId::ALL
            .into_iter()
            .filter_map(|id| km.primary_gtk_accel(id))
            .collect();

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
