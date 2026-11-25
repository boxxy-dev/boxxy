//! Process-isolated ephemeral configuration bootstrap for headless E2E tests.
//!
//! Ensures every scenario and integration test runs against a temporary config directory
//! with `dev = true` and `ephemeral = true`, preventing any reads or writes to the host
//! user's `~/.config/boxxy-terminal/` tree and disabling telemetry and ambient network fetches.

use boxxy_claw_protocol::character_loader::load_characters;
use boxxy_claw_protocol::characters::{CharacterClaim, CharacterInfo, update_caches};
use boxxy_model_selection::{CustomModelConfig, ModelProvider, OpenAiApiRoute, OpenAiCompatibleEndpoint};
use boxxy_preferences::Settings;
use boxxy_sys_utils::CliDevOverrides;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use tempfile::TempDir;

pub const MOCK_CLAW_MODEL_ID: &str = "mock-claw-model";
pub const MOCK_MEMORY_MODEL_ID: &str = "mock-memory-model";

struct IsolatedProcessConfig {
    _temp_dir: TempDir,
    config_root: PathBuf,
    catalog: Vec<CharacterInfo>,
}

static REVISION_SEQ: AtomicU64 = AtomicU64::new(1);

static ISOLATED_CONFIG: LazyLock<IsolatedProcessConfig> = LazyLock::new(|| {
    let temp_dir = tempfile::Builder::new()
        .prefix("boxxy_e2e_cfg_")
        .tempdir()
        .expect("create isolated Boxxy config TempDir");
    let config_root = temp_dir.path().to_path_buf();

    boxxy_sys_utils::configure_dev_options(CliDevOverrides {
        dev: true,
        instance: Some("e2e".to_string()),
        ephemeral: true,
        sync_settings: false,
        config_dir: Some(config_root.clone()),
        debug_context: false,
    });

    Settings::init();
    Settings::update(|s| {
        s.enable_telemetry = false;
        s.enable_os_context = false;
        s.enable_auto_dreaming = false;
        s.enable_web_search = false;
        s.web_search_on_by_default = false;
    });

    let catalog = load_characters().expect("seed bundled characters into isolated config");
    let rev = REVISION_SEQ.fetch_add(1, Ordering::Relaxed);
    update_caches(catalog.clone(), Vec::new(), rev);

    IsolatedProcessConfig {
        _temp_dir: temp_dir,
        config_root,
        catalog,
    }
});

/// Bootstraps the process-scoped isolated Boxxy config root and returns its path.
#[must_use]
pub fn ensure_isolated_config_root() -> &'static Path {
    ISOLATED_CONFIG.config_root.as_path()
}

/// Returns the seeded bundled `CharacterInfo` catalog inside the isolated config root.
#[must_use]
pub fn seeded_catalog() -> &'static [CharacterInfo] {
    let _ = ensure_isolated_config_root();
    ISOLATED_CONFIG.catalog.as_slice()
}

/// Publishes the active set of character claims into the coherent protocol caches.
pub fn set_active_claims(claims: Vec<CharacterClaim>) {
    let catalog = seeded_catalog().to_vec();
    let rev = REVISION_SEQ.fetch_add(1, Ordering::Relaxed);
    update_caches(catalog, claims, rev);
}

/// Wires `Settings` so both `claw_model` and `memory_model` route to the given
/// local mock OpenAI-compatible server `base_url` (e.g. `"http://127.0.0.1:12345/v1"`).
pub fn configure_mock_llm_endpoint(endpoint_name: &str, base_url: &str) {
    let _ = ensure_isolated_config_root();

    let mut ep = OpenAiCompatibleEndpoint::preset_local_server();
    ep.name = endpoint_name.to_string();
    ep.base_url = base_url.to_string();
    ep.zero_cost = false;
    ep.models = vec![
        CustomModelConfig {
            id: MOCK_CLAW_MODEL_ID.to_string(),
            display_name: "Mock Claw Model".to_string(),
            route: OpenAiApiRoute::ChatCompletions,
            context_window: Some(64_000),
            thinking_levels: Vec::new(),
            input_cost_per_1m: Some(2.0),
            output_cost_per_1m: Some(4.0),
        },
        CustomModelConfig {
            id: MOCK_MEMORY_MODEL_ID.to_string(),
            display_name: "Mock Memory Model".to_string(),
            route: OpenAiApiRoute::ChatCompletions,
            context_window: Some(32_000),
            thinking_levels: Vec::new(),
            input_cost_per_1m: Some(0.5),
            output_cost_per_1m: Some(1.0),
        },
    ];

    let claw_provider = ModelProvider::OpenAiCompatible {
        provider_name: endpoint_name.to_string(),
        model_id: MOCK_CLAW_MODEL_ID.to_string(),
        thinking: None,
    };
    let memory_provider = ModelProvider::OpenAiCompatible {
        provider_name: endpoint_name.to_string(),
        model_id: MOCK_MEMORY_MODEL_ID.to_string(),
        thinking: None,
    };

    Settings::update(|s| {
        s.openai_compatible_endpoints
            .retain(|existing| existing.name != endpoint_name);
        s.openai_compatible_endpoints.push(ep.clone());
        s.claw_model = Some(claw_provider.clone());
        s.memory_model = Some(memory_provider.clone());
        s.character_models.clear();
        s.vibe_trusted_dirs.clear();
    });
}
