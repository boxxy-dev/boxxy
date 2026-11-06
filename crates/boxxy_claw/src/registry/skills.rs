pub use boxxy_claw_protocol::skill_loader::SkillFrontmatter;
use boxxy_db::Db;
use boxxy_db::models::SkillRecord;
use boxxy_db::store::Store;
use log::{debug, error, info};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell, RwLock};

#[derive(Clone, Debug)]
pub struct Skill {
    pub frontmatter: SkillFrontmatter,
    pub content: String,
}

pub struct SkillRegistry {
    skills: Arc<RwLock<Vec<Skill>>>,
    _watcher: Option<RecommendedWatcher>,
    db: Arc<Mutex<Option<Db>>>,
}

static REGISTRY: OnceCell<Arc<SkillRegistry>> = OnceCell::const_new();

pub async fn global_registry() -> Arc<SkillRegistry> {
    REGISTRY
        .get_or_init(|| async {
            let db = Arc::new(Mutex::new(None));
            if let Ok(db_inst) = Db::new().await {
                *db.lock().await = Some(db_inst);
            }
            Arc::new(SkillRegistry::new(db).await)
        })
        .await
        .clone()
}

/// Reloads the skill registry only if it has already been initialized.
/// Avoids eagerly loading skills or opening SQLite at startup before an agent turn runs.
pub async fn reload_if_initialized() {
    if let Some(registry) = REGISTRY.get() {
        registry.reload().await;
    }
}

impl SkillRegistry {
    pub async fn new(db: Arc<Mutex<Option<Db>>>) -> Self {
        let skills = Arc::new(RwLock::new(Vec::new()));

        // Initial load (skips any user-disabled skills without reading their files)
        let initial_skills = Self::load_skills_from_disk();
        *skills.write().await = initial_skills.clone();

        // Initial sync to DB
        let db_clone_init = db.clone();
        tokio::spawn(async move {
            let _ = Self::sync_to_db(db_clone_init, initial_skills).await;
        });

        // Setup file watcher
        let mut watcher = None;
        let skills_dir = boxxy_claw_protocol::skill_loader::get_skills_dir();

        if skills_dir.exists() {
            let skills_clone = skills.clone();

            // The callback runs in a blocking context, so we use a tokio channel to send
            // events back to the async runtime, or spawn a task.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

            let w = notify::recommended_watcher(move |res: notify::Result<Event>| {
                if let Ok(event) = res
                    && matches!(
                        event.kind,
                        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                    )
                {
                    let _ = tx.send(());
                }
            });

            match w {
                Ok(mut w) => {
                    if let Err(e) = w.watch(&skills_dir, RecursiveMode::Recursive) {
                        error!("Failed to watch skills directory: {}", e);
                    } else {
                        info!(
                            "SkillRegistry: Watching {} for changes.",
                            skills_dir.display()
                        );
                        watcher = Some(w);

                        // Spawn an async task to handle debounce and reloading
                        let db_clone = db.clone();
                        tokio::spawn(async move {
                            while rx.recv().await.is_some() {
                                // Debounce events
                                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                // Drain any extra events that arrived during the sleep
                                while rx.try_recv().is_ok() {}

                                debug!(
                                    "SkillRegistry: File change detected, reloading skills."
                                );
                                boxxy_preferences::Settings::reload();
                                let new_skills = Self::load_skills_from_disk();
                                *skills_clone.write().await = new_skills.clone();

                                // Sync to DB
                                let _ = Self::sync_to_db(db_clone.clone(), new_skills).await;
                                debug!("SkillRegistry: Reload and DB sync complete.");
                            }
                        });
                    }
                }
                Err(e) => {
                    log::warn!(
                        "Failed to initialize skills watcher: {}. \
                        (If os error 24, your system may have exhausted 'fs.inotify.max_user_instances'.) \
                        Skills will be loaded once but will not hot-reload.",
                        e
                    );
                }
            }
        }

        Self {
            skills,
            _watcher: watcher,
            db,
        }
    }

    pub async fn reload(&self) {
        let new_skills = Self::load_skills_from_disk();
        *self.skills.write().await = new_skills.clone();
        let _ = Self::sync_to_db(self.db.clone(), new_skills).await;
    }

    pub async fn get_skills(&self) -> Vec<Skill> {
        self.skills.read().await.clone()
    }

    pub async fn search_relevant_skills(&self, query: &str, limit: i64) -> Vec<Skill> {
        let fts_query = crate::engine::context::build_fts5_query(query);
        if fts_query.is_empty() {
            return Vec::new();
        }
        let db_guard = self.db.lock().await;
        if let Some(db_val) = db_guard.as_ref() {
            let store = Store::new(db_val.pool());
            if let Ok(records) = store.search_skills(&fts_query, limit).await {
                // Map records back to Skill structs
                return records
                    .into_iter()
                    .map(|r| Skill {
                        frontmatter: SkillFrontmatter {
                            name: r.name,
                            description: r.description,
                            triggers: r.triggers
                                .split(", ")
                                .filter(|s| !s.is_empty())
                                .map(|s| s.to_string())
                                .collect(),
                            pinned: r.pinned,
                        },
                        content: r.content,
                    })
                    .collect();
            }
        }
        Vec::new()
    }

    async fn sync_to_db(db: Arc<Mutex<Option<Db>>>, skills: Vec<Skill>) -> anyhow::Result<()> {
        let db_guard = db.lock().await;
        if let Some(db_val) = db_guard.as_ref() {
            let store = Store::new(db_val.pool());
            let records: Vec<SkillRecord> = skills
                .into_iter()
                .map(|s| SkillRecord {
                    name: s.frontmatter.name,
                    description: s.frontmatter.description,
                    triggers: s.frontmatter.triggers.join(", "),
                    content: s.content,
                    pinned: s.frontmatter.pinned,
                    updated_at: None,
                })
                .collect();

            store.sync_skills(&records).await?;
            debug!(
                "SkillRegistry: Synchronized {} skills to database.",
                records.len()
            );
        }
        Ok(())
    }

    fn load_skills_from_disk() -> Vec<Skill> {
        let skills_dir = boxxy_claw_protocol::skill_loader::get_skills_dir();
        let settings = boxxy_preferences::Settings::load();
        Self::load_enabled_skills_from_dir(&skills_dir, &settings.disabled_skills)
    }

    /// Loads only enabled skills from `skills_dir`.
    /// Entries whose on-disk `slug` appears in `disabled_skills` are skipped immediately
    /// before opening or reading the markdown file.
    pub fn load_enabled_skills_from_dir(
        skills_dir: &Path,
        disabled_skills: &[String],
    ) -> Vec<Skill> {
        let mut skills = Vec::new();
        let Ok(entries) = std::fs::read_dir(skills_dir) else {
            return skills;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Some((slug, md_path)) =
                boxxy_claw_protocol::skill_loader::resolve_skill_entry(&path)
            else {
                continue;
            };

            // Fast-path: skip disabled skills before touching the file on disk.
            if disabled_skills.iter().any(|d| d == &slug) {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(&md_path) {
                let (frontmatter, body) =
                    boxxy_claw_protocol::skill_loader::parse_skill_document(&slug, &content);
                if boxxy_claw_protocol::skill_loader::is_skill_disabled(
                    disabled_skills,
                    &slug,
                    &frontmatter.name,
                ) {
                    continue;
                }
                skills.push(Skill {
                    frontmatter,
                    content: body,
                });
            }
        }

        skills.sort_by(|a, b| a.frontmatter.name.cmp(&b.frontmatter.name));
        skills
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_skills_are_not_loaded_until_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let skills_dir = dir.path();

        let skill_a_dir = skills_dir.join("alpha-skill");
        std::fs::create_dir_all(&skill_a_dir).unwrap();
        std::fs::write(
            skill_a_dir.join("SKILL.md"),
            "---\nname: Alpha Skill\ndescription: First workflow\n---\nAlpha instructions",
        )
        .unwrap();

        let skill_b_dir = skills_dir.join("beta-skill");
        std::fs::create_dir_all(&skill_b_dir).unwrap();
        std::fs::write(
            skill_b_dir.join("SKILL.md"),
            "---\nname: Beta Skill\ndescription: Second workflow\n---\nBeta instructions",
        )
        .unwrap();

        // When beta-skill is disabled by slug, only alpha-skill is loaded.
        let disabled = vec!["beta-skill".to_string()];
        let loaded = SkillRegistry::load_enabled_skills_from_dir(skills_dir, &disabled);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].frontmatter.name, "Alpha Skill");

        // When re-enabled, both skills are loaded.
        let loaded_all = SkillRegistry::load_enabled_skills_from_dir(skills_dir, &[]);
        assert_eq!(loaded_all.len(), 2);
        assert_eq!(loaded_all[0].frontmatter.name, "Alpha Skill");
        assert_eq!(loaded_all[1].frontmatter.name, "Beta Skill");
    }
}
