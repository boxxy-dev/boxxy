//! Character catalog DTOs, active lease claims, peer lookup queries, and lock-free UI/daemon caches.

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::sync::{Arc, LazyLock};
use tokio::sync::watch;

/// Reserved character ID for ephemeral `/incognito` overlay sessions.
pub const INCOGNITO_CHARACTER_ID: &str = "__incognito__";

/// Slug used for the built-in Incognito persona.
pub const INCOGNITO_CHARACTER_SLUG: &str = "inco-gnito";

/// Display name shown in the overlay, badge, and sidebar during `/incognito`.
pub const INCOGNITO_DISPLAY_NAME: &str = "Inco Gnito";

/// Signature stealth violet color for `Inco Gnito`.
pub const INCOGNITO_COLOR: &str = "#7C5CFF";

/// Subtitle description shown in the sidebar during `/incognito`.
pub const INCOGNITO_DUTIES: &str = "Private Chat • No context sent • Nothing saved";

/// Zero-allocation case-insensitive equality check (ASCII fast-path + full Unicode fallback).
#[must_use]
pub(crate) fn eq_case_insensitive(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        a.chars()
            .flat_map(char::to_lowercase)
            .eq(b.chars().flat_map(char::to_lowercase))
    }
}

/// Zero-allocation case-insensitive ordering (ASCII fast-path + full Unicode fallback).
#[must_use]
pub(crate) fn cmp_case_insensitive(a: &str, b: &str) -> Ordering {
    if a.is_ascii() && b.is_ascii() {
        a.bytes()
            .map(|c| c.to_ascii_lowercase())
            .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
    } else {
        a.chars()
            .flat_map(char::to_lowercase)
            .cmp(b.chars().flat_map(char::to_lowercase))
    }
}

/// Returns `true` if `id_or_name` identifies the built-in `/incognito` persona.
#[must_use]
pub fn is_incognito_character(id_or_name: &str) -> bool {
    let trimmed = id_or_name.trim();
    trimmed == INCOGNITO_CHARACTER_ID
        || trimmed.eq_ignore_ascii_case(INCOGNITO_CHARACTER_SLUG)
        || trimmed.eq_ignore_ascii_case(INCOGNITO_DISPLAY_NAME)
}

/// A globally accessible, lock-free reactive cache of the character registry.
/// Updated via D-Bus (in the UI) and directly by `CharacterRegistry` (in the daemon).
pub static CHARACTER_CACHE: LazyLock<ArcSwap<Vec<CharacterInfo>>> =
    LazyLock::new(|| ArcSwap::from_pointee(Vec::new()));

/// A globally accessible, lock-free reactive cache of current character assignments.
pub static CLAIMS_CACHE: LazyLock<ArcSwap<Vec<CharacterClaim>>> =
    LazyLock::new(|| ArcSwap::from_pointee(Vec::new()));

type CoherentRegistryState = (Arc<Vec<CharacterInfo>>, Arc<Vec<CharacterClaim>>, u64);

/// Coherent atomic tuple `(catalog, claims, revision)` sharing the exact same `Arc`
/// allocations as `CHARACTER_CACHE` and `CLAIMS_CACHE` so availability queries never
/// observe a torn `(new_claims, stale_catalog)` pair across concurrent updates.
static COHERENT_REGISTRY_CACHE: LazyLock<ArcSwap<CoherentRegistryState>> = LazyLock::new(|| {
    ArcSwap::from_pointee((Arc::new(Vec::new()), Arc::new(Vec::new()), 0))
});

static REGISTRY_REVISION_WATCH: LazyLock<(watch::Sender<u64>, watch::Receiver<u64>)> =
    LazyLock::new(|| watch::channel(0));

/// Globally accessible, lock-free reactive cache of MCP server connection statuses
/// `[(server_name, is_connected)]`, updated via D-Bus push signals from `boxxy-agent`.
pub static MCP_STATUS_CACHE: LazyLock<ArcSwap<Vec<(String, bool)>>> =
    LazyLock::new(|| ArcSwap::from_pointee(Vec::new()));

static MCP_STATUS_WATCH: LazyLock<(watch::Sender<u64>, watch::Receiver<u64>)> =
    LazyLock::new(|| watch::channel(0));

/// Updates `MCP_STATUS_CACHE` atomically and notifies local UI subscribers.
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
/// then notifies local subscribers.
pub fn update_caches(catalog: Vec<CharacterInfo>, claims: Vec<CharacterClaim>, revision: u64) {
    let catalog_arc = Arc::new(catalog);
    let claims_arc = Arc::new(claims);
    COHERENT_REGISTRY_CACHE.store(Arc::new((
        Arc::clone(&catalog_arc),
        Arc::clone(&claims_arc),
        revision,
    )));
    CLAIMS_CACHE.store(claims_arc);
    CHARACTER_CACHE.store(catalog_arc);
    let _ = REGISTRY_REVISION_WATCH.0.send(revision);
}

/// Loads a consistent `(catalog, claims, revision)` view from a single atomic snapshot.
#[must_use]
pub fn load_coherent_registry() -> (Arc<Vec<CharacterInfo>>, Arc<Vec<CharacterClaim>>, u64) {
    let guard = COHERENT_REGISTRY_CACHE.load();
    (Arc::clone(&guard.0), Arc::clone(&guard.1), guard.2)
}

/// Subscribes to local cache updates triggered by `update_caches`.
#[must_use]
pub fn subscribe_registry_changes() -> watch::Receiver<u64> {
    REGISTRY_REVISION_WATCH.1.clone()
}

/// Finds the first character in `catalog` that is not claimed in `claims`
/// (optionally ignoring a claim held by `exclude_holder_id`).
#[must_use]
pub fn find_first_available_in<'a>(
    catalog: &'a [CharacterInfo],
    claims: &[CharacterClaim],
    exclude_holder_id: Option<&str>,
) -> Option<&'a CharacterInfo> {
    catalog.iter().find(|info| {
        !claims.iter().any(|claim| {
            claim.character_id == info.config.id
                && exclude_holder_id.is_none_or(|excluded| claim.holder_id != excluded)
        })
    })
}

/// Returns the first unclaimed character from the coherent global registry cache,
/// optionally ignoring a claim held by `exclude_holder_id`.
#[must_use]
pub fn find_first_available_character(exclude_holder_id: Option<&str>) -> Option<CharacterInfo> {
    let guard = COHERENT_REGISTRY_CACHE.load();
    find_first_available_in(&guard.0, &guard.1, exclude_holder_id).cloned()
}

/// Returns `true` if at least one character in the catalog is currently unclaimed
/// (optionally ignoring a claim held by `exclude_holder_id`).
#[must_use]
pub fn has_available_character(exclude_holder_id: Option<&str>) -> bool {
    let guard = COHERENT_REGISTRY_CACHE.load();
    find_first_available_in(&guard.0, &guard.1, exclude_holder_id).is_some()
}

/// Returns all characters in `catalog` that are not claimed in `claims`
/// (optionally ignoring a claim held by `exclude_holder_id`).
#[must_use]
pub fn find_available_characters_in<'a>(
    catalog: &'a [CharacterInfo],
    claims: &[CharacterClaim],
    exclude_holder_id: Option<&str>,
) -> Vec<&'a CharacterInfo> {
    catalog
        .iter()
        .filter(|info| {
            !claims.iter().any(|claim| {
                claim.character_id == info.config.id
                    && exclude_holder_id.is_none_or(|excluded| claim.holder_id != excluded)
            })
        })
        .collect()
}

/// Returns all currently unclaimed characters from the coherent global registry cache,
/// optionally ignoring a claim held by `exclude_holder_id`.
#[must_use]
pub fn get_available_characters(exclude_holder_id: Option<&str>) -> Vec<CharacterInfo> {
    let guard = COHERENT_REGISTRY_CACHE.load();
    find_available_characters_in(&guard.0, &guard.1, exclude_holder_id)
        .into_iter()
        .cloned()
        .collect()
}

/// Looks up a character in `catalog` by exact `character_id` first, then by case-insensitive
/// matches on `name` or `display_name` in `fallback_names`.
///
/// Unlike [`resolve_character`], returns `None` when the character is not in `catalog`
/// (e.g., it was deleted), allowing historical rows to preserve the deleted character's
/// original name and render an initials avatar.
#[must_use]
pub fn find_character_exact<'a>(
    catalog: &'a [CharacterInfo],
    character_id: &str,
    fallback_names: &[&str],
) -> Option<&'a CharacterInfo> {
    if !character_id.is_empty()
        && let Some(found) = catalog.iter().find(|c| c.config.id == character_id)
    {
        return Some(found);
    }

    for name in fallback_names {
        let trimmed = name.trim();
        if !trimmed.is_empty()
            && let Some(found) = catalog.iter().find(|c| {
                eq_case_insensitive(&c.config.name, trimmed)
                    || eq_case_insensitive(&c.config.display_name, trimmed)
            })
        {
            return Some(found);
        }
    }

    None
}

/// Resolves a character from `catalog` by `character_id`, falling back to `fallback_names`
/// or the first catalog entry.
#[must_use]
pub fn resolve_character<'a>(
    catalog: &'a [CharacterInfo],
    character_id: &str,
    fallback_names: &[&str],
) -> Option<&'a CharacterInfo> {
    find_character_exact(catalog, character_id, fallback_names).or_else(|| catalog.first())
}

/// What kind of entity holds a character claim.
///
/// Today the only variant is `Pane` (a terminal pane). The enum exists so future
/// non-pane holders (headless background agents, voice sessions, external apps)
/// can be added without a breaking protocol change.
#[repr(u8)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum HolderKind {
    /// A terminal pane in the UI. The `holder_id` equals the `pane_id`.
    Pane = 0,
}

impl HolderKind {
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

impl From<HolderKind> for u8 {
    fn from(kind: HolderKind) -> Self {
        kind.as_u8()
    }
}

impl TryFrom<u8> for HolderKind {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Pane),
            _ => Err(format!("Unknown HolderKind variant: {value}")),
        }
    }
}

/// A single live claim of a character by some holder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharacterClaim {
    /// Opaque lifecycle/ownership identifier. For `HolderKind::Pane` this is
    /// string-equal to the pane's `pane_id`. Surface-specific tools that
    /// genuinely need a pane should match on `holder_kind == Pane` first and
    /// then use this value as a `pane_id`.
    pub holder_id: String,
    pub holder_kind: HolderKind,
    pub character_id: String,
    pub session_id: String,
    /// Petname (Red Pony Protocol) for swarm peer addressing. Stable per
    /// `holder_id` across character swaps within a single daemon run.
    pub petname: String,
    /// The bus-unique name of the client that owns this lease (e.g. `":1.42"`).
    /// Used by the daemon for `NameOwnerChanged`-driven cleanup; the UI ignores it.
    pub owner_bus_name: String,
}

/// One coherent snapshot of registry state. Sent on initial fetch and on every
/// push update.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistrySnapshot {
    /// Carried in every snapshot so a reconnecting UI can rebuild its full mirror in one round trip.
    pub catalog: Vec<CharacterInfo>,
    pub claims: Vec<CharacterClaim>,
    /// Monotonic, incremented per emitted snapshot. UI uses it to drop
    /// out-of-order signals that arrive after a reconnect.
    pub revision: u64,
}

/// Typed claim error so the UI can show specific feedback (or recover) instead
/// of relying on a desktop-notification side effect.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClaimError {
    AlreadyTaken {
        holder_id: String,
        holder_kind: HolderKind,
        holder_display_name: String,
    },
    UnknownCharacter {
        character_id: String,
    },
    /// The original character was removed from disk; daemon picked a
    /// replacement. The UI re-issues the call with `to_id`.
    Migrated {
        from_id: String,
        to_id: String,
    },
}

impl fmt::Display for ClaimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyTaken {
                holder_id,
                holder_display_name,
                ..
            } => write!(
                f,
                "Character is already claimed by {holder_display_name} ({holder_id})"
            ),
            Self::UnknownCharacter { character_id } => {
                write!(f, "Unknown character ID: {character_id}")
            }
            Self::Migrated { from_id, to_id } => {
                write!(f, "Character {from_id} was migrated to {to_id}")
            }
        }
    }
}

impl std::error::Error for ClaimError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimedSession {
    pub session_id: String,
    pub claim: CharacterClaim,
}

/// Returned by `claim_startup_token()`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartupToken {
    pub daemon_version: String,
    pub initial_revision: u64,
    #[serde(default)]
    pub telemetry_session_id: String,
}

/// Used by swarm peer discovery.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeerQuery {
    ByCharacterId(String),
    ByCharacterDisplayName(String),
    ByPetname(String),
    ByHolderId(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerInfo {
    pub holder_id: String,
    pub holder_kind: HolderKind,
    pub session_id: String,
    pub character_id: String,
    pub character_display_name: String,
    pub petname: String,
}

/// Represents the static configuration of a Character, loaded from a `CHARACTER.toml` file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharacterConfig {
    /// Stable UUID — persists even when the character is renamed or its directory moved.
    /// Written to `CHARACTER.toml` on first load if absent.
    pub id: String,
    /// Directory slug (derived from the subdirectory name, not read from TOML).
    pub name: String,
    /// Human-readable name displayed in the UI.
    pub display_name: String,
    /// CSS hex color used for UI branding (e.g., `"#7B61FF"`).
    pub color: String,
    /// High-level description of the character's role, injected into the system prompt.
    pub duties: String,
    /// Behavioral guidelines for the character, injected into the system prompt.
    pub personality: String,
}

/// A group of sessions whose recorded `character_id` no longer matches any character
/// in the loaded catalog — i.e., the character was deleted or its UUID changed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrphanGroup {
    /// The UUID that was recorded when these sessions were created.
    pub character_id: String,
    /// The display name as recorded at session-creation time (used for UI labelling).
    pub character_display_name: String,
    /// IDs of sessions in this group.
    pub session_ids: Vec<String>,
}

/// Represents the current runtime status of a Character.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CharacterStatus {
    /// The character is not currently active in any pane.
    Available,
    /// The character is currently active in a specific terminal pane.
    Active {
        /// The unique ID of the pane where this character is active.
        pane_id: String,
    },
}

/// A consolidated view of a Character's configuration and its current status.
/// This is the primary DTO sent across D-Bus for UI synchronization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharacterInfo {
    /// The static configuration loaded from disk.
    pub config: CharacterConfig,
    /// The dynamic status managed by the daemon.
    pub status: CharacterStatus,
    /// Indicates whether an `AVATAR.png` was found in the character's directory.
    pub has_avatar: bool,
}

impl CharacterInfo {
    /// Creates a new `CharacterInfo` instance with `CharacterStatus::Available`.
    #[must_use]
    pub const fn new(config: CharacterConfig, has_avatar: bool) -> Self {
        Self {
            config,
            status: CharacterStatus::Available,
            has_avatar,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_catalog() -> Vec<CharacterInfo> {
        vec![
            CharacterInfo::new(
                CharacterConfig {
                    id: "uuid-a".to_string(),
                    name: "levi-kujo".to_string(),
                    display_name: "levi".to_string(),
                    color: "#204A87".to_string(),
                    duties: "Kernel".to_string(),
                    personality: "Direct".to_string(),
                },
                false,
            ),
            CharacterInfo::new(
                CharacterConfig {
                    id: "uuid-b".to_string(),
                    name: "kuro".to_string(),
                    display_name: "Kürö".to_string(),
                    color: "#B31825".to_string(),
                    duties: "Arch".to_string(),
                    personality: "Sardonic".to_string(),
                },
                false,
            ),
        ]
    }

    #[test]
    fn resolve_character_priority_and_unicode_case_matching() {
        let catalog = sample_catalog();

        // 1. Exact UUID match wins even when Character A is first in the list
        let resolved = resolve_character(&catalog, "uuid-b", &["kuro"]).unwrap();
        assert_eq!(resolved.config.id, "uuid-b");

        // 2. Fallback by Unicode display name when UUID is stale
        let by_unicode_name = resolve_character(&catalog, "missing-uuid", &["kürö"]).unwrap();
        assert_eq!(by_unicode_name.config.id, "uuid-b");

        // 3. Fallback to first available character when deleted and no name matches
        let fallback = resolve_character(&catalog, "deleted-uuid", &["deleted-char"]).unwrap();
        assert_eq!(fallback.config.id, "uuid-a");

        // 4. Exact lookup returns None for deleted characters
        assert!(find_character_exact(&catalog, "deleted-uuid", &["deleted-char"]).is_none());
    }

    #[test]
    fn find_first_available_and_coherent_cache_sync() {
        let catalog = sample_catalog();
        let claims = vec![CharacterClaim {
            holder_id: "pane-1".to_string(),
            holder_kind: HolderKind::Pane,
            character_id: "uuid-a".to_string(),
            session_id: "sess-1".to_string(),
            petname: "red-pony".to_string(),
            owner_bus_name: ":1.1".to_string(),
        }];

        // uuid-a is claimed by pane-1, so uuid-b is returned
        let free = find_first_available_in(&catalog, &claims, None).unwrap();
        assert_eq!(free.config.id, "uuid-b");

        // Excluding pane-1 makes uuid-a available to pane-1 itself
        let free_for_pane1 = find_first_available_in(&catalog, &claims, Some("pane-1")).unwrap();
        assert_eq!(free_for_pane1.config.id, "uuid-a");

        // Coherent snapshot update mirrors both caches and revision atomically
        update_caches(catalog.clone(), claims.clone(), 42);
        let (snap_cat, snap_claims, rev) = load_coherent_registry();
        assert_eq!(rev, 42);
        assert_eq!(*snap_cat, catalog);
        assert_eq!(*snap_claims, claims);
        assert_eq!(
            find_first_available_character(None)
                .map(|c| c.config.id),
            Some("uuid-b".to_string())
        );

        // HolderKind wire round-trip
        assert_eq!(HolderKind::try_from(u8::from(HolderKind::Pane)), Ok(HolderKind::Pane));
        assert!(HolderKind::try_from(99).is_err());
    }
}
