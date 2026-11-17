//! Process-global lock-free `ArcSwap` caches and `tokio::sync::watch` change streams.

use super::{
    CharacterClaim, CharacterInfo, CharacterStatus, find_available_characters_in,
    find_first_available_in,
};
use arc_swap::ArcSwap;
use std::sync::{Arc, LazyLock};
use tokio::sync::watch;

/// A globally accessible, lock-free reactive cache of the character registry.
/// Updated via D-Bus (in the UI) and directly by `CharacterRegistry` (in the daemon).
pub static CHARACTER_CACHE: LazyLock<ArcSwap<Vec<CharacterInfo>>> =
    LazyLock::new(|| ArcSwap::from_pointee(Vec::new()));

/// A globally accessible, lock-free reactive cache of current character assignments.
pub static CLAIMS_CACHE: LazyLock<ArcSwap<Vec<CharacterClaim>>> =
    LazyLock::new(|| ArcSwap::from_pointee(Vec::new()));

struct CoherentState {
    catalog: Arc<Vec<CharacterInfo>>,
    claims: Arc<Vec<CharacterClaim>>,
    revision: u64,
    seeded: bool,
}

/// Coherent atomic state sharing the exact same `Arc` allocations as `CHARACTER_CACHE`
/// and `CLAIMS_CACHE` so availability queries never observe a torn `(new_claims, stale_catalog)`
/// pair across concurrent updates.
static COHERENT_REGISTRY_CACHE: LazyLock<ArcSwap<CoherentState>> = LazyLock::new(|| {
    ArcSwap::from_pointee(CoherentState {
        catalog: Arc::new(Vec::new()),
        claims: Arc::new(Vec::new()),
        revision: 0,
        seeded: false,
    })
});

/// Monotonic local generation counter incremented on every effective `update_caches` call
/// so subscribers never miss an initial `revision == 0` seed or collide across daemon restarts.
static REGISTRY_REVISION_WATCH: LazyLock<(watch::Sender<u64>, watch::Receiver<u64>)> =
    LazyLock::new(|| watch::channel(0));

/// Globally accessible, lock-free reactive cache of MCP server connection statuses
/// `[(server_name, is_connected)]`, updated via D-Bus push signals from `boxxy-agent`.
pub static MCP_STATUS_CACHE: LazyLock<ArcSwap<Vec<(String, bool)>>> =
    LazyLock::new(|| ArcSwap::from_pointee(Vec::new()));

static MCP_STATUS_WATCH: LazyLock<(watch::Sender<u64>, watch::Receiver<u64>)> =
    LazyLock::new(|| watch::channel(0));

/// Updates `MCP_STATUS_CACHE` atomically and notifies local UI subscribers when changed.
pub fn update_mcp_status_cache(statuses: Vec<(String, bool)>) {
    if **MCP_STATUS_CACHE.load() != statuses {
        MCP_STATUS_CACHE.store(Arc::new(statuses));
        let next = MCP_STATUS_WATCH.0.borrow().wrapping_add(1);
        let _ = MCP_STATUS_WATCH.0.send(next);
    }
}

/// Subscribes to local `MCP_STATUS_CACHE` updates.
#[must_use]
pub fn subscribe_mcp_status_changes() -> watch::Receiver<u64> {
    MCP_STATUS_WATCH.1.clone()
}

/// Updates `CHARACTER_CACHE`, `CLAIMS_CACHE`, and the coherent snapshot cache atomically,
/// skipping redundant allocations/notifications when `(catalog, claims, revision)` is unchanged.
pub fn update_caches(catalog: Vec<CharacterInfo>, claims: Vec<CharacterClaim>, revision: u64) {
    {
        let current = COHERENT_REGISTRY_CACHE.load();
        if current.seeded
            && current.revision == revision
            && *current.catalog == catalog
            && *current.claims == claims
        {
            return;
        }
    }

    let catalog_arc = Arc::new(catalog);
    let claims_arc = Arc::new(claims);
    COHERENT_REGISTRY_CACHE.store(Arc::new(CoherentState {
        catalog: Arc::clone(&catalog_arc),
        claims: Arc::clone(&claims_arc),
        revision,
        seeded: true,
    }));
    CLAIMS_CACHE.store(claims_arc);
    CHARACTER_CACHE.store(catalog_arc);

    let next_gen = REGISTRY_REVISION_WATCH.0.borrow().wrapping_add(1);
    let _ = REGISTRY_REVISION_WATCH.0.send(next_gen);
}

/// Updates the cached character catalog locally (e.g. immediately after a UI color/avatar edit)
/// while preserving active `CharacterStatus::Active` flags and `claims` from the coherent snapshot.
pub fn update_catalog_preserving_claims(mut catalog: Vec<CharacterInfo>) {
    let (claims_arc, revision, unchanged) = {
        let current = COHERENT_REGISTRY_CACHE.load();
        for info in &mut catalog {
            if let Some(claim) = current
                .claims
                .iter()
                .find(|c| c.character_id == info.config.id)
            {
                info.status = CharacterStatus::Active {
                    pane_id: claim.holder_id.clone(),
                };
            } else {
                info.status = CharacterStatus::Available;
            }
        }
        let same = current.seeded && *current.catalog == catalog;
        (Arc::clone(&current.claims), current.revision, same)
    };

    if unchanged {
        return;
    }

    let catalog_arc = Arc::new(catalog);
    COHERENT_REGISTRY_CACHE.store(Arc::new(CoherentState {
        catalog: Arc::clone(&catalog_arc),
        claims: Arc::clone(&claims_arc),
        revision,
        seeded: true,
    }));
    CHARACTER_CACHE.store(catalog_arc);

    let next_gen = REGISTRY_REVISION_WATCH.0.borrow().wrapping_add(1);
    let _ = REGISTRY_REVISION_WATCH.0.send(next_gen);
}

/// Loads a consistent `(catalog, claims, revision)` view from a single atomic snapshot.
#[must_use]
pub fn load_coherent_registry() -> (Arc<Vec<CharacterInfo>>, Arc<Vec<CharacterClaim>>, u64) {
    let guard = COHERENT_REGISTRY_CACHE.load();
    (
        Arc::clone(&guard.catalog),
        Arc::clone(&guard.claims),
        guard.revision,
    )
}

/// Subscribes to local cache updates triggered by `update_caches`.
#[must_use]
pub fn subscribe_registry_changes() -> watch::Receiver<u64> {
    REGISTRY_REVISION_WATCH.1.clone()
}

/// Returns the first unclaimed character from the coherent global registry cache,
/// optionally ignoring a claim held by `exclude_holder_id`.
#[must_use]
pub fn find_first_available_character(exclude_holder_id: Option<&str>) -> Option<CharacterInfo> {
    let guard = COHERENT_REGISTRY_CACHE.load();
    find_first_available_in(&guard.catalog, &guard.claims, exclude_holder_id).cloned()
}

/// Returns `true` if at least one character in the catalog is currently unclaimed
/// (optionally ignoring a claim held by `exclude_holder_id`).
#[must_use]
pub fn has_available_character(exclude_holder_id: Option<&str>) -> bool {
    let guard = COHERENT_REGISTRY_CACHE.load();
    find_first_available_in(&guard.catalog, &guard.claims, exclude_holder_id).is_some()
}

/// Returns all currently unclaimed characters from the coherent global registry cache,
/// optionally ignoring a claim held by `exclude_holder_id`.
#[must_use]
pub fn get_available_characters(exclude_holder_id: Option<&str>) -> Vec<CharacterInfo> {
    let guard = COHERENT_REGISTRY_CACHE.load();
    find_available_characters_in(&guard.catalog, &guard.claims, exclude_holder_id)
        .into_iter()
        .cloned()
        .collect()
}
