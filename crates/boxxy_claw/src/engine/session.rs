use crate::engine::{
    ClawEngineEvent, ClawMessage, context::load_session_context, context::retrieve_memories,
    context::summarize_and_store, dispatcher::extract_command_and_clean,
};
use boxxy_agent::ipc::AgentClawProxy;
use boxxy_db::Db;
use log::{debug, info};
use rig::message::Message;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct SessionState {
    pub agent_name: String,
    pub pending_terminal_reply: Option<tokio::sync::oneshot::Sender<Result<String, String>>>,
    pub pending_file_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_file_delete_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_kill_process_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_get_clipboard_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub pending_set_clipboard_reply: Option<tokio::sync::oneshot::Sender<bool>>,
    pub history: Vec<Message>,
    pub pending_lazy_diagnosis: Option<(String, String, String)>,
}

pub struct ClawSession {
    pub pane_id: String,
    pub name: String,
    pub rx: async_channel::Receiver<ClawMessage>,
    pub tx_self: async_channel::Sender<ClawMessage>,
    pub tx_ui: async_channel::Sender<ClawEngineEvent>,
    pub session_context: String,
    pub db: Arc<Mutex<Option<Db>>>,
    pub state: Arc<Mutex<SessionState>>,
    pub diagnosis_mode: boxxy_preferences::config::ClawAutoDiagnosisMode,
}

impl ClawSession {
    pub fn new(
        pane_id: String,
    ) -> (
        Self,
        async_channel::Sender<ClawMessage>,
        async_channel::Receiver<ClawEngineEvent>,
    ) {
        let (tx, rx) = async_channel::unbounded();
        let (tx_ui, rx_ui) = async_channel::unbounded();

        let settings = boxxy_preferences::Settings::load();

        // Generate a funny name for this agent
        let name = petname::petname(2, " ").unwrap_or_else(|| "Unknown Agent".to_string());

        // We defer loading session context and skills until the first use
        let session = Self {
            pane_id,
            name: name.clone(),
            rx,
            tx_self: tx.clone(),
            tx_ui,
            session_context: String::new(),
            db: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(SessionState {
                agent_name: name,
                pending_terminal_reply: None,
                pending_file_reply: None,
                pending_file_delete_reply: None,
                pending_kill_process_reply: None,
                pending_get_clipboard_reply: None,
                pending_set_clipboard_reply: None,
                history: Vec::new(),
                pending_lazy_diagnosis: None,
            })),
            diagnosis_mode: settings.claw_auto_diagnosis_mode,
        };

        (session, tx, rx_ui)
    }

    pub fn start(self, claw_proxy: AgentClawProxy<'static>) {
        tokio::spawn(async move {
            self.run(claw_proxy).await;
        });
    }

    async fn run(mut self, claw_proxy: AgentClawProxy<'static>) {
        // LAZY LOADING: We don't initialize the DB or load skills until we receive a message.
        // This ensures BoxxyClaw doesn't consume memory/CPU if the user doesn't use the AI.

        let mut is_initialized = false;
        let mut current_dir = String::from("/");

        // Register with global workspace
        let workspace = crate::registry::workspace::global_workspace().await;
        workspace
            .update_pane_state(
                self.pane_id.clone(),
                Some(self.name.clone()),
                current_dir.clone(),
                None,
                None,
            )
            .await;
        workspace
            .register_pane_tx(self.pane_id.clone(), self.tx_self.clone())
            .await;

        while let Ok(msg) = self.rx.recv().await {
            let needs_initialization = match &msg {
                ClawMessage::ClawQuery { .. }
                | ClawMessage::UserMessage { .. }
                | ClawMessage::DelegatedTask { .. }
                | ClawMessage::RequestLazyDiagnosis
                | ClawMessage::Initialize => true,
                ClawMessage::CommandFinished { exit_code, .. }
                    if *exit_code != 0 && *exit_code != 130 && *exit_code != 131 =>
                {
                    self.diagnosis_mode != boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
                }
                _ => false,
            };

            if !is_initialized && needs_initialization {
                info!(
                    "Initializing Claw Session for pane {} ({}) upon first request...",
                    self.pane_id, self.name
                );

                // If this is an explicit Initialize message, we'll handle the identity
                // announcement in the match block below to avoid double announcements.
                if !matches!(&msg, ClawMessage::Initialize) {
                    let _ = self
                        .tx_ui
                        .send(ClawEngineEvent::Identity {
                            agent_name: self.name.clone(),
                        })
                        .await;
                }

                let session_context = load_session_context();
                self.session_context = session_context;

                if let Ok(db) = Db::new().await {
                    *self.db.lock().await = Some(db);
                    info!(
                        "Claw Memory Database initialized for pane {} ({}).",
                        self.pane_id, self.name
                    );

                    // Sync any manual edits from MEMORY.md back to the DB
                    let _ = crate::memories::db::sync_markdown_to_db(self.db.clone()).await;

                    // Run Memory Hygiene
                    let _ = crate::memories::hygiene::run_hygiene(self.db.clone()).await;
                } else {
                    log::error!("Failed to initialize Claw Memory Database.");
                }

                is_initialized = true;
            }

            let session_ctx = self.session_context.clone();

            match msg {
                ClawMessage::Deactivate => {
                    info!("Deactivating Claw Session for pane {}...", self.pane_id);
                    is_initialized = false;
                    *self.db.lock().await = None;

                    let mut state_lock = self.state.lock().await;
                    state_lock.history.clear();
                    drop(state_lock);

                    // Update Workspace Radar to indicate no active agent
                    workspace
                        .update_pane_state(
                            self.pane_id.clone(),
                            None,
                            current_dir.clone(),
                            None,
                            None,
                        )
                        .await;
                }
                ClawMessage::Initialize => {
                    info!("Initializing NEW Claw Session for pane {}...", self.pane_id);

                    // 1. Pick a new name
                    let name =
                        petname::petname(2, " ").unwrap_or_else(|| "Unknown Agent".to_string());
                    self.name = name.clone();

                    // 2. Clear history and update agent name in state
                    let mut state_lock = self.state.lock().await;
                    state_lock.agent_name = name.clone();
                    state_lock.history.clear();
                    drop(state_lock);

                    // 3. Update Workspace Radar
                    workspace
                        .update_pane_state(
                            self.pane_id.clone(),
                            Some(name.clone()),
                            current_dir.clone(),
                            None,
                            None,
                        )
                        .await;

                    // 4. Announce Identity to UI
                    let _ = self
                        .tx_ui
                        .send(ClawEngineEvent::Identity { agent_name: name })
                        .await;
                }
                ClawMessage::Reload => {
                    info!("Reloading Claw Session state...");
                    let new_ctx = load_session_context();
                    self.session_context = new_ctx;
                    if let Ok(db) = Db::new().await {
                        *self.db.lock().await = Some(db);
                    }
                }
                ClawMessage::ForegroundProcessChanged { process_name } => {
                    let status = if process_name.is_empty() {
                        None
                    } else {
                        Some(format!("Running: {}", process_name))
                    };
                    workspace
                        .set_pane_status(self.pane_id.clone(), status)
                        .await;
                }
                ClawMessage::CommandFinished {
                    exit_code,
                    snapshot,
                    cwd,
                } => {
                    current_dir = cwd.clone();

                    // Update workspace state
                    let last_cmd = snapshot.lines().next().unwrap_or("").to_string();
                    workspace
                        .update_pane_state(
                            self.pane_id.clone(),
                            Some(self.name.clone()),
                            current_dir.clone(),
                            Some(last_cmd),
                            Some(snapshot.clone()),
                        )
                        .await;

                    let mut state_lock = self.state.lock().await;
                    if let Some(reply) = state_lock.pending_terminal_reply.take() {
                        if exit_code == 0 {
                            let _ = reply.send(Ok(snapshot.clone()));
                        } else {
                            let _ = reply.send(Err(format!(
                                "Command failed with exit code {exit_code}:\n{snapshot}"
                            )));
                        }
                    } else if exit_code != 0 {
                        if exit_code == 130 || exit_code == 131 {
                            continue;
                        }

                        let prompt = format!(
                            "The user's command failed with exit code {exit_code}. Please analyze the terminal snapshot and suggest a fix."
                        );

                        if self.diagnosis_mode
                            == boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
                        {
                            state_lock.pending_lazy_diagnosis = Some((prompt, snapshot, cwd));
                            drop(state_lock);

                            let tx_ui = self.tx_ui.clone();
                            let agent_name = self.name.clone();
                            tokio::spawn(async move {
                                let _ = tx_ui
                                    .send(ClawEngineEvent::LazyErrorIndicator { agent_name })
                                    .await;
                            });
                        } else {
                            drop(state_lock);
                            spawn_turn(
                                self.pane_id.clone(),
                                self.name.clone(),
                                &prompt,
                                &snapshot,
                                &session_ctx,
                                cwd,
                                false,
                                claw_proxy.clone(),
                                self.db.clone(),
                                self.state.clone(),
                                self.tx_ui.clone(),
                                None,
                                vec![],
                            );
                        }
                    }
                }
                ClawMessage::RequestLazyDiagnosis => {
                    let mut state_lock = self.state.lock().await;
                    if let Some((prompt, snapshot, cwd)) = state_lock.pending_lazy_diagnosis.take()
                    {
                        drop(state_lock);
                        spawn_turn(
                            self.pane_id.clone(),
                            self.name.clone(),
                            &prompt,
                            &snapshot,
                            &session_ctx,
                            cwd,
                            false,
                            claw_proxy.clone(),
                            self.db.clone(),
                            self.state.clone(),
                            self.tx_ui.clone(),
                            None,
                            vec![],
                        );
                    }
                }
                ClawMessage::ClawQuery {
                    query,
                    snapshot,
                    cwd,
                    image_attachments,
                } => {
                    current_dir = cwd.clone();

                    // Update workspace state
                    workspace
                        .update_pane_state(
                            self.pane_id.clone(),
                            Some(self.name.clone()),
                            current_dir.clone(),
                            Some(query.clone()),
                            Some(snapshot.clone()),
                        )
                        .await;

                    debug!(
                        "Pane {} ({}): Direct Claw query: {query}. Starting analysis.",
                        self.pane_id, self.name
                    );
                    spawn_turn(
                        self.pane_id.clone(),
                        self.name.clone(),
                        &query,
                        &snapshot,
                        &session_ctx,
                        cwd,
                        false,
                        claw_proxy.clone(),
                        self.db.clone(),
                        self.state.clone(),
                        self.tx_ui.clone(),
                        None,
                        image_attachments,
                    );
                }
                ClawMessage::FileWriteReply { approved } => {
                    let mut state_lock = self.state.lock().await;
                    if let Some(reply) = state_lock.pending_file_reply.take() {
                        let _ = reply.send(approved);
                    }
                }
                ClawMessage::FileDeleteReply { approved } => {
                    let mut state_lock = self.state.lock().await;
                    if let Some(reply) = state_lock.pending_file_delete_reply.take() {
                        let _ = reply.send(approved);
                    }
                }
                ClawMessage::KillProcessReply { approved } => {
                    let mut state_lock = self.state.lock().await;
                    if let Some(reply) = state_lock.pending_kill_process_reply.take() {
                        let _ = reply.send(approved);
                    }
                }
                ClawMessage::GetClipboardReply { approved } => {
                    let mut state_lock = self.state.lock().await;
                    if let Some(reply) = state_lock.pending_get_clipboard_reply.take() {
                        let _ = reply.send(approved);
                    }
                }
                ClawMessage::SetClipboardReply { approved } => {
                    let mut state_lock = self.state.lock().await;
                    if let Some(reply) = state_lock.pending_set_clipboard_reply.take() {
                        let _ = reply.send(approved);
                    }
                }
                ClawMessage::UserMessage {
                    message,
                    snapshot,
                    cwd,
                    image_attachments,
                } => {
                    current_dir = cwd.clone();
                    debug!(
                        "Pane {} ({}): User reply: {message}. Checking for pending tools.",
                        self.pane_id, self.name
                    );

                    // Update workspace state
                    workspace
                        .update_pane_state(
                            self.pane_id.clone(),
                            Some(self.name.clone()),
                            current_dir.clone(),
                            None,
                            Some(snapshot.clone()),
                        )
                        .await;

                    let mut state_lock = self.state.lock().await;
                    let mut fulfilled = false;

                    if let Some(reply) = state_lock.pending_terminal_reply.take() {
                        let _ = reply.send(Err(format!(
                            "User provided text feedback instead of running command: {message}"
                        )));
                        fulfilled = true;
                    } else if let Some(reply) = state_lock.pending_file_reply.take() {
                        let _ = reply.send(false);
                        fulfilled = true;
                    }

                    if fulfilled {
                        debug!(
                            "Pane {} ({}): Fulfilled pending tool with user feedback.",
                            self.pane_id, self.name
                        );
                    } else {
                        drop(state_lock);
                        spawn_turn(
                            self.pane_id.clone(),
                            self.name.clone(),
                            &message,
                            &snapshot,
                            &session_ctx,
                            cwd,
                            false,
                            claw_proxy.clone(),
                            self.db.clone(),
                            self.state.clone(),
                            self.tx_ui.clone(),
                            None,
                            image_attachments,
                        );
                    }
                }
                ClawMessage::DelegatedTask {
                    source_agent_name,
                    prompt,
                    reply_tx,
                } => {
                    let snapshot = workspace
                        .get_pane_snapshot(self.pane_id.clone())
                        .await
                        .unwrap_or_default();

                    let full_prompt = format!(
                        "*Task delegated from agent '{}'*: {}",
                        source_agent_name, prompt
                    );

                    debug!(
                        "Pane {} ({}): Received delegated task from {}.",
                        self.pane_id, self.name, source_agent_name
                    );

                    spawn_turn(
                        self.pane_id.clone(),
                        self.name.clone(),
                        &full_prompt,
                        &snapshot,
                        &session_ctx,
                        current_dir.clone(),
                        false,
                        claw_proxy.clone(),
                        self.db.clone(),
                        self.state.clone(),
                        self.tx_ui.clone(),
                        Some(reply_tx),
                        vec![],
                    );
                }
                ClawMessage::CancelPending => {
                    let mut state_lock = self.state.lock().await;
                    if let Some(reply) = state_lock.pending_terminal_reply.take() {
                        let _ = reply.send(Err("[USER_EXPLICIT_REJECT]".to_string()));
                    }
                    if let Some(reply) = state_lock.pending_file_reply.take() {
                        let _ = reply.send(false);
                    }
                    if let Some(reply) = state_lock.pending_file_delete_reply.take() {
                        let _ = reply.send(false);
                    }
                    if let Some(reply) = state_lock.pending_kill_process_reply.take() {
                        let _ = reply.send(false);
                    }
                    if let Some(reply) = state_lock.pending_get_clipboard_reply.take() {
                        let _ = reply.send(false);
                    }
                    if let Some(reply) = state_lock.pending_set_clipboard_reply.take() {
                        let _ = reply.send(false);
                    }
                    debug!("Pane {}: User cancelled pending proposals.", self.pane_id);
                    let _ = self
                        .tx_ui
                        .send(ClawEngineEvent::ProposalResolved {
                            agent_name: self.name.clone(),
                        })
                        .await;
                }
                ClawMessage::UpdateDiagnosisMode(mode) => {
                    self.diagnosis_mode = mode;
                }
            }
        }

        // Unregister on loop exit
        workspace.unregister_pane(self.pane_id).await;
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_turn(
    pane_id: String,
    agent_name: String,
    prompt: &str,
    snapshot: &str,
    session_ctx: &str,
    cwd: String,
    is_new_task: bool,
    claw_proxy: AgentClawProxy<'static>,
    db: Arc<Mutex<Option<Db>>>,
    state: Arc<Mutex<SessionState>>,
    tx_ui: async_channel::Sender<ClawEngineEvent>,
    delegate_reply_tx: Option<tokio::sync::oneshot::Sender<String>>,
    image_attachments: Vec<String>,
) {
    let prompt_clone = prompt.to_string();
    let snapshot_clone = snapshot.to_string();
    let session_ctx_clone = session_ctx.to_string();
    let cwd_clone = cwd.clone();
    let images_clone = image_attachments.clone();

    tokio::spawn(async move {
        let _ = tx_ui
            .send(ClawEngineEvent::AgentThinking {
                agent_name: agent_name.clone(),
                is_thinking: true,
            })
            .await;

        if is_new_task {
            if let Ok(mut lock) = state.try_lock() {
                lock.history.clear();
            } else {
                state.lock().await.history.clear();
            }
        }

        let db_guard = db.lock().await;
        let past_memories = retrieve_memories(&db_guard, &prompt_clone, &cwd_clone).await;
        drop(db_guard);

        let state_lock = state.lock().await;
        let settings = boxxy_preferences::Settings::load();

        let mut active_skills_text = String::new();
        let mut available_skills_text = String::new();
        let query_lower = prompt_clone.to_lowercase();

        let registry = crate::registry::skills::global_registry().await;

        // Fetch Global Workspace Radar
        let workspace = crate::registry::workspace::global_workspace().await;
        let radar = workspace.get_global_radar(pane_id.clone()).await;

        let mut tui_warning = String::new();
        let all_agents = workspace.get_all_agents().await;
        if let Some(me) = all_agents.iter().find(|a| a.id == pane_id) {
            let status = &me.status;
            if status.starts_with("Running: ") {
                tui_warning = format!(
                    "\n--- TUI WARNING ---\nYour pane is currently running an interactive application: {}.\nYou cannot run standard bash commands or delegate tasks to yourself to run bash commands. To interact with this application, you MUST use `send_keystrokes_to_pane` with your own agent name ('{}'). Note: You do not know the exact internal state (e.g. insert mode vs normal mode in vim), so send escape characters (\\e) first if necessary.\n-------------------\n",
                    status, me.name
                );
            }
        }

        // 1. Semantic Search: Get the top 3 most relevant skills via SQLite FTS5 to be "Active"
        let mut active_skills = registry.search_relevant_skills(&prompt_clone, 3).await;

        // 2. Keyword Fallback: If semantic search missed something explicitly triggered, add it to Active
        let all_skills = registry.get_skills().await;
        for skill in &all_skills {
            // Avoid duplicates
            if active_skills
                .iter()
                .any(|s| s.frontmatter.name == skill.frontmatter.name)
            {
                continue;
            }

            let mut should_load = false;
            // Skills with NO triggers are "Always Active" (like system-specs)
            if skill.frontmatter.triggers.is_empty() {
                should_load = true;
            } else {
                for trigger in &skill.frontmatter.triggers {
                    if !trigger.is_empty() && query_lower.contains(&trigger.to_lowercase()) {
                        should_load = true;
                        break;
                    }
                }
            }

            if should_load {
                active_skills.push(skill.clone());
            }
        }

        // 3. Build Active Skills Text (Full Content)
        if !active_skills.is_empty() {
            active_skills_text.push_str("\n--- ACTIVE SKILLS (FULL INSTRUCTIONS) ---\n");
            for skill in active_skills.iter().take(5) {
                active_skills_text.push_str(&format!("\n### SKILL: {}\n", skill.frontmatter.name));
                active_skills_text.push_str(&skill.content);
                active_skills_text.push('\n');
            }
        }

        // 4. Build Available Skills Text (Compact Toolbox)
        // Include all skills NOT in active_skills as compact metadata
        let mut toolbox_count = 0;
        for skill in &all_skills {
            if active_skills
                .iter()
                .any(|s| s.frontmatter.name == skill.frontmatter.name)
            {
                continue;
            }

            if toolbox_count == 0 {
                available_skills_text.push_str("\n--- AVAILABLE SKILLS (TOOLBOX - Compact) ---\n");
                available_skills_text.push_str("Use `activate_skill(name)` if you need the full instructions for any of these:\n");
            }

            available_skills_text.push_str(&format!(
                "- {}: {}\n",
                skill.frontmatter.name, skill.frontmatter.description
            ));
            toolbox_count += 1;
        }

        let data = gtk4::gio::resources_lookup_data(
            "/dev/boxxy/BoxxyTerminal/prompts/claw.md",
            gtk4::gio::ResourceLookupFlags::NONE,
        )
        .expect("Failed to load claw prompt resource");
        let system_prompt_template =
            String::from_utf8(data.to_vec()).expect("Prompt resource is not valid UTF-8");

        let identity_injection = format!(
            "## YOUR IDENTITY\n\
            You are the Boxxy-Claw agent managing terminal pane: **{}**\n\
            Your unique internal ID is: `{}`\n",
            agent_name, pane_id
        );

        let system_prompt = system_prompt_template
            .replace(
                "{{session_context}}",
                &format!("{}\n\n{}", identity_injection, session_ctx_clone),
            )
            .replace("{{active_skills}}", &active_skills_text)
            .replace("{{available_skills}}", &available_skills_text)
            .replace("{{past_memories}}", &past_memories)
            .replace("{{current_dir}}", &cwd_clone)
            .replace("{{workspace_radar}}", &radar)
            .replace("{{tui_warning}}", &tui_warning);

        let creds = boxxy_ai_core::AiCredentials::new(
            settings.api_keys.clone(),
            settings.ollama_base_url.clone(),
        );

        let agent = crate::engine::agent::create_claw_agent(
            &settings.claw_model,
            &creds,
            &system_prompt,
            &claw_proxy,
            &cwd_clone,
            tx_ui.clone(),
            state.clone(),
            db.clone(),
            &settings,
        );

        let full_prompt =
            format!("{prompt_clone}\n\nTerminal Snapshot:\n```\n{snapshot_clone}\n```");

        let history = state_lock.history.clone();
        drop(state_lock);

        let mut user_msg = vec![rig::message::Message::User {
            content: rig::OneOrMany::one(rig::message::UserContent::text(full_prompt.clone())),
        }];

        let mut is_multimodal = false;

        if !images_clone.is_empty() {
            // Rig requires multimodal content for images
            let mut contents = vec![rig::message::UserContent::text(full_prompt.clone())];
            for b64 in images_clone {
                contents.push(rig::message::UserContent::image_base64(
                    b64,
                    Some(rig::message::ImageMediaType::PNG),
                    None,
                ));
            }
            if let Ok(many) = rig::OneOrMany::many(contents) {
                user_msg = vec![rig::message::Message::User { content: many }];
                is_multimodal = true;
            }
        }

        // We temporarily adapt the ClawAgent to accept `Vec<Message>` for the current prompt
        // instead of just `&str` since we need to send the multimodal `user_msg`.
        let mut final_history = history.clone();

        let query_for_chat = if is_multimodal {
            final_history.push(user_msg.into_iter().next().unwrap());
            ""
        } else {
            &full_prompt
        };

        match agent.chat(query_for_chat, final_history).await {
            Ok(response) => {
                let _ = tx_ui
                    .send(ClawEngineEvent::AgentThinking {
                        agent_name: agent_name.clone(),
                        is_thinking: false,
                    })
                    .await;

                let mut state_lock = state.lock().await;
                state_lock
                    .history
                    .push(rig::message::Message::user(full_prompt));
                state_lock
                    .history
                    .push(rig::message::Message::assistant(response.clone()));

                // Optional: Trigger Memory Flush if history is too long
                let creds = boxxy_ai_core::AiCredentials::new(
                    settings.api_keys.clone(),
                    settings.ollama_base_url.clone(),
                );

                let _ = crate::memories::flush::flush_history(
                    db.clone(),
                    &mut state_lock.history,
                    &settings.claw_model,
                    &creds,
                    &cwd_clone,
                )
                .await;

                drop(state_lock);
                let prompt_for_db = prompt_clone.clone();
                let resp_for_db = response.clone();
                let db_for_summary = db.clone();
                let cwd_for_db = cwd_clone.clone();
                let mem_model = settings
                    .memory_model
                    .clone()
                    .or(settings.claw_model.clone());
                let creds = boxxy_ai_core::AiCredentials::new(
                    settings.api_keys.clone(),
                    settings.ollama_base_url.clone(),
                );

                tokio::spawn(async move {
                    let db_guard = db_for_summary.lock().await;
                    if db_guard.is_some() {
                        summarize_and_store(
                            &db_guard,
                            &prompt_for_db,
                            &resp_for_db,
                            &cwd_for_db,
                            creds.clone(),
                        )
                        .await;
                    }
                    drop(db_guard);

                    crate::memories::extraction::extract_implicit_memory(
                        db_for_summary.clone(),
                        prompt_for_db,
                        resp_for_db,
                        mem_model,
                        creds,
                        cwd_for_db,
                    )
                    .await;
                });
                let mut command_opt = None;
                let mut clean_diagnosis = response.clone();

                let extracted = extract_command_and_clean(&response);
                if extracted.0.is_some() {
                    command_opt = extracted.0;
                    clean_diagnosis = extracted.1;
                }

                if clean_diagnosis.trim() == "[SILENT_ACK]" {
                    info!(
                        "Pane {} ({}): Agent acknowledged rejection silently. Not sending UI event.",
                        pane_id, agent_name
                    );
                } else if clean_diagnosis.trim().is_empty() && command_opt.is_none() {
                    info!(
                        "Pane {} ({}): Agent response was empty (likely just tool calls). Not sending UI event.",
                        pane_id, agent_name
                    );
                } else if let Some(command) = command_opt {
                    let _ = tx_ui
                        .send(ClawEngineEvent::InjectCommand {
                            agent_name,
                            command,
                            diagnosis: clean_diagnosis,
                        })
                        .await;
                } else {
                    let _ = tx_ui
                        .send(ClawEngineEvent::DiagnosisComplete {
                            agent_name,
                            diagnosis: clean_diagnosis,
                        })
                        .await;
                }
                if let Some(tx) = delegate_reply_tx {
                    let _ = tx.send(response.clone());
                }
            }
            Err(e) => {
                let _ = tx_ui
                    .send(ClawEngineEvent::AgentThinking {
                        agent_name: agent_name.clone(),
                        is_thinking: false,
                    })
                    .await;
                log::error!(
                    "Pane {} ({}): Boxxy-Claw agent failed: {e}",
                    pane_id,
                    agent_name
                );
                
                let error_msg = format!("{}", e);
                let friendly_msg = if error_msg.contains("does not support tools") {
                    format!("**Error:** The selected Ollama model does not support tool calling.\n\nBoxxy-Claw requires a highly capable reasoning model with native tool support (like `llama3.2`, `qwen2.5`, or `mistral`) to interact with your system.\n\nPlease select a different model for Boxxy-Claw in the Model Selection menu (Ctrl+Shift+P).")
                } else {
                    format!("**Boxxy-Claw encountered an error:**\n```\n{}\n```", e)
                };

                let _ = tx_ui
                    .send(ClawEngineEvent::DiagnosisComplete {
                        agent_name: agent_name.clone(),
                        diagnosis: friendly_msg,
                    })
                    .await;

                if let Some(tx) = delegate_reply_tx {
                    let _ = tx.send(format!("Error: {}", e));
                }
            }
        }
    });
}
