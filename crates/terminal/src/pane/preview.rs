use super::PaneInner;
use crate::preview::ImagePreviewPopover;
use boxxy_preferences::ImagePreviewTrigger;
use boxxy_vte::terminal::TerminalWidget;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, gio};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn setup_preview(terminal: &TerminalWidget, inner: &Rc<RefCell<PaneInner>>) {
    let preview = ImagePreviewPopover::new();
    preview.popover().set_parent(terminal);

    let preview_popover = preview.popover().clone();
    terminal.connect_destroy(move |_| {
        preview_popover.unparent();
    });

    let primary_gesture = gtk::GestureClick::new();
    primary_gesture.set_button(gdk::BUTTON_PRIMARY);
    primary_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

    let term_for_primary_click = terminal.downgrade();
    let preview_clone_click = preview.clone();
    let inner_for_primary = Rc::downgrade(inner);

    primary_gesture.connect_pressed(move |gesture: &gtk::GestureClick, _n_press, x, y| {
        let (Some(inner), Some(term_for_primary_click)) =
            (inner_for_primary.upgrade(), term_for_primary_click.upgrade())
        else {
            return;
        };
        let state = gesture.current_event_state();
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            if let Some(hyperlink) = term_for_primary_click.check_hyperlink_at(x, y) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                let _ = gio::AppInfo::launch_default_for_uri(
                    hyperlink.as_str(),
                    None::<&gio::AppLaunchContext>,
                );
            } else {
                let (match_opt, _tag, _) = term_for_primary_click.check_match_at(x, y);
                if let Some(matched) = match_opt {
                    let matched_str = matched.to_string();
                    if matched_str.starts_with("http://")
                        || matched_str.starts_with("https://")
                        || matched_str.starts_with("file://")
                    {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        let _ = gio::AppInfo::launch_default_for_uri(
                            &matched_str,
                            None::<&gio::AppLaunchContext>,
                        );
                    } else {
                        let cwd = inner.borrow().working_dir.clone();
                        if let Some(uri) = resolve_local_path_to_file_uri(&matched_str, cwd.as_deref()) {
                            gesture.set_state(gtk::EventSequenceState::Claimed);
                            let _ = gio::AppInfo::launch_default_for_uri(
                                &uri,
                                None::<&gio::AppLaunchContext>,
                            );
                        }
                    }
                }
            }
        } else if state.contains(gdk::ModifierType::SHIFT_MASK) {
            if let Some(hyperlink) = term_for_primary_click.check_hyperlink_at(x, y) {
                let trigger = inner
                    .borrow()
                    .current_settings
                    .as_ref()
                    .map_or(ImagePreviewTrigger::Click, |s| {
                        s.image_preview_trigger.clone()
                    });

                if trigger == ImagePreviewTrigger::Click
                    && (hyperlink.to_lowercase().ends_with(".png")
                        || hyperlink.to_lowercase().ends_with(".jpg")
                        || hyperlink.to_lowercase().ends_with(".jpeg")
                        || hyperlink.to_lowercase().ends_with(".gif")
                        || hyperlink.to_lowercase().ends_with(".webp")
                        || hyperlink.to_lowercase().ends_with(".bmp")
                        || hyperlink.to_lowercase().ends_with(".svg")
                        || hyperlink.to_lowercase().ends_with(".mp4")
                        || hyperlink.to_lowercase().ends_with(".webm")
                        || hyperlink.to_lowercase().ends_with(".mkv")
                        || hyperlink.to_lowercase().ends_with(".avi")
                        || hyperlink.to_lowercase().ends_with(".mov"))
                {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    preview_clone_click.show_preview(&hyperlink, x, y, false);
                } else {
                    preview_clone_click.hide_preview_if_not_pinned();
                }
            } else {
                preview_clone_click.hide_preview_if_not_pinned();
            }
        } else {
            preview_clone_click.hide_preview_if_not_pinned();
        }
    });
    terminal.add_controller(primary_gesture);

    let motion_ctrl = gtk::EventControllerMotion::new();
    let term_for_motion = terminal.downgrade();
    let preview_clone_motion = preview.clone();
    let inner_for_motion = Rc::downgrade(inner);

    motion_ctrl.connect_motion(move |_, x, y| {
        let (Some(inner), Some(term_for_motion)) =
            (inner_for_motion.upgrade(), term_for_motion.upgrade())
        else {
            return;
        };
        let trigger = inner
            .borrow()
            .current_settings
            .as_ref()
            .map_or(ImagePreviewTrigger::Click, |s| {
                s.image_preview_trigger.clone()
            });

        if trigger == ImagePreviewTrigger::Hover {
            if let Some(hyperlink) = term_for_motion.check_hyperlink_at(x, y) {
                if hyperlink.to_lowercase().ends_with(".png")
                    || hyperlink.to_lowercase().ends_with(".jpg")
                    || hyperlink.to_lowercase().ends_with(".jpeg")
                    || hyperlink.to_lowercase().ends_with(".gif")
                    || hyperlink.to_lowercase().ends_with(".webp")
                    || hyperlink.to_lowercase().ends_with(".bmp")
                    || hyperlink.to_lowercase().ends_with(".svg")
                    || hyperlink.to_lowercase().ends_with(".mp4")
                    || hyperlink.to_lowercase().ends_with(".webm")
                    || hyperlink.to_lowercase().ends_with(".mkv")
                    || hyperlink.to_lowercase().ends_with(".avi")
                    || hyperlink.to_lowercase().ends_with(".mov")
                {
                    preview_clone_motion.show_preview(&hyperlink, x, y, true);
                } else {
                    preview_clone_motion.hide_preview_if_not_pinned();
                }
            } else {
                preview_clone_motion.hide_preview_if_not_pinned();
            }
        }
    });

    let preview_clone_leave = preview.clone();
    motion_ctrl.connect_leave(move |_| {
        preview_clone_leave.hide_preview_if_not_pinned();
    });

    terminal.add_controller(motion_ctrl);
}

/// Resolves a matched terminal path (supporting `~/`, `./`, `../`, `/`, and optional
/// `:line` or `:line:col` suffixes) against `working_dir` and returns a `file://` URI
/// if the target exists on disk.
fn resolve_local_path_to_file_uri(raw_match: &str, working_dir: Option<&str>) -> Option<String> {
    use std::path::{Path, PathBuf};

    fn strip_line_col(s: &str) -> &str {
        if let Some((before, last)) = s.rsplit_once(':')
            && !last.is_empty()
            && last.bytes().all(|b| b.is_ascii_digit())
        {
            if let Some((before2, mid)) = before.rsplit_once(':')
                && !mid.is_empty()
                && mid.bytes().all(|b| b.is_ascii_digit())
            {
                return before2;
            }
            return before;
        }
        s
    }

    let path_str = if Path::new(raw_match).exists() {
        raw_match
    } else {
        strip_line_col(raw_match)
    };

    let expanded: PathBuf = if let Some(rest) = path_str.strip_prefix("~/") {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(home).join(rest)
    } else if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else if let Some(cwd) = working_dir {
        let clean_cwd = cwd.strip_prefix("file://").unwrap_or(cwd);
        PathBuf::from(clean_cwd).join(path_str)
    } else {
        PathBuf::from(path_str)
    };

    if expanded.exists() {
        let canonical = expanded.canonicalize().unwrap_or(expanded);
        Some(format!("file://{}", canonical.to_string_lossy()))
    } else {
        None
    }
}
