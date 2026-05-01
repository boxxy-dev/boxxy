//! Terminal-pane-backed implementation of `ClawHost`.
//!
//! This lives in `crates/terminal/src/pane/` (not in the root) because it
//! needs access to the module-private `PaneInner` struct. When
//! `ClawHost` moves into the `boxxy_claw_widget` crate later, this impl
//! stays in `crates/terminal` — it's the *terminal's* adapter for the
//! widget's abstract host.

use super::PaneInner;
use crate::PaneOutput;
use boxxy_claw_protocol::{ClawEngineEvent, ClawMessage};
use boxxy_claw_widget::ClawHost;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Weak;
use std::sync::Arc;

pub struct PaneClawHost {
    pub id: String,
    pub inner_weak: Weak<RefCell<PaneInner>>,
    pub claw_sender: async_channel::Sender<ClawMessage>,
    pub event_sender: async_channel::Sender<ClawEngineEvent>,
    pub callback: Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
}

impl ClawHost for PaneClawHost {
    fn host_id(&self) -> String {
        self.id.clone()
    }

    fn inject_line(&self, text: String) {
        let Some(inner) = self.inner_weak.upgrade() else {
            return;
        };
        let mut bytes = text.into_bytes();
        bytes.push(b'\r');
        inner.borrow().terminal.write_all(bytes);
    }

    fn execute_script(&self, filename: &str, script: String) {
        // Ephemeral execution file: write the expanded script to a
        // bookmark-runs cache dir, make it executable, then inject the
        // path with a leading space so it doesn't pollute shell history.
        let Some(dirs) = directories::ProjectDirs::from("org", "boxxy", "boxxy-terminal") else {
            // No project dirs available — fall back to piping the script
            // directly; the shell will interpret it line by line.
            self.inject_line(script);
            return;
        };

        let runs_dir = dirs
            .config_dir()
            .join("cache")
            .join("bookmarks")
            .join("runs");
        if !runs_dir.exists() {
            let _ = std::fs::create_dir_all(&runs_dir);
        }

        let uuid = uuid::Uuid::new_v4().to_string();
        let short_uuid = &uuid[0..6];

        let (stem, ext) = if let Some(idx) = filename.rfind('.') {
            (&filename[..idx], &filename[idx..])
        } else {
            (filename, "")
        };

        let temp_filename = format!("{}-{}{}", stem, short_uuid, ext);
        let temp_path = runs_dir.join(&temp_filename);

        if std::fs::write(&temp_path, &script).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(mut perms) = std::fs::metadata(&temp_path).map(|m| m.permissions()) {
                    perms.set_mode(0o755);
                    let _ = std::fs::set_permissions(&temp_path, perms);
                }
            }

            // Leading space = HISTCONTROL=ignorespace; keeps the runs
            // dir out of the user's shell history.
            let cmd_line = format!(" {}", temp_path.display());
            self.inject_line(cmd_line);
        }
    }

    fn snapshot(
        &self,
        max_lines: usize,
        offset_lines: usize,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + 'static>> {
        let terminal = self
            .inner_weak
            .upgrade()
            .map(|i| i.borrow().terminal.clone());
        Box::pin(async move {
            match terminal {
                Some(term) => term.get_text_snapshot(max_lines, offset_lines).await,
                None => None,
            }
        })
    }

    fn grab_focus(&self) {
        if let Some(inner) = self.inner_weak.upgrade() {
            inner.borrow().terminal.grab_focus();
        }
    }

    fn set_focusable(&self, focusable: bool) {
        if let Some(inner) = self.inner_weak.upgrade() {
            inner.borrow().terminal.set_focusable(focusable);
        }
    }

    fn is_busy(&self) -> bool {
        self.inner_weak
            .upgrade()
            .map(|i| i.borrow().terminal.is_alt_screen())
            .unwrap_or(false)
    }

    fn working_dir(&self) -> Option<String> {
        self.inner_weak
            .upgrade()
            .and_then(|i| i.borrow().working_dir.clone())
    }

    fn send_claw(&self, msg: ClawMessage) {
        let tx = self.claw_sender.clone();
        gtk::glib::spawn_future_local(async move {
            let _ = tx.send(msg).await;
        });
    }

    fn focus_sidebar(&self) {
        (self.callback)(PaneOutput::FocusClawSidebar(self.id.clone()));
    }

    fn focus(&self) {
        (self.callback)(PaneOutput::RequestFocus(self.id.clone()));
    }

    fn cd(&self, path: String) {
        let Some(inner) = self.inner_weak.upgrade() else {
            return;
        };
        let terminal = inner.borrow().terminal.clone();
        let cb = self.callback.clone();
        let id = self.id.clone();
        // Full-screen TUIs (vim, less) should not be fought with cd;
        // announce the skip instead so the user knows the resumed
        // session's intent without clobbering their editor buffer.
        if terminal.is_alt_screen() {
            cb(PaneOutput::Notification(
                id,
                "Session resumed, but folder switch skipped (Terminal Busy).".to_string(),
            ));
            return;
        }
        if std::path::Path::new(&path).exists() {
            terminal.write_all(format!("cd \"{}\"\n", path).into_bytes());
        } else {
            cb(PaneOutput::Notification(
                id,
                format!(
                    "Directory '{}' no longer exists. Staying in current folder.",
                    path
                ),
            ));
        }
    }

    fn notify(&self, message: String) {
        (self.callback)(PaneOutput::Notification(self.id.clone(), message));
    }

    fn forward_event(&self, event: ClawEngineEvent) {
        (self.callback)(PaneOutput::ClawEvent(self.id.clone(), event));
    }

    fn request_character(&self, character_id: String) {
        let Some(inner_rc) = self.inner_weak.upgrade() else {
            return;
        };

        let id = self.id.clone();
        let event_sender = self.event_sender.clone();

        gtk::glib::spawn_future_local(async move {
            let agent = crate::get_agent().await;

            if let Ok((session_id, rx_events)) = agent
                .create_claw_session(id.clone(), character_id.clone())
                .await
            {
                if session_id.is_empty() {
                    return; // Exclusivity check failed
                }

                if let Some(inner) = inner_rc.try_borrow_mut().ok() {
                    let session_arc = inner.session_id.clone();
                    tokio::spawn(async move {
                        let mut lock = session_arc.lock().await;
                        *lock = Some(session_id.clone());
                    });
                }

                let tx = event_sender.clone();
                tokio::spawn(async move {
                    while let Ok(event) = rx_events.recv().await {
                        let _ = tx.send(event).await;
                    }
                });
            } else {
                log::warn!("Failed to request character id={}", character_id);
            }
        });
    }
}
