use arc_swap::ArcSwap;
use boxxy_model_selection::ModelProvider;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(test))]
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, LazyLock};
use tokio::sync::broadcast;
pub use tokio::sync::broadcast::error::RecvError as BroadcastRecvError;

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

// --- Currency ---
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum Currency {
    #[default]
    #[serde(alias = "usd")]
    Usd,
    #[serde(alias = "eur")]
    Eur,
    #[serde(alias = "cny")]
    Cny,
    #[serde(alias = "inr")]
    Inr,
    #[serde(alias = "brl")]
    Brl,
}

impl Currency {
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::Usd, Self::Eur, Self::Cny, Self::Inr, Self::Brl]
    }

    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Usd => "USD",
            Self::Eur => "EUR",
            Self::Cny => "CNY",
            Self::Inr => "INR",
            Self::Brl => "BRL",
        }
    }

    #[must_use]
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Usd => "$",
            Self::Eur => "€",
            Self::Cny => "¥",
            Self::Inr => "₹",
            Self::Brl => "R$",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Usd => "US Dollar ($ - USD)",
            Self::Eur => "Euro (€ - EUR)",
            Self::Cny => "Chinese Yuan (¥ - CNY)",
            Self::Inr => "Indian Rupee (₹ - INR)",
            Self::Brl => "Brazilian Real (R$ - BRL)",
        }
    }

    /// Static offline fallback conversion rate from 1.0 USD (sourced from ECB/Frankfurter Oct 2026).
    #[must_use]
    pub const fn fallback_rate_from_usd(self) -> f64 {
        match self {
            Self::Usd => 1.0,
            Self::Eur => 0.89222,
            Self::Cny => 6.691,
            Self::Inr => 95.06,
            Self::Brl => 5.057,
        }
    }

    /// Returns the live exchange rate from 1.0 USD if cached in `boxxy-sys-utils`,
    /// falling back to [`Self::fallback_rate_from_usd`].
    #[must_use]
    pub fn rate_from_usd(self) -> f64 {
        if self == Self::Usd {
            return 1.0;
        }
        boxxy_sys_utils::get_exchange_rate(self.code())
            .unwrap_or_else(|| self.fallback_rate_from_usd())
    }

    /// Formats a USD amount converted into this currency (e.g. `"$0.05"`, `"€0.05"`, `"R$0.29"`).
    #[must_use]
    pub fn format_from_usd(self, usd_amount: f64) -> String {
        let safe_usd = if usd_amount.is_finite() && usd_amount > 0.0 {
            usd_amount
        } else {
            0.0
        };
        let converted = safe_usd * self.rate_from_usd();
        format!("{}{converted:.2}", self.symbol())
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
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

pub const NAUTILUS_EXTENSION_SCRIPT: &str =
    include_str!("../../../resources/nautilus/open_with_boxxy.py");

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

fn deserialize_optional_model_provider<'de, D>(
    deserializer: D,
) -> Result<Option<ModelProvider>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(v) = val else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    match serde_json::from_value::<ModelProvider>(v) {
        Ok(mp) => Ok(Some(mp)),
        Err(e) => {
            log::warn!("Ignoring unrecognized model provider in settings.json: {e}");
            Ok(None)
        }
    }
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
    #[serde(default)]
    pub mouse_autohide: bool,
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
    pub nautilus_integration: bool,
    pub show_vte_grid: bool,
    pub custom_regex: String,
    pub image_preview_trigger: ImagePreviewTrigger,
    pub preview_max_width: i32,
    pub preview_max_height: i32,
    #[serde(default, deserialize_with = "deserialize_optional_model_provider")]
    pub ai_chat_model: Option<ModelProvider>,
    #[serde(default, deserialize_with = "deserialize_optional_model_provider")]
    pub claw_model: Option<ModelProvider>,
    #[serde(default, deserialize_with = "deserialize_optional_model_provider")]
    pub memory_model: Option<ModelProvider>,
    pub enable_auto_dreaming: bool,
    pub invert_scroll: bool,

    pub hide_character_badge: bool,
    pub web_search_on_by_default: bool,
    #[serde(default)]
    pub vibe_trusted_dirs: Vec<String>,
    pub enable_file_tools: bool,
    pub enable_system_tools: bool,
    pub enable_dangerous_tools: bool,
    pub enable_web_tools: bool,
    pub enable_clipboard_tools: bool,
    pub enable_web_search: bool,
    pub use_native_search: bool,
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
    #[serde(default = "default_overlay_font_scale")]
    pub overlay_font_scale: u32,
    #[serde(default)]
    pub currency: Currency,
    pub claw_msgbar_shortcut: String,
    pub enable_telemetry: bool,
    pub install_id: Option<String>,
    pub mcp_servers: Vec<boxxy_mcp::config::McpServerConfig>,
    #[serde(default)]
    pub openai_compatible_endpoints: Vec<boxxy_model_selection::OpenAiCompatibleEndpoint>,
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
            mouse_autohide: false,
            hide_scrollbars: true,
            dim_inactive_panes: true,
            fixed_width_tabs: true,
            api_keys: std::collections::HashMap::new(),
            ollama_base_url: "http://localhost:11434".to_string(),
            login_shell: true,
            pty_persistence: false,
            nautilus_integration: true,
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
            vibe_trusted_dirs: vec![],
            enable_file_tools: true,
            enable_system_tools: true,
            enable_dangerous_tools: true,
            enable_web_tools: true,
            enable_clipboard_tools: true,
            enable_web_search: true,
            use_native_search: true,
            enable_os_context: true,
            enable_progress_bar: true,
            enable_timer_sounds: true,
            enable_task_sounds: true,
            colored_tabs: false,
            maintain_overlay_history: true,
            enable_tips: true,
            overlay_font_scale: default_overlay_font_scale(),
            currency: Currency::Usd,
            claw_msgbar_shortcut: "<Ctrl>slash".to_string(),
            enable_telemetry: true, // ON by default during Preview
            install_id: None,
            mcp_servers: vec![],
            openai_compatible_endpoints: vec![],
        }
    }
}

const fn default_overlay_font_scale() -> u32 {
    100
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

        Self::sync_nautilus_extension(Self::load().nautilus_integration);
        let _ = boxxy_model_selection::resolve_any_catalog_spec("deepseek-v4-pro");
    }

    pub fn get_nautilus_extension_path() -> Option<PathBuf> {
        if let Ok(val) = std::env::var("BOXXY_CONFIG_DIR") {
            let trimmed = val.trim();
            if !trimmed.is_empty() && PathBuf::from(trimmed).is_absolute() {
                return Some(
                    boxxy_sys_utils::get_config_dir()
                        .join("nautilus-python/extensions/open_with_boxxy.py"),
                );
            }
        }

        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        Some(home.join(".local/share/nautilus-python/extensions/open_with_boxxy.py"))
    }

    pub fn sync_nautilus_extension(enabled: bool) {
        let Some(ext_path) = Self::get_nautilus_extension_path() else {
            return;
        };

        if enabled {
            if let Some(parent) = ext_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let needs_write = match fs::read_to_string(&ext_path) {
                Ok(existing) => existing != NAUTILUS_EXTENSION_SCRIPT,
                Err(_) => true,
            };
            if needs_write {
                let _ = fs::write(&ext_path, NAUTILUS_EXTENSION_SCRIPT);
            }
        } else if ext_path.exists() {
            let _ = fs::remove_file(&ext_path);
        }
    }

    /// Parses `Settings` from JSON, falling back to field-by-field recovery if a single field
    /// contains an invalid or legacy value so `api_keys` and user preferences are never lost.
    pub fn parse_lenient(content: &str) -> Result<Self, serde_json::Error> {
        match serde_json::from_str::<Settings>(content) {
            Ok(s) => Ok(s),
            Err(primary_err) => {
                let raw_val = serde_json::from_str::<serde_json::Value>(content)?;
                let Some(raw_obj) = raw_val.as_object() else {
                    return Err(primary_err);
                };
                log::warn!(
                    "Settings JSON had an invalid field ({primary_err}); recovering valid fields individually."
                );
                let mut base_val =
                    serde_json::to_value(Settings::default()).unwrap_or_default();
                if let Some(base_obj) = base_val.as_object_mut() {
                    for (k, v) in raw_obj {
                        let prev = base_obj.insert(k.clone(), v.clone());
                        if serde_json::from_value::<Settings>(serde_json::Value::Object(
                            base_obj.clone(),
                        ))
                        .is_err()
                        {
                            log::warn!("Skipping invalid field '{k}' in settings.json");
                            if let Some(old) = prev {
                                base_obj.insert(k.clone(), old);
                            } else {
                                base_obj.remove(k);
                            }
                        }
                    }
                }
                serde_json::from_value::<Settings>(base_val)
            }
        }
    }

    pub fn init() {
        if SETTINGS_INITIALIZED.swap(true, Ordering::AcqRel) {
            return;
        }

        let mut settings = Self::default();
        let mut had_unrecoverable_parse_error = false;
        if let Some(path) = Self::get_path()
            && let Ok(content) = fs::read_to_string(path)
            && !content.trim().is_empty()
        {
            match Self::parse_lenient(&content) {
                Ok(s) => settings = s,
                Err(e) => {
                    log::error!("Failed to load settings: {}", e);
                    had_unrecoverable_parse_error = true;
                }
            }
        }

        // Generate install_id if missing, but NEVER overwrite an existing settings.json
        // that failed to parse due to a syntax error.
        let needs_initial_save = if !had_unrecoverable_parse_error && settings.install_id.is_none()
        {
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
            match Self::parse_lenient(&content) {
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
            .insert(provider.to_string(), key);
    }

    pub fn get_env_api_key(provider: &str) -> Option<String> {
        ENV_API_KEYS.read().get(provider).cloned()
    }

    pub fn remove_env_api_key(provider: &str) {
        ENV_API_KEYS.write().remove(provider);
    }

    pub fn get_effective_api_key(&self, provider: &str) -> String {
        if let Some(key) = self.api_keys.get(provider)
            && !key.trim().is_empty()
        {
            return key.clone();
        }
        if let Some(env_key) = Self::get_env_api_key(provider)
            && !env_key.trim().is_empty()
        {
            return env_key;
        }
        if let Some(ep) = self
            .openai_compatible_endpoints
            .iter()
            .find(|e| e.name == provider)
            && let Some(env_var) = ep.api_key_env.as_deref().map(str::trim).filter(|s| !s.is_empty())
        {
            if let Some(k) = Self::get_env_api_key(env_var).filter(|k| !k.trim().is_empty()) {
                return k;
            }
            if let Ok(k) = std::env::var(env_var)
                && !k.trim().is_empty()
            {
                return k;
            }
        }
        String::new()
    }

    pub fn get_effective_api_keys(&self) -> std::collections::HashMap<String, String> {
        let mut keys = self.api_keys.clone();
        let env_keys = ENV_API_KEYS.read();
        for (provider, key) in env_keys.iter() {
            if !keys.contains_key(provider)
                || keys.get(provider).map(|k| k.trim().is_empty()).unwrap_or(true)
            {
                keys.insert(provider.clone(), key.clone());
            }
        }
        drop(env_keys);

        for ep in &self.openai_compatible_endpoints {
            let needs_key = keys
                .get(&ep.name)
                .map(|k| k.trim().is_empty())
                .unwrap_or(true);
            if needs_key {
                let resolved = self.get_effective_api_key(&ep.name);
                if !resolved.is_empty() {
                    keys.insert(ep.name.clone(), resolved);
                }
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

        if let ModelProvider::OpenAiCompatible {
            provider_name,
            model_id,
            ..
        } = m
        {
            if model_id.trim().is_empty() {
                return ModelConfigState::Unconfigured;
            }
            let Some(ep) = self
                .openai_compatible_endpoints
                .iter()
                .find(|e| e.name == *provider_name)
            else {
                return ModelConfigState::Unconfigured;
            };
            if !ep.enabled || ep.base_url.trim().is_empty() {
                return ModelConfigState::Unconfigured;
            }
            if !ep.requires_api_key || !self.get_effective_api_key(provider_name).is_empty() {
                return ModelConfigState::Ready(m.clone());
            }
            return ModelConfigState::MissingApiKey {
                provider_name: provider_name.clone(),
            };
        }

        let provider_name = m.provider_name();
        let requires_key = boxxy_model_selection::get_providers_with_custom(
            &self.openai_compatible_endpoints,
        )
        .iter()
        .any(|p| p.name() == provider_name && p.requires_api_key());

        if !requires_key {
            return ModelConfigState::Ready(m.clone());
        }

        let has_key = !self.get_effective_api_key(provider_name).is_empty();

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

    /// Returns `true` if `cwd` is inside (or equal to) at least one valid directory
    /// in `self.vibe_trusted_dirs`.
    pub fn is_dir_vibe_trusted(&self, cwd: &str) -> bool {
        let Some(cwd_norm) = normalize_vibe_dir_path(cwd) else {
            return false;
        };
        if validate_vibe_trusted_dir(&cwd_norm.to_string_lossy()).is_err() {
            return false;
        }
        let cwd_path = std::path::Path::new(&cwd_norm);
        self.vibe_trusted_dirs.iter().any(|trusted| {
            validate_vibe_trusted_dir(trusted)
                .ok()
                .is_some_and(|valid_root| cwd_path.starts_with(std::path::Path::new(&valid_root)))
        })
    }
}

/// Expands leading `~` and lexically resolves `.` and `..` components without disk I/O.
pub fn normalize_vibe_dir_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let expanded = if trimmed == "~" || trimmed == "~/" {
        std::env::var_os("HOME").map(PathBuf::from)?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        home.join(rest)
    } else {
        PathBuf::from(trimmed)
    };

    if !expanded.is_absolute() {
        return None;
    }

    let mut clean = PathBuf::new();
    for comp in expanded.components() {
        match comp {
            std::path::Component::RootDir => clean.push(comp.as_os_str()),
            std::path::Component::Normal(c) => clean.push(c),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                clean.pop();
                if clean.as_os_str().is_empty() {
                    clean.push("/");
                }
            }
            std::path::Component::Prefix(p) => clean.push(p.as_os_str()),
        }
    }
    Some(clean)
}

/// Validates whether a directory path can be added as a Trusted Vibe Location.
/// Rejects `/`, top-level system roots, bare `$HOME`, `~/.ssh`, `~/.gnupg`, and Boxxy's config dir.
pub fn validate_vibe_trusted_dir(raw: &str) -> Result<String, &'static str> {
    let Some(norm) = normalize_vibe_dir_path(raw) else {
        return Err("Enter an absolute path or a ~/subfolder (e.g. ~/Projects).");
    };

    let norm_str = norm.to_string_lossy();
    if norm_str == "/" {
        return Err("Cannot trust the filesystem root (/).");
    }

    const FORBIDDEN_SYSTEM_PREFIXES: &[&str] = &[
        "/etc", "/usr", "/var", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/proc", "/sys",
        "/dev", "/run", "/root",
    ];
    for sys in FORBIDDEN_SYSTEM_PREFIXES {
        if norm == std::path::Path::new(sys) || norm.starts_with(sys) {
            return Err("System directories cannot be trusted for Vibe Mode.");
        }
    }

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if norm == home {
            return Err(
                "Cannot trust your entire Home folder (~). Choose a subfolder like ~/Projects or ~/test.",
            );
        }
        let ssh_dir = home.join(".ssh");
        let gnupg_dir = home.join(".gnupg");
        if norm.starts_with(&ssh_dir) || norm.starts_with(&gnupg_dir) {
            return Err("Sensitive credential folders (~/.ssh, ~/.gnupg) cannot be trusted.");
        }
    }

    let config_dir = boxxy_sys_utils::get_config_dir();
    if norm.starts_with(&config_dir) {
        return Err("Boxxy's own configuration directory cannot be trusted for Vibe Mode.");
    }

    Ok(norm_str.into_owned())
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

    #[test]
    fn settings_use_native_search_roundtrip_and_default() {
        // Legacy JSON without `use_native_search` or `nautilus_integration` defaults to true
        let legacy: Settings = serde_json::from_str("{}").unwrap();
        assert!(legacy.use_native_search);
        assert!(legacy.nautilus_integration);

        // Explicit false and true roundtrip cleanly
        let mut s = Settings::default();
        assert!(s.use_native_search);
        s.use_native_search = false;
        s.currency = Currency::Eur;
        let json = serde_json::to_string(&s).unwrap();
        let decoded: Settings = serde_json::from_str(&json).unwrap();
        assert!(!decoded.use_native_search);
        assert_eq!(decoded.currency, Currency::Eur);
        assert_eq!(Currency::Usd.format_from_usd(0.05), "$0.05");
        assert_eq!(Currency::Eur.format_from_usd(1.0), "€0.89");
        assert_eq!(Currency::Cny.format_from_usd(1.0), "¥6.69");
        assert_eq!(Currency::Inr.format_from_usd(1.0), "₹95.06");
        assert_eq!(Currency::Brl.format_from_usd(1.0), "R$5.06");
    }

    #[test]
    fn resolve_model_state_handles_openai_compatible_endpoints() {
        let mut s = Settings::default();
        let go_ep = boxxy_model_selection::OpenAiCompatibleEndpoint::preset_opencode_go();
        let local_ep = boxxy_model_selection::OpenAiCompatibleEndpoint::preset_local_server();
        s.openai_compatible_endpoints = vec![go_ep, local_ep];

        let go_model = Some(ModelProvider::OpenAiCompatible {
            provider_name: "OpenCode Go".to_string(),
            model_id: "deepseek-v4-pro".to_string(),
            thinking: None,
        });

        // Missing key initially
        assert_eq!(
            s.resolve_model_state(&go_model),
            ModelConfigState::MissingApiKey {
                provider_name: "OpenCode Go".to_string()
            }
        );

        // Ready once key is added
        s.api_keys
            .insert("OpenCode Go".to_string(), "sk-opencode-test".to_string());
        assert_eq!(
            s.resolve_model_state(&go_model),
            ModelConfigState::Ready(go_model.clone().unwrap())
        );

        // Keyless local server is immediately Ready without an API key
        let local_model = Some(ModelProvider::OpenAiCompatible {
            provider_name: "Local OpenAI Server".to_string(),
            model_id: "qwen3-coder".to_string(),
            thinking: None,
        });
        assert_eq!(
            s.resolve_model_state(&local_model),
            ModelConfigState::Ready(local_model.unwrap())
        );
    }

    #[test]
    fn parse_lenient_preserves_api_keys_and_options_when_a_field_is_invalid() {
        let json = r#"{
            "font_name": "JetBrains Mono",
            "font_size": 15,
            "theme": "Dracula",
            "api_keys": {
                "Gemini": "secret-gemini-key",
                "OpenAI": "secret-openai-key"
            },
            "claw_model": {
                "FutureUnknownProvider": { "foo": "bar" }
            },
            "cursor_shape": "invalid_shape_from_future",
            "install_id": "preserved-install-uuid"
        }"#;

        let parsed = Settings::parse_lenient(json).expect("lenient parse should succeed");
        assert_eq!(parsed.font_name, "JetBrains Mono");
        assert_eq!(parsed.font_size, 15);
        assert_eq!(parsed.theme, "Dracula");
        assert_eq!(
            parsed.api_keys.get("Gemini").map(String::as_str),
            Some("secret-gemini-key")
        );
        assert_eq!(
            parsed.api_keys.get("OpenAI").map(String::as_str),
            Some("secret-openai-key")
        );
        assert_eq!(parsed.claw_model, None);
        assert_eq!(parsed.cursor_shape, CursorShape::Block);
        assert_eq!(
            parsed.install_id.as_deref(),
            Some("preserved-install-uuid")
        );
    }
}
