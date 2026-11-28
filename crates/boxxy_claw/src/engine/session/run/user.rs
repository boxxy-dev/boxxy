use base64::Engine as _;
use log::debug;
use sha2::{Digest, Sha256};

use crate::engine::session::ClawSession;
use crate::engine::{ClawEngineEvent, ClawMessage, turn::spawn_turn};

use super::context::{QueuedPrompt, RunContext};

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
            let (spk_name, spk_cid) = state_lock.effective_speaker();
            let tagged_summary =
                crate::engine::turn::history::tag_assistant_text(&spk_cid, &spk_name, &summary);
            state_lock.history.push(rig::message::Message::user(
                "[System: The user ran /undo to revert file changes from the last turn.]"
                    .to_string(),
            ));
            state_lock
                .history
                .push(rig::message::Message::assistant(tagged_summary));
        }

        Some(summary)
    }

    /// Cancels any awaiting tool proposals and aborts any in-flight LLM turn cleanly,
    /// returning `true` if either an active turn or a pending proposal was cancelled.
    pub(super) async fn cancel_active_turn_and_proposals(&self, ctx: &mut RunContext) -> bool {
        let mut had_active = false;

        if let Some(handle) = ctx.current_turn.take() {
            had_active = true;
            let (thinking_name, thinking_cid) = {
                let mut state_lock = self.state.lock().await;
                // Salvage completed mid-turn tool steps BEFORE `handle.abort()` drops `InFlightClearGuard`.
                let _ = state_lock.salvage_in_flight_tool_steps();
                state_lock.current_turn_user_images.clear();
                state_lock.generated_images_this_turn.clear();
                handle.abort();
                state_lock.effective_speaker()
            };
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
        if !ctx.queued_prompts.is_empty() && !ctx.queue_paused {
            ctx.queue_paused = true;
            self.emit_queue_updated(ctx).await;
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
        let host = crate::engine::session::roundtable::resolve_host_speaker(
            &self.character_id,
            &self.name,
            &catalog,
        );

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
        image_attachment_meta: Vec<boxxy_claw_protocol::ImageAttachmentMeta>,
    ) {
        ctx.current_dir.clone_from(&cwd);

        // 1. `/undo` cancels any active turn/proposal and reverts file snapshots locally
        //    via a SystemMessage without echoing `/undo` as a user bubble.
        if !is_whats_new && display_query.trim().eq_ignore_ascii_case("/undo") {
            let _ = self.cancel_active_turn_and_proposals(ctx).await;
            if let Some(undo_msg) = self.try_handle_undo(&display_query, &ctx.env).await {
                self.persist_history_if_present().await;
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
                if ctx.current_turn.is_none() {
                    let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
                }
            }
            UserTurnAction::Stop {
                is_slash,
                was_roundtable_active,
                user_bubble,
            } => {
                let had_active = self.cancel_active_turn_and_proposals(ctx).await;
                self.persist_history_if_present().await;
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

                // Only steer in-flight tool proposals via text `ToolResult` when no new vision
                // images were attached; otherwise cancel and start a fresh turn so the model
                // receives the attached `UserContent::Image` blocks.
                if allow_steering && image_attachments.is_empty() {
                    let (has_local_pending, cmd_already_accepted) = {
                        let state_lock = self.state.lock().await;
                        (
                            state_lock.has_in_turn_approval(),
                            state_lock.terminal_command_accepted,
                        )
                    };
                    let has_remote_pending = !cmd_already_accepted
                        && ctx
                            .workspace
                            .has_pending_commands_for_source(&self.pane_id)
                            .await;

                    if has_local_pending || has_remote_pending {
                        let user_steered =
                            crate::engine::turn::extract_user_authored_prompt(&effective_query);
                        let resolved_refs =
                            crate::engine::turn::file_refs::resolve_prompt_file_refs(
                                user_steered,
                                &cwd,
                            )
                            .await;
                        let steered_payload = if resolved_refs.context_block.trim().is_empty() {
                            effective_query.clone()
                        } else {
                            format!("{effective_query}\n\n{}", resolved_refs.context_block.trim())
                        };

                        let mut fulfilled = {
                            let mut state_lock = self.state.lock().await;
                            state_lock.current_dir.clone_from(&cwd);
                            for (path, hash) in &resolved_refs.read_hashes {
                                state_lock.read_file_hashes.insert(path.clone(), *hash);
                            }
                            state_lock.drain_pending_with_interruption(&steered_payload)
                        };
                        let outcome =
                            boxxy_core_toolbox::ApprovalOutcome::Interrupted(steered_payload);
                        if ctx
                            .workspace
                            .drain_pending_commands_for_source(
                                &self.pane_id,
                                outcome.rejection_message(),
                            )
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
                                    None,
                                    Some(snapshot),
                                )
                                .await;
                            return;
                        }
                    }
                }

                let _ = self.cancel_active_turn_and_proposals(ctx).await;

                ctx.workspace
                    .update_pane_state(
                        self.pane_id.clone(),
                        Some(self.session_id.clone()),
                        Some(self.name.clone()),
                        ctx.current_dir.clone(),
                        None,
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
                    image_attachment_meta,
                ));
            }
        }
    }

    pub(super) async fn emit_queue_updated(&self, ctx: &RunContext) {
        let _ = self
            .tx_ui
            .send(ClawEngineEvent::QueueUpdated {
                items: ctx.queue_summaries(),
                paused: ctx.queue_paused,
            })
            .await;
    }

    async fn should_bypass_queue(
        &self,
        ctx: &RunContext,
        raw_query: &str,
        display_query: &str,
    ) -> bool {
        if ctx.current_turn.is_none() {
            return true;
        }
        let trimmed = display_query.trim();
        if let Some((cmd, args)) = boxxy_claw_protocol::SlashCommand::parse(trimmed) {
            match cmd {
                boxxy_claw_protocol::SlashCommand::Stop
                | boxxy_claw_protocol::SlashCommand::Undo => return true,
                boxxy_claw_protocol::SlashCommand::Drama if args.is_empty() => return true,
                boxxy_claw_protocol::SlashCommand::Drama
                | boxxy_claw_protocol::SlashCommand::Debate
                    if self.incognito =>
                {
                    return true;
                }
                _ => {}
            }
        }
        if self.incognito
            && let Some((
                boxxy_claw_protocol::SlashCommand::Stop
                | boxxy_claw_protocol::SlashCommand::Drama
                | boxxy_claw_protocol::SlashCommand::Debate
                | boxxy_claw_protocol::SlashCommand::Undo,
                _,
            )) = boxxy_claw_protocol::SlashCommand::parse(raw_query)
        {
            return true;
        }

        let state_lock = self.state.lock().await;
        if state_lock.has_in_turn_approval() {
            return true;
        }
        let roundtable_active = state_lock.roundtable.is_active()
            || state_lock.roundtable.auto_alternate
            || state_lock.active_speaker.is_some();
        drop(state_lock);
        roundtable_active && trimmed.eq_ignore_ascii_case("stop")
    }

    pub(super) async fn handle_remove_queued_prompt(
        &self,
        ctx: &mut RunContext,
        id: uuid::Uuid,
        recall_to_input: bool,
    ) {
        let Some(pos) = ctx.queued_prompts.iter().position(|item| item.id == id) else {
            return;
        };
        let Some(removed) = ctx.queued_prompts.remove(pos) else {
            return;
        };
        if ctx.queued_prompts.is_empty() {
            ctx.queue_paused = false;
        }
        self.emit_queue_updated(ctx).await;
        if recall_to_input {
            let _ = self
                .tx_ui
                .send(ClawEngineEvent::QueuedPromptRecalled {
                    id,
                    fallback_text: removed.display_query,
                })
                .await;
        }
    }

    pub(super) async fn handle_clear_queued_prompts(&self, ctx: &mut RunContext) {
        if ctx.clear_queue() {
            self.emit_queue_updated(ctx).await;
        }
    }

    pub(super) async fn handle_send_queued_prompt(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        id: uuid::Uuid,
        snapshot: String,
        cwd: String,
    ) {
        let Some(pos) = ctx.queued_prompts.iter().position(|item| item.id == id) else {
            return;
        };
        let Some(mut item) = ctx.queued_prompts.remove(pos) else {
            return;
        };

        if ctx.current_turn.is_some() {
            // A turn is already active: promote to front of queue and unpause.
            ctx.queued_prompts.push_front(item);
            ctx.queue_paused = false;
            self.emit_queue_updated(ctx).await;
            return;
        }

        ctx.queue_paused = false;

        if !self.incognito {
            if !snapshot.is_empty() {
                item.snapshot = snapshot;
            } else if let Some(latest_snap) =
                ctx.workspace.get_pane_snapshot(self.pane_id.clone()).await
            {
                item.snapshot = latest_snap;
            }
            if !cwd.is_empty() {
                item.cwd = cwd;
            } else if !ctx.current_dir.is_empty() && ctx.current_dir != "/" {
                item.cwd.clone_from(&ctx.current_dir);
            }
        }

        self.execute_dequeued_prompt(ctx, session_ctx, item).await;
        self.emit_queue_updated(ctx).await;
    }

    pub(super) async fn execute_dequeued_prompt(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        item: QueuedPrompt,
    ) {
        {
            let mut state_lock = self.state.lock().await;
            state_lock.next_roundtable_turn = None;
        }
        if item.is_user_message_variant {
            self.execute_user_message_now(
                ctx,
                session_ctx,
                item.raw_query,
                item.snapshot,
                item.cwd,
                item.image_attachments,
                item.image_attachment_meta,
            )
            .await;
        } else {
            self.execute_claw_query_now(
                ctx,
                session_ctx,
                item.raw_query,
                item.snapshot,
                item.cwd,
                item.image_attachments,
                item.image_attachment_meta,
            )
            .await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_claw_query(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        prompt_id: Option<uuid::Uuid>,
        query: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
        image_attachment_meta: Vec<boxxy_claw_protocol::ImageAttachmentMeta>,
    ) {
        let display_query = crate::engine::turn::extract_user_authored_prompt(&query).to_string();

        if !self.should_bypass_queue(ctx, &query, &display_query).await {
            let text_attachments = crate::engine::turn::extract_text_attachment_labels(&query);
            let thumb_paths = if self.incognito {
                Vec::new()
            } else {
                cache_user_attachment_thumbnails(&image_attachments).await
            };
            ctx.queue_paused = false;
            ctx.queued_prompts.push_back(QueuedPrompt {
                id: prompt_id.unwrap_or_else(uuid::Uuid::new_v4),
                raw_query: query,
                display_query,
                snapshot,
                cwd,
                image_attachments,
                image_attachment_meta,
                thumb_paths,
                text_attachments,
                is_user_message_variant: false,
            });
            self.emit_queue_updated(ctx).await;
            return;
        }

        if ctx.current_turn.is_none() && ctx.queue_paused {
            ctx.queue_paused = false;
            if !ctx.queued_prompts.is_empty() {
                self.emit_queue_updated(ctx).await;
            }
        }

        self.execute_claw_query_now(
            ctx,
            session_ctx,
            query,
            snapshot,
            cwd,
            image_attachments,
            image_attachment_meta,
        )
        .await;
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn execute_claw_query_now(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        query: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
        image_attachment_meta: Vec<boxxy_claw_protocol::ImageAttachmentMeta>,
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
                        if !ctx.queued_prompts.is_empty() && !ctx.queue_paused {
                            ctx.queue_paused = true;
                            self.emit_queue_updated(ctx).await;
                        }
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
                        if ctx.current_turn.is_none() {
                            let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
                        }
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
                image_attachment_meta,
            ));
            return;
        }

        let is_whats_new = crate::engine::turn::is_whats_new_prompt(&query);
        let display_query = crate::engine::turn::extract_user_authored_prompt(&query).to_string();

        // Pre-resolve any `@path/to/image.png` file references from user-authored text and order
        // all images by reading position so square thumbnails and vision blocks align 1:1.
        let (image_attachments, image_attachment_meta) = if is_whats_new {
            (image_attachments, image_attachment_meta)
        } else {
            let pre_resolved =
                crate::engine::turn::file_refs::resolve_prompt_file_refs(&display_query, &cwd)
                    .await;
            crate::engine::turn::multimodal::merge_and_order_turn_images(
                &display_query,
                image_attachments,
                image_attachment_meta,
                pre_resolved.image_attachments,
                &pre_resolved.image_attachment_meta,
            )
        };

        self.process_user_submission(
            ctx,
            session_ctx,
            query,
            display_query,
            is_whats_new,
            snapshot,
            cwd,
            image_attachments,
            image_attachment_meta,
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_user_message(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        message: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
        image_attachment_meta: Vec<boxxy_claw_protocol::ImageAttachmentMeta>,
    ) {
        if !self.should_bypass_queue(ctx, &message, &message).await {
            let thumb_paths = if self.incognito {
                Vec::new()
            } else {
                cache_user_attachment_thumbnails(&image_attachments).await
            };
            ctx.queue_paused = false;
            ctx.queued_prompts.push_back(QueuedPrompt {
                id: uuid::Uuid::new_v4(),
                raw_query: message.clone(),
                display_query: message,
                snapshot,
                cwd,
                image_attachments,
                image_attachment_meta,
                thumb_paths,
                text_attachments: Vec::new(),
                is_user_message_variant: true,
            });
            self.emit_queue_updated(ctx).await;
            return;
        }

        if ctx.current_turn.is_none() && ctx.queue_paused {
            ctx.queue_paused = false;
            if !ctx.queued_prompts.is_empty() {
                self.emit_queue_updated(ctx).await;
            }
        }

        self.execute_user_message_now(
            ctx,
            session_ctx,
            message,
            snapshot,
            cwd,
            image_attachments,
            image_attachment_meta,
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_user_message_now(
        &self,
        ctx: &mut RunContext,
        session_ctx: &str,
        message: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
        image_attachment_meta: Vec<boxxy_claw_protocol::ImageAttachmentMeta>,
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
            image_attachment_meta,
        )
        .await;
    }

    /// Resolves the live working directory for autonomous or delegated turns (`DelegatedTask`,
    /// `SubscriptionEvent`, `TaskCompletedEvent`, `ScheduledTaskTriggered`) when `ctx.current_dir`
    /// has not yet been seeded by a direct user `ClawQuery` on this pane.
    pub(super) async fn resolve_live_cwd(&self, ctx: &mut RunContext) -> String {
        if let Some(ws_cwd) = ctx
            .workspace
            .get_pane_cwd(self.pane_id.clone())
            .await
            .filter(|c| !c.is_empty() && c != "/")
        {
            ctx.current_dir.clone_from(&ws_cwd);
            return ws_cwd;
        }
        let state_cwd = self.state.lock().await.current_dir.clone();
        if !state_cwd.is_empty() && state_cwd != "/" {
            ctx.current_dir.clone_from(&state_cwd);
            return state_cwd;
        }
        ctx.current_dir.clone()
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
        let live_cwd = self.resolve_live_cwd(ctx).await;

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

        let is_headless = self.state.lock().await.is_headless;
        if !is_headless {
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
        }

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
            live_cwd,
            false,
            ctx.env.clone(),
            self.db.clone(),
            self.state.clone(),
            self.tx_ui.clone(),
            self.tx_self.clone(),
            Some(request_id),
            vec![],
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
                None,
                "/drama".to_string(),
                String::new(),
                String::new(),
                Vec::new(),
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
                None,
                "/undo".to_string(),
                String::new(),
                String::new(),
                Vec::new(),
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
                None,
                "/stop".to_string(),
                String::new(),
                String::new(),
                Vec::new(),
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

    #[tokio::test]
    async fn prompts_submitted_during_active_turn_enqueue_in_fifo_order_and_support_remove_recall_and_park() {
        let (session, _tx, rx_ui) = ClawSession::new("pane-queue-test".to_string());
        let session = session.with_identity(
            "char-host".to_string(),
            "Host Agent".to_string(),
            "sess-queue-test".to_string(),
        );
        let workspace = Arc::new(crate::registry::workspace::WorkspaceRegistry::new());
        let mut ctx = RunContext::new(Arc::new(NoopEnv), workspace);

        // Simulate an active running turn (`current_turn = Some(...)`).
        let (hold_tx, hold_rx) = tokio::sync::oneshot::channel::<()>();
        ctx.current_turn = Some(tokio::spawn(async move {
            let _ = hold_rx.await;
        }));

        let first_id = uuid::Uuid::new_v4();
        let second_id = uuid::Uuid::new_v4();

        // 1. Submitting two prompts while `current_turn.is_some()` enqueues them without aborting the turn.
        session
            .handle_claw_query(
                &mut ctx,
                "",
                Some(first_id),
                "Inspect src/parser.rs".to_string(),
                String::new(),
                "/workspace".to_string(),
                Vec::new(),
                Vec::new(),
            )
            .await;

        session
            .handle_claw_query(
                &mut ctx,
                "",
                Some(second_id),
                "Run unit tests [Image 1 · diagram.png]".to_string(),
                String::new(),
                "/workspace".to_string(),
                vec!["aGVsbG8=".to_string()],
                Vec::new(),
            )
            .await;

        assert!(
            ctx.current_turn
                .as_ref()
                .is_some_and(|h| !h.is_finished()),
            "Active turn must keep running when follow-up messages are queued"
        );
        assert_eq!(ctx.queued_prompts.len(), 2);
        assert!(!ctx.queue_paused);

        let mut events = Vec::new();
        while let Ok(ev) = rx_ui.try_recv() {
            events.push(ev);
        }
        assert!(matches!(
            events.last(),
            Some(ClawEngineEvent::QueueUpdated { items, paused: false })
                if items.len() == 2
                    && items[0].id == first_id
                    && items[0].display_text == "Inspect src/parser.rs"
                    && items[0].images.is_empty()
                    && items[1].id == second_id
                    && items[1].images.len() == 1
        ));

        // 2. Recalling `second_id` removes it from the queue and emits `QueuedPromptRecalled`.
        session
            .handle_remove_queued_prompt(&mut ctx, second_id, true)
            .await;
        assert_eq!(ctx.queued_prompts.len(), 1);
        assert_eq!(ctx.queued_prompts[0].id, first_id);

        let mut recall_events = Vec::new();
        while let Ok(ev) = rx_ui.try_recv() {
            recall_events.push(ev);
        }
        assert!(recall_events.iter().any(|e| matches!(
            e,
            ClawEngineEvent::QueueUpdated { items, paused: false } if items.len() == 1 && items[0].id == first_id
        )));
        assert!(recall_events.iter().any(|e| matches!(
            e,
            ClawEngineEvent::QueuedPromptRecalled { id, fallback_text }
                if *id == second_id && fallback_text == "Run unit tests [Image 1 · diagram.png]"
        )));

        // 3. Aborting the active turn parks (`paused: true`) the remaining queued item instead of deleting it.
        session.handle_abort(&mut ctx).await;
        assert!(ctx.current_turn.is_none());
        assert_eq!(ctx.queued_prompts.len(), 1);
        assert!(ctx.queue_paused);

        let mut abort_events = Vec::new();
        while let Ok(ev) = rx_ui.try_recv() {
            abort_events.push(ev);
        }
        assert!(abort_events.iter().any(|e| matches!(
            e,
            ClawEngineEvent::QueueUpdated { items, paused: true } if items.len() == 1 && items[0].id == first_id
        )));

        // 4. Removing the last item resets `queue_paused` and empties the queue.
        session
            .handle_remove_queued_prompt(&mut ctx, first_id, false)
            .await;
        assert!(ctx.queued_prompts.is_empty());
        assert!(!ctx.queue_paused);
        let _ = hold_tx.send(());
    }

    #[tokio::test]
    async fn pending_in_turn_proposal_is_steered_immediately_instead_of_queuing() {
        let (session, _tx, _rx_ui) = ClawSession::new("pane-steer-test".to_string());
        let session = session.with_identity(
            "char-host".to_string(),
            "Host Agent".to_string(),
            "sess-steer-test".to_string(),
        );
        let workspace = Arc::new(crate::registry::workspace::WorkspaceRegistry::new());
        let mut ctx = RunContext::new(Arc::new(NoopEnv), workspace);

        let (_hold_tx, hold_rx) = tokio::sync::oneshot::channel::<()>();
        ctx.current_turn = Some(tokio::spawn(async move {
            let _ = hold_rx.await;
        }));
        let (file_tx, file_rx) = tokio::sync::oneshot::channel();
        session.state.lock().await.pending_file_reply = Some(file_tx);

        session
            .handle_claw_query(
                &mut ctx,
                "",
                Some(uuid::Uuid::new_v4()),
                "Use src/config.rs instead".to_string(),
                String::new(),
                "/workspace".to_string(),
                Vec::new(),
                Vec::new(),
            )
            .await;

        assert!(
            ctx.queued_prompts.is_empty(),
            "Steering an awaiting tool approval must resolve the approval immediately, not sit in the queue"
        );
        assert_eq!(
            file_rx.await.unwrap(),
            boxxy_core_toolbox::ApprovalOutcome::Interrupted(
                "Use src/config.rs instead".to_string()
            )
        );
    }
}
