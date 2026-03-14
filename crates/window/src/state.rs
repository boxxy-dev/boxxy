use boxxy_about::AboutComponent;
use boxxy_sidebar::AiSidebarComponent;
use boxxy_app_menu::AppMenuComponent;
use boxxy_apps::BoxxyAppsComponent;
use boxxy_claw::ClawSidebarComponent;
use boxxy_command_palette::CommandPaletteComponent;
use boxxy_preferences::{AppState, PreferencesComponent, Settings};
use boxxy_shortcuts::ShortcutsComponent;
use boxxy_terminal::TerminalEvent;
use std::cell::Cell;
use std::rc::Rc;
use gtk4 as gtk;
use libadwaita as adw;

use crate::init::TerminalController;

#[derive(Debug, Clone)]
pub enum AppInput {
    NewWindow,
    NewTab,
    CloseTabRequest(usize),
    MoveTabToNewWindowRequest(usize),
    CloseTab(String),
    CloseActiveTab,
    HandleTerminalEvent(Option<TerminalEvent>),
    AdoptOrphanTabs,
    ToggleSidebar,
    SidebarVisibleChanged(bool),
    SidebarPageChanged(String),
    OpenPreferences,
    OpenBoxxyApps,
    OpenShortcuts,
    OpenAbout,
    OpenInFiles,
    ShowAppMenu(f64, f64),
    ShowCommandPaletteMenu,
    SettingsChanged(Settings),
    ShowThemesSidebar,
    ShowAiChat,
    ShowClawSidebar,
    SetClawActive(bool),
    ModelSelection,
    ThemeSelected(Box<boxxy_themes::ParsedPaletteStatic>),
    CommandPalette,
    ReloadEngine,
    ZoomIn,
    ZoomOut,
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
}

pub struct AppWindowInner {
    pub window: adw::ApplicationWindow,
    pub tabs: Vec<TerminalController>,
    pub boxxy_apps_controller: Option<BoxxyAppsComponent>,
    pub boxxy_apps_page: Option<adw::TabPage>,
    pub tab_view: adw::TabView,
    pub tab_bar: adw::TabBar,
    pub single_tab_title: adw::WindowTitle,
    pub header_title_stack: gtk::Stack,
    pub content_header: adw::HeaderBar,
    pub _split_view: adw::OverlaySplitView,
    pub sidebar_toolbar: adw::ToolbarView,
    pub menu_btn: gtk::Button,
    pub view_stack: adw::ViewStack,
    pub next_id: usize,
    pub sidebar_visible: bool,
    pub preferences: PreferencesComponent,
    pub app_menu: AppMenuComponent,
    pub ai_chat: AiSidebarComponent,
    pub claw: ClawSidebarComponent,
    pub theme_selector: boxxy_themes::ThemeSelectorComponent,
    pub shortcuts: ShortcutsComponent,
    pub about: AboutComponent,
    pub command_palette: CommandPaletteComponent,
    pub current_settings: Settings,
    pub app_state: AppState,
    pub bell_indicator: gtk::Image,
    pub claw_indicator: gtk::Button,
    pub claw_popover: crate::boxxyclaw_indicator_popover::BoxxyclawIndicatorPopover,
    pub initial_working_dir: Option<String>,
    pub force_close: Rc<Cell<bool>>,
    pub tx: async_channel::Sender<AppInput>,
}
