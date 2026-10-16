//! Persistent pane-to-character assignment mapping and character catalog.
//!
//! The daemon is the single source of truth for character assignments. Each
//! holder gets a stable `(character_id, session_id, petname)` that is stored
//! in memory for the daemon's lifetime. Assignments are NOT persisted to DB;
//! they are owned by the D-Bus unique name of the client UI process.

use anyhow::Result;
use boxxy_claw_protocol::characters::{
    CharacterClaim, CharacterInfo, CharacterStatus, ClaimError, ClaimedSession, HolderKind,
    RegistrySnapshot,
};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use tokio::sync::RwLock;

fn hash_personality(config: &boxxy_claw_protocol::characters::CharacterConfig) -> u64 {
    let mut hasher = DefaultHasher::new();
    config.duties.hash(&mut hasher);
    config.personality.hash(&mut hasher);
    hasher.finish()
}

/// Finds the replacement character in `new_catalog` when `deleted_id` is removed.
/// If `deleted_id` was in `old_catalog` at index `idx`, selects the next character
/// in `old_catalog[idx + 1..]` that still exists in `new_catalog`, wrapping around
/// to `new_catalog.first()` (the default character) if none follow.
fn find_replacement_character<'a>(
    old_catalog: &[CharacterInfo],
    new_catalog: &'a [CharacterInfo],
    deleted_id: &str,
) -> Option<&'a CharacterInfo> {
    if let Some(old_idx) = old_catalog.iter().position(|c| c.config.id == deleted_id) {
        for next_old in old_catalog.iter().skip(old_idx + 1) {
            if let Some(found) = new_catalog
                .iter()
                .find(|c| c.config.id == next_old.config.id)
            {
                return Some(found);
            }
        }
    }
    new_catalog.first()
}

pub struct CharacterRegistry {
    inner: RwLock<RegistryInner>,
    db: boxxy_db::Db,
    /// Notifier for snapshot subscribers.
    on_change: tokio::sync::broadcast::Sender<RegistrySnapshot>,
}

struct RegistryInner {
    catalog: Vec<CharacterInfo>, // immutable after load
    claims: HashMap<String /* holder_id */, OwnedClaim>,
    migrations: HashMap<String /* old_id */, String /* new_id */>,
    revision: u64,
    /// Reverse index (owner_bus_name → holder_ids) — built and maintained
    /// alongside `claims`. Used by `release_owner` to avoid scanning.
    by_owner: HashMap<String, HashSet<String>>,
}

#[derive(Debug, Clone)]
struct OwnedClaim {
    holder_kind: HolderKind,
    character_id: String,
    session_id: String,
    petname: String,
    owner_bus_name: String,
}

impl CharacterRegistry {
    pub async fn load_or_default(db: &boxxy_db::Db) -> Self {
        let catalog = boxxy_claw_protocol::character_loader::load_characters().unwrap_or_default();
        let (on_change, _) = tokio::sync::broadcast::channel(16);

        // Orphan character migration:
        // Find all sessions that reference a character_id not in the catalog,
        // and reassign them to the first available character in the catalog.
        let mut migrations = HashMap::new();
        if let Some(first_char) = catalog.first() {
            let catalog_ids: HashSet<String> =
                catalog.iter().map(|c| c.config.id.clone()).collect();

            let store = boxxy_db::store::Store::new(db.pool());
            if let Ok(session_char_ids) = store.get_distinct_session_character_ids().await {
                for char_id in session_char_ids {
                    if !catalog_ids.contains(&char_id) {
                        log::info!(
                            "Migrating orphan character ID '{}' -> '{}' ({})",
                            char_id,
                            first_char.config.id,
                            first_char.config.display_name
                        );
                        let _ = store
                            .migrate_character_sessions(
                                &char_id,
                                &first_char.config.id,
                                &first_char.config.display_name,
                            )
                            .await;
                        if !char_id.is_empty() {
                            migrations.insert(char_id, first_char.config.id.clone());
                        }
                    }
                }
            }
        }

        let inner = RegistryInner {
            catalog,
            claims: HashMap::new(),
            migrations,
            revision: 0,
            by_owner: HashMap::new(),
        };
        let initial_snapshot = inner.snapshot();
        boxxy_claw_protocol::characters::update_caches(
            initial_snapshot.catalog,
            initial_snapshot.claims,
            initial_snapshot.revision,
        );

        Self {
            inner: RwLock::new(inner),
            db: db.clone(),
            on_change,
        }
    }

    pub async fn snapshot(&self) -> RegistrySnapshot {
        let inner = self.inner.read().await;
        inner.snapshot()
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<RegistrySnapshot> {
        self.on_change.subscribe()
    }

    /// Returns `(changed_char_ids, migrated_holder_ids)`.
    /// - `changed_char_ids`: character UUIDs whose duties/personality content changed.
    /// - `migrated_holder_ids`: pane holder IDs whose character was deleted and remapped.
    /// Kept separate so callers can match on `claim.character_id` vs `claim.holder_id`
    /// without accidentally invalidating unrelated sessions sharing the fallback character.
    pub async fn reload_catalog(&self) -> Result<(Vec<String>, Vec<String>)> {
        // 1. Load new catalog (if all characters were deleted, load_characters rebuilds defaults)
        let new_catalog = boxxy_claw_protocol::character_loader::load_characters()?;

        let mut inner = self.inner.write().await;

        // 2. Snapshot old catalog & compute hashes of OLD catalog
        let old_catalog = inner.catalog.clone();
        let mut old_hashes = HashMap::new();
        for c in &old_catalog {
            old_hashes.insert(c.config.id.clone(), hash_personality(&c.config));
        }

        // 3. Replace catalog
        inner.catalog = new_catalog;

        // 4. Find changed character_ids (based on duties + personality)
        let mut changed_char_ids = Vec::new();
        for c in &inner.catalog {
            let new_hash = hash_personality(&c.config);
            if let Some(old_hash) = old_hashes.get(&c.config.id)
                && new_hash != *old_hash
            {
                changed_char_ids.push(c.config.id.clone());
            }
        }

        // 5. Orphan check (migrate deleted characters to the next character in the list,
        // or the first/default character if wrapping around or rebuilding defaults).
        let catalog_ids: HashSet<String> =
            inner.catalog.iter().map(|c| c.config.id.clone()).collect();
        let mut new_migrations: HashMap<String, (String, String)> = HashMap::new();
        let mut migrated_holder_ids = Vec::new();

        if !inner.catalog.is_empty() {
            // Check removed characters from old_catalog
            for old_char in &old_catalog {
                if !catalog_ids.contains(&old_char.config.id)
                    && let Some(target) = find_replacement_character(
                        &old_catalog,
                        &inner.catalog,
                        &old_char.config.id,
                    )
                {
                    new_migrations.insert(
                        old_char.config.id.clone(),
                        (
                            target.config.id.clone(),
                            target.config.display_name.clone(),
                        ),
                    );
                }
            }

            // Check active claims
            for (holder_id, claim) in &inner.claims {
                if !catalog_ids.contains(&claim.character_id)
                    && let Some(target) = find_replacement_character(
                        &old_catalog,
                        &inner.catalog,
                        &claim.character_id,
                    )
                {
                    log::warn!(
                        "Character {} was deleted, mapping holder {} to {} ({})",
                        claim.character_id,
                        holder_id,
                        target.config.id,
                        target.config.display_name
                    );
                    new_migrations.insert(
                        claim.character_id.clone(),
                        (
                            target.config.id.clone(),
                            target.config.display_name.clone(),
                        ),
                    );
                    migrated_holder_ids.push(holder_id.clone());
                }
            }

            for claim in inner.claims.values_mut() {
                if let Some((new_id, _)) = new_migrations.get(&claim.character_id) {
                    claim.character_id = new_id.clone();
                }
            }

            // Check DB sessions for any orphaned character_ids
            let store = boxxy_db::store::Store::new(self.db.pool());
            if let Ok(session_char_ids) = store.get_distinct_session_character_ids().await {
                for char_id in session_char_ids {
                    if !catalog_ids.contains(&char_id)
                        && !new_migrations.contains_key(&char_id)
                        && let Some(target) =
                            find_replacement_character(&old_catalog, &inner.catalog, &char_id)
                    {
                        new_migrations.insert(
                            char_id,
                            (
                                target.config.id.clone(),
                                target.config.display_name.clone(),
                            ),
                        );
                    }
                }
            }

            // Persist all session character migrations to SQLite
            for (old_id, (new_id, new_display_name)) in &new_migrations {
                let _ = store
                    .migrate_character_sessions(old_id, new_id, new_display_name)
                    .await;
            }
        }

        // Update transitive migrations so A -> B -> C resolves A -> C
        for existing_target in inner.migrations.values_mut() {
            if let Some((new_id, _)) = new_migrations.get(existing_target) {
                *existing_target = new_id.clone();
            }
        }
        for (old_id, (new_id, _)) in new_migrations {
            if !old_id.is_empty() {
                inner.migrations.insert(old_id, new_id);
            }
        }

        // 6. Broadcast updated registry and sync in-process caches
        inner.revision += 1;
        let snapshot = inner.snapshot();
        boxxy_claw_protocol::characters::update_caches(
            snapshot.catalog.clone(),
            snapshot.claims.clone(),
            snapshot.revision,
        );
        let _ = self.on_change.send(snapshot);

        Ok((changed_char_ids, migrated_holder_ids))
    }

    /// Resolves the target character for a resumed session and updates `holder_id`'s
    /// active claim in the registry so all panes and caches reflect the session's character.
    pub async fn resolve_and_update_holder_character(
        &self,
        holder_id: &str,
        session_character_id: &str,
        session_display_name: &str,
        session_agent_name: Option<&str>,
    ) -> Option<(String, String)> {
        let mut inner = self.inner.write().await;

        let effective_id = inner
            .migrations
            .get(session_character_id)
            .map(String::as_str)
            .unwrap_or(session_character_id);

        let fallback_names = [
            session_display_name,
            session_agent_name.unwrap_or_default(),
        ];
        let resolved = boxxy_claw_protocol::characters::resolve_character(
            &inner.catalog,
            effective_id,
            &fallback_names,
        )?;

        let target_id = resolved.config.id.clone();
        let target_display = resolved.config.display_name.clone();

        let mut claim_changed = false;
        if let Some(claim) = inner.claims.get_mut(holder_id)
            && claim.character_id != target_id
        {
            claim.character_id = target_id.clone();
            claim_changed = true;
        }

        if claim_changed {
            inner.revision += 1;
            let snapshot = inner.snapshot();
            boxxy_claw_protocol::characters::update_caches(
                snapshot.catalog.clone(),
                snapshot.claims.clone(),
                snapshot.revision,
            );
            let _ = self.on_change.send(snapshot);
        }

        Some((target_id, target_display))
    }

    pub async fn try_claim(
        &self,
        holder_id: String,
        holder_kind: HolderKind,
        character_id: String,
        owner_bus_name: String,
    ) -> Result<ClaimedSession, ClaimError> {
        let mut inner = self.inner.write().await;

        // 1. Check for migrations
        if let Some(to_id) = inner.migrations.get(&character_id) {
            return Err(ClaimError::Migrated {
                from_id: character_id,
                to_id: to_id.clone(),
            });
        }

        // 2. Validate character exists
        if !inner.catalog.iter().any(|c| c.config.id == character_id) {
            return Err(ClaimError::UnknownCharacter { character_id });
        }

        // 3. Validate character is not already taken by another holder
        for (other_holder_id, claim) in &inner.claims {
            if other_holder_id != &holder_id && claim.character_id == character_id {
                let holder_display_name = inner
                    .catalog
                    .iter()
                    .find(|c| c.config.id == character_id)
                    .map(|c| c.config.display_name.clone())
                    .unwrap_or_else(|| "Unknown".to_string());

                return Err(ClaimError::AlreadyTaken {
                    holder_id: other_holder_id.clone(),
                    holder_kind: claim.holder_kind,
                    holder_display_name,
                });
            }
        }

        // 4. Mint or reuse identity
        let petname = if let Some(existing) = inner.claims.get(&holder_id) {
            existing.petname.clone()
        } else {
            // New holder! Mint a petname.
            petname::petname(2, "-").unwrap_or_else(|| "red-pony".to_string())
        };

        let session_id = uuid::Uuid::new_v4().to_string();

        let new_claim = OwnedClaim {
            holder_kind,
            character_id: character_id.clone(),
            session_id: session_id.clone(),
            petname: petname.clone(),
            owner_bus_name: owner_bus_name.clone(),
        };

        // 5. Record claim
        inner.claims.insert(holder_id.clone(), new_claim.clone());
        inner
            .by_owner
            .entry(owner_bus_name.clone())
            .or_default()
            .insert(holder_id.clone());

        inner.revision += 1;
        let snapshot = inner.snapshot();
        boxxy_claw_protocol::characters::update_caches(
            snapshot.catalog.clone(),
            snapshot.claims.clone(),
            snapshot.revision,
        );
        let _ = self.on_change.send(snapshot);

        Ok(ClaimedSession {
            session_id: session_id.clone(),
            claim: CharacterClaim {
                holder_id,
                holder_kind,
                character_id,
                session_id,
                petname,
                owner_bus_name,
            },
        })
    }

    pub async fn release_holder(&self, holder_id: &str) -> Option<CharacterClaim> {
        let mut inner = self.inner.write().await;
        if let Some(claim) = inner.claims.remove(holder_id) {
            if let Some(holders) = inner.by_owner.get_mut(&claim.owner_bus_name) {
                holders.remove(holder_id);
                if holders.is_empty() {
                    inner.by_owner.remove(&claim.owner_bus_name);
                }
            }
            inner.revision += 1;
            let snapshot = inner.snapshot();
            boxxy_claw_protocol::characters::update_caches(
                snapshot.catalog.clone(),
                snapshot.claims.clone(),
                snapshot.revision,
            );
            let _ = self.on_change.send(snapshot);

            return Some(CharacterClaim {
                holder_id: holder_id.to_string(),
                holder_kind: claim.holder_kind,
                character_id: claim.character_id,
                session_id: claim.session_id,
                petname: claim.petname,
                owner_bus_name: claim.owner_bus_name,
            });
        }
        None
    }

    pub async fn release_owner(&self, owner_bus_name: &str) -> Vec<CharacterClaim> {
        let mut inner = self.inner.write().await;
        let Some(holder_ids) = inner.by_owner.remove(owner_bus_name) else {
            return Vec::new();
        };

        let holder_ids_vec: Vec<String> = holder_ids.into_iter().collect();
        let mut released_claims = Vec::new();
        for holder_id in &holder_ids_vec {
            if let Some(claim) = inner.claims.remove(holder_id) {
                released_claims.push(CharacterClaim {
                    holder_id: holder_id.clone(),
                    holder_kind: claim.holder_kind,
                    character_id: claim.character_id,
                    session_id: claim.session_id,
                    petname: claim.petname,
                    owner_bus_name: claim.owner_bus_name,
                });
            }
        }

        if !released_claims.is_empty() {
            inner.revision += 1;
            let snapshot = inner.snapshot();
            boxxy_claw_protocol::characters::update_caches(
                snapshot.catalog.clone(),
                snapshot.claims.clone(),
                snapshot.revision,
            );
            let _ = self.on_change.send(snapshot);
        }

        released_claims
    }

    pub async fn has_owner(&self, owner_bus_name: &str) -> bool {
        let inner = self.inner.read().await;
        inner.by_owner.contains_key(owner_bus_name)
    }
}

impl RegistryInner {
    fn snapshot(&self) -> RegistrySnapshot {
        let mut catalog = self.catalog.clone();
        let mut claims = Vec::new();

        for (holder_id, owned) in &self.claims {
            claims.push(CharacterClaim {
                holder_id: holder_id.clone(),
                holder_kind: owned.holder_kind,
                character_id: owned.character_id.clone(),
                session_id: owned.session_id.clone(),
                petname: owned.petname.clone(),
                owner_bus_name: owned.owner_bus_name.clone(),
            });

            // Update status in the catalog snapshot
            if let Some(info) = catalog
                .iter_mut()
                .find(|c| c.config.id == owned.character_id)
            {
                if owned.holder_kind == HolderKind::Pane {
                    info.status = CharacterStatus::Active {
                        pane_id: holder_id.clone(),
                    };
                }
            }
        }

        RegistrySnapshot {
            catalog,
            claims,
            revision: self.revision,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_claw_protocol::characters::CharacterConfig;

    fn make_char(id: &str, name: &str) -> CharacterInfo {
        CharacterInfo::new(
            CharacterConfig {
                id: id.to_string(),
                name: name.to_string(),
                display_name: name.to_string(),
                color: "#000000".to_string(),
                duties: String::new(),
                personality: String::new(),
            },
            false,
        )
    }

    #[test]
    fn test_find_replacement_character_transfers_to_next_or_wraps() {
        let c1 = make_char("id-1", "char1");
        let c2 = make_char("id-2", "char2");
        let c3 = make_char("id-3", "char3");
        let old_catalog = vec![c1.clone(), c2.clone(), c3.clone()];

        // Removing middle character (c2) transfers to next character in list (c3)
        let after_remove_c2 = vec![c1.clone(), c3.clone()];
        let target = find_replacement_character(&old_catalog, &after_remove_c2, "id-2").unwrap();
        assert_eq!(target.config.id, "id-3");

        // Removing last character (c3) wraps around to first character (c1)
        let after_remove_c3 = vec![c1.clone(), c2.clone()];
        let target_last =
            find_replacement_character(&old_catalog, &after_remove_c3, "id-3").unwrap();
        assert_eq!(target_last.config.id, "id-1");

        // Removing first character (c1) transfers to next character (c2)
        let after_remove_c1 = vec![c2.clone(), c3.clone()];
        let target_first =
            find_replacement_character(&old_catalog, &after_remove_c1, "id-1").unwrap();
        assert_eq!(target_first.config.id, "id-2");
    }
}
