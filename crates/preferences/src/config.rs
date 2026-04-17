use boxxy_model_selection::ModelProvider;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::{LazyLock, OnceLock, RwLock};
use tokio::sync::broadcast;

pub static SETTINGS_EVENT_BUS: LazyLock<broadcast::Sender<Settings>> = LazyLock::new(|| {
    let (tx, _) = broadcast::channel(16);
    tx
});

static SETTINGS_CACHE: OnceLock<RwLock<Settings>> = OnceLock::new();
static APP_STATE_CACHE: OnceLock<RwLock<AppState>> = OnceLock::new();
static ENV_API_KEYS: LazyLock<RwLock<std::collections::HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(std::collections::HashMap::new()));

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

// --- Color Scheme ---
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ColorScheme {
    #[default]
    Default,
    Light,
    Dark,
}

impl fmt::Display for ColorScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ColorScheme::Default => write!(f, "Follow System"),
            ColorScheme::Light => write!(f, "Light"),
            ColorScheme::Dark => write!(f, "Dark"),
        }
    }
}

pub const DEFAULT_FILE_REGEX: &str =
    r#"(?:https?://[^\s"'<>]+|/[\w.@:/-]+|~[\w.@:/-]+|\.{1,2}/[\w.@:/-]+)"#;

#[derive(Debug, Clone, Copy)]
pub struct DimensionBounds {
    pub min: i32,
    pub max: i32,
    pub default: i32,
}

pub const CLAW_WIDTH_BOUNDS: DimensionBounds = DimensionBounds {
    min: 400,
    max: 1600,
    default: 600,
};

pub const CLAW_HEIGHT_BOUNDS: DimensionBounds = DimensionBounds {
    min: 400,
    max: 1600,
    default: 900,
};

// --- User Configurable Settings ---
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub font_name: String,
    pub font_size: u16,
    pub theme: String,
    pub color_scheme: ColorScheme,
    pub opacity: f64,
    pub background_image_path: Option<String>,
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
    pub fixed_width_tabs: bool,
    pub api_keys: std::collections::HashMap<String, String>,
    pub ollama_base_url: String,
    pub login_shell: bool,
    pub show_vte_grid: bool,
    pub custom_regex: String,
    pub image_preview_trigger: ImagePreviewTrigger,
    pub preview_max_width: i32,
    pub preview_max_height: i32,
    pub ai_chat_model: Option<ModelProvider>,
    pub claw_model: Option<ModelProvider>,
    pub memory_model: Option<ModelProvider>,
    pub enable_auto_dreaming: bool,
    pub claw_on_by_default: bool,
    pub invert_scroll: bool,

    pub hide_agent_badge: bool,
    pub web_search_on_by_default: bool,
    pub enable_file_tools: bool,
    pub enable_system_tools: bool,
    pub enable_dangerous_tools: bool,
    pub enable_web_tools: bool,
    pub enable_clipboard_tools: bool,
    pub enable_web_search: bool,
    pub enable_os_context: bool,
    pub enable_progress_bar: bool,
    pub colored_tabs: bool,
    pub claw_popover_width: i32,
    pub claw_popover_max_height: i32,
    pub claw_msgbar_shortcut: String,
    pub enable_telemetry: bool,
    pub install_id: Option<String>,
    pub mcp_servers: Vec<boxxy_mcp::config::McpServerConfig>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_name: "Monospace".to_string(),
            font_size: 12,
            theme: "Adwaita Dark".to_string(),
            color_scheme: ColorScheme::Default,
            opacity: 1.0,
            background_image_path: None,
            padding: 10,
            ai_chat_width: 400,
            preserve_working_dir: true,
            cell_height_scale: 1.0,
            cell_width_scale: 1.0,
            cursor_shape: CursorShape::Block,
            cursor_color_override: false,
            cursor_color: "rgb(255,255,255)".to_string(),
            cursor_blinking: true,
            hide_scrollbars: true,
            dim_inactive_panes: true,
            fixed_width_tabs: true,
            api_keys: std::collections::HashMap::new(),
            ollama_base_url: "http://localhost:11434".to_string(),
            login_shell: true,
            show_vte_grid: false,
            custom_regex: DEFAULT_FILE_REGEX.to_string(),
            image_preview_trigger: ImagePreviewTrigger::Click,
            preview_max_width: 300,
            preview_max_height: 200,
            ai_chat_model: None,
            claw_model: None,
            memory_model: None,
            enable_auto_dreaming: true,
            invert_scroll: true,
            hide_agent_badge: false,
            claw_on_by_default: false,
            web_search_on_by_default: true,
            enable_file_tools: true,
            enable_system_tools: true,
            enable_dangerous_tools: true,
            enable_web_tools: true,
            enable_clipboard_tools: true,
            enable_web_search: true,
            enable_os_context: true,
            enable_progress_bar: true,
            colored_tabs: false,
            claw_popover_width: CLAW_WIDTH_BOUNDS.default,
            claw_popover_max_height: CLAW_HEIGHT_BOUNDS.default,
            claw_msgbar_shortcut: "<Ctrl>slash".to_string(),
            enable_telemetry: true, // ON by default during Preview
            install_id: None,
            mcp_servers: vec![],
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
        }
    }

    pub fn init() {
        let _ = SETTINGS_CACHE.get_or_init(|| {
            let mut settings = Self::default();
            if let Some(path) = Self::get_path()
                && let Ok(content) = fs::read_to_string(path)
            {
                match serde_json::from_str::<Settings>(&content) {
                    Ok(s) => settings = s,
                    Err(e) => log::error!("Failed to load settings: {}", e),
                }
            }

            // Generate install_id if missing
            if settings.install_id.is_none() {
                settings.install_id = Some(uuid::Uuid::new_v4().to_string());
                settings.save();
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
            && let Ok(content) = serde_json::to_string_pretty(self)
        {
            let _ = fs::write(path, content);
            let _ = SETTINGS_EVENT_BUS.send(self.clone());
        }
    }

    pub fn set_env_api_key(provider: &str, key: String) {
        ENV_API_KEYS
            .write()
            .unwrap()
            .insert(provider.to_string(), key);
    }

    pub fn get_env_api_key(provider: &str) -> Option<String> {
        ENV_API_KEYS.read().unwrap().get(provider).cloned()
    }

    pub fn get_effective_api_key(&self, provider: &str) -> String {
        if let Some(key) = self.api_keys.get(provider)
            && !key.is_empty()
        {
            return key.clone();
        }
        Self::get_env_api_key(provider).unwrap_or_default()
    }

    pub fn get_effective_api_keys(&self) -> std::collections::HashMap<String, String> {
        let mut keys = self.api_keys.clone();
        let env_keys = ENV_API_KEYS.read().unwrap();
        for (provider, key) in env_keys.iter() {
            if !keys.contains_key(provider)
                || keys.get(provider).map(|k| k.is_empty()).unwrap_or(true)
            {
                keys.insert(provider.clone(), key.clone());
            }
        }
        keys
    }
}

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
            window_width: 1200,
            window_height: 800,
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
                && let Ok(s) = serde_json::from_str::<AppState>(&content)
            {
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
            && let Ok(content) = serde_json::to_string_pretty(self)
        {
            let _ = fs::write(path, content);
        }
    }
}
