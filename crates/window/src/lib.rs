pub mod actions;
pub mod agent_deployer;
pub mod app_menu;
pub mod application_detector;
pub mod init;
pub mod sound;
pub mod state;
pub mod tab_menu;
pub mod tab_title;
pub mod ui;
pub mod update;
pub mod updater;
pub mod widgets;

pub use init::{AppInit, TabLaunchSpec};
pub use ui::AppWindow;
