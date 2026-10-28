use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock};
use tokio::sync::watch;

/// A globally accessible, lock-free reactive cache of the character registry.
/// This is updated via D-Bus (in the UI) and directly by `CharacterRegistry` (in the daemon).
pub static CHARACTER_CACHE: LazyLock<ArcSwap<Vec<CharacterInfo>>> =
    LazyLock::new(|| ArcSwap::from_pointee(Vec::new()));

/// A globally accessible, lock-free reactive cache of current character assignments.
pub static CLAIMS_CACHE: LazyLock<ArcSwap<Vec<CharacterClaim>>> =
    LazyLock::new(|| ArcSwap::from_pointee(Vec::new()));

static REGISTRY_REVISION_WATCH: LazyLock<(watch::Sender<u64>, watch::Receiver<u64>)> =
    LazyLock::new(|| watch::channel(0));

/// Updates both `CHARACTER_CACHE` and `CLAIMS_CACHE` atomically and notifies local subscribers.
pub fn update_caches(catalog: Vec<CharacterInfo>, claims: Vec<CharacterClaim>, revision: u64) {
    CLAIMS_CACHE.store(Arc::new(claims));
    CHARACTER_CACHE.store(Arc::new(catalog));
    let _ = REGISTRY_REVISION_WATCH.0.send(revision);
}

/// Subscribes to local cache updates triggered by `update_caches`.
pub fn subscribe_registry_changes() -> watch::Receiver<u64> {
    REGISTRY_REVISION_WATCH.1.clone()
}

/// Finds the first character in `catalog` that is not claimed in `claims`
/// (optionally ignoring a claim held by `exclude_holder_id`).
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

/// Returns the first unclaimed character from the global `CHARACTER_CACHE` and `CLAIMS_CACHE`,
/// optionally ignoring a claim held by `exclude_holder_id`.
pub fn find_first_available_character(exclude_holder_id: Option<&str>) -> Option<CharacterInfo> {
    let catalog = CHARACTER_CACHE.load();
    let claims = CLAIMS_CACHE.load();
    find_first_available_in(&catalog, &claims, exclude_holder_id).cloned()
}

/// Returns `true` if at least one character in the catalog is currently unclaimed
/// (optionally ignoring a claim held by `exclude_holder_id`).
pub fn has_available_character(exclude_holder_id: Option<&str>) -> bool {
    let catalog = CHARACTER_CACHE.load();
    let claims = CLAIMS_CACHE.load();
    find_first_available_in(&catalog, &claims, exclude_holder_id).is_some()
}

/// Resolves a character from `catalog` by matching `character_id` first, then
/// falling back to case-insensitive matches on `name` or `display_name` in
/// `fallback_names`, and finally falling back to the first character in `catalog`.
pub fn resolve_character<'a>(
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
                c.config.name.eq_ignore_ascii_case(trimmed)
                    || c.config.display_name.eq_ignore_ascii_case(trimmed)
            })
        {
            return Some(found);
        }
    }

    catalog.first()
}

/// What kind of entity holds a character claim. Today the only variant is
/// `Pane` (a terminal pane). The enum exists so future non-pane holders
/// (headless background agents, voice sessions, external apps) can be added
/// without a breaking protocol change.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HolderKind {
    /// A terminal pane in the UI. The holder_id equals the pane_id.
    Pane,
    // Future: Headless, Voice, External, ...
}

impl TryFrom<u8> for HolderKind {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(HolderKind::Pane),
            _ => Err(format!("Unknown HolderKind variant: {}", value)),
        }
    }
}

/// A single live claim of a character by some holder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    /// holder_id across character swaps within a single daemon run.
    pub petname: String,
    /// The bus-unique name of the client that owns this lease (e.g. ":1.42").
    /// Used by the daemon for NameOwnerChanged-driven cleanup; the UI ignores it.
    pub owner_bus_name: String,
}

/// One coherent snapshot of registry state. Sent on initial fetch and on every
/// push update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    /// Static for the daemon's lifetime. Carried in every snapshot so a
    /// reconnecting UI can rebuild its full mirror in one round trip.
    pub catalog: Vec<CharacterInfo>,
    pub claims: Vec<CharacterClaim>,
    /// Monotonic, incremented per emitted snapshot. UI uses it to drop
    /// out-of-order signals that arrive after a reconnect.
    pub revision: u64,
}

/// Typed claim error so the UI can show specific feedback (or recover) instead
/// of relying on a desktop-notification side effect.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimedSession {
    pub session_id: String,
    pub claim: CharacterClaim,
}

/// Returned by `claim_startup_token()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupToken {
    pub daemon_version: String,
    pub db_was_reset: bool,
    pub initial_revision: u64,
    #[serde(default)]
    pub telemetry_session_id: String,
}

/// Used by swarm peer discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PeerQuery {
    ByCharacterId(String),
    ByCharacterDisplayName(String),
    ByPetname(String),
    ByHolderId(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub holder_id: String,
    pub holder_kind: HolderKind,
    pub session_id: String,
    pub character_id: String,
    pub character_display_name: String,
    pub petname: String,
}

/// Represents the static configuration of a Character, loaded from a CHARACTER.toml file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharacterConfig {
    /// Stable UUID — persists even when the character is renamed or its directory moved.
    /// Written to CHARACTER.toml on first load if absent.
    pub id: String,
    /// Directory slug (derived from the subdirectory name, not read from TOML).
    pub name: String,
    /// Human-readable name displayed in the UI.
    pub display_name: String,
    /// CSS hex color used for UI branding (e.g., "#7B61FF").
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
    /// Indicates whether an AVATAR.png was found in the character's directory.
    pub has_avatar: bool,
}

impl CharacterInfo {
    /// Creates a new CharacterInfo instance.
    pub fn new(config: CharacterConfig, has_avatar: bool) -> Self {
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
                    display_name: "kuro".to_string(),
                    color: "#B31825".to_string(),
                    duties: "Arch".to_string(),
                    personality: "Sardonic".to_string(),
                },
                false,
            ),
        ]
    }

    #[test]
    fn test_resolve_character_priority() {
        let catalog = sample_catalog();

        // 1. Exact UUID match wins even when Character A is first in the list
        let resolved = resolve_character(&catalog, "uuid-b", &["kuro"]).unwrap();
        assert_eq!(resolved.config.id, "uuid-b");

        // 2. Fallback by display name / slug when UUID is empty or stale
        let by_name = resolve_character(&catalog, "missing-uuid", &["Kuro"]).unwrap();
        assert_eq!(by_name.config.id, "uuid-b");

        // 3. Fallback to first available character when deleted and no name matches
        let fallback = resolve_character(&catalog, "deleted-uuid", &["deleted-char"]).unwrap();
        assert_eq!(fallback.config.id, "uuid-a");
    }

    #[test]
    fn test_find_first_available_in() {
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

        // When all characters are claimed, returns None
        let all_claimed = vec![
            claims[0].clone(),
            CharacterClaim {
                holder_id: "pane-2".to_string(),
                holder_kind: HolderKind::Pane,
                character_id: "uuid-b".to_string(),
                session_id: "sess-2".to_string(),
                petname: "blue-fox".to_string(),
                owner_bus_name: ":1.1".to_string(),
            },
        ];
        assert!(find_first_available_in(&catalog, &all_claimed, None).is_none());
    }
}
