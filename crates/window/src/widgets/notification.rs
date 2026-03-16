use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
    Update,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationAction {
    pub label: String,
    pub action_name: String, // Full action name like "win.apply-update"
    pub is_primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub level: NotificationLevel,
    pub title: String,
    pub message: String,
    pub icon_name: String,
    pub actions: Vec<NotificationAction>,
    pub details: Vec<(String, String)>, // Key-Value pairs for the popover (e.g. "Version": "v1.2")
}

impl Notification {
    pub fn new_update(version: &str, url: &str) -> Self {
        Self {
            id: "update-available".to_string(),
            level: NotificationLevel::Update,
            title: "Update Available".to_string(),
            message: format!("Version {} is available.", version),
            icon_name: "software-update-available-symbolic".to_string(),
            actions: vec![
                NotificationAction {
                    label: "Download".to_string(),
                    action_name: "win.start-download".to_string(),
                    is_primary: true,
                },
                NotificationAction {
                    label: "Later".to_string(),
                    action_name: "win.dismiss-notification".to_string(),
                    is_primary: false,
                },
            ],
            details: vec![
                ("Version".to_string(), version.to_string()),
                ("Source".to_string(), "github/nightly".to_string()),
                ("Url".to_string(), url.to_string()), // Keep url for logic but hide in loop if needed
            ],
        }
    }

    pub fn new_update_ready(version: &str) -> Self {
        Self {
            id: "update-ready".to_string(),
            level: NotificationLevel::Success,
            title: "Update Ready".to_string(),
            message: "The update has been downloaded and is ready to install.".to_string(),
            icon_name: "software-update-available-symbolic".to_string(),
            actions: vec![
                NotificationAction {
                    label: "Restart to Update".to_string(),
                    action_name: "win.apply-update".to_string(),
                    is_primary: true,
                },
                NotificationAction {
                    label: "Later".to_string(),
                    action_name: "win.dismiss-notification".to_string(),
                    is_primary: false,
                },
            ],
            details: vec![
                ("Version".to_string(), version.to_string()),
            ],
        }
    }
}
