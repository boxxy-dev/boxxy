use log::{error, info, debug};
use notify::{Watcher, RecursiveMode, RecommendedWatcher, Event, EventKind};
use std::sync::Arc;
use tokio::sync::{RwLock, OnceCell, Mutex};
use boxxy_db::models::SkillRecord;
use boxxy_db::store::Store;
use boxxy_db::Db;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
}

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
    REGISTRY.get_or_init(|| async {
        let db = Arc::new(Mutex::new(None));
        if let Ok(db_inst) = Db::new().await {
            *db.lock().await = Some(db_inst);
        }
        Arc::new(SkillRegistry::new(db).await)
    }).await.clone()
}

impl SkillRegistry {
    pub async fn new(db: Arc<Mutex<Option<Db>>>) -> Self {
        let skills = Arc::new(RwLock::new(Vec::new()));
        
        // Initial load
        let initial_skills = Self::load_skills_from_disk();
        *skills.write().await = initial_skills.clone();
        
        // Initial sync to DB
        let db_clone_init = db.clone();
        tokio::spawn(async move {
            let _ = Self::sync_to_db(db_clone_init, initial_skills).await;
        });
        
        // Setup file watcher
        let mut watcher = None;
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let config_dir = dirs.config_dir();
            let skills_dir = config_dir.join("boxxyclaw").join("skills");
            
            if skills_dir.exists() {
                let skills_clone = skills.clone();
                
                // The callback runs in a blocking context, so we use a tokio channel to send 
                // events back to the async runtime, or spawn a task.
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                
                let w = notify::recommended_watcher(move |res: notify::Result<Event>| {
                    if let Ok(event) = res {
                        if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)) {
                            let _ = tx.send(());
                        }
                    }
                });

                match w {
                    Ok(mut w) => {
                        if let Err(e) = w.watch(&skills_dir, RecursiveMode::Recursive) {
                            error!("Failed to watch skills directory: {}", e);
                        } else {
                            info!("SkillRegistry: Watching {} for changes.", skills_dir.display());
                            watcher = Some(w);
                            
                            // Spawn an async task to handle debounce and reloading
                            let db_clone = db.clone();
                            tokio::spawn(async move {
                                while rx.recv().await.is_some() {
                                    // Debounce events
                                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                    // Drain any extra events that arrived during the sleep
                                    while let Ok(_) = rx.try_recv() {}
                                    
                                    debug!("SkillRegistry: File change detected, reloading skills.");
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
                        error!("Failed to initialize skills watcher: {}", e);
                    }
                }
            }
        }

        Self {
            skills,
            _watcher: watcher,
            db,
        }
    }

    pub async fn get_skills(&self) -> Vec<Skill> {
        self.skills.read().await.clone()
    }

    pub async fn search_relevant_skills(&self, query: &str, limit: i64) -> Vec<Skill> {
        let db_guard = self.db.lock().await;
        if let Some(db_val) = db_guard.as_ref() {
            let store = Store::new(db_val.pool());
            // Sanitize query for FTS5
            let sanitized = query.replace('"', "").replace('\'', "").replace('?', "");
            if let Ok(records) = store.search_skills(&sanitized, limit).await {
                // Map records back to Skill structs
                return records.into_iter().map(|r| Skill {
                    frontmatter: SkillFrontmatter {
                        name: r.name,
                        description: r.description,
                        triggers: r.triggers.split(", ").map(|s| s.to_string()).collect(),
                        pinned: r.pinned,
                    },
                    content: r.content,
                }).collect();
            }
        }
        Vec::new()
    }

    async fn sync_to_db(db: Arc<Mutex<Option<Db>>>, skills: Vec<Skill>) -> anyhow::Result<()> {
        let db_guard = db.lock().await;
        if let Some(db_val) = db_guard.as_ref() {
            let store = Store::new(db_val.pool());
            let records: Vec<SkillRecord> = skills.into_iter().map(|s| SkillRecord {
                name: s.frontmatter.name,
                description: s.frontmatter.description,
                triggers: s.frontmatter.triggers.join(", "),
                content: s.content,
                pinned: s.frontmatter.pinned,
                updated_at: None,
            }).collect();
            
            store.sync_skills(&records).await?;
            debug!("SkillRegistry: Synchronized {} skills to database.", records.len());
        }
        Ok(())
    }

    fn load_skills_from_disk() -> Vec<Skill> {
        let mut skills = Vec::new();
        if let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") {
            let config_dir = dirs.config_dir();
            let skills_dir = config_dir.join("boxxyclaw").join("skills");
            
            if let Ok(entries) = std::fs::read_dir(skills_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let mut file_path = None;
                    if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
                        file_path = Some(path);
                    } else if path.is_dir() {
                        let skill_md = path.join("SKILL.md");
                        if skill_md.exists() {
                            file_path = Some(skill_md);
                        }
                    }

                    if let Some(md_path) = file_path 
                        && let Ok(content) = std::fs::read_to_string(md_path) 
                    {
                        let mut has_frontmatter = false;
                        if (content.starts_with("---\n") || content.starts_with("---\r\n"))
                            && let Some(end_idx) = content[4..].find("\n---") 
                        {
                            let frontmatter_str = &content[4..4+end_idx];
                            let body = &content[4+end_idx+4..];
                            if let Ok(frontmatter) = serde_yml::from_str::<SkillFrontmatter>(frontmatter_str) {
                                skills.push(Skill {
                                    frontmatter,
                                    content: body.trim_start().to_string(),
                                });
                                has_frontmatter = true;
                            }
                        }
                        
                        if !has_frontmatter {
                            skills.push(Skill {
                                frontmatter: SkillFrontmatter {
                                    name: "legacy-skill".to_string(),
                                    description: "Legacy skill".to_string(),
                                    triggers: Vec::new(),
                                    pinned: false,
                                },
                                content,
                            });
                        }
                    }
                }
            }
        }
        skills
    }
}
