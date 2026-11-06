pub mod about;
pub mod advanced;
pub mod apis;
pub mod appearance;
pub mod characters;
pub mod claw_ui;
pub mod config;
pub mod integrations;
pub mod mcp;
pub mod previews;
pub mod shortcuts;
pub mod skills;
pub mod themes_page;
pub mod toolbox;
pub mod user_profile;

pub mod component;
pub use component::PreferencesComponent;
pub use config::{
    AppState, AppThemeMode, CursorShape, ImagePreviewTrigger, SETTINGS_EVENT_BUS, Settings,
};
