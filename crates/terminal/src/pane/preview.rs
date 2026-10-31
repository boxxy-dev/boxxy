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
                launch_uri(&term_for_primary_click, hyperlink.as_str());
            } else {
                let (match_opt, _tag, _) = term_for_primary_click.check_match_at(x, y);
                if let Some(matched) = match_opt {
                    let matched_str = matched.to_string();
                    if matched_str.starts_with("http://")
                        || matched_str.starts_with("https://")
                        || matched_str.starts_with("file://")
                    {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        launch_uri(&term_for_primary_click, &matched_str);
                    } else {
                        let cwd = inner.borrow().working_dir.clone();
                        if let Some(path) = resolve_local_path(&matched_str, cwd.as_deref()) {
                            gesture.set_state(gtk::EventSequenceState::Claimed);
                            launch_local_path(&term_for_primary_click, &path);
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

/// Strips an optional hostname authority from `file://hostname/path` URIs
/// (as emitted by OSC 8 producers like `ls --hyperlink` or `eza`) so GIO treats
/// them as local `file:///path` URIs.
pub(crate) fn normalize_file_uri(uri: &str) -> String {
    if let Some(after_scheme) = uri.strip_prefix("file://")
        && !after_scheme.starts_with('/')
        && let Some(slash_idx) = after_scheme.find('/')
    {
        return format!("file://{}", &after_scheme[slash_idx..]);
    }
    uri.to_string()
}

fn launch_local_path(terminal: &TerminalWidget, path: &std::path::Path) {
    let file = gio::File::for_path(path);
    let uri = file.uri();
    let ctx = terminal.display().app_launch_context();
    if gio::AppInfo::launch_default_for_uri(uri.as_str(), Some(&ctx)).is_ok() {
        return;
    }
    let parent_win = terminal
        .root()
        .and_then(|r| r.downcast::<gtk::Window>().ok());
    let launcher = gtk::FileLauncher::new(Some(&file));
    launcher.set_always_ask(false);
    gtk::glib::spawn_future_local(async move {
        let _ = launcher.launch_future(parent_win.as_ref()).await;
    });
}

fn launch_uri(terminal: &TerminalWidget, raw_uri: &str) {
    let normalized = normalize_file_uri(raw_uri);
    if normalized.starts_with("file://") {
        let file = gio::File::for_uri(&normalized);
        if let Some(local_path) = file.path() {
            launch_local_path(terminal, &local_path);
            return;
        }
        let ctx = terminal.display().app_launch_context();
        if gio::AppInfo::launch_default_for_uri(&normalized, Some(&ctx)).is_ok() {
            return;
        }
        let parent_win = terminal
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok());
        let launcher = gtk::FileLauncher::new(Some(&file));
        launcher.set_always_ask(false);
        gtk::glib::spawn_future_local(async move {
            let _ = launcher.launch_future(parent_win.as_ref()).await;
        });
        return;
    }

    let ctx = terminal.display().app_launch_context();
    if gio::AppInfo::launch_default_for_uri(&normalized, Some(&ctx)).is_ok() {
        return;
    }
    let parent_win = terminal
        .root()
        .and_then(|r| r.downcast::<gtk::Window>().ok());
    let launcher = gtk::UriLauncher::new(&normalized);
    gtk::glib::spawn_future_local(async move {
        let _ = launcher.launch_future(parent_win.as_ref()).await;
    });
}

/// Resolves a matched terminal path (supporting `~/`, `./`, `../`, `/`, relative
/// paths, and optional `:line` or `:line:col` suffixes) against `working_dir`
/// (and its ancestor directories or `$HOME`) if the target exists on disk.
pub(crate) fn resolve_local_path(
    raw_match: &str,
    working_dir: Option<&str>,
) -> Option<std::path::PathBuf> {
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
    if path_str.is_empty() {
        return None;
    }

    let canonicalize_existing = |candidate: PathBuf| -> Option<PathBuf> {
        candidate
            .exists()
            .then(|| candidate.canonicalize().unwrap_or(candidate))
    };

    if path_str == "~" || path_str == "~/" {
        let home = std::env::var("HOME").ok()?;
        return canonicalize_existing(PathBuf::from(home));
    }

    if let Some(rest) = path_str.strip_prefix("~/") {
        let home = std::env::var("HOME").ok()?;
        return canonicalize_existing(PathBuf::from(home).join(rest));
    }

    if path_str.starts_with('/') {
        return canonicalize_existing(PathBuf::from(path_str));
    }

    // Relative path: check against `working_dir` first, then its ancestor directories
    // (so clicking a relative path from an earlier `cd subdir/nested/` command line
    // still resolves after the shell has changed into that directory), then `$HOME`.
    if let Some(cwd) = working_dir {
        let clean_cwd = cwd.strip_prefix("file://").unwrap_or(cwd);
        if !clean_cwd.is_empty() {
            let cwd_path = Path::new(clean_cwd);
            for ancestor in cwd_path.ancestors() {
                if let Some(found) = canonicalize_existing(ancestor.join(path_str)) {
                    return Some(found);
                }
            }
        }
    }

    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && let Some(found) = canonicalize_existing(PathBuf::from(home).join(path_str))
    {
        return Some(found);
    }

    canonicalize_existing(PathBuf::from(path_str))
}

#[cfg(test)]
mod tests {
    use super::{normalize_file_uri, resolve_local_path};
    use std::fs;

    #[test]
    fn normalize_file_uri_strips_hostname_authority() {
        assert_eq!(
            normalize_file_uri("file://workstation/tmp/archive"),
            "file:///tmp/archive"
        );
        assert_eq!(
            normalize_file_uri("file:///tmp/archive"),
            "file:///tmp/archive"
        );
        assert_eq!(
            normalize_file_uri("https://example.com/docs"),
            "https://example.com/docs"
        );
    }

    #[test]
    fn resolve_local_path_resolves_relative_and_ancestor_directories() {
        let tmp = std::env::temp_dir().join(format!("boxxy-test-{}", uuid::Uuid::new_v4()));
        let nested = tmp.join("workspace").join("subdir");
        fs::create_dir_all(&nested).expect("create nested dir");

        let cwd_root = tmp.to_string_lossy().into_owned();
        let cwd_nested = nested.to_string_lossy().into_owned();
        let expected = nested.canonicalize().unwrap_or(nested.clone());

        // 1. Relative to current working directory (with trailing slash)
        assert_eq!(
            resolve_local_path("workspace/subdir/", Some(&cwd_root)),
            Some(expected.clone())
        );

        // 2. Relative to an ancestor directory after `cd workspace/subdir/`
        assert_eq!(
            resolve_local_path("workspace/subdir/", Some(&cwd_nested)),
            Some(expected)
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}
