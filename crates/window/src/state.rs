use crate::app_menu::AppMenuComponent;
use boxxy_bookmarks::{sidebar::BookmarksSidebarComponent, tab::BookmarksTabComponent};
use boxxy_claw_ui::ClawSidebarComponent;
use boxxy_command_palette::CommandPaletteComponent;
use boxxy_preferences::{AppState, PreferencesComponent, Settings};
use boxxy_terminal::TerminalEvent;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

use crate::init::TerminalController;
use crate::widgets::notification::Notification;

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
    pub fn persistent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            timeout_secs: 0,
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
    EnsureInitialTab,
    NewTabInDir(Option<String>),
    CloseTabRequest(usize),
    MoveTabToNewWindowRequest(usize),
    SetTabColor(usize, TabColor),
    SetTabTitle(usize, Option<String>),
    SyncTabColors,
    CloseTab(String),
    CloseActiveTab,
    HandleTerminalEvent(Option<TerminalEvent>),
    AdoptOrphanTabs,
    ToggleSidebar,
    SidebarVisibleChanged(bool),
    SidebarPageChanged(String),
    OpenPreferences,
    OpenApisPreferences,
    OpenMcpPreferences,
    OpenBookmarks,
    OpenShortcuts,
    OpenWhatsNew,
    OpenAbout,
    OpenInFiles,
    ShowAppMenu(f64, f64),
    ShowCommandPaletteMenu,
    SettingsChanged(Settings),
    ShowThemesSidebar,
    ShowClawSidebar,
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
    Copy,
    Paste,
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
    PushGlobalNotification(Notification),
    ShowToast(ToastRequest),
    DismissNotification(String),
    StartUpdateDownload(String, String, String, Option<String>), // (url, date, new_commit, checksum_url)
    UpdateDownloaded(String),
    ApplyUpdateAndRestart,
    ShowWhatsNewDialog(
        crate::updater::whats_new::WhatsNewPending,
        Option<crate::updater::whats_new::ChangelogSummary>,
    ),
    StartWhatsNewClaw(String),
    GrabFocus,
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
    RefreshMaintenanceStatus(String, bool),
    RefreshMcpStatuses(Vec<(String, bool)>),
    PrewarmDeferredUi,
}

pub struct AppWindowInner {
    pub window: adw::ApplicationWindow,
    pub tabs: Vec<TerminalController>,
    pub tab_view: adw::TabView,
    pub tab_bar: adw::TabBar,
    pub content_header: adw::HeaderBar,
    pub split_view: adw::OverlaySplitView,
    pub sidebar_overlay: gtk::Overlay,
    pub sidebar_toolbar: adw::ToolbarView,
    pub menu_btn: gtk::Button,
    pub detached_btn: gtk::Button,
    pub detached_count_lbl: gtk::Label,
    pub view_stack: adw::ViewStack,
    pub next_id: usize,
    pub sidebar_visible: bool,
    pub preferences: Option<PreferencesComponent>,
    pub app_menu: AppMenuComponent,
    pub claw: ClawSidebarComponent,
    pub bookmarks_sidebar: BookmarksSidebarComponent,
    pub bookmarks_controller: Option<BookmarksTabComponent>,
    pub bookmarks_page: Option<adw::TabPage>,
    pub theme_selector: boxxy_themes::ThemeSelectorComponent,
    pub command_palette: CommandPaletteComponent,
    pub current_settings: Settings,
    pub app_state: AppState,
    pub bell_indicator: gtk::Image,
    pub claw_active: bool,
    pub toast_overlay: adw::ToastOverlay,
    pub frosted_container: crate::widgets::frosted_glass::FrostedGlassContainer,
    pub notifications: Vec<Notification>,
    pub initial_working_dir: Option<String>,
    pub force_close: Rc<Cell<bool>>,
    pub tx: async_channel::Sender<AppInput>,
}

impl AppWindowInner {
    pub fn ensure_preferences(&mut self) -> &PreferencesComponent {
        if self.preferences.is_none() {
            let tx_pref = self.tx.clone();
            let tx_themes_nav = self.tx.clone();
            let tx_reload = self.tx.clone();
            let preferences = PreferencesComponent::new(
                move |settings| {
                    let _ = tx_pref.send_blocking(AppInput::SettingsChanged(settings));
                },
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
        use gtk::prelude::{GtkWindowExt, WidgetExt};
        use libadwaita::glib::types::StaticType;
        use libadwaita::prelude::AdwApplicationWindowExt;

        win.visible_dialog().is_some()
            || win
                .focus()
                .and_then(|w| w.ancestor(gtk::Popover::static_type()))
                .is_some()
    }

    pub fn has_modal_or_popover_open(&self) -> bool {
        self.claw.is_options_popover_open() || Self::is_dialog_or_popover_open(&self.window)
    }
}
