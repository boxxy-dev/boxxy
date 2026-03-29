pub mod about;
pub mod advanced;
pub mod agents;
pub mod apis;
pub mod appearance;
pub mod claw_ui;
pub mod config;
pub mod previews;
pub mod shortcuts;

pub mod component;
pub use component::PreferencesComponent;
pub use config::{
    AppState, ClawAutoDiagnosisMode, CursorShape, ImagePreviewTrigger, SETTINGS_EVENT_BUS, Settings,
    CLAW_HEIGHT_BOUNDS, CLAW_WIDTH_BOUNDS,
};
