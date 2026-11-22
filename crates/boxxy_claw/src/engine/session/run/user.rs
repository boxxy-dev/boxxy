use base64::Engine as _;
use log::debug;
use sha2::{Digest, Sha256};

use crate::engine::session::ClawSession;
use crate::engine::{ClawEngineEvent, ClawMessage, turn::spawn_turn};

use super::context::RunContext;

const ATTACHMENT_SQUARE_THUMB_PX: u32 = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
enum UserTurnAction {
    /// Handled locally with a `SystemMessage` row and no LLM turn (e.g. `/drama` toggle).
    LocalNotice { system_message: String },
    /// Stops an active roundtable or running turn (`is_slash == true` for `/stop` omits the user bubble).
    Stop {
        is_slash: bool,
        was_roundtable_active: bool,
        user_bubble: Option<String>,
    },
    /// Proceeds to an LLM turn (with optional `SystemMessage` banner and optional `UserMessage` bubble).
    RunTurn {
        user_bubble: Option<String>,
        system_message: Option<String>,
        effective_query: String,
        allow_steering: bool,
    },
}

/// Resizes base64-encoded PNG attachments into square thumbnails via `boxxy-assets`
/// (`SquareCrop` + `Resize`) and persists them under `<config_dir>/cache/attachments/<sha256>_sq.png`,
/// returning the list of cached file paths to display as square thumbnails before the user text.
async fn cache_user_attachment_thumbnails(image_attachments: &[String]) -> Vec<String> {
    let cache_dir = boxxy_sys_utils::get_config_dir()
        .join("cache")
        .join("attachments");
    cache_user_attachment_thumbnails_in_dir(image_attachments, &cache_dir).await
}

async fn cache_user_attachment_thumbnails_in_dir(
    image_attachments: &[String],
    cache_dir: &std::path::Path,
) -> Vec<String> {
    if image_attachments.is_empty() {
        return Vec::new();
    }

    if let Err(e) = tokio::fs::create_dir_all(cache_dir).await {
        log::warn!("Failed to create attachments cache dir: {e}");
        return Vec::new();
    }

    let mut paths = Vec::with_capacity(image_attachments.len());
    for b64 in image_attachments {
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
            continue;
        };
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let digest = hasher.finalize();
        let mut hex = String::with_capacity(64);
        for b in digest {
            use std::fmt::Write as _;
            let _ = write!(&mut hex, "{b:02x}");
        }

        let thumb_path = cache_dir.join(format!("{hex}_sq.png"));
        if !thumb_path.exists() {
            let bytes_for_thumb = bytes.clone();
            let thumb_bytes = match tokio::task::spawn_blocking(move || {
                boxxy_assets::image::generate_square_thumbnail_png(
                    &bytes_for_thumb,
                    ATTACHMENT_SQUARE_THUMB_PX,
                )
            })
            .await
            {
                Ok(Ok(out)) => out,
                Ok(Err(e)) => {
                    log::warn!("Failed to generate square attachment thumbnail: {e}");
                    bytes
                }
                Err(e) => {
                    log::warn!("Square thumbnail task panicked: {e}");
                    continue;
                }
            };

            if let Err(e) = tokio::fs::write(&thumb_path, &thumb_bytes).await {
                log::warn!("Failed to write cached attachment thumbnail: {e}");
                continue;
            }
        }

        paths.push(thumb_path.to_string_lossy().into_owned());
    }

    paths
}

impl ClawSession {
    /// Executes `/undo` if `raw_input` is `/undo`, restoring all files modified in the
    /// most recent turn (and deleting newly created files) without spending an LLM turn.
    async fn try_handle_undo(
        &self,
        raw_input: &str,
        env: &std::sync::Arc<dyn crate::engine::ClawEnvironment>,
    ) -> Option<String> {
        if !raw_input.trim().eq_ignore_ascii_case("/undo") {
            return None;
        }

        let snapshots = {
            let mut state_lock = self.state.lock().await;
            if state_lock.current_turn_snapshots.is_empty() {
                std::mem::take(&mut state_lock.last_turn_snapshots)
            } else {
                std::mem::take(&mut state_lock.current_turn_snapshots)
            }
        };

        if snapshots.is_empty() {
            return Some(
                "↩️ Nothing to undo — no file edits have been recorded in the recent turn."
                    .to_string(),
            );
        }

        let mut restored_lines = Vec::new();
        let mut updated_hashes = Vec::new();
        for (path, prev_opt) in snapshots.into_iter().rev() {
            match prev_opt {
                Some(prev_content) => {
                    let h = boxxy_core_toolbox::hash_file_content(&prev_content);
                    match env.write_file(path.clone(), prev_content).await {
                        Ok(()) => {
                            restored_lines.push(format!("- Restored `{path}`"));
                            updated_hashes.push((path, Some(h)));
                        }
                        Err(e) => {
                            restored_lines.push(format!("- Failed to restore `{path}`: {e}"));
                        }
                    }
                }
                None => match env.delete_file(path.clone()).await {
                    Ok(()) => {
                        restored_lines.push(format!("- Removed newly created file `{path}`"));
                        updated_hashes.push((path, None));
                    }
                    Err(e) => {
                        restored_lines.push(format!("- Failed to remove `{path}`: {e}"));
                    }
                },
            }
        }

        let summary = format!(
            "↩️ **Reverted file changes from the last turn:**\n{}",
            restored_lines.join("\n")
        );

        {
            let mut state_lock = self.state.lock().await;
            for (path, hash_opt) in updated_hashes {
                match hash_opt {
                    Some(h) => {
                        state_lock.read_file_hashes.insert(path, h);
                    }
                    None => {
                        state_lock.read_file_hashes.remove(&path);
                    }
                }
            }
            state_lock.history.push(rig::message::Message::user(
                "[System: The user ran /undo to revert file changes from the last turn.]"
                    .to_string(),
            ));
            state_lock
                .history
                .push(rig::message::Message::assistant(summary.clone()));
        }

        Some(summary)
    }

    /// Cancels any awaiting tool proposals and aborts any in-flight LLM turn cleanly,
    /// returning `true` if either an active turn or a pending proposal was cancelled.
    async fn cancel_active_turn_and_proposals(&self, ctx: &mut RunContext) -> bool {
        let mut had_active = false;

        if let Some(handle) = ctx.current_turn.take() {
            handle.abort();
            had_active = true;
            let (thinking_name, thinking_cid) = self.state.lock().await.effective_speaker();
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::AgentThinking {
                    agent_name: thinking_name,
                    character_id: thinking_cid,
                    is_thinking: false,
                })
                .await;
        }
        if !ctx.backlog.is_empty() {
            ctx.backlog.clear();
            had_active = true;
        }

        let local_cancelled = {
            let mut state_lock = self.state.lock().await;
            state_lock.drain_pending_cancel()
        };
        let remote_cancelled = ctx
            .workspace
            .drain_pending_commands_for_source(&self.pane_id, "[USER_EXPLICIT_REJECT]".to_string())
            .await;
        let proposal_cancelled = local_cancelled || remote_cancelled;

        if proposal_cancelled {
            had_active = true;
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::ProposalResolved {
                    agent_name: self.name.clone(),
                    character_id: self.character_id.clone(),
                    approved: false,
                })
                .await;
        }

        had_active
    }

    /// Evaluates the user's message for `/stop`, `/drama`, `/debate`, `@character` mentions, or
    /// participation in an active roundtable, returning a structured `UserTurnAction` so slash
    /// commands emit `SystemMessage` rows rather than echoing raw `/command` text or character replies.
    #[allow(clippy::too_many_lines)]
    async fn prepare_user_roundtable_turn(
        &self,
        raw_input: &str,
        attachments_tail: &str,
    ) -> UserTurnAction {
        use crate::engine::session::roundtable::{
            SpeakerIdentity, UserRoundtableIntent, format_participants_banner, get_active_catalog,
            parse_user_roundtable_intent,
        };

        let append_tail = |base: String| -> String {
            if attachments_tail.is_empty() {
                base
            } else {
                format!("{base}{attachments_tail}")
            }
        };

        let catalog = get_active_catalog();
        let host_slug = catalog
            .iter()
            .find(|c| c.config.id == self.character_id)
            .map_or_else(
                || self.name.to_lowercase().replace(' ', "-"),
                |c| c.config.name.clone(),
            );

        let host = SpeakerIdentity {
            character_id: self.character_id.clone(),
            display_name: crate::engine::session::roundtable::format_character_title(&host_slug),
            slug: host_slug,
            is_guest: false,
            home_pane_id: None,
        };

        let mut state_lock = self.state.lock().await;
        state_lock.next_roundtable_turn = None;
        let was_active = state_lock.roundtable.is_active()
            || state_lock.roundtable.auto_alternate
            || state_lock.active_speaker.is_some();

        match parse_user_roundtable_intent(raw_input, &host, was_active, &catalog) {
            UserRoundtableIntent::Stop { is_slash } => {
                state_lock.roundtable.clear();
                state_lock.active_speaker = None;
                UserTurnAction::Stop {
                    is_slash,
                    was_roundtable_active: was_active,
                    user_bubble: (!is_slash).then(|| raw_input.to_string()),
                }
            }
            UserRoundtableIntent::ToggleDrama => {
                let next = !state_lock.roundtable.drama_mode;
                state_lock.roundtable.drama_mode = next;
                let notice = if next {
                    "🎭 **Drama Mode enabled!** Characters will now speak in short, casual 1-sentence Visual Novel lines with a typewriter effect."
                } else {
                    "🎭 **Drama Mode disabled.** Characters will return to full technical responses."
                };
                UserTurnAction::LocalNotice {
                    system_message: notice.to_string(),
                }
            }
            UserRoundtableIntent::Drama {
                guests,
                topic,
                custom_topic,
            } => {
                if guests.is_empty() {
                    // Single-character Drama Mode: chat 1-on-1 with the Host character in
                    // Visual Novel style (1-2 sentences + typewriter) and wait for the user!
                    let was_drama = state_lock.roundtable.drama_mode;
                    state_lock.roundtable.clear();
                    state_lock.roundtable.drama_mode = true;
                    state_lock.active_speaker = None;
                    let system_message = (!was_drama).then(|| {
                        "🎭 **Drama Mode enabled!** Characters will now speak in short, casual 1-sentence Visual Novel lines with a typewriter effect.".to_string()
                    });
                    return UserTurnAction::RunTurn {
                        user_bubble: Some(topic.clone()),
                        system_message,
                        effective_query: append_tail(topic),
                        allow_steering: false,
                    };
                }

                let resolved_guests: Vec<SpeakerIdentity> = guests
                    .into_iter()
                    .map(|mut g| {
                        if let Some(info) = catalog.iter().find(|c| c.config.id == g.character_id) {
                            g = crate::engine::session::roundtable::speaker_from_info(
                                info,
                                &self.character_id,
                                &self.pane_id,
                            );
                        }
                        g
                    })
                    .collect();

                state_lock
                    .roundtable
                    .start_debate(host, resolved_guests, topic.clone());
                state_lock.roundtable.drama_mode = true;
                state_lock.active_speaker = state_lock.roundtable.speaker_queue.pop_front();

                let roster = format_participants_banner(&state_lock.roundtable.participants);
                drop(state_lock);
                let banner = custom_topic.map_or_else(
                    || format!("🎭 **Drama Mode** • {roster}"),
                    |ct| format!("🎭 **Drama Mode** • {roster} — {ct}"),
                );

                UserTurnAction::RunTurn {
                    user_bubble: None,
                    system_message: Some(banner),
                    effective_query: append_tail(format!(
                        "[VISUAL NOVEL DRAMA MODE STARTED]\nTopic: {topic}\n\
                        Reply in ONLY 1 to 2 short, expressive spoken sentences (NO bullet points, NO lists, NO headers) and directly address the other participant(s)!"
                    )),
                    allow_steering: false,
                }
            }
            UserRoundtableIntent::Debate {
                guests,
                topic,
                custom_topic,
            } => {
                // Re-resolve guests with `self.pane_id` so active swarm peers get their `home_pane_id` populated
                let resolved_guests: Vec<SpeakerIdentity> = guests
                    .into_iter()
                    .map(|mut g| {
                        if let Some(info) = catalog.iter().find(|c| c.config.id == g.character_id) {
                            g = crate::engine::session::roundtable::speaker_from_info(
                                info,
                                &self.character_id,
                                &self.pane_id,
                            );
                        }
                        g
                    })
                    .collect();

                state_lock
                    .roundtable
                    .start_debate(host, resolved_guests, topic.clone());
                state_lock.active_speaker = state_lock.roundtable.speaker_queue.pop_front();

                let roster = format_participants_banner(&state_lock.roundtable.participants);
                drop(state_lock);
                let banner = custom_topic.map_or_else(
                    || format!("🎙️ **Debate started** • {roster}"),
                    |ct| format!("🎙️ **Debate started** • {roster} — {ct}"),
                );

                UserTurnAction::RunTurn {
                    user_bubble: None,
                    system_message: Some(banner),
                    effective_query: append_tail(format!(
                        "[ROUNDTABLE DEBATE STARTED]\nTopic: {topic}\n\
                        Kick off the discussion in your character's voice and directly engage the other participant(s) and the user!"
                    )),
                    allow_steering: false,
                }
            }
            UserRoundtableIntent::Mention {
                mentioned,
                multi_turn,
                prompt,
            } => {
                let resolved_mentions: Vec<SpeakerIdentity> = mentioned
                    .into_iter()
                    .map(|mut m| {
                        if let Some(info) = catalog.iter().find(|c| c.config.id == m.character_id) {
                            m = crate::engine::session::roundtable::speaker_from_info(
                                info,
                                &self.character_id,
                                &self.pane_id,
                            );
                        }
                        m
                    })
                    .collect();

                state_lock.roundtable.start_mention_turn(
                    host,
                    resolved_mentions,
                    multi_turn,
                    prompt.clone(),
                );
                state_lock.active_speaker = state_lock.roundtable.speaker_queue.pop_front();
                drop(state_lock);
                UserTurnAction::RunTurn {
                    user_bubble: Some(raw_input.to_string()),
                    system_message: None,
                    effective_query: append_tail(prompt),
                    allow_steering: !multi_turn,
                }
            }
            UserRoundtableIntent::None => {
                if state_lock.roundtable.auto_alternate
                    && state_lock.roundtable.participants.len() >= 2
                {
                    // User is interjecting / participating in an active multi-turn roundtable —
                    // queue Host + Guest(s) and reset the round budget!
                    state_lock
                        .roundtable
                        .resume_after_interjection(host, raw_input.to_string());
                    state_lock.active_speaker = state_lock.roundtable.speaker_queue.pop_front();
                } else {
                    state_lock.roundtable.clear();
                    state_lock.active_speaker = None;
                }
                drop(state_lock);
                UserTurnAction::RunTurn {
                    user_bubble: Some(raw_input.to_string()),
                    system_message: None,
                    effective_query: append_tail(raw_input.to_string()),
                    allow_steering: true,
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn process_user_submission(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        raw_query: String,
        display_query: String,
        is_whats_new: bool,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
    ) {
        ctx.current_dir.clone_from(&cwd);

        // 1. `/undo` cancels any active turn/proposal and reverts file snapshots locally
        //    via a SystemMessage without echoing `/undo` as a user bubble.
        if !is_whats_new && display_query.trim().eq_ignore_ascii_case("/undo") {
            let _ = self.cancel_active_turn_and_proposals(ctx).await;
            if let Some(undo_msg) = self.try_handle_undo(&display_query, &ctx.env).await {
                self.send_ui(ClawEngineEvent::SystemMessage { text: undo_msg })
                    .await;
            }
            let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
            return;
        }

        let text_attachments = crate::engine::turn::extract_text_attachment_labels(&raw_query);

        let action = if is_whats_new {
            UserTurnAction::RunTurn {
                user_bubble: Some(display_query),
                system_message: None,
                effective_query: raw_query,
                allow_steering: true,
            }
        } else {
            let attachments_tail = raw_query
                .find(crate::engine::turn::ATTACHMENTS_CONTEXT_MARKER)
                .map_or_default(|idx| &raw_query[idx..]);
            self.prepare_user_roundtable_turn(&display_query, attachments_tail)
                .await
        };

        match action {
            UserTurnAction::LocalNotice { system_message } => {
                self.send_ui(ClawEngineEvent::SystemMessage {
                    text: system_message,
                })
                .await;
                let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
            }
            UserTurnAction::Stop {
                is_slash,
                was_roundtable_active,
                user_bubble,
            } => {
                let had_active = self.cancel_active_turn_and_proposals(ctx).await;
                if let Some(bubble) = user_bubble {
                    let thumb_paths = cache_user_attachment_thumbnails(&image_attachments).await;
                    self.send_ui(ClawEngineEvent::UserMessage {
                        content: bubble,
                        images: thumb_paths,
                        text_attachments,
                    })
                    .await;
                }
                let notice = if !is_slash {
                    Some("⏹️ **Multi-character conversation paused.**")
                } else if was_roundtable_active {
                    Some("⏹️ **Multi-character conversation stopped.**")
                } else if had_active {
                    Some("⏹️ **Stopped.**")
                } else {
                    None
                };
                if let Some(text) = notice {
                    self.send_ui(ClawEngineEvent::SystemMessage {
                        text: text.to_string(),
                    })
                    .await;
                }
                let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
            }
            UserTurnAction::RunTurn {
                user_bubble,
                system_message,
                effective_query,
                allow_steering,
            } => {
                self.ensure_session_persisted(&cwd).await;

                if let Some(sys_msg) = system_message {
                    self.send_ui(ClawEngineEvent::SystemMessage { text: sys_msg })
                        .await;
                }

                let thumb_paths = cache_user_attachment_thumbnails(&image_attachments).await;
                if user_bubble.is_some()
                    || !thumb_paths.is_empty()
                    || !text_attachments.is_empty()
                {
                    self.send_ui(ClawEngineEvent::UserMessage {
                        content: user_bubble.unwrap_or_default(),
                        images: thumb_paths,
                        text_attachments,
                    })
                    .await;
                }

                if allow_steering {
                    let mut fulfilled = {
                        let mut state_lock = self.state.lock().await;
                        state_lock.drain_pending_with_interruption(&effective_query)
                    };
                    let outcome =
                        boxxy_core_toolbox::ApprovalOutcome::Interrupted(effective_query.clone());
                    if ctx
                        .workspace
                        .drain_pending_commands_for_source(&self.pane_id, outcome.rejection_message())
                        .await
                    {
                        fulfilled = true;
                    }

                    if fulfilled {
                        debug!(
                            "Pane {} ({}): Fulfilled pending tool approval with user query: {effective_query}.",
                            self.pane_id, self.name
                        );
                        ctx.workspace
                            .update_pane_state(
                                self.pane_id.clone(),
                                Some(self.session_id.clone()),
                                Some(self.name.clone()),
                                ctx.current_dir.clone(),
                                Some(effective_query),
                                Some(snapshot),
                            )
                            .await;
                        return;
                    }
                }

                let _ = self.cancel_active_turn_and_proposals(ctx).await;

                ctx.workspace
                    .update_pane_state(
                        self.pane_id.clone(),
                        Some(self.session_id.clone()),
                        Some(self.name.clone()),
                        ctx.current_dir.clone(),
                        Some(effective_query.clone()),
                        Some(snapshot.clone()),
                    )
                    .await;

                debug!(
                    "Pane {} ({}): Direct Claw query: {effective_query}. Starting analysis.",
                    self.pane_id, self.name
                );
                ctx.current_turn = Some(spawn_turn(
                    self.pane_id.clone(),
                    self.session_id.clone(),
                    self.name.clone(),
                    &effective_query,
                    &snapshot,
                    session_ctx,
                    cwd,
                    false,
                    ctx.env.clone(),
                    self.db.clone(),
                    self.state.clone(),
                    self.tx_ui.clone(),
                    self.tx_self.clone(),
                    None,
                    image_attachments,
                ));
            }
        }
    }

    pub(super) async fn handle_claw_query(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        query: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
    ) {
        if self.incognito {
            if let Some((cmd, _)) = boxxy_claw_protocol::SlashCommand::parse(&query) {
                match cmd {
                    boxxy_claw_protocol::SlashCommand::Stop => {
                        if let Some(handle) = ctx.current_turn.take() {
                            handle.abort();
                            let _ = self
                                .tx_ui
                                .send(ClawEngineEvent::AgentThinking {
                                    agent_name: self.name.clone(),
                                    character_id: self.character_id.clone(),
                                    is_thinking: false,
                                })
                                .await;
                        }
                        ctx.backlog.clear();
                        let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
                        return;
                    }
                    boxxy_claw_protocol::SlashCommand::Drama
                    | boxxy_claw_protocol::SlashCommand::Debate
                    | boxxy_claw_protocol::SlashCommand::Undo => {
                        let _ = self
                            .tx_ui
                            .send(ClawEngineEvent::SystemMessage {
                                text: "🕵️ Session commands (/drama, /debate, /undo) are unavailable in Incognito Mode.".to_string(),
                            })
                            .await;
                        let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
                        return;
                    }
                    _ => {}
                }
            }

            if let Some(handle) = ctx.current_turn.take() {
                handle.abort();
                let _ = self
                    .tx_ui
                    .send(ClawEngineEvent::AgentThinking {
                        agent_name: self.name.clone(),
                        character_id: self.character_id.clone(),
                        is_thinking: false,
                    })
                    .await;
            }
            ctx.backlog.clear();

            let incognito_display =
                crate::engine::turn::extract_user_authored_prompt(&query).to_string();
            let incognito_text_attachments =
                crate::engine::turn::extract_text_attachment_labels(&query);
            self.send_ui(ClawEngineEvent::UserMessage {
                content: incognito_display,
                images: Vec::new(),
                text_attachments: incognito_text_attachments,
            })
            .await;

            ctx.current_turn = Some(spawn_turn(
                self.pane_id.clone(),
                self.session_id.clone(),
                self.name.clone(),
                &query,
                "",
                session_ctx,
                String::new(),
                false,
                ctx.env.clone(),
                self.db.clone(),
                self.state.clone(),
                self.tx_ui.clone(),
                self.tx_self.clone(),
                None,
                image_attachments,
            ));
            return;
        }

        let mut image_attachments = image_attachments;
        let is_whats_new = crate::engine::turn::is_whats_new_prompt(&query);
        let display_query = crate::engine::turn::extract_user_authored_prompt(&query).to_string();

        // Pre-resolve any `@path/to/image.png` file references from user-authored text so their
        // square thumbnails render in the user message bubble and are passed to vision models.
        if !is_whats_new {
            let pre_resolved =
                crate::engine::turn::file_refs::resolve_prompt_file_refs(&display_query, &cwd)
                    .await;
            for b64 in pre_resolved.image_attachments {
                if !image_attachments.contains(&b64) {
                    image_attachments.push(b64);
                }
            }
        }

        self.process_user_submission(
            ctx,
            session_ctx,
            query,
            display_query,
            is_whats_new,
            snapshot,
            cwd,
            image_attachments,
        )
        .await;
    }

    pub(super) async fn handle_user_message(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        message: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
    ) {
        let display_query = message.clone();
        self.process_user_submission(
            ctx,
            session_ctx,
            message,
            display_query,
            false,
            snapshot,
            cwd,
            image_attachments,
        )
        .await;
    }

    pub(super) async fn handle_delegated_task(
        &self,
        ctx: &mut RunContext,
        source_agent_name: String,
        prompt: String,
        request_id: uuid::Uuid,
    ) {
        if ctx.current_turn.is_some() {
            debug!(
                "Pane {} ({}): Agent is busy. Queueing delegated task from {}.",
                self.pane_id, self.name, source_agent_name
            );
            ctx.backlog.push_back(ClawMessage::DelegatedTask {
                source_agent_name,
                prompt,
                request_id,
            });
            return;
        }

        let snapshot = ctx
            .workspace
            .get_pane_snapshot(self.pane_id.clone())
            .await
            .unwrap_or_default();

        let catalog = crate::engine::session::roundtable::get_active_catalog();
        let (caller_display_name, caller_character_id) = if let Some(resolved) =
            crate::engine::session::roundtable::resolve_speaker_token(
                &source_agent_name,
                &self.character_id,
                &self.pane_id,
                &catalog,
            ) {
            (resolved.display_name, resolved.character_id)
        } else {
            (source_agent_name.clone(), String::new())
        };

        let incoming_event = ClawEngineEvent::DiagnosisComplete {
            agent_name: caller_display_name.clone(),
            character_id: caller_character_id,
            diagnosis: prompt.clone(),
            trace: Vec::new(),
            usage: None,
            typewriter: false,
        };
        crate::engine::persist_visual_event(
            self.db.clone(),
            self.session_id.clone(),
            self.pane_id.clone(),
            &incoming_event,
        );
        let _ = self.tx_ui.send(incoming_event).await;

        let full_prompt = format!("*Task delegated from agent '{caller_display_name}'*: {prompt}");

        debug!(
            "Pane {} ({}): Received delegated task from {}.",
            self.pane_id, self.name, source_agent_name
        );

        ctx.current_turn = Some(spawn_turn(
            self.pane_id.clone(),
            self.session_id.clone(),
            self.name.clone(),
            &full_prompt,
            &snapshot,
            &self.session_context,
            ctx.current_dir.clone(),
            false,
            ctx.env.clone(),
            self.db.clone(),
            self.state.clone(),
            self.tx_ui.clone(),
            self.tx_self.clone(),
            Some(request_id),
            vec![],
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct NoopEnv;

    #[async_trait::async_trait]
    impl crate::engine::ClawEnvironment for NoopEnv {
        async fn exec_shell(&self, _: String) -> anyhow::Result<(i32, String, String)> {
            Ok((0, String::new(), String::new()))
        }
        async fn spawn_detached(&self, _: String, _: String) -> anyhow::Result<u32> {
            Ok(1)
        }
        async fn read_file(&self, _: String, _: u32, _: u32) -> anyhow::Result<String> {
            Ok(String::new())
        }
        async fn write_file(&self, _: String, _: String) -> anyhow::Result<()> {
            Ok(())
        }
        async fn list_directory(&self, _: String) -> anyhow::Result<Vec<(String, bool, u64)>> {
            Ok(vec![])
        }
        async fn delete_file(&self, _: String) -> anyhow::Result<()> {
            Ok(())
        }
        async fn kill_process(&self, _: u32, _: i32) -> anyhow::Result<()> {
            Ok(())
        }
        async fn get_clipboard(&self) -> anyhow::Result<String> {
            Ok(String::new())
        }
        async fn set_clipboard(&self, _: String) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn cache_user_attachment_thumbnails_creates_square_pngs() {
        let dir = tempfile::tempdir().unwrap();
        let synthetic_bytes = b"synthetic-png-payload";
        let b64 = base64::engine::general_purpose::STANDARD.encode(synthetic_bytes);

        let paths = cache_user_attachment_thumbnails_in_dir(&[b64], dir.path()).await;

        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with("_sq.png"));
        assert!(std::path::Path::new(&paths[0]).exists());
    }

    #[tokio::test]
    async fn slash_commands_emit_system_messages_without_raw_command_user_bubbles() {
        let (session, _tx, rx_ui) = ClawSession::new("pane-slash-test".to_string());
        let session = session.with_identity(
            "char-host".to_string(),
            "Host Agent".to_string(),
            "sess-slash-test".to_string(),
        );
        let workspace = Arc::new(crate::registry::workspace::WorkspaceRegistry::new());
        let mut ctx = RunContext::new(Arc::new(NoopEnv), workspace);

        // 1. `/drama` toggle emits ONLY a `SystemMessage` (never `UserMessage("/drama")` or `DiagnosisComplete`).
        session
            .handle_claw_query(
                &mut ctx,
                "",
                "/drama".to_string(),
                String::new(),
                String::new(),
                Vec::new(),
            )
            .await;

        let mut events = Vec::new();
        while let Ok(ev) = rx_ui.try_recv() {
            events.push(ev);
        }
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ClawEngineEvent::SystemMessage { text } if text.contains("Drama Mode enabled")
        ));
        assert!(session.state.lock().await.roundtable.drama_mode);

        // 2. `/undo` while a tool approval is pending cancels the proposal and emits a `SystemMessage`
        //    rather than feeding `"/undo"` into the awaiting tool channel.
        let (file_tx, file_rx) = tokio::sync::oneshot::channel();
        session.state.lock().await.pending_file_reply = Some(file_tx);

        session
            .handle_claw_query(
                &mut ctx,
                "",
                "/undo".to_string(),
                String::new(),
                String::new(),
                Vec::new(),
            )
            .await;

        assert_eq!(
            file_rx.await.unwrap(),
            boxxy_core_toolbox::ApprovalOutcome::Rejected
        );

        let mut undo_events = Vec::new();
        while let Ok(ev) = rx_ui.try_recv() {
            undo_events.push(ev);
        }
        assert!(
            !undo_events
                .iter()
                .any(|e| matches!(e, ClawEngineEvent::UserMessage { .. })),
            "Expected no UserMessage row for /undo"
        );
        assert!(
            undo_events.iter().any(|e| matches!(
                e,
                ClawEngineEvent::SystemMessage { text } if text.contains("Nothing to undo")
            )),
            "Expected SystemMessage for /undo"
        );

        // 3. `/stop` when completely idle emits neither a `/stop` UserMessage nor noise.
        session
            .handle_claw_query(
                &mut ctx,
                "",
                "/stop".to_string(),
                String::new(),
                String::new(),
                Vec::new(),
            )
            .await;
        assert!(rx_ui.try_recv().is_err());

        // 4. `/drama <message>` strips `/drama ` from the user bubble!
        let solo_action = session
            .prepare_user_roundtable_turn("/drama Explain src/module.rs", "")
            .await;
        assert_eq!(
            solo_action,
            UserTurnAction::RunTurn {
                user_bubble: Some("Explain src/module.rs".to_string()),
                // Already enabled in step 1 above, so no duplicate enable banner is emitted
                system_message: None,
                effective_query: "Explain src/module.rs".to_string(),
                allow_steering: false,
            }
        );
    }
}
