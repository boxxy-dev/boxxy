use arc_swap::ArcSwap;
pub use boxxy_model_selection::{ModelProvider, ThinkingLevel};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use tokio::sync::broadcast;
pub use tokio::sync::broadcast::error::RecvError as BroadcastRecvError;

pub static SETTINGS_EVENT_BUS: LazyLock<broadcast::Sender<Settings>> = LazyLock::new(|| {
    let (tx, _) = broadcast::channel(32);
    tx
});

static SETTINGS_INITIALIZED: AtomicBool = AtomicBool::new(false);
static FILE_GENERATION: AtomicU64 = AtomicU64::new(0);
#[cfg(not(test))]
static PERSIST_LOCK: Mutex<()> = Mutex::new(());
static APP_STATE_PERSIST_LOCK: Mutex<()> = Mutex::new(());

static SETTINGS_CACHE: LazyLock<ArcSwap<Settings>> =
    LazyLock::new(|| ArcSwap::from_pointee(Settings::default()));
static APP_STATE_CACHE: LazyLock<ArcSwap<AppState>> =
    LazyLock::new(|| ArcSwap::from_pointee(AppState::default()));
static ENV_API_KEYS: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

// --- Cursor Shape ---
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
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
            Self::Block => write!(f, "Block"),
            Self::IBeam => write!(f, "I-Beam"),
            Self::Underline => write!(f, "Underline"),
        }
    }
}

// --- Image Preview Trigger ---
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
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
            Self::None => write!(f, "Disabled"),
            Self::Click => write!(f, "On Click (Shift+Click)"),
            Self::Hover => write!(f, "On Hover"),
        }
    }
}

// --- Pane Move Handle ---
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PaneMoveHandleMode {
    Always,
    #[default]
    #[serde(alias = "splits_only")]
    SplitPanesOnly,
    Never,
}

impl PaneMoveHandleMode {
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::SplitPanesOnly => "split_panes_only",
            Self::Never => "never",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "always" => Some(Self::Always),
            "split_panes_only" => Some(Self::SplitPanesOnly),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    /// Returns whether the top hover move handle should be shown for a pane,
    /// given whether the enclosing tab currently displays multiple split panes.
    #[must_use]
    pub const fn should_show(self, is_split: bool) -> bool {
        match self {
            Self::Always => true,
            Self::SplitPanesOnly => is_split,
            Self::Never => false,
        }
    }
}

impl fmt::Display for PaneMoveHandleMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Always => write!(f, "Always"),
            Self::SplitPanesOnly => write!(f, "Split Panes"),
            Self::Never => write!(f, "Never"),
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
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
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
            Self::Default => write!(f, "Follow System"),
            Self::Light => write!(f, "Light"),
            Self::Dark => write!(f, "Dark"),
        }
    }
}

// --- App Theme Mode ---
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AppThemeMode {
    DefaultGtk,
    #[default]
    Boxxy,
}

impl fmt::Display for AppThemeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DefaultGtk => write!(f, "Default GTK (System)"),
            Self::Boxxy => write!(f, "Boxxy Custom Theme"),
        }
    }
}

pub const DEFAULT_FILE_REGEX: &str =
    r#"https?://[^\s"'<>]+|/[\w.@:/-]+|~[\w.@:/-]+|\.{1,2}/[\w.@:/-]+"#;

// Preserve exact default string with non-capturing group for backward test/config compatibility.
const DEFAULT_FILE_REGEX_LEGACY_COMPAT: &str =
    r#"(?:https?://[^\s"'<>]+|/[\w.@:/-]+|~[\w.@:/-]+|\.{1,2}/[\w.@:/-]+)"#;

pub const NAUTILUS_EXTENSION_SCRIPT: &str =
    include_str!("../../../resources/nautilus/open_with_boxxy.py");

pub const BOXXY_THEME_CREATOR_SLUG: &str = "boxxy-theme-creator";
pub const BOXXY_THEME_CREATOR_SKILL: &str =
    include_str!("../../../resources/skills/boxxy-theme-creator/SKILL.md");

pub const BOXXY_CHARACTER_CREATOR_SLUG: &str = "boxxy-character-creator";
pub const BOXXY_CHARACTER_CREATOR_SKILL: &str =
    include_str!("../../../resources/skills/boxxy-character-creator/SKILL.md");

pub const BOXXY_SELF_DEV_SLUG: &str = "boxxy-self-dev";
pub const BOXXY_SELF_DEV_SKILL: &str =
    include_str!("../../../resources/skills/boxxy-self-dev/SKILL.md");

const BUNDLED_DEFAULT_OFF_SKILLS: &[(&str, &str)] = &[
    (BOXXY_THEME_CREATOR_SLUG, BOXXY_THEME_CREATOR_SKILL),
    (BOXXY_CHARACTER_CREATOR_SLUG, BOXXY_CHARACTER_CREATOR_SKILL),
    (BOXXY_SELF_DEV_SLUG, BOXXY_SELF_DEV_SKILL),
];

// --- User Profile ---
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
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

fn deserialize_character_models<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, ModelProvider>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(serde_json::Value::Object(map)) = val else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::with_capacity(map.len());
    for (char_id, entry) in map {
        if char_id.trim().is_empty()
            || boxxy_claw_protocol::characters::is_incognito_character(&char_id)
            || entry.is_null()
        {
            continue;
        }
        match serde_json::from_value::<ModelProvider>(entry) {
            Ok(mp) => {
                out.insert(char_id, mp);
            }
            Err(e) => {
                log::warn!(
                    "Ignoring unrecognized character model override for '{char_id}' in settings.json: {e}"
                );
            }
        }
    }
    Ok(out)
}

// --- User Configurable Settings ---
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub font_name: String,
    pub font_size: u16,
    pub theme: String,
    pub color_scheme: ColorScheme,
    #[serde(default)]
    pub app_theme_mode: AppThemeMode,
    #[serde(default = "default_app_theme_id")]
    pub app_theme_id: String,
    #[serde(default)]
    pub custom_accent_color: Option<String>,
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
    #[serde(default)]
    pub pane_move_handle: PaneMoveHandleMode,
    #[serde(default = "default_smart_tab_naming")]
    pub smart_tab_naming: bool,
    pub api_keys: HashMap<String, String>,
    pub ollama_base_url: String,
    pub login_shell: bool,
    /// When true, closing a pane detaches its shell into the daemon's
    /// background instead of killing it.
    #[serde(default)]
    pub pty_persistence: bool,
    pub nautilus_integration: bool,
    pub show_vte_grid: bool,
    pub custom_regex: String,
    pub image_preview_trigger: ImagePreviewTrigger,
    pub preview_max_width: i32,
    pub preview_max_height: i32,
    #[serde(default, deserialize_with = "deserialize_optional_model_provider")]
    pub claw_model: Option<ModelProvider>,
    #[serde(default, deserialize_with = "deserialize_character_models")]
    pub character_models: HashMap<String, ModelProvider>,
    #[serde(default, deserialize_with = "deserialize_optional_model_provider")]
    pub memory_model: Option<ModelProvider>,
    pub enable_auto_dreaming: bool,
    pub invert_scroll: bool,

    pub hide_character_badge: bool,
    pub web_search_on_by_default: bool,
    #[serde(default = "default_max_roundtable_turns")]
    pub max_roundtable_turns: u32,
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
    #[serde(default)]
    pub overlay_bottom_gap: bool,
    #[serde(default)]
    pub always_show_code_blocks: bool,
    pub enable_tips: bool,
    #[serde(default = "default_overlay_font_scale")]
    pub overlay_font_scale: u32,
    #[serde(default)]
    pub currency: Currency,
    pub claw_msgbar_shortcut: String,
    pub enable_telemetry: bool,
    pub install_id: Option<String>,
    pub mcp_servers: Vec<boxxy_mcp::config::McpServerConfig>,
    #[serde(default = "default_disabled_skills")]
    pub disabled_skills: Vec<String>,
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
            app_theme_mode: AppThemeMode::Boxxy,
            app_theme_id: default_app_theme_id(),
            custom_accent_color: None,
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
            pane_move_handle: PaneMoveHandleMode::SplitPanesOnly,
            smart_tab_naming: default_smart_tab_naming(),
            api_keys: HashMap::new(),
            ollama_base_url: "http://localhost:11434".to_string(),
            login_shell: true,
            pty_persistence: false,
            nautilus_integration: true,
            show_vte_grid: false,
            custom_regex: DEFAULT_FILE_REGEX_LEGACY_COMPAT.to_string(),
            image_preview_trigger: ImagePreviewTrigger::Click,
            preview_max_width: 300,
            preview_max_height: 200,
            claw_model: None,
            character_models: HashMap::new(),
            memory_model: None,
            enable_auto_dreaming: true,
            invert_scroll: true,
            hide_character_badge: false,
            web_search_on_by_default: true,
            max_roundtable_turns: default_max_roundtable_turns(),
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
            overlay_bottom_gap: false,
            always_show_code_blocks: false,
            enable_tips: true,
            overlay_font_scale: default_overlay_font_scale(),
            currency: Currency::Usd,
            claw_msgbar_shortcut: "<Ctrl>slash".to_string(),
            enable_telemetry: true, // ON by default during Preview
            install_id: None,
            mcp_servers: vec![],
            disabled_skills: default_disabled_skills(),
            openai_compatible_endpoints: vec![],
        }
    }
}

fn default_app_theme_id() -> String {
    "boxxy-dark".to_string()
}

fn default_disabled_skills() -> Vec<String> {
    vec![
        BOXXY_THEME_CREATOR_SLUG.to_string(),
        BOXXY_CHARACTER_CREATOR_SLUG.to_string(),
        BOXXY_SELF_DEV_SLUG.to_string(),
    ]
}

const fn default_smart_tab_naming() -> bool {
    true
}

const fn default_overlay_font_scale() -> u32 {
    100
}

const fn default_max_roundtable_turns() -> u32 {
    20
}

fn atomic_write_file(path: &Path, content: &str) -> std::io::Result<()> {
    let generation = FILE_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let pid = std::process::id();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("json");
    let tmp_path = path.with_extension(format!("{ext}.{pid}.{generation}.tmp"));
    fs::write(&tmp_path, content)?;
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

const FIRST_BOOT_PENDING_FILE: &str = ".first_boot_pending";

/// Seeds `<config_dir>/.first_boot_pending` when initializing a brand-new installation
/// (no prior `state.json`, `last_seen_commit`, `whats_new_last.json`, or `boxxyclaw/boxxy.db`).
pub(crate) fn seed_first_boot_pending_if_fresh_in(config_dir: &Path) {
    let existing_install_markers = [
        config_dir.join("state.json"),
        config_dir.join("last_seen_commit"),
        config_dir.join("whats_new_last.json"),
        config_dir.join("boxxyclaw/boxxy.db"),
    ];
    if existing_install_markers.iter().any(|p| p.exists()) {
        return;
    }
    if fs::create_dir_all(config_dir).is_ok() {
        let _ = atomic_write_file(&config_dir.join(FIRST_BOOT_PENDING_FILE), "1\n");
    }
}

/// Atomically consumes `<config_dir>/.first_boot_pending`, returning `true` once on the
/// initial interactive launch of a brand-new installation.
#[must_use]
pub fn take_first_boot_pending_in(config_dir: &Path) -> bool {
    let marker = config_dir.join(FIRST_BOOT_PENDING_FILE);
    match fs::remove_file(&marker) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            log::warn!(
                "Failed to consume first-boot marker '{}': {e}",
                marker.display()
            );
            false
        }
    }
}

/// Checks and consumes `.first_boot_pending` in [`boxxy_sys_utils::get_config_dir()`].
/// Performs synchronous filesystem I/O; call off the GTK main thread via `spawn_blocking`.
#[must_use]
pub fn take_first_boot_pending() -> bool {
    take_first_boot_pending_in(&boxxy_sys_utils::get_config_dir())
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

        if !boxxyclaw_dir.exists()
            && let Err(e) = fs::create_dir_all(&boxxyclaw_dir)
        {
            log::warn!(
                "Failed to create boxxyclaw directory '{}': {e}",
                boxxyclaw_dir.display()
            );
        }

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
            if let Err(e) = fs::write(&blacklist_md, blacklist_content) {
                log::warn!("Failed to seed '{}': {e}", blacklist_md.display());
            }
        }

        let skills_root = boxxyclaw_dir.join("skills");
        if !skills_root.exists()
            && let Err(e) = fs::create_dir_all(&skills_root)
        {
            log::warn!(
                "Failed to create skills directory '{}': {e}",
                skills_root.display()
            );
        }

        // Seed and always overwrite bundled skills on startup
        // (disabled by default only on first creation so user toggle state is preserved)
        let mut newly_seeded_slugs: Vec<&str> = Vec::new();
        for &(slug, content) in BUNDLED_DEFAULT_OFF_SKILLS {
            let bundled_dir = skills_root.join(slug);
            let skill_file = bundled_dir.join("SKILL.md");
            let is_new_skill = !skill_file.exists();
            let _ = fs::create_dir_all(&bundled_dir);
            if let Err(e) = fs::write(&skill_file, content) {
                log::warn!(
                    "Failed to write bundled skill '{}': {e}",
                    skill_file.display()
                );
            }
            if is_new_skill {
                newly_seeded_slugs.push(slug);
            }
        }
        if !newly_seeded_slugs.is_empty() {
            Self::update(|s| {
                for &slug in &newly_seeded_slugs {
                    if !s.disabled_skills.iter().any(|existing| existing == slug) {
                        s.disabled_skills.push(slug.to_string());
                    }
                }
            });
        }

        Self::sync_nautilus_extension(Self::load().nautilus_integration);
        let _ = boxxy_model_selection::resolve_any_catalog_spec("deepseek-v4-pro");
    }

    #[must_use]
    pub fn get_nautilus_extension_path() -> Option<PathBuf> {
        let active_cfg = boxxy_sys_utils::get_config_dir();
        if active_cfg != boxxy_sys_utils::get_stable_config_dir() {
            return Some(active_cfg.join("nautilus-python/extensions/open_with_boxxy.py"));
        }

        if let Some(xdg_data) = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            return Some(xdg_data.join("nautilus-python/extensions/open_with_boxxy.py"));
        }

        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        Some(home.join(".local/share/nautilus-python/extensions/open_with_boxxy.py"))
    }

    pub fn sync_nautilus_extension(enabled: bool) {
        let Some(ext_path) = Self::get_nautilus_extension_path() else {
            return;
        };

        if enabled {
            if let Some(parent) = ext_path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                log::warn!(
                    "Failed to create Nautilus extension directory '{}': {e}",
                    parent.display()
                );
                return;
            }
            let needs_write = fs::read_to_string(&ext_path)
                .map_or(true, |existing| existing != NAUTILUS_EXTENSION_SCRIPT);
            if needs_write && let Err(e) = fs::write(&ext_path, NAUTILUS_EXTENSION_SCRIPT) {
                log::warn!(
                    "Failed to write Nautilus extension '{}': {e}",
                    ext_path.display()
                );
            }
        } else if ext_path.exists()
            && let Err(e) = fs::remove_file(&ext_path)
        {
            log::warn!(
                "Failed to remove Nautilus extension '{}': {e}",
                ext_path.display()
            );
        }
    }

    /// Parses `Settings` from JSON, falling back to field-by-field recovery if a single field
    /// contains an invalid or legacy value so `api_keys` and user preferences are never lost.
    ///
    /// # Errors
    /// Returns a `serde_json::Error` if `content` is not valid JSON or not a JSON object.
    pub fn parse_lenient(content: &str) -> Result<Self, serde_json::Error> {
        match serde_json::from_str::<Self>(content) {
            Ok(s) => Ok(s),
            Err(primary_err) => {
                let raw_val = serde_json::from_str::<serde_json::Value>(content)?;
                let Some(raw_obj) = raw_val.as_object() else {
                    return Err(primary_err);
                };
                log::warn!(
                    "Settings JSON had an invalid field ({primary_err}); recovering valid fields individually."
                );
                let mut base_val = serde_json::to_value(Self::default()).unwrap_or_default();
                if base_val.is_object() {
                    for (k, v) in raw_obj {
                        let prev = base_val
                            .as_object_mut()
                            .and_then(|obj| obj.insert(k.clone(), v.clone()));
                        if Self::deserialize(&base_val).is_err() {
                            log::warn!("Skipping invalid field '{k}' in settings.json");
                            if let Some(obj) = base_val.as_object_mut() {
                                if let Some(old) = prev {
                                    obj.insert(k.clone(), old);
                                } else {
                                    obj.remove(k);
                                }
                            }
                        }
                    }
                }
                Self::deserialize(&base_val)
            }
        }
    }

    pub fn init() {
        if SETTINGS_INITIALIZED.swap(true, Ordering::AcqRel) {
            return;
        }

        let _ = boxxy_ai_core::refresh_chatgpt_auth_status();

        let mut settings = Self::default();
        let mut had_unrecoverable_parse_error = false;
        if let Some(path) = Self::get_path()
            && let Ok(content) = fs::read_to_string(path)
            && !content.trim().is_empty()
        {
            match Self::parse_lenient(&content) {
                Ok(s) => settings = s,
                Err(e) => {
                    log::error!("Failed to load settings: {e}");
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

        settings.sanitize_unconfigured_models();
        SETTINGS_CACHE.store(Arc::new(settings));

        if needs_initial_save {
            #[cfg(not(test))]
            seed_first_boot_pending_if_fresh_in(&boxxy_sys_utils::get_config_dir());
            Self::persist_latest_to_disk();
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

    /// Atomically mutates the cached `Settings` via RCU (`ArcSwap::rcu`).
    /// If the mutation produces an actual state change, persists the latest
    /// snapshot to disk using a generation-stamped atomic rename and broadcasts
    /// on `SETTINGS_EVENT_BUS` after the rename completes.
    pub fn update<F: FnMut(&mut Self)>(mut f: F) -> Arc<Self> {
        if !SETTINGS_INITIALIZED.load(Ordering::Acquire) {
            Self::init();
        }
        let mut changed = false;
        let mut updated = None;
        SETTINGS_CACHE.rcu(|current| {
            let mut next = (**current).clone();
            f(&mut next);
            if next == **current {
                changed = false;
                let kept = Arc::clone(current);
                updated = Some(Arc::clone(&kept));
                kept
            } else {
                changed = true;
                let arc = Arc::new(next);
                updated = Some(Arc::clone(&arc));
                arc
            }
        });
        let new_arc = updated.unwrap_or_else(|| SETTINGS_CACHE.load_full());
        if changed {
            Self::persist_latest_to_disk();
        }
        new_arc
    }

    /// Re-reads `settings.json` from disk and replaces the in-memory cache.
    /// The daemon calls this when the UI process reports a save, so
    /// cross-process settings changes (model selection, toggles, etc.)
    /// take effect on the next turn without restarting the daemon.
    pub fn reload() {
        Self::init();
        let _ = boxxy_ai_core::refresh_chatgpt_auth_status();

        let Some(path) = Self::get_path() else {
            return;
        };
        let content = match fs::read_to_string(&path) {
            Ok(c) if !c.trim().is_empty() => c,
            Ok(_) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                log::warn!(
                    "Failed to read settings file '{}' during reload: {e}",
                    path.display()
                );
                return;
            }
        };

        let mut fresh = match Self::parse_lenient(&content) {
            Ok(s) => s,
            Err(e) => {
                log::error!("Failed to reload settings: {e}");
                return;
            }
        };

        if fresh.install_id.is_none() {
            fresh
                .install_id
                .clone_from(&SETTINGS_CACHE.load().install_id);
        }
        fresh.sanitize_unconfigured_models();
        SETTINGS_CACHE.store(Arc::new(fresh));
    }

    pub fn save(&self) {
        SETTINGS_INITIALIZED.store(true, Ordering::Release);
        SETTINGS_CACHE.store(Arc::new(self.clone()));
        Self::persist_latest_to_disk();
    }

    /// Broadcasts the current `Settings` snapshot on `SETTINGS_EVENT_BUS` without
    /// rewriting `settings.json` (used when external `.toml` theme files or user avatars change).
    pub fn notify_changed() {
        let snapshot = (*Self::load()).clone();
        let _ = SETTINGS_EVENT_BUS.send(snapshot);
    }

    fn persist_latest_to_disk() {
        #[cfg(test)]
        {
            let snapshot = (*SETTINGS_CACHE.load_full()).clone();
            let _ = SETTINGS_EVENT_BUS.send(snapshot);
        }

        #[cfg(not(test))]
        {
            let _guard = PERSIST_LOCK.lock();
            let snapshot = (*SETTINGS_CACHE.load_full()).clone();
            if let Some(path) = Self::get_path() {
                match serde_json::to_string_pretty(&snapshot) {
                    Ok(content) => {
                        if let Err(e) = atomic_write_file(&path, &content) {
                            log::warn!("Failed to persist settings to '{}': {e}", path.display());
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to serialize settings to JSON: {e}");
                    }
                }
            } else {
                log::warn!("Could not resolve settings.json path for persistence");
            }
            // Broadcast after the atomic rename completes so cross-process listeners
            // (daemon reload) never read a stale file.
            let _ = SETTINGS_EVENT_BUS.send(snapshot);
        }
    }

    pub fn set_env_api_key(provider: &str, key: String) {
        ENV_API_KEYS.write().insert(provider.to_string(), key);
    }

    #[must_use]
    pub fn get_env_api_key(provider: &str) -> Option<String> {
        ENV_API_KEYS.read().get(provider).cloned()
    }

    pub fn remove_env_api_key(provider: &str) {
        ENV_API_KEYS.write().remove(provider);
    }

    #[must_use]
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

        // Check canonical built-in provider environment variables (e.g. GEMINI_API_KEY, OPENCODE_API_KEY, TAVILY_API_KEY).
        for &(env_var, mapped_provider) in boxxy_sys_utils::env::PROVIDER_ENV_KEY_MAP {
            if mapped_provider == provider {
                if let Some(k) = Self::get_env_api_key(env_var).filter(|k| !k.trim().is_empty()) {
                    return k;
                }
                if let Ok(k) = std::env::var(env_var)
                    && !k.trim().is_empty()
                {
                    return k;
                }
            }
        }

        if provider == "ChatGPT" && boxxy_ai_core::is_chatgpt_oauth_signed_in() {
            return boxxy_ai_core::CHATGPT_OAUTH_SENTINEL.to_string();
        }

        if let Some(ep) = self
            .openai_compatible_endpoints
            .iter()
            .find(|e| e.name == provider)
            && let Some(env_var) = ep
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
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

    #[must_use]
    pub fn get_effective_api_keys(&self) -> HashMap<String, String> {
        let mut keys = self.api_keys.clone();
        {
            let env_keys = ENV_API_KEYS.read();
            for (provider, key) in env_keys.iter() {
                if !key.trim().is_empty() && keys.get(provider).is_none_or(|k| k.trim().is_empty())
                {
                    keys.insert(provider.clone(), key.clone());
                }
            }
        }

        for &(_, provider_name) in boxxy_sys_utils::env::PROVIDER_ENV_KEY_MAP {
            if keys.get(provider_name).is_none_or(|k| k.trim().is_empty()) {
                let resolved = self.get_effective_api_key(provider_name);
                if !resolved.is_empty() {
                    keys.insert(provider_name.to_string(), resolved);
                }
            }
        }

        for ep in &self.openai_compatible_endpoints {
            if keys.get(&ep.name).is_none_or(|k| k.trim().is_empty()) {
                let resolved = self.get_effective_api_key(&ep.name);
                if !resolved.is_empty() {
                    keys.insert(ep.name.clone(), resolved);
                }
            }
        }

        keys
    }

    /// Resolves the effective model for a character UUID.
    ///
    /// Returns the character's custom override from `character_models` when present,
    /// otherwise falls back to the mandatory global `claw_model`. Ephemeral `/incognito`
    /// sessions and empty character IDs always resolve to `claw_model`.
    #[must_use]
    pub fn resolve_character_model(&self, character_id: &str) -> Option<ModelProvider> {
        let trimmed = character_id.trim();
        if !trimmed.is_empty()
            && !boxxy_claw_protocol::characters::is_incognito_character(trimmed)
            && let Some(custom) = self.character_models.get(trimmed)
        {
            return Some(custom.clone());
        }
        self.claw_model.clone()
    }

    /// Returns `true` when `character_id` has an explicit custom model override.
    #[must_use]
    pub fn has_custom_character_model(&self, character_id: &str) -> bool {
        let trimmed = character_id.trim();
        !trimmed.is_empty()
            && !boxxy_claw_protocol::characters::is_incognito_character(trimmed)
            && self.character_models.contains_key(trimmed)
    }

    /// Resolves the system-wide model for Memories and Dreams (`memory_model`, falling
    /// back to the mandatory global `claw_model` when `None`).
    #[must_use]
    pub fn effective_memory_model(&self) -> Option<ModelProvider> {
        self.memory_model
            .clone()
            .or_else(|| self.claw_model.clone())
    }

    /// Reverts non-default model overrides (`character_models` and `memory_model`) that are
    /// no longer `Ready` back to the global default when a provider or endpoint is deleted,
    /// while intentionally preserving `claw_model` so `default_model_broken_reason()` can
    /// notify the user if their mandatory Default model broke.
    ///
    /// Returns the number of character overrides that were reverted to Default.
    pub fn revert_non_default_invalid_models(&mut self) -> usize {
        if !matches!(
            self.resolve_model_state(&self.memory_model),
            ModelConfigState::Ready(_)
        ) {
            self.memory_model = None;
        }
        let before = self.character_models.len();
        let invalid_ids: Vec<String> = self
            .character_models
            .iter()
            .filter(|(_, mp)| {
                !matches!(
                    self.resolve_model_state(&Some((*mp).clone())),
                    ModelConfigState::Ready(_)
                )
            })
            .map(|(char_id, _)| char_id.clone())
            .collect();
        for id in invalid_ids {
            self.character_models.remove(&id);
        }
        before.saturating_sub(self.character_models.len())
    }

    /// Legacy helper retained for callers that explicitly clear all invalid slots;
    /// prefer [`Self::revert_non_default_invalid_models`] when removing a provider so
    /// broken Default models surface a clear warning notification.
    pub fn check_and_unset_invalid_models(&mut self) {
        let _ = self.revert_non_default_invalid_models();
    }

    /// Clears empty/transient placeholder model entries (`Unconfigured` or `OllamaOffline`)
    /// while preserving `MissingApiKey` and `ProviderUnavailable` slots across startup/reload.
    pub fn sanitize_unconfigured_models(&mut self) {
        if matches!(
            self.resolve_model_state(&self.claw_model),
            ModelConfigState::Unconfigured | ModelConfigState::OllamaOffline
        ) {
            self.claw_model = None;
        }
        if matches!(
            self.resolve_model_state(&self.memory_model),
            ModelConfigState::Unconfigured | ModelConfigState::OllamaOffline
        ) {
            self.memory_model = None;
        }
        let invalid_ids: Vec<String> = self
            .character_models
            .iter()
            .filter(|(_, mp)| {
                matches!(
                    self.resolve_model_state(&Some((*mp).clone())),
                    ModelConfigState::Unconfigured | ModelConfigState::OllamaOffline
                )
            })
            .map(|(char_id, _)| char_id.clone())
            .collect();
        for id in invalid_ids {
            self.character_models.remove(&id);
        }
    }

    #[must_use]
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
                return ModelConfigState::ProviderUnavailable {
                    provider_name: provider_name.clone(),
                };
            };
            if !ep.enabled || ep.base_url.trim().is_empty() {
                return ModelConfigState::ProviderUnavailable {
                    provider_name: provider_name.clone(),
                };
            }
            if !ep.requires_api_key || !self.get_effective_api_key(provider_name).is_empty() {
                return ModelConfigState::Ready(m.clone());
            }
            return ModelConfigState::MissingApiKey {
                provider_name: provider_name.clone(),
            };
        }

        let provider_name = m.provider_name();
        let requires_key =
            boxxy_model_selection::get_providers_with_custom(&self.openai_compatible_endpoints)
                .iter()
                .any(|p| p.name() == provider_name && p.requires_api_key());

        if !requires_key || !self.get_effective_api_key(provider_name).is_empty() {
            ModelConfigState::Ready(m.clone())
        } else {
            ModelConfigState::MissingApiKey {
                provider_name: provider_name.to_string(),
            }
        }
    }

    /// Returns a human-readable explanation when the mandatory global Default model is broken
    /// (its provider was removed/disabled, its API key is missing, or character overrides exist
    /// without a Default model), returning `None` on a brand-new unconfigured install or when
    /// `claw_model` is `Ready`.
    #[must_use]
    pub fn default_model_broken_reason(&self) -> Option<String> {
        match self.resolve_model_state(&self.claw_model) {
            ModelConfigState::Ready(_) => None,
            ModelConfigState::Unconfigured => {
                if self.character_models.is_empty() {
                    None
                } else {
                    Some("No default model is set (required for all characters).".to_string())
                }
            }
            ModelConfigState::MissingApiKey { provider_name } => {
                if provider_name == "ChatGPT" {
                    Some(
                        "ChatGPT is signed out. Reconnect under Preferences -> APIs & Subscriptions."
                            .to_string(),
                    )
                } else {
                    Some(format!(
                        "{provider_name} API key was removed or is missing."
                    ))
                }
            }
            ModelConfigState::ProviderUnavailable { provider_name } => {
                Some(format!("{provider_name} provider was removed or disabled."))
            }
            ModelConfigState::OllamaOffline => {
                Some("Ollama is offline for the default model.".to_string())
            }
        }
    }

    /// Returns the first broken non-default model reason (a custom character override or the
    /// system Dreams model whose provider/key is unavailable), if any.
    #[must_use]
    pub fn secondary_model_broken_reason(&self) -> Option<String> {
        if self.memory_model.is_some() {
            match self.resolve_model_state(&self.memory_model) {
                ModelConfigState::MissingApiKey { provider_name } => {
                    if provider_name == "ChatGPT" {
                        return Some("Dreams model: ChatGPT is signed out.".to_string());
                    }
                    return Some(format!("Dreams model: {provider_name} API key is missing."));
                }
                ModelConfigState::ProviderUnavailable { provider_name } => {
                    return Some(format!(
                        "Dreams model: {provider_name} provider is unavailable."
                    ));
                }
                ModelConfigState::OllamaOffline => {
                    return Some("Dreams model: Ollama is offline.".to_string());
                }
                ModelConfigState::Ready(_) | ModelConfigState::Unconfigured => {}
            }
        }
        for mp in self.character_models.values() {
            match self.resolve_model_state(&Some(mp.clone())) {
                ModelConfigState::MissingApiKey { provider_name } => {
                    if provider_name == "ChatGPT" {
                        return Some("Character override: ChatGPT is signed out.".to_string());
                    }
                    return Some(format!(
                        "Character override: {provider_name} API key is missing."
                    ));
                }
                ModelConfigState::ProviderUnavailable { provider_name } => {
                    return Some(format!(
                        "Character override: {provider_name} provider is unavailable."
                    ));
                }
                ModelConfigState::OllamaOffline => {
                    return Some("Character override: Ollama is offline.".to_string());
                }
                ModelConfigState::Ready(_) | ModelConfigState::Unconfigured => {}
            }
        }
        None
    }

    #[must_use]
    pub fn is_models_selection_complete(&self) -> bool {
        if !matches!(
            self.resolve_model_state(&self.claw_model),
            ModelConfigState::Ready(_)
        ) {
            return false;
        }
        if self.memory_model.is_some()
            && !matches!(
                self.resolve_model_state(&self.memory_model),
                ModelConfigState::Ready(_)
            )
        {
            return false;
        }
        self.character_models.values().all(|mp| {
            matches!(
                self.resolve_model_state(&Some(mp.clone())),
                ModelConfigState::Ready(_)
            )
        })
    }

    /// Returns `true` if `cwd` is inside (or equal to) at least one valid directory
    /// in `self.vibe_trusted_dirs`.
    #[must_use]
    pub fn is_dir_vibe_trusted(&self, cwd: &str) -> bool {
        let Some(cwd_norm) = normalize_vibe_dir_path(cwd) else {
            return false;
        };
        let ctx = VibeValidationContext::current();
        if validate_normalized_vibe_dir(&cwd_norm, &ctx).is_err() {
            return false;
        }
        self.vibe_trusted_dirs.iter().any(|trusted| {
            normalize_vibe_dir_path(trusted)
                .filter(|root| validate_normalized_vibe_dir(root, &ctx).is_ok())
                .is_some_and(|valid_root| cwd_norm.starts_with(&valid_root))
        })
    }
}

/// Expands leading `~` and lexically resolves `.` and `..` components without disk I/O.
#[must_use]
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
                    clean = PathBuf::from("/");
                }
            }
            std::path::Component::Prefix(p) => clean.push(p.as_os_str()),
        }
    }
    Some(clean)
}

struct VibeValidationContext {
    home: Option<PathBuf>,
    config_dir: PathBuf,
}

impl VibeValidationContext {
    fn current() -> Self {
        Self {
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .and_then(|h| normalize_vibe_dir_path(&h.to_string_lossy())),
            config_dir: boxxy_sys_utils::get_config_dir(),
        }
    }
}

const FORBIDDEN_SYSTEM_PREFIXES: &[&str] = &[
    "/etc", "/usr", "/var", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/proc", "/sys", "/dev",
    "/run", "/root",
];

const SENSITIVE_HOME_RELPATHS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".kube",
    ".pki",
    ".local/share/keyrings",
];

fn validate_normalized_vibe_dir(
    norm: &Path,
    ctx: &VibeValidationContext,
) -> Result<(), &'static str> {
    if norm == Path::new("/") {
        return Err("Cannot trust the filesystem root (/).");
    }

    for sys in FORBIDDEN_SYSTEM_PREFIXES {
        if norm == Path::new(sys) || norm.starts_with(sys) {
            return Err("System directories cannot be trusted for Vibe Mode.");
        }
    }

    if let Some(ref home) = ctx.home {
        // Block bare $HOME as well as any ancestor of $HOME (such as /home).
        if home.starts_with(norm) {
            return Err(
                "Cannot trust your entire Home folder (~). Choose a subfolder like ~/Projects or ~/test.",
            );
        }
        for rel in SENSITIVE_HOME_RELPATHS {
            if norm.starts_with(home.join(rel)) {
                return Err(
                    "Sensitive credential folders (~/.ssh, ~/.gnupg, ~/.aws, ~/.kube, keyrings) cannot be trusted.",
                );
            }
        }
    }

    if norm.starts_with(&ctx.config_dir) || ctx.config_dir.starts_with(norm) {
        return Err("Boxxy's own configuration directory cannot be trusted for Vibe Mode.");
    }

    Ok(())
}

/// Validates whether a directory path can be added as a Trusted Vibe Location.
///
/// Rejects `/`, top-level system roots, bare `$HOME` (and its ancestors like `/home`),
/// sensitive credential directories, and Boxxy's config dir.
///
/// # Errors
/// Returns a static user-facing validation message when `raw` is not an allowed Vibe folder.
pub fn validate_vibe_trusted_dir(raw: &str) -> Result<String, &'static str> {
    let Some(norm) = normalize_vibe_dir_path(raw) else {
        return Err("Enter an absolute path or a ~/subfolder (e.g. ~/Projects).");
    };
    let ctx = VibeValidationContext::current();
    validate_normalized_vibe_dir(&norm, &ctx)?;
    Ok(norm.to_string_lossy().into_owned())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelConfigState {
    /// No provider or model has been selected.
    Unconfigured,

    /// A provider is selected, but the required API key is empty/missing.
    MissingApiKey { provider_name: String },

    /// A custom OpenAI-compatible endpoint was removed or disabled.
    ProviderUnavailable { provider_name: String },

    /// Ollama is selected, but the local instance is offline or unreachable.
    OllamaOffline,

    /// The model is fully resolved, validated, and ready for use.
    Ready(ModelProvider),
}

// --- Internal App State ---
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(default)]
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
            active_sidebar_page: "claw".to_string(),
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
        let state = if let Some(path) = Self::get_path()
            && let Ok(content) = fs::read_to_string(path)
            && !content.trim().is_empty()
            && let Ok(mut s) = serde_json::from_str::<Self>(&content)
        {
            if s.active_sidebar_page == "assistant" {
                s.active_sidebar_page = "claw".to_string();
            }
            s.window_width = s.window_width.max(320);
            s.window_height = s.window_height.max(240);
            s
        } else {
            Self::default()
        };
        APP_STATE_CACHE.store(Arc::new(state));
    }

    #[must_use]
    pub fn load() -> Self {
        (**APP_STATE_CACHE.load()).clone()
    }

    pub fn save(&self) {
        APP_STATE_CACHE.store(Arc::new(self.clone()));
        let _guard = APP_STATE_PERSIST_LOCK.lock();
        if let Some(path) = Self::get_path() {
            match serde_json::to_string_pretty(self) {
                Ok(content) => {
                    if let Err(e) = atomic_write_file(&path, &content) {
                        log::warn!("Failed to persist app state to '{}': {e}", path.display());
                    }
                }
                Err(e) => {
                    log::warn!("Failed to serialize app state: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial(chatgpt_prefs)]
    fn settings_update_mutates_cache_atomically_and_skips_noop() {
        let initial = Settings::load().font_size;
        let mut rx = SETTINGS_EVENT_BUS.subscribe();
        let updated = Settings::update(|s| {
            s.font_size = initial + 3;
        });
        assert_eq!(updated.font_size, initial + 3);
        assert_eq!(Settings::load().font_size, initial + 3);
        assert!(rx.try_recv().is_ok());

        // No-op update must not emit a duplicate broadcast event.
        let same = Settings::update(|s| {
            s.font_size = initial + 3;
        });
        assert_eq!(same.font_size, initial + 3);
        assert!(rx.try_recv().is_err());

        // Restore
        Settings::update(|s| {
            s.font_size = initial;
        });
    }

    #[test]
    fn settings_use_native_search_roundtrip_and_default() {
        let legacy: Settings = serde_json::from_str("{}").unwrap();
        assert!(legacy.use_native_search);
        assert!(legacy.nautilus_integration);
        assert!(legacy.smart_tab_naming);
        assert_eq!(legacy.pane_move_handle, PaneMoveHandleMode::SplitPanesOnly);
        assert_eq!(
            legacy.disabled_skills,
            vec![
                "boxxy-theme-creator",
                "boxxy-character-creator",
                "boxxy-self-dev"
            ]
        );
        assert_eq!(legacy.app_theme_mode, AppThemeMode::Boxxy);
        assert_eq!(legacy.app_theme_id, "boxxy-dark");
        assert!(legacy.custom_accent_color.is_none());

        let s = Settings {
            use_native_search: false,
            smart_tab_naming: false,
            disabled_skills: vec!["sample-skill".to_string()],
            currency: Currency::Eur,
            ..Settings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let decoded: Settings = serde_json::from_str(&json).unwrap();
        assert!(!decoded.use_native_search);
        assert!(!decoded.smart_tab_naming);
        assert_eq!(decoded.disabled_skills, vec!["sample-skill".to_string()]);
        assert_eq!(decoded.currency, Currency::Eur);
        assert_eq!(Currency::Usd.format_from_usd(0.05), "$0.05");
        assert_eq!(Currency::Eur.format_from_usd(1.0), "€0.89");
        assert_eq!(Currency::Cny.format_from_usd(1.0), "¥6.69");
        assert_eq!(Currency::Inr.format_from_usd(1.0), "₹95.06");
        assert_eq!(Currency::Brl.format_from_usd(1.0), "R$5.06");
    }

    #[test]
    fn pane_move_handle_mode_roundtrip_and_visibility_rules() {
        assert_eq!(
            PaneMoveHandleMode::default(),
            PaneMoveHandleMode::SplitPanesOnly
        );

        assert!(PaneMoveHandleMode::Always.should_show(false));
        assert!(PaneMoveHandleMode::Always.should_show(true));
        assert!(!PaneMoveHandleMode::SplitPanesOnly.should_show(false));
        assert!(PaneMoveHandleMode::SplitPanesOnly.should_show(true));
        assert!(!PaneMoveHandleMode::Never.should_show(false));
        assert!(!PaneMoveHandleMode::Never.should_show(true));

        for mode in [
            PaneMoveHandleMode::Always,
            PaneMoveHandleMode::SplitPanesOnly,
            PaneMoveHandleMode::Never,
        ] {
            assert_eq!(PaneMoveHandleMode::from_name(mode.as_name()), Some(mode));
            let s = Settings {
                pane_move_handle: mode,
                ..Settings::default()
            };
            let json = serde_json::to_string(&s).unwrap();
            let decoded: Settings = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded.pane_move_handle, mode);
        }
        assert_eq!(PaneMoveHandleMode::from_name("unknown"), None);
    }

    #[test]
    fn resolve_model_state_handles_openai_compatible_endpoints() {
        let mut s = Settings::default();
        let mut custom_ep = boxxy_model_selection::OpenAiCompatibleEndpoint::preset_litellm();
        custom_ep.name = "Synthetic Gateway".to_string();
        custom_ep.api_key_env = Some("SYNTHETIC_TEST_GATEWAY_KEY_UNSET".to_string());
        let local_ep = boxxy_model_selection::OpenAiCompatibleEndpoint::preset_local_server();
        s.openai_compatible_endpoints = vec![custom_ep, local_ep];

        let gateway_provider = ModelProvider::OpenAiCompatible {
            provider_name: "Synthetic Gateway".to_string(),
            model_id: "deepseek-v4-pro".to_string(),
            thinking: None,
        };
        let gateway_model = Some(gateway_provider.clone());

        assert_eq!(
            s.resolve_model_state(&gateway_model),
            ModelConfigState::MissingApiKey {
                provider_name: "Synthetic Gateway".to_string()
            }
        );

        // sanitize_unconfigured_models preserves MissingApiKey slots across startup/reload
        s.claw_model = Some(gateway_provider.clone());
        s.sanitize_unconfigured_models();
        assert_eq!(s.claw_model, Some(gateway_provider.clone()));

        s.api_keys.insert(
            "Synthetic Gateway".to_string(),
            "sk-synthetic-test".to_string(),
        );
        assert_eq!(
            s.resolve_model_state(&gateway_model),
            ModelConfigState::Ready(gateway_provider)
        );

        let local_provider = ModelProvider::OpenAiCompatible {
            provider_name: "Local OpenAI Server".to_string(),
            model_id: "qwen3-coder".to_string(),
            thinking: None,
        };
        let local_model = Some(local_provider.clone());
        assert_eq!(
            s.resolve_model_state(&local_model),
            ModelConfigState::Ready(local_provider)
        );
    }

    #[test]
    fn effective_api_keys_resolves_opencode_and_provider_env_map() {
        Settings::set_env_api_key("OPENCODE_API_KEY", "sk-shared-opencode".to_string());
        let s = Settings::default();
        assert_eq!(
            s.get_effective_api_key("OpenCode Zen"),
            "sk-shared-opencode"
        );
        assert_eq!(s.get_effective_api_key("OpenCode Go"), "sk-shared-opencode");
        let all_keys = s.get_effective_api_keys();
        assert_eq!(
            all_keys.get("OpenCode Zen").map(String::as_str),
            Some("sk-shared-opencode")
        );
        assert_eq!(
            all_keys.get("OpenCode Go").map(String::as_str),
            Some("sk-shared-opencode")
        );
        Settings::remove_env_api_key("OPENCODE_API_KEY");
    }

    #[test]
    #[serial_test::serial(chatgpt_prefs)]
    fn vibe_trusted_dir_rejects_root_system_and_home_ancestors() {
        assert!(validate_vibe_trusted_dir("/").is_err());
        assert!(validate_vibe_trusted_dir("/etc/nginx").is_err());
        assert!(validate_vibe_trusted_dir("~").is_err());
        assert!(validate_vibe_trusted_dir("~/.ssh").is_err());
        assert!(validate_vibe_trusted_dir("~/.aws").is_err());
        assert!(validate_vibe_trusted_dir("~/.local/share/keyrings").is_err());

        // If $HOME has a parent (e.g. /home), that parent ancestor must also be rejected.
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
            && let Some(parent) = home.parent()
            && parent != Path::new("/")
        {
            assert!(validate_vibe_trusted_dir(&parent.to_string_lossy()).is_err());
        }

        let valid = validate_vibe_trusted_dir("/opt/synthetic-workspace/../synthetic-workspace")
            .expect("synthetic /opt workspace should be valid");
        assert_eq!(valid, "/opt/synthetic-workspace");

        let mut s = Settings::default();
        s.vibe_trusted_dirs.push(valid);
        assert!(s.is_dir_vibe_trusted("/opt/synthetic-workspace/subproject"));
        assert!(!s.is_dir_vibe_trusted("/opt/other-workspace"));
    }

    #[test]
    fn app_state_deserializes_partial_json_with_defaults() {
        let partial: AppState = serde_json::from_str(r#"{"sidebar_visible": true}"#).unwrap();
        assert!(partial.sidebar_visible);
        assert_eq!(partial.window_width, 1200);
        assert_eq!(partial.window_height, 800);
        assert_eq!(partial.active_sidebar_page, "claw");
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
        assert_eq!(parsed.install_id.as_deref(), Some("preserved-install-uuid"));
    }

    #[test]
    fn bundled_default_off_skills_have_valid_frontmatter() {
        for &(slug, content) in BUNDLED_DEFAULT_OFF_SKILLS {
            let (fm, body) = boxxy_claw_protocol::skill_loader::parse_frontmatter_block(content)
                .unwrap_or_else(|| panic!("Bundled skill '{slug}' has invalid YAML frontmatter"));
            assert!(!fm.name.trim().is_empty(), "Skill '{slug}' missing name");
            assert!(
                !fm.description.trim().is_empty(),
                "Skill '{slug}' missing description"
            );
            assert!(
                !fm.triggers.is_empty(),
                "Skill '{slug}' should define at least one trigger"
            );
            assert!(!fm.pinned, "Default-off skill '{slug}' must not be pinned");
            assert!(!body.trim().is_empty(), "Skill '{slug}' body is empty");
        }
    }

    #[test]
    fn per_character_models_and_mandatory_default_health() {
        let mut s = Settings::default();
        // Brand-new install: unconfigured, not "broken".
        assert_eq!(s.default_model_broken_reason(), None);
        assert!(!s.is_models_selection_complete());

        let mut primary_ep = boxxy_model_selection::OpenAiCompatibleEndpoint::preset_litellm();
        primary_ep.name = "Synthetic Default Gateway".to_string();
        primary_ep.api_key_env = Some("SYNTHETIC_DEFAULT_GW_UNSET".to_string());

        let mut custom_ep = boxxy_model_selection::OpenAiCompatibleEndpoint::preset_litellm();
        custom_ep.name = "Synthetic Override Gateway".to_string();
        custom_ep.api_key_env = Some("SYNTHETIC_OVERRIDE_GW_UNSET".to_string());

        s.openai_compatible_endpoints = vec![primary_ep, custom_ep];
        s.api_keys.insert(
            "Synthetic Default Gateway".to_string(),
            "sk-default-ok".to_string(),
        );
        s.api_keys.insert(
            "Synthetic Override Gateway".to_string(),
            "sk-override-ok".to_string(),
        );

        let default_model = ModelProvider::OpenAiCompatible {
            provider_name: "Synthetic Default Gateway".to_string(),
            model_id: "gpt-6.1-terra".to_string(),
            thinking: None,
        };
        let override_model = ModelProvider::OpenAiCompatible {
            provider_name: "Synthetic Override Gateway".to_string(),
            model_id: "claude-sonnet".to_string(),
            thinking: None,
        };

        // Bypassing default (setting character override only) must be flagged as broken!
        s.character_models
            .insert("char-alpha-uuid".to_string(), override_model.clone());
        assert!(s.default_model_broken_reason().is_some());
        assert!(!s.is_models_selection_complete());

        // Setting mandatory default completes setup.
        s.claw_model = Some(default_model.clone());
        assert_eq!(s.default_model_broken_reason(), None);
        assert!(s.is_models_selection_complete());
        assert_eq!(
            s.resolve_character_model("char-alpha-uuid"),
            Some(override_model.clone())
        );
        assert_eq!(
            s.resolve_character_model("char-beta-uuid"),
            Some(default_model.clone())
        );
        // Ephemeral incognito always uses the mandatory default model.
        assert_eq!(
            s.resolve_character_model("__incognito__"),
            Some(default_model)
        );

        // Removing the Default model's provider key marks models broken EVEN THOUGH
        // char-alpha-uuid's override model is still Ready and working!
        s.api_keys.remove("Synthetic Default Gateway");
        let reverted = s.revert_non_default_invalid_models();
        assert_eq!(reverted, 0);
        assert!(
            s.default_model_broken_reason()
                .as_deref()
                .is_some_and(|r| r.contains("Synthetic Default Gateway"))
        );
        assert!(!s.is_models_selection_complete());
        // Working character override stays intact.
        assert_eq!(
            s.resolve_character_model("char-alpha-uuid"),
            Some(override_model)
        );

        // Restoring Default key and removing Override provider reverts char-alpha-uuid to Default.
        s.api_keys.insert(
            "Synthetic Default Gateway".to_string(),
            "sk-default-ok".to_string(),
        );
        s.openai_compatible_endpoints
            .retain(|e| e.name != "Synthetic Override Gateway");
        let reverted_override = s.revert_non_default_invalid_models();
        assert_eq!(reverted_override, 1);
        assert!(!s.has_custom_character_model("char-alpha-uuid"));
        assert_eq!(s.default_model_broken_reason(), None);
        assert!(s.is_models_selection_complete());
    }

    #[test]
    fn deserialize_character_models_skips_invalid_and_incognito_entries() {
        let json = r#"{
            "character_models": {
                "char-valid": { "Ollama": "qwen3:8b" },
                "char-invalid": { "UnknownVendor": 123 },
                "__incognito__": { "Ollama": "llama3.2" }
            }
        }"#;
        let parsed = Settings::parse_lenient(json).expect("should parse leniently");
        assert_eq!(parsed.character_models.len(), 1);
        assert_eq!(
            parsed.character_models.get("char-valid"),
            Some(&ModelProvider::Ollama("qwen3:8b".to_string()))
        );
    }

    #[test]
    #[serial_test::serial(chatgpt_prefs)]
    fn chatgpt_oauth_resolves_model_state_and_broken_reason() {
        let tmp = tempfile::tempdir().expect("tempdir");
        boxxy_sys_utils::configure_dev_options(boxxy_sys_utils::CliDevOverrides {
            config_dir: Some(tmp.path().to_path_buf()),
            ..Default::default()
        });
        let _ = boxxy_ai_core::refresh_chatgpt_auth_status();

        let chatgpt_model = ModelProvider::ChatGpt(
            boxxy_model_selection::ChatGptModel::new("gpt-6.2-sol"),
            Some(ThinkingLevel::Medium),
        );
        let s = Settings {
            claw_model: Some(chatgpt_model.clone()),
            ..Settings::default()
        };

        // Signed out -> MissingApiKey with OAuth-specific broken message
        assert_eq!(
            s.resolve_model_state(&s.claw_model),
            ModelConfigState::MissingApiKey {
                provider_name: "ChatGPT".to_string(),
            }
        );
        assert_eq!(
            s.default_model_broken_reason().as_deref(),
            Some("ChatGPT is signed out. Reconnect under Preferences -> APIs & Subscriptions.")
        );

        // Write synthetic OAuth token record -> Ready + sentinel in effective_api_keys
        let auth_dir = tmp.path().join("auth");
        std::fs::create_dir_all(&auth_dir).expect("create auth dir");
        std::fs::write(
            auth_dir.join("chatgpt.json"),
            r#"{"refresh_token":"synthetic-refresh-token"}"#,
        )
        .expect("write chatgpt.json");
        let _ = boxxy_ai_core::refresh_chatgpt_auth_status();

        assert_eq!(
            s.get_effective_api_key("ChatGPT"),
            boxxy_ai_core::CHATGPT_OAUTH_SENTINEL
        );
        assert_eq!(
            s.get_effective_api_keys()
                .get("ChatGPT")
                .map(String::as_str),
            Some(boxxy_ai_core::CHATGPT_OAUTH_SENTINEL)
        );
        assert_eq!(
            s.resolve_model_state(&s.claw_model),
            ModelConfigState::Ready(chatgpt_model)
        );
        assert_eq!(s.default_model_broken_reason(), None);

        // Reset dev options so sibling tests see default config resolution
        boxxy_sys_utils::configure_dev_options(boxxy_sys_utils::CliDevOverrides::default());
        let _ = boxxy_ai_core::refresh_chatgpt_auth_status();
    }

    #[test]
    fn first_boot_marker_seeds_once_on_clean_dir_and_skips_existing_installs() {
        let fresh_tmp = tempfile::tempdir().expect("fresh tempdir");
        seed_first_boot_pending_if_fresh_in(fresh_tmp.path());
        assert!(take_first_boot_pending_in(fresh_tmp.path()));
        assert!(!take_first_boot_pending_in(fresh_tmp.path()));

        // Existing install with `state.json` or `last_seen_commit` must not seed first-boot marker.
        let existing_tmp = tempfile::tempdir().expect("existing tempdir");
        std::fs::write(existing_tmp.path().join("last_seen_commit"), "a1b2c3d")
            .expect("write commit marker");
        seed_first_boot_pending_if_fresh_in(existing_tmp.path());
        assert!(!take_first_boot_pending_in(existing_tmp.path()));
    }
}
