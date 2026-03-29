use crate::app_menu::AppMenuComponent;
use boxxy_bookmarks::{sidebar::BookmarksSidebarComponent, tab::BookmarksTabComponent};
use boxxy_claw::ClawSidebarComponent;
use boxxy_command_palette::CommandPaletteComponent;
use boxxy_preferences::{AppState, PreferencesComponent, Settings};
use boxxy_sidebar::AiSidebarComponent;
use boxxy_terminal::TerminalEvent;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;

use crate::init::TerminalController;
use crate::widgets::notification::Notification;

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
    OpenBookmarks,
    OpenShortcuts,
    OpenAbout,
    OpenInFiles,
    ShowAppMenu(f64, f64),
    ShowCommandPaletteMenu,
    SettingsChanged(Settings),
    ShowThemesSidebar,
    ShowAiChat,
    ShowClawSidebar,
    ShowBookmarksSidebar,
    ExecuteBookmark(String, String, String), // Name, Filename, Script
    ExecuteInNewTab(String, String, String), // Name, Filename, Script
    SetClawActive(bool, Option<String>),
    SetClawActiveGlobal(bool),
    SetClawProactive(bool, Option<String>),
    SetClawProactiveGlobal(bool),
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
    PushNotification(Notification),
    DismissNotification(String),
    StartUpdateDownload(String, String, Option<String>), // (url, date, checksum_url)
    UpdateDownloaded(String),
    ApplyUpdateAndRestart,
    GrabFocus,
    CancelTask(uuid::Uuid, Option<String>), // (task_id, pane_id)
}

pub struct AppWindowInner {
    pub window: adw::ApplicationWindow,
    pub tabs: Vec<TerminalController>,
    pub tab_view: adw::TabView,
    pub tab_bar: adw::TabBar,
    pub content_header: adw::HeaderBar,
    pub split_view: adw::OverlaySplitView,
    pub sidebar_toolbar: adw::ToolbarView,
    pub menu_btn: gtk::Button,
    pub view_stack: adw::ViewStack,
    pub next_id: usize,
    pub sidebar_visible: bool,
    pub preferences: PreferencesComponent,
    pub app_menu: AppMenuComponent,
    pub ai_chat: AiSidebarComponent,
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
    pub claw_proactive: bool,
    pub toast_overlay: adw::ToastOverlay,
    pub notifications: Vec<Notification>,
    pub initial_working_dir: Option<String>,
    pub force_close: Rc<Cell<bool>>,
    pub tx: async_channel::Sender<AppInput>,
}
