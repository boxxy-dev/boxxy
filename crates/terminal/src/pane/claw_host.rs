//! Terminal-pane-backed implementation of `ClawHost`.
//!
//! Lives in `crates/terminal/src/pane/` because it adapts `PaneInner` +
//! `claw_sender` + `PaneOutput` to the widget crate's surface-agnostic trait.

use super::PaneInner;
use crate::PaneOutput;
use boxxy_claw_protocol::{ClawEngineEvent, ClawMessage};
use boxxy_claw_widget::ClawHost;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Weak;
use std::sync::Arc;

pub(crate) fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn build_cd_command(path: &str) -> String {
    format!("cd -- {}\r", shell_single_quote(path))
}

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

    fn inject_line_to_pane(&self, target_pane_id: &str, text: String) {
        if let Some(target_inner) = crate::find_live_pane(target_pane_id) {
            let mut bytes = text.into_bytes();
            bytes.push(b'\r');
            target_inner.borrow().terminal.write_all(bytes);
        } else {
            self.inject_line(text);
        }
    }

    fn execute_script(&self, filename: &str, script: String) {
        let filename_owned = filename.to_string();
        let inner_weak = self.inner_weak.clone();

        // Write and chmod the ephemeral script off the GTK main thread, then
        // inject its shell-quoted path with a leading space (`HISTCONTROL=ignorespace`).
        gtk::glib::spawn_future_local(async move {
            let write_res = boxxy_sys_utils::runtime()
                .spawn_blocking(move || -> std::io::Result<PathBuf> {
                    let runs_dir = boxxy_sys_utils::get_config_dir()
                        .join("cache")
                        .join("bookmarks")
                        .join("runs");
                    std::fs::create_dir_all(&runs_dir)?;

                    let uuid = uuid::Uuid::new_v4().to_string();
                    let short_uuid = &uuid[..6];
                    let (stem, ext) = if let Some(idx) = filename_owned.rfind('.') {
                        (&filename_owned[..idx], &filename_owned[idx..])
                    } else {
                        (filename_owned.as_str(), "")
                    };
                    let temp_path = runs_dir.join(format!("{stem}-{short_uuid}{ext}"));
                    std::fs::write(&temp_path, script.as_bytes())?;

                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mut perms = std::fs::metadata(&temp_path)?.permissions();
                        perms.set_mode(0o755);
                        std::fs::set_permissions(&temp_path, perms)?;
                    }

                    Ok(temp_path)
                })
                .await;

            if let Ok(Ok(temp_path)) = write_res
                && let Some(inner) = inner_weak.upgrade()
            {
                let cmd_line = format!(" {}\r", shell_single_quote(&temp_path.to_string_lossy()));
                inner.borrow().terminal.write_all(cmd_line.into_bytes());
            }
        });
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

    fn snapshot_pane(
        &self,
        target_pane_id: Option<String>,
        max_lines: usize,
        offset_lines: usize,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + 'static>> {
        let terminal = if let Some(ref tid) = target_pane_id
            && !tid.is_empty()
            && tid != &self.id
        {
            crate::find_live_pane(tid).map(|i| i.borrow().terminal.clone())
        } else {
            self.inner_weak
                .upgrade()
                .map(|i| i.borrow().terminal.clone())
        };
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
            .is_some_and(|i| i.borrow().terminal.is_alt_screen())
    }

    fn has_foreground_process(&self) -> bool {
        self.inner_weak.upgrade().is_some_and(|i| {
            let b = i.borrow();
            b.terminal.is_alt_screen()
                || b.foreground_process
                    .as_deref()
                    .is_some_and(|p| !p.trim().is_empty())
        })
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
        let inner_weak = self.inner_weak.clone();
        let cb = self.callback.clone();
        let id = self.id.clone();

        gtk::glib::spawn_future_local(async move {
            let path_for_check = path.clone();
            let exists = boxxy_sys_utils::runtime()
                .spawn_blocking(move || std::path::Path::new(&path_for_check).is_dir())
                .await
                .unwrap_or(false);

            let Some(inner) = inner_weak.upgrade() else {
                return;
            };
            let terminal = inner.borrow().terminal.clone();

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

            if exists {
                terminal.write_all(build_cd_command(&path).into_bytes());
            } else {
                cb(PaneOutput::Notification(
                    id,
                    format!("Directory '{path}' no longer exists. Staying in current folder."),
                ));
            }
        });
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

            if let Ok((session_id, sub_guard)) = agent
                .create_claw_session_with_sender(id, character_id.clone(), event_sender)
                .await
            {
                if session_id.is_empty() {
                    return; // Exclusivity check failed
                }

                if let Ok(inner) = inner_rc.try_borrow_mut() {
                    *inner.claw_subscription.lock() = Some(sub_guard);
                    let session_arc = inner.session_id.clone();
                    tokio::spawn(async move {
                        let mut lock = session_arc.lock().await;
                        *lock = Some(session_id);
                    });
                }
            } else {
                log::warn!("Failed to request character id={character_id}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{build_cd_command, shell_single_quote};

    #[test]
    fn cd_command_prevents_expansion_and_handles_quotes_and_dashes() {
        assert_eq!(
            build_cd_command("/tmp/workspace/it's \"$HOME\" $(whoami)"),
            "cd -- '/tmp/workspace/it'\\''s \"$HOME\" $(whoami)'\r"
        );
        assert_eq!(
            shell_single_quote("-leading-dash"),
            "'-leading-dash'"
        );
    }
}
