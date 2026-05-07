pub mod about;
pub mod advanced;
pub mod toolbox;
pub mod apis;
pub mod appearance;
pub mod characters;
pub mod claw_ui;
pub mod config;
pub mod mcp;
pub mod previews;
pub mod shortcuts;

pub mod component;
pub use component::PreferencesComponent;
pub use config::{AppState, CursorShape, ImagePreviewTrigger, SETTINGS_EVENT_BUS, Settings};
