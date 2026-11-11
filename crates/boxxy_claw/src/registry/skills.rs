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

        // Initial load off the async executor (skips user-disabled skills without reading files)
        let initial_skills = tokio::task::spawn_blocking(Self::load_skills_from_disk)
            .await
            .unwrap_or_default();
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
            // events back to the async runtime.
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
                        error!("Failed to watch skills directory: {e}");
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

                                debug!("SkillRegistry: File change detected, reloading skills.");
                                let new_skills = tokio::task::spawn_blocking(|| {
                                    boxxy_preferences::Settings::reload();
                                    Self::load_skills_from_disk()
                                })
                                .await
                                .unwrap_or_default();
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
                        "Failed to initialize skills watcher: {e}. \
                        (If os error 24, your system may have exhausted 'fs.inotify.max_user_instances'.) \
                        Skills will be loaded once but will not hot-reload."
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
        let new_skills = tokio::task::spawn_blocking(Self::load_skills_from_disk)
            .await
            .unwrap_or_default();
        *self.skills.write().await = new_skills.clone();
        let _ = Self::sync_to_db(self.db.clone(), new_skills).await;
    }

    pub async fn get_skills(&self) -> Vec<Skill> {
        self.skills.read().await.clone()
    }

    pub async fn search_relevant_skills(&self, query: &str, limit: i64) -> Vec<Skill> {
        let query_tokens = extract_skill_match_tokens(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }
        let fts_query = crate::engine::context::build_fts5_query(query);
        if fts_query.is_empty() {
            return Vec::new();
        }
        let db_opt = self.db.lock().await.clone();
        if let Some(db_val) = db_opt {
            let store = Store::new(db_val.pool());
            // Fetch a small candidate pool so we can filter out false-positive hits where a
            // generic word only matched deep inside a CLI `--help` dump rather than the
            // skill's `name`, `description`, or `triggers`.
            let fetch_limit = limit.saturating_mul(4).max(4);
            if let Ok(records) = store.search_skills(&fts_query, fetch_limit).await {
                let take_n = usize::try_from(limit.max(0)).unwrap_or(1);
                return records
                    .into_iter()
                    .filter(|r| {
                        skill_header_matches_query(
                            &r.name,
                            &r.description,
                            &r.triggers,
                            &query_tokens,
                        )
                    })
                    .take(take_n)
                    .map(|r| Skill {
                        frontmatter: SkillFrontmatter {
                            name: r.name,
                            description: r.description,
                            triggers: r
                                .triggers
                                .split(", ")
                                .filter(|s| !s.is_empty())
                                .map(ToString::to_string)
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
        let db_opt = db.lock().await.clone();
        if let Some(db_val) = db_opt {
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
    #[must_use]
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

/// Conversational and generic action verbs excluded when checking whether a user prompt
/// genuinely relates to a skill's `name`, `description`, or `triggers`.
const SKILL_NOISE_WORDS: &[&str] = &[
    "all",
    "also",
    "any",
    "ask",
    "boxxy",
    "can",
    "change",
    "check",
    "code",
    "command",
    "commands",
    "create",
    "does",
    "edit",
    "file",
    "files",
    "fix",
    "for",
    "from",
    "good",
    "great",
    "hello",
    "help",
    "hey",
    "how",
    "into",
    "just",
    "let",
    "like",
    "look",
    "make",
    "more",
    "new",
    "nice",
    "now",
    "okay",
    "one",
    "only",
    "open",
    "out",
    "pane",
    "panes",
    "please",
    "put",
    "read",
    "run",
    "say",
    "script",
    "see",
    "set",
    "show",
    "some",
    "split",
    "splits",
    "sure",
    "tab",
    "tabs",
    "take",
    "tell",
    "terminal",
    "terminals",
    "test",
    "thank",
    "thanks",
    "that",
    "the",
    "then",
    "there",
    "these",
    "this",
    "turn",
    "update",
    "use",
    "using",
    "very",
    "want",
    "well",
    "what",
    "when",
    "where",
    "which",
    "window",
    "windows",
    "with",
    "work",
    "write",
    "you",
    "your",
];

fn extract_skill_match_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric()) {
        if raw.len() < 3 {
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        if crate::engine::context::is_fts_stopword(&lower)
            || SKILL_NOISE_WORDS.binary_search(&lower.as_str()).is_ok()
        {
            continue;
        }
        if !tokens.contains(&lower) {
            tokens.push(lower);
        }
    }
    tokens
}

fn skill_header_matches_query(
    name: &str,
    description: &str,
    triggers: &str,
    query_tokens: &[String],
) -> bool {
    let header = format!("{name} {description} {triggers}").to_ascii_lowercase();
    let header_words: Vec<&str> = header
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| {
            w.len() >= 3
                && !crate::engine::context::is_fts_stopword(w)
                && SKILL_NOISE_WORDS.binary_search(w).is_err()
        })
        .collect();

    query_tokens.iter().any(|qt| {
        header_words.iter().any(|hw| {
            *hw == qt.as_str()
                || (qt.len() >= 4 && hw.starts_with(qt.as_str()))
                || (hw.len() >= 4 && qt.starts_with(*hw))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_header_filter_rejects_incidental_body_noise_and_matches_domain_terms() {
        assert!(
            SKILL_NOISE_WORDS.windows(2).all(|w| w[0] < w[1]),
            "SKILL_NOISE_WORDS must remain strictly sorted for binary_search"
        );

        let greeting_tokens = extract_skill_match_tokens("hey there, is all good and okay?");
        assert!(!skill_header_matches_query(
            "Container Inspector",
            "Inspect OCI container layers and registries",
            "oci, podman",
            &greeting_tokens,
        ));

        let domain_tokens = extract_skill_match_tokens("inspect podman container layers");
        assert!(skill_header_matches_query(
            "Container Inspector",
            "Inspect OCI container layers and registries",
            "oci, podman",
            &domain_tokens,
        ));
    }

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
