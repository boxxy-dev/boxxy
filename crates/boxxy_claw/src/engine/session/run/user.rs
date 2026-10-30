use base64::Engine as _;
use log::debug;
use sha2::{Digest, Sha256};

use crate::engine::session::ClawSession;
use crate::engine::{ClawEngineEvent, ClawMessage, turn::spawn_turn};

use super::context::RunContext;

const ATTACHMENT_SQUARE_THUMB_PX: u32 = 128;

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
    /// Evaluates the user's message for `/stop`, `/drama`, `/debate`, `@character` mentions, or
    /// participation in an active roundtable. Returns `Err(status_msg)` if the command was handled
    /// locally (like `/stop` or `/drama` toggle) without needing an immediate LLM turn.
    async fn prepare_user_roundtable_turn(&self, raw_input: &str) -> Result<String, String> {
        use crate::engine::session::roundtable::{
            SpeakerIdentity, UserRoundtableIntent, get_active_catalog, parse_user_roundtable_intent,
        };

        let catalog = get_active_catalog();
        let host_slug = catalog
            .iter()
            .find(|c| c.config.id == self.character_id)
            .map(|c| c.config.name.clone())
            .unwrap_or_else(|| self.name.to_lowercase().replace(' ', "-"));

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
            UserRoundtableIntent::Stop => {
                state_lock.roundtable.clear();
                state_lock.active_speaker = None;
                Err("*Multi-character conversation paused. Let me know what you'd like to do next!*".to_string())
            }
            UserRoundtableIntent::ToggleDrama => {
                let next = !state_lock.roundtable.drama_mode;
                state_lock.roundtable.drama_mode = next;
                if next {
                    Err("🎭 **Drama Mode enabled!** Characters will now speak in short, casual 1-sentence Visual Novel lines with a typewriter effect.".to_string())
                } else {
                    Err("🎭 **Drama Mode disabled.** Characters will return to full technical responses.".to_string())
                }
            }
            UserRoundtableIntent::Drama { guests, topic } => {
                if guests.is_empty() {
                    // Single-character Drama Mode: chat 1-on-1 with the Host character in
                    // Visual Novel style (1-2 sentences + typewriter) and wait for the user!
                    state_lock.roundtable.clear();
                    state_lock.roundtable.drama_mode = true;
                    state_lock.active_speaker = None;
                    return Ok(topic);
                }

                let resolved_guests: Vec<SpeakerIdentity> = guests
                    .into_iter()
                    .map(|mut g| {
                        if let Some(info) =
                            catalog.iter().find(|c| c.config.id == g.character_id)
                        {
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
                Ok(format!(
                    "[VISUAL NOVEL DRAMA MODE STARTED]\nTopic: {topic}\n\
                    Reply in ONLY 1 to 2 short, expressive spoken sentences (NO bullet points, NO lists, NO headers) and directly address the other participant(s)!"
                ))
            }
            UserRoundtableIntent::Debate { guests, topic } => {
                // Re-resolve guests with `self.pane_id` so active swarm peers get their `home_pane_id` populated
                let resolved_guests: Vec<SpeakerIdentity> = guests
                    .into_iter()
                    .map(|mut g| {
                        if let Some(info) =
                            catalog.iter().find(|c| c.config.id == g.character_id)
                        {
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
                Ok(format!(
                    "[ROUNDTABLE DEBATE STARTED]\nTopic: {topic}\n\
                    Kick off the discussion in your character's voice and directly engage the other participant(s) and the user!"
                ))
            }
            UserRoundtableIntent::Mention {
                mentioned,
                multi_turn,
                prompt,
            } => {
                let resolved_mentions: Vec<SpeakerIdentity> = mentioned
                    .into_iter()
                    .map(|mut m| {
                        if let Some(info) =
                            catalog.iter().find(|c| c.config.id == m.character_id)
                        {
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
                Ok(prompt)
            }
            UserRoundtableIntent::None => {
                if state_lock.roundtable.participants.len() >= 2 {
                    // User is interjecting / participating in an active roundtable conversation —
                    // queue Host + Guest(s) and reset the 20-round budget!
                    state_lock
                        .roundtable
                        .resume_after_interjection(host, raw_input.to_string());
                    state_lock.active_speaker = state_lock.roundtable.speaker_queue.pop_front();
                } else {
                    state_lock.active_speaker = None;
                }
                Ok(raw_input.to_string())
            }
        }
    }

    pub(super) async fn handle_claw_query(
        &mut self,
        ctx: &mut RunContext,
        session_ctx: &str,
        query: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
    ) {
        ctx.current_dir = cwd.clone();
        self.ensure_session_persisted(&cwd).await;
        let thumb_paths = cache_user_attachment_thumbnails(&image_attachments).await;
        let display_query = query
            .split("\n\n--- UPDATE CHANGELOG CONTEXT ---")
            .next()
            .unwrap_or(&query)
            .to_string();
        self.send_ui(ClawEngineEvent::UserMessage {
            content: display_query,
            images: thumb_paths,
        })
        .await;

        // Resolve any waiting tool approval channel with the user's new instruction
        // so an in-flight turn can immediately steer without tearing down and restarting.
        let mut fulfilled = {
            let mut state_lock = self.state.lock().await;
            state_lock.drain_pending_with_interruption(&query)
        };
        let outcome = boxxy_core_toolbox::ApprovalOutcome::Interrupted(query.clone());
        if ctx
            .workspace
            .drain_pending_commands_for_source(&self.pane_id, outcome.rejection_message())
            .await
        {
            fulfilled = true;
        }

        if fulfilled {
            debug!(
                "Pane {} ({}): Fulfilled pending tool approval with user query: {query}.",
                self.pane_id, self.name
            );
            ctx.workspace
                .update_pane_state(
                    self.pane_id.clone(),
                    Some(self.session_id.clone()),
                    Some(self.name.clone()),
                    ctx.current_dir.clone(),
                    Some(query),
                    Some(snapshot),
                )
                .await;
            return;
        }

        if let Some(handle) = ctx.current_turn.take() {
            handle.abort();
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
        ctx.backlog.clear();

        let effective_query = match self.prepare_user_roundtable_turn(&query).await {
            Ok(q) => q,
            Err(status_msg) => {
                self.send_ui(ClawEngineEvent::DiagnosisComplete {
                    agent_name: self.name.clone(),
                    character_id: self.character_id.clone(),
                    diagnosis: status_msg,
                    usage: None,
                    typewriter: false,
                })
                .await;
                let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
                return;
            }
        };

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

    pub(super) async fn handle_user_message(
        &mut self,
        ctx: &mut RunContext,
        session_ctx: &str,
        message: String,
        snapshot: String,
        cwd: String,
        image_attachments: Vec<String>,
    ) {
        ctx.current_dir = cwd.clone();
        self.ensure_session_persisted(&cwd).await;
        let thumb_paths = cache_user_attachment_thumbnails(&image_attachments).await;
        self.send_ui(ClawEngineEvent::UserMessage {
            content: message.clone(),
            images: thumb_paths,
        })
        .await;

        debug!(
            "Pane {} ({}): User reply: {message}. Checking for pending tools.",
            self.pane_id, self.name
        );

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

        let mut state_lock = self.state.lock().await;
        let mut fulfilled = state_lock.drain_pending_with_interruption(&message);
        drop(state_lock);
        let outcome = boxxy_core_toolbox::ApprovalOutcome::Interrupted(message.clone());
        if ctx
            .workspace
            .drain_pending_commands_for_source(&self.pane_id, outcome.rejection_message())
            .await
        {
            fulfilled = true;
        }

        if fulfilled {
            debug!(
                "Pane {} ({}): Fulfilled pending tool with user feedback.",
                self.pane_id, self.name
            );
        } else {
            if let Some(handle) = ctx.current_turn.take() {
                handle.abort();
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
            ctx.backlog.clear();

            let effective_message = match self.prepare_user_roundtable_turn(&message).await {
                Ok(m) => m,
                Err(status_msg) => {
                    self.send_ui(ClawEngineEvent::DiagnosisComplete {
                        agent_name: self.name.clone(),
                        character_id: self.character_id.clone(),
                        diagnosis: status_msg,
                        usage: None,
                        typewriter: false,
                    })
                    .await;
                    let _ = self.tx_self.send(ClawMessage::TurnFinished).await;
                    return;
                }
            };

            ctx.current_turn = Some(spawn_turn(
                self.pane_id.clone(),
                self.session_id.clone(),
                self.name.clone(),
                &effective_message,
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

    pub(super) async fn handle_delegated_task(
        &mut self,
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

        let full_prompt = format!(
            "*Task delegated from agent '{}'*: {}",
            source_agent_name, prompt
        );

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
}
