use arc_swap::ArcSwap;
use boxxy_model_selection::ModelProvider;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, RwLock};
use tokio::sync::broadcast;

pub static SETTINGS_EVENT_BUS: LazyLock<broadcast::Sender<Settings>> = LazyLock::new(|| {
    let (tx, _) = broadcast::channel(16);
    tx
});

static SETTINGS_INITIALIZED: AtomicBool = AtomicBool::new(false);
#[cfg(not(test))]
static SETTINGS_GENERATION: AtomicU64 = AtomicU64::new(0);
static SETTINGS_CACHE: LazyLock<ArcSwap<Settings>> =
    LazyLock::new(|| ArcSwap::from_pointee(Settings::default()));
static APP_STATE_CACHE: LazyLock<ArcSwap<AppState>> =
    LazyLock::new(|| ArcSwap::from_pointee(AppState::default()));
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

// --- User Profile ---
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct UserProfile {
    /// User's display name. Empty string = not set (show placeholder "You").
    #[serde(default)]
    pub display_name: String,
    /// Hex color override (e.g. "#a1b2c3"). None uses the built-in default.
    #[serde(default)]
    pub color: Option<String>,
}

// --- User Configurable Settings ---
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub font_name: String,
    pub font_size: u16,
    pub theme: String,
    pub color_scheme: ColorScheme,
    pub opacity: f64,
    pub frosted_glass: bool,
    pub background_image_path: Option<String>,
    pub padding: i32,
    pub ai_chat_width: i32,
    pub preserve_working_dir: bool,
    pub use_custom_command: bool,
    pub custom_command: String,
    pub cell_height_scale: f64,
    pub cell_width_scale: f64,
    pub user_profile: UserProfile,
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
    /// When true, closing a pane detaches its shell into the daemon's
    /// background instead of killing it. The shell keeps running and
    /// its output is buffered into a 4 MB ring buffer so a future
    /// session can reattach and see the tail. Default `false`.
    #[serde(default)]
    pub pty_persistence: bool,
    pub show_vte_grid: bool,
    pub custom_regex: String,
    pub image_preview_trigger: ImagePreviewTrigger,
    pub preview_max_width: i32,
    pub preview_max_height: i32,
    pub ai_chat_model: Option<ModelProvider>,
    pub claw_model: Option<ModelProvider>,
    pub memory_model: Option<ModelProvider>,
    pub enable_auto_dreaming: bool,
    pub invert_scroll: bool,

    pub hide_character_badge: bool,
    pub web_search_on_by_default: bool,
    pub enable_file_tools: bool,
    pub enable_system_tools: bool,
    pub enable_dangerous_tools: bool,
    pub enable_web_tools: bool,
    pub enable_clipboard_tools: bool,
    pub enable_web_search: bool,
    pub enable_os_context: bool,
    pub enable_progress_bar: bool,
    pub enable_timer_sounds: bool,
    pub enable_task_sounds: bool,
    pub colored_tabs: bool,
    /// When true, the in-terminal Claw overlay keeps a scrollable history
    /// of messages (mirroring the sidebar logger). When false, each new
    /// message replaces the previous one in the overlay. Off by default
    /// because the overlay normally shows only the latest turn.
    pub maintain_overlay_history: bool,
    pub enable_tips: bool,
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
            frosted_glass: false,
            background_image_path: None,
            padding: 10,
            ai_chat_width: 400,
            preserve_working_dir: true,
            use_custom_command: false,
            custom_command: String::new(),
            cell_height_scale: 1.0,
            cell_width_scale: 1.0,
            user_profile: UserProfile::default(),
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
            pty_persistence: false,
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
            hide_character_badge: false,
            web_search_on_by_default: true,
            enable_file_tools: true,
            enable_system_tools: true,
            enable_dangerous_tools: true,
            enable_web_tools: true,
            enable_clipboard_tools: true,
            enable_web_search: true,
            enable_os_context: true,
            enable_progress_bar: true,
            enable_timer_sounds: true,
            enable_task_sounds: true,
            colored_tabs: false,
            maintain_overlay_history: true,
            enable_tips: true,
            claw_msgbar_shortcut: "<Ctrl>slash".to_string(),
            enable_telemetry: true, // ON by default during Preview
            install_id: None,
            mcp_servers: vec![],
        }
    }
}

impl Settings {
    fn get_path() -> Option<PathBuf> {
        let config_dir = boxxy_sys_utils::get_config_dir();
        if !config_dir.exists() {
            fs::create_dir_all(&config_dir).ok()?;
        }
        Some(config_dir.join("settings.json"))
    }

    pub fn ensure_claw_skills() {
        let config_dir = boxxy_sys_utils::get_config_dir();
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

        let skills_dir = boxxyclaw_dir.join("skills");
        if !skills_dir.exists() {
            let _ = fs::create_dir_all(&skills_dir);
        }
    }

    pub fn init() {
        if SETTINGS_INITIALIZED.swap(true, Ordering::AcqRel) {
            return;
        }

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
        let needs_initial_save = if settings.install_id.is_none() {
            settings.install_id = Some(uuid::Uuid::new_v4().to_string());
            true
        } else {
            false
        };

        settings.check_and_unset_invalid_models();
        SETTINGS_CACHE.store(Arc::new(settings.clone()));

        if needs_initial_save {
            settings.persist_to_disk();
        }
    }

    /// Lock-free atomic pointer load of the current `Settings` snapshot.
    #[must_use]
    pub fn load() -> Arc<Self> {
        if !SETTINGS_INITIALIZED.load(Ordering::Acquire) {
            Self::init();
        }
        SETTINGS_CACHE.load_full()
    }

    /// Atomically mutates the cached `Settings` via RCU (`ArcSwap::rcu`),
    /// persists the updated snapshot to disk using a generation-stamped atomic
    /// rename, and broadcasts on `SETTINGS_EVENT_BUS` after the write completes.
    pub fn update<F: FnMut(&mut Self)>(mut f: F) -> Arc<Self> {
        if !SETTINGS_INITIALIZED.load(Ordering::Acquire) {
            Self::init();
        }
        let mut updated = None;
        SETTINGS_CACHE.rcu(|current| {
            let mut next = (**current).clone();
            f(&mut next);
            let arc = Arc::new(next);
            updated = Some(arc.clone());
            arc
        });
        let new_arc = updated.expect("rcu closure always runs at least once");
        new_arc.persist_to_disk();
        new_arc
    }

    /// Re-reads `settings.json` from disk and replaces the in-memory cache.
    /// The daemon calls this when the UI process reports a save, so
    /// cross-process settings changes (model selection, toggles, etc.)
    /// take effect on the next turn without restarting the daemon.
    pub fn reload() {
        Self::init();

        let mut fresh = Self::default();
        if let Some(path) = Self::get_path()
            && let Ok(content) = fs::read_to_string(path)
        {
            match serde_json::from_str::<Settings>(&content) {
                Ok(s) => fresh = s,
                Err(e) => {
                    log::error!("Failed to reload settings: {}", e);
                    return;
                }
            }
        }

        fresh.check_and_unset_invalid_models();
        SETTINGS_CACHE.store(Arc::new(fresh));
    }

    pub fn save(&self) {
        SETTINGS_CACHE.store(Arc::new(self.clone()));
        self.persist_to_disk();
    }

    fn persist_to_disk(&self) {
        #[cfg(test)]
        {
            let _ = SETTINGS_EVENT_BUS.send(self.clone());
            return;
        }

        #[cfg(not(test))]
        if let Some(path) = Self::get_path()
            && let Ok(content) = serde_json::to_string_pretty(self)
        {
            let generation = SETTINGS_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
            let pid = std::process::id();
            let tmp_path = path.with_extension(format!("json.{pid}.{generation}.tmp"));

            if fs::write(&tmp_path, content).is_ok() {
                if fs::rename(&tmp_path, &path).is_ok() {
                    // Only broadcast after the durable atomic rename has completed
                    // so cross-process listeners (daemon reload) never read a stale file.
                    let _ = SETTINGS_EVENT_BUS.send(self.clone());
                } else {
                    let _ = fs::remove_file(&tmp_path);
                }
            }
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

    pub fn check_and_unset_invalid_models(&mut self) {
        let ai_ready   = matches!(self.resolve_model_state(&self.ai_chat_model),  ModelConfigState::Ready(_));
        let claw_ready = matches!(self.resolve_model_state(&self.claw_model),     ModelConfigState::Ready(_));
        let mem_ready  = matches!(self.resolve_model_state(&self.memory_model),   ModelConfigState::Ready(_));

        if !ai_ready   { self.ai_chat_model  = None; }
        if !claw_ready { self.claw_model     = None; }
        if !mem_ready  { self.memory_model   = None; }
    }

    pub fn resolve_model_state(&self, model: &Option<ModelProvider>) -> ModelConfigState {
        let Some(m) = model else {
            return ModelConfigState::Unconfigured;
        };

        if let ModelProvider::Ollama(name) = m {
            return if name.is_empty() || name == "Loading..." || name == "Ollama Offline" {
                ModelConfigState::OllamaOffline
            } else {
                ModelConfigState::Ready(m.clone())
            };
        }

        let provider_name = m.provider_name();
        let requires_key = boxxy_model_selection::get_providers()
            .iter()
            .any(|p| p.name() == provider_name && p.requires_api_key());

        if !requires_key {
            return ModelConfigState::Ready(m.clone());
        }

        let has_key = self.api_keys.get(provider_name).map(|k| !k.is_empty()).unwrap_or(false)
            || Self::get_env_api_key(provider_name).map(|k| !k.is_empty()).unwrap_or(false);

        if has_key {
            ModelConfigState::Ready(m.clone())
        } else {
            ModelConfigState::MissingApiKey {
                provider_name: provider_name.to_string(),
            }
        }
    }

    pub fn is_models_selection_complete(&self) -> bool {
        matches!(self.resolve_model_state(&self.ai_chat_model), ModelConfigState::Ready(_))
            && matches!(self.resolve_model_state(&self.claw_model), ModelConfigState::Ready(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelConfigState {
    /// No provider or model has been selected.
    Unconfigured,

    /// A provider is selected, but the required API key is empty/missing.
    MissingApiKey { provider_name: String },

    /// Ollama is selected, but the local instance is offline or unreachable.
    OllamaOffline,

    /// The model is fully resolved, validated, and ready for use.
    Ready(ModelProvider),
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
        let config_dir = boxxy_sys_utils::get_config_dir();
        if !config_dir.exists() {
            fs::create_dir_all(&config_dir).ok()?;
        }
        Some(config_dir.join("state.json"))
    }

    pub fn init() {
        let mut state = Self::default();
        if let Some(path) = Self::get_path()
            && let Ok(content) = fs::read_to_string(path)
            && let Ok(s) = serde_json::from_str::<AppState>(&content)
        {
            state = s;
        }
        APP_STATE_CACHE.store(Arc::new(state));
    }

    pub fn load() -> Self {
        (**APP_STATE_CACHE.load()).clone()
    }

    pub fn save(&self) {
        APP_STATE_CACHE.store(Arc::new(self.clone()));
        if let Some(path) = Self::get_path()
            && let Ok(content) = serde_json::to_string_pretty(self)
        {
            let _ = fs::write(path, content);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_update_mutates_cache_atomically() {
        let initial = Settings::load().font_size;
        let updated = Settings::update(|s| {
            s.font_size = initial + 3;
        });
        assert_eq!(updated.font_size, initial + 3);
        assert_eq!(Settings::load().font_size, initial + 3);

        // Restore
        Settings::update(|s| {
            s.font_size = initial;
        });
    }
}
