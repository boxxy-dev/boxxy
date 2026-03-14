use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, OnceLock, RwLock};
use tokio::sync::broadcast;
use boxxy_model_selection::ModelProvider;

pub static SETTINGS_EVENT_BUS: LazyLock<broadcast::Sender<Settings>> = LazyLock::new(|| {
    let (tx, _) = broadcast::channel(16);
    tx
});

static SETTINGS_CACHE: OnceLock<RwLock<Settings>> = OnceLock::new();
static APP_STATE_CACHE: OnceLock<RwLock<AppState>> = OnceLock::new();

// --- Cursor Shape ---
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    #[default]
    Block,
    IBeam,
    Underline,
}

impl fmt::Display for CursorShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CursorShape::Block => write!(f, "Block"),
            CursorShape::IBeam => write!(f, "I-Beam"),
            CursorShape::Underline => write!(f, "Underline"),
        }
    }
}

// --- Image Preview Trigger ---
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImagePreviewTrigger {
    None,
    #[default]
    Click,
    Hover,
}

impl fmt::Display for ImagePreviewTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImagePreviewTrigger::None => write!(f, "Disabled"),
            ImagePreviewTrigger::Click => write!(f, "On Click (Shift+Click)"),
            ImagePreviewTrigger::Hover => write!(f, "On Hover"),
        }
    }
}

// --- Claw Auto-Diagnosis Mode ---
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ClawAutoDiagnosisMode {
    #[default]
    Proactive,
    Lazy,
}

impl fmt::Display for ClawAutoDiagnosisMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClawAutoDiagnosisMode::Proactive => write!(f, "Proactive (Background Analysis)"),
            ClawAutoDiagnosisMode::Lazy => write!(f, "Lazy (On Demand)"),
        }
    }
}

pub const DEFAULT_FILE_REGEX: &str = r#"(?:https?://[^\s"'<>]+|/[\w.@:/-]+|~[\w.@:/-]+|\.{1,2}/[\w.@:/-]+)"#;

// --- User Configurable Settings ---
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub font_name: String,
    pub font_size: u16,
    pub theme: String,
    pub padding: i32,
    pub ai_chat_width: i32,
    pub preserve_working_dir: bool,
    pub cell_height_scale: f64,
    pub cell_width_scale: f64,
    pub cursor_shape: CursorShape,
    pub cursor_color_override: bool,
    pub cursor_color: String,
    pub cursor_blinking: bool,
    pub hide_scrollbars: bool,
    pub dim_inactive_panes: bool,
    pub always_show_tabs: bool,
    pub fixed_width_tabs: bool,
    pub gemini_api_key: String,
    pub ollama_base_url: String,
    pub login_shell: bool,
    pub show_vte_grid: bool,
    pub custom_regex: String,
    pub image_preview_trigger: ImagePreviewTrigger,
    pub preview_max_width: i32,
    pub preview_max_height: i32,
    pub ai_chat_model: ModelProvider,
    pub claw_model: ModelProvider,
    pub memory_model: Option<ModelProvider>,
    pub invert_scroll: bool,
    pub claw_auto_diagnosis_mode: ClawAutoDiagnosisMode,
    pub claw_terminal_suggestions: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_name: "Monospace".to_string(),
            font_size: 12,
            theme: "Adwaita Dark".to_string(),
            padding: 10,
            ai_chat_width: 400,
            preserve_working_dir: false,
            cell_height_scale: 1.0,
            cell_width_scale: 1.0,
            cursor_shape: CursorShape::Block,
            cursor_color_override: false,
            cursor_color: "rgb(255,255,255)".to_string(),
            cursor_blinking: true,
            hide_scrollbars: false,
            dim_inactive_panes: false,
            always_show_tabs: false,
            fixed_width_tabs: false,
            gemini_api_key: String::new(),
            ollama_base_url: "http://localhost:11434".to_string(),
            login_shell: true,
            show_vte_grid: false,
            custom_regex: DEFAULT_FILE_REGEX.to_string(),
            image_preview_trigger: ImagePreviewTrigger::Click,
            preview_max_width: 300,
            preview_max_height: 200,
            ai_chat_model: ModelProvider::default(),
            claw_model: ModelProvider::default(),
            memory_model: None,
            invert_scroll: true,
            claw_auto_diagnosis_mode: ClawAutoDiagnosisMode::Proactive,
            claw_terminal_suggestions: true,
        }
    }
}

impl Settings {
    fn get_path() -> Option<PathBuf> {
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let config_dir = dirs.config_dir();
            if !config_dir.exists() {
                fs::create_dir_all(config_dir).ok()?;
            }
            return Some(config_dir.join("settings.json"));
        }
        None
    }

    pub fn ensure_claw_skills() {
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let config_dir = dirs.config_dir();
            let boxxyclaw_dir = config_dir.join("boxxyclaw");
            
            if !boxxyclaw_dir.exists() {
                let _ = fs::create_dir_all(&boxxyclaw_dir);
            }
            
            // Generate BLACKLIST.md
            let blacklist_md = boxxyclaw_dir.join("BLACKLIST.md");
            if !blacklist_md.exists() {
                let blacklist_content = "# Sensitive Path Blacklist\n\n\
This file defines paths that Boxxy-Claw is strictly forbidden from reading or modifying.\n\
The agent will respect this list for all file operations (reading config files, writing, etc.).\n\
You can add your own sensitive paths here. One path per line. Lines starting with `#` are ignored.\n\n\
/etc/shadow\n\
/etc/gshadow\n\
/root/.ssh\n\
.ssh/id_rsa\n\
.ssh/id_ed25519\n";
                let _ = fs::write(blacklist_md, blacklist_content);
            }

            // Generate Default Character: Boxxy
            let characters_dir = boxxyclaw_dir.join("characters");
            let boxxy_dir = characters_dir.join("boxxy");
            if !boxxy_dir.exists() && fs::create_dir_all(&boxxy_dir).is_ok() {
                let boxxy_md = boxxy_dir.join("BOXXY.md");
                if !boxxy_md.exists() {
                    let boxxy_content = "# Boxxy\n\n\
Name: Boxxy\n\
Role: Expert Linux System Administrator & Terminal Assistant\n\n\
Personality:\n\
Boxxy is a nice, friendly, and energetic AI assistant. Despite her bubbly attitude, she is technically \
sharp and provides extremely accurate and efficient Linux advice. She values your security and \
loves to help you master the terminal.\n\n\
Instructions:\n\
- Be concise and technically precise.\n\
- Use markdown for all responses.\n\
- Provide executable bash blocks for fixes.\n\
- Keep the tone friendly and encouraging.\n";
                    let _ = fs::write(boxxy_md, boxxy_content);
                }
            }

            let skills_dir = boxxyclaw_dir.join("skills");
            if !skills_dir.exists() {
                let _ = fs::create_dir_all(&skills_dir);
            }

            let linux_system_dir = skills_dir.join("linux-system");
            if !linux_system_dir.exists() {
                let _ = fs::create_dir_all(&linux_system_dir);
            }

            let linux_system_md = linux_system_dir.join("SKILL.md");
            if !linux_system_md.exists() {
                let content = "---\n\
name: linux-system\n\
description: Information about the user's Linux system and preferences. Use when interacting with packages or system administration.\n\
triggers:\n\
  - update\n\
  - install\n\
  - package\n\
  - system\n\
  - os\n\
  - linux\n\
  - distro\n\
---\n\
# Linux System Skill\n\n\
This file provides Boxxy-Claw with specific information about your system to help it operate more effectively. \
You can modify this file to include details such as:\n\n\
- **Distribution**: Which Linux distro you are using (e.g., Ubuntu 24.04, Fedora 40, Arch Linux).\n\
- **Desktop Environment**: The DE or Window Manager in use (e.g., GNOME 46, KDE Plasma, Sway).\n\
- **Package Managers**: Preferred tools (e.g., `apt`, `dnf`, `pacman`, `flatpak`).\n\
- **Additional Tools**: Any specific system administration or developer tools you want Claw to utilize.\n\
- **System Quirks**: Any non-standard configurations or paths that Claw should be aware of.\n\n\
Providing this context allows Boxxy-Claw to tailor its commands and diagnostics to your exact environment.\n";
                let _ = fs::write(linux_system_md, content);
            }
        }
    }

    pub fn init() {
        let _ = SETTINGS_CACHE.get_or_init(|| {
            let mut settings = Self::default();
            if let Some(path) = Self::get_path()
                && let Ok(content) = fs::read_to_string(path) {
                    match serde_json::from_str::<Settings>(&content) {
                        Ok(s) => settings = s,
                        Err(e) => log::error!("Failed to load settings: {}", e),
                    }
                }
            RwLock::new(settings)
        });
    }

    pub fn load() -> Self {
        if let Some(cache) = SETTINGS_CACHE.get() {
            return cache.read().unwrap().clone();
        }

        // Fallback for tests or uninitialized state
        Self::default()
    }

    pub fn save(&self) {
        if let Some(cache) = SETTINGS_CACHE.get() {
            *cache.write().unwrap() = self.clone();
        }

        if let Some(path) = Self::get_path()
            && let Ok(content) = serde_json::to_string_pretty(self) {
                let _ = fs::write(path, content);
                let _ = SETTINGS_EVENT_BUS.send(self.clone());
            }
    }}

// --- Internal App State ---
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppState {
    pub sidebar_visible: bool,
    pub active_sidebar_page: String,
    pub window_width: i32,
    pub window_height: i32,
    pub is_maximized: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            sidebar_visible: false,
            active_sidebar_page: "assistant".to_string(),
            window_width: 900,
            window_height: 600,
            is_maximized: false,
        }
    }
}

impl AppState {
    fn get_path() -> Option<PathBuf> {
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let config_dir = dirs.config_dir();
            if !config_dir.exists() {
                fs::create_dir_all(config_dir).ok()?;
            }
            return Some(config_dir.join("state.json"));
        }
        None
    }

    pub fn init() {
        let _ = APP_STATE_CACHE.get_or_init(|| {
            let mut state = Self::default();
            if let Some(path) = Self::get_path()
                && let Ok(content) = fs::read_to_string(path)
                    && let Ok(s) = serde_json::from_str::<AppState>(&content) {
                        state = s;
                    }
            RwLock::new(state)
        });
    }

    pub fn load() -> Self {
        if let Some(cache) = APP_STATE_CACHE.get() {
            return cache.read().unwrap().clone();
        }
        Self::default()
    }

    pub fn save(&self) {
        if let Some(cache) = APP_STATE_CACHE.get() {
            *cache.write().unwrap() = self.clone();
        }
        if let Some(path) = Self::get_path()
            && let Ok(content) = serde_json::to_string_pretty(self) {
                let _ = fs::write(path, content);
            }
    }
}

