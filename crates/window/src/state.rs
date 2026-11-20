use crate::init::{TabLaunchSpec, TerminalController};
use crate::tab_menu::TabContextMenu;
use crate::widgets::notification::Notification;
use boxxy_bookmarks::{sidebar::BookmarksSidebarComponent, tab::BookmarksTabComponent};
use boxxy_claw_ui::ClawSidebarComponent;
use boxxy_command_palette::CommandPaletteComponent;
use boxxy_preferences::{AppState, PreferencesComponent, Settings};
use boxxy_terminal::TerminalEvent;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

/// Carries everything needed to display an in-app toast.
/// `timeout_secs = 0` means the toast stays until the user dismisses it.
#[derive(Debug, Clone)]
pub struct ToastRequest {
    pub message: String,
    pub timeout_secs: u32,
}

impl ToastRequest {
    pub fn ephemeral(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            timeout_secs: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TabColor {
    Default,
    Blue,
    Teal,
    Green,
    Yellow,
    Orange,
    Red,
    Pink,
    Purple,
    Slate,
}

impl TabColor {
    pub fn all() -> &'static [TabColor] {
        &[
            TabColor::Blue,
            TabColor::Teal,
            TabColor::Green,
            TabColor::Yellow,
            TabColor::Orange,
            TabColor::Red,
            TabColor::Pink,
            TabColor::Purple,
            TabColor::Slate,
        ]
    }

    pub fn as_css_class(&self) -> Option<&'static str> {
        match self {
            TabColor::Default => None,
            TabColor::Blue => Some("tab-blue"),
            TabColor::Teal => Some("tab-teal"),
            TabColor::Green => Some("tab-green"),
            TabColor::Yellow => Some("tab-yellow"),
            TabColor::Orange => Some("tab-orange"),
            TabColor::Red => Some("tab-red"),
            TabColor::Pink => Some("tab-pink"),
            TabColor::Purple => Some("tab-purple"),
            TabColor::Slate => Some("tab-slate"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum AppInput {
    NewWindow,
    NewTab,
    NextTab,
    PrevTab,
    SelectTabIndex(i32),
    EnsureInitialTab,
    NewTabLaunch(TabLaunchSpec),
    CloseTabRequest(usize),
    MoveTabToNewWindowRequest(usize),
    SetTabColor(usize, TabColor),
    SetTabTitle(usize, Option<String>),
    CloseTab(String),
    CloseActiveTab,
    HandleTerminalEvent(Box<TerminalEvent>),
    AdoptOrphanTabs,
    ToggleSidebar,
    SidebarVisibleChanged(bool),
    SidebarPageChanged(String),
    OpenPreferences,
    OpenPreferencesPage(String),
    OpenThemesPreferences,
    OpenApisPreferences,
    OpenMcpPreferences,
    OpenBookmarks,
    OpenShortcuts,
    OpenWhatsNew,
    OpenAbout,
    OpenInFiles,
    SettingsChanged(Box<Settings>),
    ShowThemesSidebar,
    ShowClawSidebar,
    ToggleClawCodeBlocks,
    ShowBookmarksSidebar,
    ExecuteBookmark(String, String, String), // Name, Filename, Script
    ExecuteInNewTab(String, String, String), // Name, Filename, Script
    SetClawActive(bool, Option<String>),
    SetClawActiveGlobal(bool),
    ModelSelection,
    ThemeSelected(Box<boxxy_themes::ParsedPaletteStatic>),
    CommandPalette,
    ReloadEngine,
    ZoomIn,
    ZoomOut,
    ResetZoom,
    SplitVertical,
    SplitHorizontal,
    CloseSplit,
    ToggleMaximize,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    SwapLeft,
    SwapRight,
    SwapUp,
    SwapDown,
    MovePaneToSplit {
        source_pane_id: String,
        target_pane_id: String,
        direction: boxxy_terminal::Direction,
    },
    DropPaneOnTab {
        source_pane_id: String,
        target_page_key: usize,
    },
    FocusActiveTerminal,
    TabPageDetached(usize),
    TabPageAttached(usize),
    CloseRequested,
    SidebarWidthChanged(i32),
    SaveWindowState {
        width: i32,
        height: i32,
        is_maximized: bool,
    },
    PushGlobalNotification(Box<Notification>),
    ShowToast(ToastRequest),
    DismissNotification(String),
    StartUpdateDownload(String, String, String, Option<String>), // (url, date, new_commit, checksum_url)
    UpdateDownloaded(String),
    ApplyUpdateAndRestart,
    ShowWhatsNewDialog(
        crate::updater::whats_new::WhatsNewPending,
        Option<Box<crate::updater::whats_new::ChangelogSummary>>,
    ),
    StartWhatsNewClaw(String),
    CancelTask(uuid::Uuid, Option<String>), // (task_id, pane_id)
    TogglePauseTask(uuid::Uuid, Option<String>), // (task_id, pane_id)
    CancelAllTasks(Option<String>),         // (pane_id)
    ClearClawHistory(Option<String>),       // (pane_id)
    ResumeClawSession(String, Option<String>), // (session_id, override_character_id)
    ShowDetachedSessions,
    RefreshDetachedSessions,
    UpdateDetachedSessionsBadge(Vec<boxxy_terminal::DetachedSessionInfo>),
    ReattachDetachedSession(u32, Option<String>),
    ReattachDetachedBatch(Vec<boxxy_terminal::DetachedSessionInfo>),
    SpawnCharacterTab(String, Option<String>),
    RefreshMaintenanceStatus(String, bool),
    RefreshMcpStatuses(Vec<(String, bool)>),
    WindowAnimationStateChanged,
    PrewarmDeferredUi,
}

pub struct AppWindowInner {
    pub window: adw::ApplicationWindow,
    pub tabs: Vec<TerminalController>,
    pub tab_view: adw::TabView,
    pub tab_bar: crate::widgets::tab_bar::BoxxyTabBar,
    pub content_header: adw::HeaderBar,
    pub split_view: adw::OverlaySplitView,
    pub sidebar_overlay: gtk::Overlay,
    pub sidebar_toolbar: adw::ToolbarView,
    pub detached_btn: gtk::Button,
    pub detached_count_lbl: gtk::Label,
    pub view_stack: adw::ViewStack,
    pub sidebar_visible: bool,
    pub preferences: Option<PreferencesComponent>,
    pub tab_menu: Option<TabContextMenu>,
    pub claw: ClawSidebarComponent,
    pub bookmarks_sidebar: BookmarksSidebarComponent,
    pub bookmarks_controller: Option<BookmarksTabComponent>,
    pub bookmarks_page: Option<adw::TabPage>,
    pub theme_selector: boxxy_themes::ThemeSelectorComponent,
    pub command_palette: CommandPaletteComponent,
    pub current_settings: Settings,
    pub app_state: AppState,
    pub claw_active: bool,
    pub toast_overlay: adw::ToastOverlay,
    pub frosted_container: crate::widgets::frosted_glass::FrostedGlassContainer,
    pub active_update_notification: Option<Notification>,
    pub last_notified_broken_default: Option<String>,
    pub initial_launch: Option<TabLaunchSpec>,
    pub force_close: Rc<Cell<bool>>,
    pub tx: async_channel::Sender<AppInput>,
}

impl AppWindowInner {
    /// Returns the `TerminalController` for the currently selected `AdwTabView` page,
    /// or `None` if no page is selected or the selected page is a non-terminal tab (e.g., Bookmarks).
    pub fn active_tab(&self) -> Option<&TerminalController> {
        let page = self.tab_view.selected_page()?;
        let child = page.child();
        self.tabs.iter().find(|c| c.controller.widget() == &child)
    }

    pub fn active_tab_pos(&self) -> Option<usize> {
        let page = self.tab_view.selected_page()?;
        let child = page.child();
        self.tabs
            .iter()
            .position(|c| c.controller.widget() == &child)
    }

    pub fn tab_by_widget_key(&self, key: usize) -> Option<&TerminalController> {
        self.tabs
            .iter()
            .find(|t| t.controller.widget().as_ptr() as usize == key)
    }

    pub fn tab_pos_by_widget_key(&self, key: usize) -> Option<usize> {
        self.tabs
            .iter()
            .position(|t| t.controller.widget().as_ptr() as usize == key)
    }

    pub fn ensure_preferences(&mut self) -> &PreferencesComponent {
        if self.preferences.is_none() {
            let tx_themes_nav = self.tx.clone();
            let tx_reload = self.tx.clone();
            // All preference mutations flow through `Settings::update` -> `SETTINGS_EVENT_BUS`
            // (subscribed in `ui.rs`), avoiding duplicate `SettingsChanged` dispatches.
            let preferences = PreferencesComponent::new(
                move || {
                    let _ = tx_themes_nav.send_blocking(AppInput::ShowThemesSidebar);
                },
                move || {
                    let _ = tx_reload.send_blocking(AppInput::ReloadEngine);
                },
            );
            let tx_pref_close = self.tx.clone();
            use libadwaita::prelude::AdwDialogExt;
            preferences.widget().connect_closed(move |_| {
                let _ = tx_pref_close.send_blocking(AppInput::FocusActiveTerminal);
            });
            preferences.sync_settings(&self.current_settings);
            self.preferences = Some(preferences);
        }
        self.preferences.as_ref().unwrap()
    }

    pub fn is_dialog_or_popover_open(win: &adw::ApplicationWindow) -> bool {
        use libadwaita::glib::types::StaticType;
        use libadwaita::prelude::AdwApplicationWindowExt;

        win.visible_dialog().is_some()
            || gtk::prelude::GtkWindowExt::focus(win)
                .and_then(|w| w.ancestor(gtk::Popover::static_type()))
                .is_some()
    }

    pub fn has_modal_or_popover_open(&self) -> bool {
        self.command_palette.widget().is_visible()
            || self.claw.is_options_popover_open()
            || Self::is_dialog_or_popover_open(&self.window)
    }
}
