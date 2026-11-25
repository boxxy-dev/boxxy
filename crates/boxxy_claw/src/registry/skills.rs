pub use boxxy_claw_protocol::skill_loader::SkillFrontmatter;
use boxxy_db::Db;
use boxxy_db::models::SkillRecord;
use boxxy_db::store::Store;
use log::{debug, info, warn};
use notify::{
    Config as NotifyConfig, ErrorKind, Event, EventKindMask, PathOp, RecommendedWatcher,
    RecursiveMode, WatchPathConfig, Watcher,
};
use std::ffi::OsStr;
use std::path::{Component, Path};
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell, RwLock};

/// Per-watch `notify` v9 `user_data` identifiers distinguishing events on the recursive
/// `skills/` tree from the non-recursive parent watch (`boxxyclaw/`).
const WATCH_SKILLS_DIR: u64 = 1;
const WATCH_SKILLS_PARENT: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkillWatchAction {
    Ignore,
    Reload { ensure_watch: bool },
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub frontmatter: SkillFrontmatter,
    pub content: String,
}

pub struct SkillRegistry {
    skills: Arc<RwLock<Vec<Skill>>>,
    _watcher: Option<Arc<std::sync::Mutex<RecommendedWatcher>>>,
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

fn is_hidden_or_temp_skill_name(name: &OsStr) -> bool {
    let Some(s) = name.to_str() else {
        return true;
    };
    s.is_empty()
        || s.starts_with('.')
        || s.ends_with('~')
        || Path::new(s)
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| {
                ext.eq_ignore_ascii_case("tmp")
                    || ext.eq_ignore_ascii_case("swp")
                    || ext.eq_ignore_ascii_case("swo")
                    || ext.eq_ignore_ascii_case("bak")
            })
}

/// Returns `true` when `path` inside `skills_dir` can affect the loaded skill catalog:
/// - `skills_dir` itself (`depth == 0`)
/// - Direct skill files (`skills/<slug>.md`) or skill package directories (`skills/<slug>`)
/// - Package manifests (`skills/<slug>/SKILL.md`)
///
/// Ignores hidden/swap files and runtime files deeper inside `<slug>/` (`depth >= 3`, such as
/// helper script caches, `__pycache__`, or `node_modules`) so they never re-index SQLite.
fn is_relevant_skill_path(skills_dir: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(skills_dir) else {
        return false;
    };
    if rel.as_os_str().is_empty() {
        return true;
    }

    let mut depth = 0usize;
    let mut last_name: Option<&OsStr> = None;
    for comp in rel.components() {
        let Component::Normal(name) = comp else {
            return false;
        };
        if is_hidden_or_temp_skill_name(name) {
            return false;
        }
        depth += 1;
        last_name = Some(name);
        if depth > 2 {
            return false;
        }
    }

    match depth {
        0 => true,
        1 => {
            // Direct `.md` skill file (`<slug>.md`) or skill package directory (`<slug>`,
            // including dotted directory slugs like `node.js` on rename/removal).
            let name = last_name.unwrap_or_default();
            Path::new(name)
                .extension()
                .and_then(OsStr::to_str)
                .is_none_or(|ext| ext.eq_ignore_ascii_case("md") || !path.is_file())
        }
        2 => last_name
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("skill.md")),
        _ => false,
    }
}

fn classify_skill_event(skills_dir: &Path, res: &notify::Result<Event>) -> SkillWatchAction {
    match res {
        Ok(event) => {
            // Handle rescan notifications before filtering by user_data, kind, or path.
            if event.need_rescan() {
                return SkillWatchAction::Reload { ensure_watch: true };
            }

            if event.user_data() == Some(WATCH_SKILLS_PARENT) {
                return if event.paths.iter().any(|p| p == skills_dir) {
                    SkillWatchAction::Reload { ensure_watch: true }
                } else {
                    SkillWatchAction::Ignore
                };
            }

            if !(event.kind.is_create() || event.kind.is_modify() || event.kind.is_remove()) {
                return SkillWatchAction::Ignore;
            }

            if event.paths.iter().any(|p| p == skills_dir) {
                SkillWatchAction::Reload { ensure_watch: true }
            } else if event.paths.iter().any(|p| is_relevant_skill_path(skills_dir, p)) {
                SkillWatchAction::Reload {
                    ensure_watch: false,
                }
            } else {
                SkillWatchAction::Ignore
            }
        }
        Err(err) => match &err.kind {
            ErrorKind::PathNotFound => {
                debug!("SkillRegistry watched path missing ({:?}): {err}", err.paths);
                SkillWatchAction::Reload { ensure_watch: true }
            }
            ErrorKind::MaxFilesWatch => {
                warn!(
                    "SkillRegistry watcher hit OS file watch limit ({err}). \
                     Consider increasing 'fs.inotify.max_user_watches'."
                );
                SkillWatchAction::Ignore
            }
            ErrorKind::WatchStartFailed => {
                warn!("SkillRegistry watcher failed to start or restart stream: {err}");
                SkillWatchAction::Reload { ensure_watch: true }
            }
            #[cfg(unix)]
            ErrorKind::Io(io_err) if io_err.raw_os_error() == Some(24) => {
                // OS error 24 = EMFILE (`fs.inotify.max_user_instances` or open FD limit)
                warn!(
                    "SkillRegistry watcher exhausted file descriptors ({err}). \
                     Check 'fs.inotify.max_user_instances'."
                );
                SkillWatchAction::Ignore
            }
            _ => {
                warn!("SkillRegistry watcher error: {err}");
                SkillWatchAction::Ignore
            }
        },
    }
}

fn init_skills_watcher(
    skills_dir: &Path,
    tx: tokio::sync::mpsc::UnboundedSender<notify::Result<Event>>,
) -> notify::Result<RecommendedWatcher> {
    // Filter to `EventKindMask::CORE` at the kernel (`inotify`) level so reading `SKILL.md`
    // or listing `skills/` never generates `IN_OPEN` / `IN_CLOSE_NOWRITE` self-trigger events.
    let config = NotifyConfig::default().with_event_kinds(EventKindMask::CORE);
    let mut watcher = RecommendedWatcher::new(tx, config)?;

    let mut ops = vec![PathOp::Watch(
        skills_dir.to_path_buf(),
        WatchPathConfig::new(RecursiveMode::Recursive).with_user_data(WATCH_SKILLS_DIR),
    )];
    if let Some(parent) = skills_dir.parent()
        && !parent.as_os_str().is_empty()
        && parent.exists()
    {
        ops.push(PathOp::Watch(
            parent.to_path_buf(),
            WatchPathConfig::new(RecursiveMode::NonRecursive).with_user_data(WATCH_SKILLS_PARENT),
        ));
    }

    if let Err(err) = watcher.update_paths(ops) {
        if err.source.user_data() == Some(WATCH_SKILLS_PARENT) {
            debug!(
                "SkillRegistry optional parent watch for {} unavailable: {}",
                skills_dir.display(),
                err.source
            );
        } else {
            return Err(err.source);
        }
    }

    Ok(watcher)
}

fn ensure_skills_watch(watcher: &mut RecommendedWatcher, skills_dir: &Path) {
    if !skills_dir.exists() {
        return;
    }
    if let Err(e) = watcher.watch_with(
        skills_dir,
        WatchPathConfig::new(RecursiveMode::Recursive).with_user_data(WATCH_SKILLS_DIR),
    ) {
        warn!(
            "SkillRegistry failed to re-watch {}: {e}",
            skills_dir.display()
        );
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

        // Sync to SQLite before returning so Turn-1 FTS5 skill lookup (`search_relevant_skills`)
        // never races ahead of initial skill indexing on a fresh database.
        let _ = Self::sync_to_db(Arc::clone(&db), initial_skills).await;

        // Setup file watcher off the async executor using `notify` v9's native Tokio sender
        // handler, `EventKindMask::CORE` kernel filtering, and `user_data`-tagged batch watches.
        let mut watcher = None;
        let skills_dir = boxxy_claw_protocol::skill_loader::get_skills_dir();

        if skills_dir.exists() {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let dir_for_init = skills_dir.clone();

            match tokio::task::spawn_blocking(move || init_skills_watcher(&dir_for_init, tx)).await
            {
                Ok(Ok(w)) => {
                    info!(
                        "SkillRegistry: Watching {} for changes.",
                        skills_dir.display()
                    );
                    let watcher_arc = Arc::new(std::sync::Mutex::new(w));
                    let weak_watcher = Arc::downgrade(&watcher_arc);
                    watcher = Some(watcher_arc);

                    let skills_clone = Arc::clone(&skills);
                    let db_clone = Arc::clone(&db);
                    tokio::spawn(async move {
                        while let Some(first) = rx.recv().await {
                            let SkillWatchAction::Reload { mut ensure_watch } =
                                classify_skill_event(&skills_dir, &first)
                            else {
                                continue;
                            };

                            // Debounce bursty filesystem writes and coalesce queued notifications.
                            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                            while let Ok(extra) = rx.try_recv() {
                                if let SkillWatchAction::Reload {
                                    ensure_watch: extra_rewatch,
                                } = classify_skill_event(&skills_dir, &extra)
                                {
                                    ensure_watch |= extra_rewatch;
                                }
                            }

                            debug!("SkillRegistry: File change detected, reloading skills.");
                            let dir_for_step = skills_dir.clone();
                            let weak_for_step = weak_watcher.clone();
                            let new_skills = tokio::task::spawn_blocking(move || {
                                if ensure_watch
                                    && let Some(w_arc) = weak_for_step.upgrade()
                                    && let Ok(mut guard) = w_arc.lock()
                                {
                                    ensure_skills_watch(&mut guard, &dir_for_step);
                                }
                                boxxy_preferences::Settings::reload();
                                Self::load_skills_from_disk()
                            })
                            .await
                            .unwrap_or_default();
                            *skills_clone.write().await = new_skills.clone();

                            // Sync to DB
                            let _ = Self::sync_to_db(Arc::clone(&db_clone), new_skills).await;
                            debug!("SkillRegistry: Reload and DB sync complete.");
                        }
                    });
                }
                Ok(Err(e)) => {
                    warn!(
                        "Failed to initialize skills watcher: {e}. \
                        (If os error 24, your system may have exhausted 'fs.inotify.max_user_instances'.) \
                        Skills will be loaded once but will not hot-reload."
                    );
                }
                Err(join_err) => {
                    warn!("SkillRegistry watcher setup task failed: {join_err}");
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
    "repo",
    "repos",
    "repositories",
    "repository",
    "run",
    "say",
    "script",
    "search",
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

fn words_stem_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    // Allow simple plural (`layer`/`layers`, `theme`/`themes`) or >= 6-char shared stems
    // (`contain`/`container`), while preventing short false-positive prefix collisions
    // such as `repo` matching `report`/`reporter` or `port` matching `portable`.
    (shorter.len() >= 3
        && longer.len() == shorter.len() + 1
        && longer.ends_with('s')
        && longer.starts_with(shorter))
        || (shorter.len() >= 6 && longer.starts_with(shorter))
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

    query_tokens
        .iter()
        .any(|qt| header_words.iter().any(|hw| words_stem_match(hw, qt)))
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

        // Short prefix stems like `repo` must not false-positive against `report` / `reporter`.
        let generic_repo_tokens = extract_skill_match_tokens("list module entries in the repo");
        assert!(!skill_header_matches_query(
            "Issue Reporter",
            "Triage software bugs and file upstream tickets",
            "report bug, bug report, ticket",
            &generic_repo_tokens,
        ));

        let generic_search_tokens =
            extract_skill_match_tokens("search online reference documentation");
        assert!(!skill_header_matches_query(
            "Issue Reporter",
            "Triage software bugs and file upstream tickets",
            "report bug, bug report, ticket",
            &generic_search_tokens,
        ));

        // Explicit plural domain words (`bugs` -> `bug`) still match cleanly.
        let plural_domain_tokens = extract_skill_match_tokens("triage upstream bugs");
        assert!(skill_header_matches_query(
            "Issue Reporter",
            "Triage software bugs and file upstream tickets",
            "report bug, bug report, ticket",
            &plural_domain_tokens,
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

    #[test]
    fn skill_event_classifier_handles_rescan_user_data_and_noise() {
        use notify::event::{CreateKind, Flag, ModifyKind};
        use notify::{Error, EventKind};

        let parent = Path::new("/tmp/synthetic_cfg/boxxyclaw");
        let skills_dir = parent.join("skills");

        // Rescan notifications always trigger a reload + watch verification, even without paths.
        let rescan = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert_eq!(
            classify_skill_event(&skills_dir, &Ok(rescan)),
            SkillWatchAction::Reload { ensure_watch: true }
        );

        // Parent watch (`WATCH_SKILLS_PARENT`) ignores sibling files (`MEMORY.md`) but reloads
        // with `ensure_watch: true` when `skills_dir` itself is recreated/replaced.
        let sibling_write = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(parent.join("MEMORY.md"))
            .set_user_data(WATCH_SKILLS_PARENT);
        assert_eq!(
            classify_skill_event(&skills_dir, &Ok(sibling_write)),
            SkillWatchAction::Ignore
        );

        let root_created = Event::new(EventKind::Create(CreateKind::Folder))
            .add_path(skills_dir.clone())
            .set_user_data(WATCH_SKILLS_PARENT);
        assert_eq!(
            classify_skill_event(&skills_dir, &Ok(root_created)),
            SkillWatchAction::Reload { ensure_watch: true }
        );

        // Skill manifests and top-level `.md` files reload; swap files and nested depth >= 3 caches do not.
        let manifest_edit = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(skills_dir.join("alpha-skill/SKILL.md"))
            .set_user_data(WATCH_SKILLS_DIR);
        assert_eq!(
            classify_skill_event(&skills_dir, &Ok(manifest_edit)),
            SkillWatchAction::Reload {
                ensure_watch: false
            }
        );

        let swap_edit = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(skills_dir.join("alpha-skill/.SKILL.md.swp"))
            .set_user_data(WATCH_SKILLS_DIR);
        assert_eq!(
            classify_skill_event(&skills_dir, &Ok(swap_edit)),
            SkillWatchAction::Ignore
        );

        let nested_runtime = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(skills_dir.join("alpha-skill/scripts/cache.json"))
            .set_user_data(WATCH_SKILLS_DIR);
        assert_eq!(
            classify_skill_event(&skills_dir, &Ok(nested_runtime)),
            SkillWatchAction::Ignore
        );

        // `ErrorKind::PathNotFound` schedules recovery; `MaxFilesWatch` logs without looping.
        assert_eq!(
            classify_skill_event(&skills_dir, &Err(Error::path_not_found())),
            SkillWatchAction::Reload { ensure_watch: true }
        );
        assert_eq!(
            classify_skill_event(&skills_dir, &Err(Error::new(ErrorKind::MaxFilesWatch))),
            SkillWatchAction::Ignore
        );
    }

    #[tokio::test]
    async fn skills_watcher_ignores_reads_and_recovers_after_dir_replacement() {
        use std::time::Duration;

        let tmp = tempfile::tempdir().unwrap();
        let boxxyclaw_dir = tmp.path().join("boxxyclaw");
        let skills_dir = boxxyclaw_dir.join("skills");
        let alpha_dir = skills_dir.join("alpha-skill");
        std::fs::create_dir_all(&alpha_dir).unwrap();
        let skill_md = alpha_dir.join("SKILL.md");
        std::fs::write(
            &skill_md,
            "---\nname: Alpha Skill\ndescription: Initial\n---\nBody",
        )
        .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher = init_skills_watcher(&skills_dir, tx).unwrap();

        // Reading SKILL.md must not emit kernel access events (`EventKindMask::CORE`).
        let _ = std::fs::read_to_string(&skill_md).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(120), rx.recv())
                .await
                .is_err(),
            "reading SKILL.md must not emit events when EventKindMask::CORE is configured"
        );

        // Replacing `skills/` (`remove_dir_all` + `rename`) emits a reload with `ensure_watch: true`.
        let staged = tmp.path().join("staged_skills");
        let beta_dir = staged.join("beta-skill");
        std::fs::create_dir_all(&beta_dir).unwrap();
        std::fs::write(
            beta_dir.join("SKILL.md"),
            "---\nname: Beta Skill\ndescription: Replacement\n---\nBody",
        )
        .unwrap();

        std::fs::remove_dir_all(&skills_dir).unwrap();
        std::fs::rename(&staged, &skills_dir).unwrap();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut saw_ensure_watch = false;
        while tokio::time::Instant::now() < deadline && !saw_ensure_watch {
            let ev = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .expect("timed out waiting for skills dir replacement event")
                .expect("watcher channel closed");
            if let SkillWatchAction::Reload { ensure_watch } =
                classify_skill_event(&skills_dir, &ev)
            {
                saw_ensure_watch |= ensure_watch;
            }
        }
        assert!(saw_ensure_watch);
        while rx.try_recv().is_ok() {}

        ensure_skills_watch(&mut watcher, &skills_dir);

        // Subsequent edits inside the replaced `skills/` directory are reported cleanly.
        std::fs::write(
            skills_dir.join("beta-skill/SKILL.md"),
            "---\nname: Beta Skill\ndescription: Updated\n---\nUpdated body",
        )
        .unwrap();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let ev = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .expect("timed out waiting for edit after skills dir replacement")
                .expect("watcher channel closed");
            if matches!(
                classify_skill_event(&skills_dir, &ev),
                SkillWatchAction::Reload { .. }
            ) {
                break;
            }
        }
    }
}
