//! High-level headless `TestWorkspace` orchestrator for Boxxy E2E workflows.
//!
//! Wires together a single shared `AgentState` (`ClawSubsystem` + `PtyRegistry`), an isolated
//! per-test workspace `TempDir`, the process-isolated `boxxy-db` handle, and a scriptable
//! `MockLlmServer` — completely free of GTK, Libadwaita, or session-bus collisions.

use crate::config_env::{
    configure_mock_llm_endpoint, ensure_isolated_config_root, seeded_catalog, set_active_claims,
};
use crate::mock_llm::MockLlmServer;
use crate::pane_driver::{CompanionTable, PaneDriver};
use crate::pty_driver::PtyShellSession;
use anyhow::{Context, Result, anyhow};
use boxxy_agent::claw::ClawSubsystem;
use boxxy_agent::core::state::AgentState;
use boxxy_claw::registry::workspace::global_workspace;
use boxxy_claw_protocol::ClawEnvironment;
use boxxy_claw_protocol::characters::{
    CharacterClaim, HolderKind, INCOGNITO_CHARACTER_ID, INCOGNITO_DISPLAY_NAME,
};
use boxxy_db::Db;
use boxxy_preferences::Settings;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use uuid::Uuid;

/// Isolated headless Boxxy workspace for deterministic E2E workflow tests.
pub struct TestWorkspace {
    pub llm: MockLlmServer,
    pub agent_state: AgentState,
    pub env: Arc<dyn ClawEnvironment>,
    pub db: Db,
    pub companions: CompanionTable,
    workspace_dir: TempDir,
    claims: Vec<CharacterClaim>,
}

impl TestWorkspace {
    /// Creates a fresh headless workspace backed by a local [`MockLlmServer`] and a clean
    /// temporary working directory.
    ///
    /// # Errors
    /// Returns an error if starting the mock server, creating temp directories, or opening SQLite fails.
    pub async fn new() -> Result<Self> {
        let config_root = ensure_isolated_config_root();
        global_workspace().await.reset_all().await;

        let llm = MockLlmServer::start().await?;
        let endpoint_name = format!("MockEndpoint-{}", Uuid::new_v4());
        configure_mock_llm_endpoint(&endpoint_name, &llm.base_url());

        let workspace_dir = tempfile::Builder::new()
            .prefix("boxxy_e2e_ws_")
            .tempdir()
            .context("create TestWorkspace TempDir")?;

        // Ensure custom BLACKLIST.md, MEMORY.md, DREAMS.md, and custom skills from an earlier test do not carry over.
        let boxxyclaw_dir = config_root.join("boxxyclaw");
        let _ = tokio::fs::remove_file(boxxyclaw_dir.join("BLACKLIST.md")).await;
        let _ = tokio::fs::remove_file(boxxyclaw_dir.join("MEMORY.md")).await;
        let _ = tokio::fs::remove_file(boxxyclaw_dir.join("DREAMS.md")).await;
        let _ = tokio::fs::remove_dir_all(boxxyclaw_dir.join("skills")).await;
        boxxy_claw::registry::skills::reload_if_initialized().await;

        let agent_state = AgentState::new();
        let claw_subsystem = ClawSubsystem::new(agent_state.clone());
        let env: Arc<dyn ClawEnvironment> = Arc::new(claw_subsystem);
        let db = Db::new().await.context("initialize isolated boxxy-db")?;
        {
            let store = boxxy_db::store::Store::new(db.pool());
            if let Ok(stale) = store.get_undreamed_interactions().await
                && !stale.is_empty()
            {
                let ids: Vec<i64> = stale.into_iter().map(|i| i.id).collect();
                let _ = store.mark_interactions_as_dreamed(&ids).await;
            }
        }

        set_active_claims(Vec::new());

        Ok(Self {
            llm,
            agent_state,
            env,
            db,
            companions: CompanionTable::new(),
            workspace_dir,
            claims: Vec::new(),
        })
    }

    #[must_use]
    pub fn config_root(&self) -> &Path {
        ensure_isolated_config_root()
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.workspace_dir.path()
    }

    /// Resolves a relative workspace file path inside the temporary project root.
    #[must_use]
    pub fn file_path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.workspace_dir.path().join(rel)
    }

    /// Writes synthetic UTF-8 file contents under the temporary project root, creating parent directories.
    ///
    /// # Errors
    /// Returns an error if directory creation or file writing fails.
    pub async fn write_file(&self, rel: impl AsRef<Path>, content: &str) -> Result<PathBuf> {
        let full = self.file_path(rel);
        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&full, content).await?;
        Ok(full)
    }

    /// Reads UTF-8 file contents from a relative path inside the temporary project root.
    ///
    /// # Errors
    /// Returns an error if reading the file fails.
    pub async fn read_file(&self, rel: impl AsRef<Path>) -> Result<String> {
        let full = self.file_path(rel);
        tokio::fs::read_to_string(&full)
            .await
            .with_context(|| format!("read workspace file {}", full.display()))
    }

    /// Marks this workspace directory as trusted for Vibe Mode ("Scoped YOLO") in `Settings`.
    pub fn trust_workspace_for_vibe(&self) {
        let ws_path = self.path().to_string_lossy().into_owned();
        Settings::update(|s| {
            if !s.vibe_trusted_dirs.contains(&ws_path) {
                s.vibe_trusted_dirs.push(ws_path.clone());
            }
        });
    }

    /// Writes custom `BLACKLIST.md` rules inside the isolated Boxxy config directory.
    ///
    /// # Errors
    /// Returns an error if creating or writing `BLACKLIST.md` fails.
    pub async fn set_blacklist_markdown(&self, rules_markdown: &str) -> Result<PathBuf> {
        let path = self.config_root().join("boxxyclaw").join("BLACKLIST.md");
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, rules_markdown).await?;
        Ok(path)
    }

    /// Writes a synthetic skill package (`<config_dir>/boxxyclaw/skills/<slug>/SKILL.md`)
    /// and reloads `SkillRegistry` if already initialized.
    ///
    /// # Errors
    /// Returns an error if creating the skill directory or writing `SKILL.md` fails.
    pub async fn write_skill(&self, slug: &str, skill_markdown: &str) -> Result<PathBuf> {
        let dir = self
            .config_root()
            .join("boxxyclaw")
            .join("skills")
            .join(slug);
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join("SKILL.md");
        tokio::fs::write(&path, skill_markdown).await?;
        boxxy_claw::registry::skills::reload_if_initialized().await;
        Ok(path)
    }

    /// Spawns a headless `ClawSession` pane bound to the `character_index`-th bundled character
    /// (`0` = Niko Una, `1` = Levi Kujo, `2` = Kuro) and registers its claim in `CLAIMS_CACHE`.
    ///
    /// # Errors
    /// Returns an error if the seeded character catalog does not contain `character_index`.
    pub async fn spawn_pane(
        &mut self,
        pane_id: impl Into<String>,
        character_index: usize,
    ) -> Result<PaneDriver> {
        let catalog = seeded_catalog();
        let char_info = catalog
            .get(character_index)
            .ok_or_else(|| anyhow!("character_index {character_index} out of bounds"))?;

        let pane_id = pane_id.into();
        let session_id = Uuid::new_v4().to_string();
        let petname = format!("e2e-agent-{}", self.claims.len() + 1);

        self.claims.retain(|c| c.holder_id != pane_id);
        self.claims.push(CharacterClaim {
            holder_id: pane_id.clone(),
            holder_kind: HolderKind::Pane,
            character_id: char_info.config.id.clone(),
            session_id: session_id.clone(),
            petname,
            owner_bus_name: ":1.e2e".to_string(),
        });
        set_active_claims(self.claims.clone());

        Ok(PaneDriver::spawn(
            pane_id,
            session_id,
            char_info.config.id.clone(),
            char_info.config.display_name.clone(),
            self.workspace_dir.path().to_path_buf(),
            Arc::clone(&self.env),
            self.companions.clone(),
        )
        .await)
    }

    /// Spawns a headless `/incognito` (`Inco Gnito`) pane without claiming a regular character.
    pub async fn spawn_incognito_pane(&self, pane_id: impl Into<String>) -> PaneDriver {
        PaneDriver::spawn(
            pane_id.into(),
            Uuid::new_v4().to_string(),
            INCOGNITO_CHARACTER_ID.to_string(),
            INCOGNITO_DISPLAY_NAME.to_string(),
            self.workspace_dir.path().to_path_buf(),
            Arc::clone(&self.env),
            self.companions.clone(),
        )
        .await
    }

    /// Spawns a real Linux PTY + `/bin/bash` child wired to this workspace's `PtyRegistry`.
    ///
    /// # Errors
    /// Returns an error if opening the PTY or spawning `/bin/bash` fails.
    pub async fn spawn_pty_bash(
        &self,
        pane_id: impl Into<String>,
        owner_bus_name: impl Into<String>,
    ) -> Result<PtyShellSession> {
        PtyShellSession::spawn_bash(
            self.agent_state.pty_registry.clone(),
            pane_id,
            owner_bus_name,
            self.workspace_dir.path(),
            self.config_root(),
        )
        .await
    }

    /// Cleans up companion panes, detached PTYs, workspace radar state, and global claims at the end of a test.
    pub async fn teardown(&mut self) {
        self.companions.cleanup_all().await;
        self.agent_state.pty_registry.terminate_all().await;
        global_workspace().await.reset_all().await;
        self.claims.clear();
        set_active_claims(Vec::new());
        let skills_dir = self.config_root().join("boxxyclaw").join("skills");
        let _ = tokio::fs::remove_dir_all(&skills_dir).await;
        boxxy_claw::registry::skills::reload_if_initialized().await;
        let _ = ensure_isolated_config_root();
    }
}
