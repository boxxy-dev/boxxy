use std::cell::RefCell;
use std::rc::Rc;
use gtk4::{gdk, gio};
use gtk4::prelude::*;
use gtk4 as gtk;
use boxxy_vte::terminal::TerminalWidget;
use boxxy_preferences::ImagePreviewTrigger;
use crate::preview::ImagePreviewPopover;
use super::PaneInner;

pub(super) fn setup_preview(
    terminal: &TerminalWidget,
    inner: &Rc<RefCell<PaneInner>>,
) {
    let preview = ImagePreviewPopover::new();
    preview.popover().set_parent(terminal);

    let primary_gesture = gtk::GestureClick::new();
    primary_gesture.set_button(gdk::BUTTON_PRIMARY);
    primary_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

    let term_for_primary_click = terminal.clone();
    let preview_clone_click = preview.clone();
    let inner_for_primary = Rc::downgrade(inner);
    
    primary_gesture.connect_pressed(move |gesture: &gtk::GestureClick, _n_press, x, y| {
        let Some(inner) = inner_for_primary.upgrade() else { return; };
        let state = gesture.current_event_state();
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            if let Some(hyperlink) = term_for_primary_click.check_hyperlink_at(x, y) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                let _ = gio::AppInfo::launch_default_for_uri(hyperlink.as_str(), None::<&gio::AppLaunchContext>);
            } else {
                let (match_opt, _tag) = term_for_primary_click.check_match_at(x, y);
                if let Some(matched) = match_opt {
                    let matched_str = matched.to_string();
                    if matched_str.starts_with("http://") || matched_str.starts_with("https://") {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        let _ = gio::AppInfo::launch_default_for_uri(&matched_str, None::<&gio::AppLaunchContext>);
                    }
                }
            }
        } else if state.contains(gdk::ModifierType::SHIFT_MASK) {
            if let Some(hyperlink) = term_for_primary_click.check_hyperlink_at(x, y) {
                let trigger = inner.borrow().current_settings.as_ref().map_or(ImagePreviewTrigger::Click, |s| s.image_preview_trigger.clone());
                
                if trigger == ImagePreviewTrigger::Click &&
                    (hyperlink.to_lowercase().ends_with(".png") ||
                     hyperlink.to_lowercase().ends_with(".jpg") ||
                     hyperlink.to_lowercase().ends_with(".jpeg") ||
                     hyperlink.to_lowercase().ends_with(".gif") ||
                     hyperlink.to_lowercase().ends_with(".webp") ||
                     hyperlink.to_lowercase().ends_with(".bmp") ||
                     hyperlink.to_lowercase().ends_with(".svg") ||
                     hyperlink.to_lowercase().ends_with(".mp4") ||
                     hyperlink.to_lowercase().ends_with(".webm") ||
                     hyperlink.to_lowercase().ends_with(".mkv") ||
                     hyperlink.to_lowercase().ends_with(".avi") ||
                     hyperlink.to_lowercase().ends_with(".mov")) {
                    
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
    let term_for_motion = terminal.clone();
    let preview_clone_motion = preview.clone();
    let inner_for_motion = Rc::downgrade(inner);

    motion_ctrl.connect_motion(move |_, x, y| {
        let Some(inner) = inner_for_motion.upgrade() else { return; };
        let trigger = inner.borrow().current_settings.as_ref().map_or(ImagePreviewTrigger::Click, |s| s.image_preview_trigger.clone());
        
        if trigger == ImagePreviewTrigger::Hover {
            if let Some(hyperlink) = term_for_motion.check_hyperlink_at(x, y) {
                if hyperlink.to_lowercase().ends_with(".png") ||
                   hyperlink.to_lowercase().ends_with(".jpg") ||
                   hyperlink.to_lowercase().ends_with(".jpeg") ||
                   hyperlink.to_lowercase().ends_with(".gif") ||
                   hyperlink.to_lowercase().ends_with(".webp") ||
                   hyperlink.to_lowercase().ends_with(".bmp") ||
                   hyperlink.to_lowercase().ends_with(".svg") ||
                   hyperlink.to_lowercase().ends_with(".mp4") ||
                   hyperlink.to_lowercase().ends_with(".webm") ||
                   hyperlink.to_lowercase().ends_with(".mkv") ||
                   hyperlink.to_lowercase().ends_with(".avi") ||
                   hyperlink.to_lowercase().ends_with(".mov") {
                    
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
