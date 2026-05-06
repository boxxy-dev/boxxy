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
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;
use tokio::sync::RwLock;

fn hash_personality(config: &boxxy_claw_protocol::characters::CharacterConfig) -> u64 {
    let mut hasher = DefaultHasher::new();
    config.duties.hash(&mut hasher);
    config.personality.hash(&mut hasher);
    hasher.finish()
}

pub struct CharacterRegistry {
    inner: RwLock<RegistryInner>,
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
        // Find all sessions that reference a character_id not in the catalog.
        // Map them to the first available character in the catalog.
        let mut migrations = HashMap::new();
        if let Some(first_char) = catalog.first() {
            let catalog_ids: HashSet<String> =
                catalog.iter().map(|c| c.config.id.clone()).collect();

            let query = "SELECT DISTINCT character_id FROM sessions";
            let res = sqlx::query_as::<sqlx::Sqlite, (String,)>(query)
                .fetch_all(db.pool())
                .await;

            if let Ok(session_char_ids) = res {
                for (char_id,) in session_char_ids {
                    if !char_id.is_empty() && !catalog_ids.contains(&char_id) {
                        log::info!(
                            "Migrating orphan character ID {} -> {}",
                            char_id,
                            first_char.config.id
                        );
                        migrations.insert(char_id, first_char.config.id.clone());
                    }
                }
            }
        }

        Self {
            inner: RwLock::new(RegistryInner {
                catalog,
                claims: HashMap::new(),
                migrations,
                revision: 0,
                by_owner: HashMap::new(),
            }),
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
        // 1. Load new catalog
        let new_catalog = boxxy_claw_protocol::character_loader::load_characters()?;

        let mut inner = self.inner.write().await;

        // 2. Compute hashes of OLD catalog
        let mut old_hashes = HashMap::new();
        for c in &inner.catalog {
            old_hashes.insert(c.config.id.clone(), hash_personality(&c.config));
        }

        // 3. Replace catalog
        inner.catalog = new_catalog;

        // 4. Find changed character_ids (based on duties + personality)
        let mut changed_char_ids = Vec::new();
        for c in &inner.catalog {
            let new_hash = hash_personality(&c.config);
            if let Some(old_hash) = old_hashes.get(&c.config.id) {
                if new_hash != *old_hash {
                    changed_char_ids.push(c.config.id.clone());
                }
            }
        }

        // 5. Orphan check (migrate deleted characters)
        let catalog_ids: HashSet<String> =
            inner.catalog.iter().map(|c| c.config.id.clone()).collect();
        let mut new_migrations = HashMap::new();
        let mut migrated_holder_ids = Vec::new();
        if let Some(first_char) = inner.catalog.first() {
            for (holder_id, claim) in &inner.claims {
                if !catalog_ids.contains(&claim.character_id) {
                    log::warn!(
                        "Character {} was deleted, mapping holder {} to {}",
                        claim.character_id,
                        holder_id,
                        first_char.config.id
                    );
                    new_migrations
                        .insert(claim.character_id.clone(), first_char.config.id.clone());
                    migrated_holder_ids.push(holder_id.clone());
                }
            }
            for claim in inner.claims.values_mut() {
                if let Some(new_id) = new_migrations.get(&claim.character_id) {
                    claim.character_id = new_id.clone();
                }
            }
        }
        inner.migrations.extend(new_migrations);

        // 6. Broadcast updated registry
        inner.revision += 1;
        let _ = self.on_change.send(inner.snapshot());

        Ok((changed_char_ids, migrated_holder_ids))
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
        let _ = self.on_change.send(inner.snapshot());

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
            let _ = self.on_change.send(inner.snapshot());

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
            let _ = self.on_change.send(inner.snapshot());
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
