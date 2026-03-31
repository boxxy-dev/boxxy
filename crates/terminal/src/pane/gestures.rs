use super::app_menu::{AppMenuComponent, AppMenuContext};
use crate::PaneOutput;
use crate::search_bar::SearchBarComponent;
use boxxy_vte::terminal::TerminalWidget;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn setup_gestures(
    terminal: &TerminalWidget,
    search_bar: &Rc<SearchBarComponent>,
    callback: std::sync::Arc<dyn Fn(PaneOutput) + Send + Sync + 'static>,
    id: String,
) {
    let focus_ctrl = gtk::EventControllerFocus::new();
    let cb_clone = callback.clone();
    focus_ctrl.connect_enter(move |_| {
        cb_clone(PaneOutput::Focused(id.clone()));
    });
    terminal.add_controller(focus_ctrl);

    let action_group = gio::SimpleActionGroup::new();
    let copy_action = gio::SimpleAction::new("copy", None);
    let term_copy = terminal.clone();
    copy_action.connect_activate(move |_, _| {
        term_copy.copy_clipboard();
    });
    action_group.add_action(&copy_action);

    let paste_action = gio::SimpleAction::new("paste", None);
    let term_paste = terminal.clone();
    paste_action.connect_activate(move |_, _| {
        term_paste.paste_clipboard();
    });
    action_group.add_action(&paste_action);

    let select_all_action = gio::SimpleAction::new("select-all", None);
    let term_select = terminal.clone();
    select_all_action.connect_activate(move |_, _| {
        term_select.select_all();
    });
    action_group.add_action(&select_all_action);

    let copy_path_action = gio::SimpleAction::new("copy-path", Some(glib::VariantTy::STRING));
    let term_copy_path = terminal.clone();
    copy_path_action.connect_activate(move |_, param: Option<&glib::Variant>| {
        if let Some(path) = param.and_then(|v| v.get::<String>()) {
            term_copy_path.display().clipboard().set_text(&path);
        }
    });
    action_group.add_action(&copy_path_action);

    terminal.insert_action_group("term", Some(&action_group));

    let search_bar_clone_ev = search_bar.clone();
    let ev_ctrl_search = gtk::EventControllerKey::new();
    ev_ctrl_search.connect_key_pressed(move |_, keyval, _, state| {
        if (keyval == gtk::gdk::Key::f || keyval == gtk::gdk::Key::F)
            && state.contains(gtk::gdk::ModifierType::CONTROL_MASK)
            && state.contains(gtk::gdk::ModifierType::SHIFT_MASK)
        {
            if !search_bar_clone_ev.is_visible() {
                search_bar_clone_ev.reveal();
            } else {
                search_bar_clone_ev.hide();
            }
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    terminal.add_controller(ev_ctrl_search);

    let app_menu = AppMenuComponent::new();
    app_menu.widget().set_parent(terminal);

    let app_menu_widget = app_menu.widget().clone();
    terminal.connect_destroy(move |_| {
        app_menu_widget.unparent();
    });

    let middle_gesture = gtk::GestureClick::new();
    middle_gesture.set_button(gdk::BUTTON_MIDDLE);
    middle_gesture.set_propagation_phase(gtk::PropagationPhase::Capture);

    let term_for_middle_click = terminal.clone();
    middle_gesture.connect_pressed(move |gesture: &gtk::GestureClick, _n_press, _x, _y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        term_for_middle_click.paste_primary();
    });
    terminal.add_controller(middle_gesture);

    // Right-click context menu is driven by the terminal engine's ownership
    // decision (inside boxxy-vte's click_gesture).  When the engine decides
    // the terminal owns the right-click (no mouse-reporting mode active, or
    // Shift is held), it calls this callback instead of forwarding to the PTY.
    let term_for_menu = terminal.clone();
    let app_menu_clone = app_menu.clone();
    terminal.on_context_menu(move |x, y| {
        let mut path_to_copy = None;

        if let Some(hyperlink) = term_for_menu.check_hyperlink_at(x, y) {
            let hyper_str = hyperlink.to_string();
            if hyper_str.starts_with("file://") {
                if let Some(after_scheme) = hyper_str.strip_prefix("file://") {
                    if let Some(slash_idx) = after_scheme.find('/') {
                        path_to_copy = Some(after_scheme[slash_idx..].to_string());
                    } else {
                        path_to_copy = Some(hyper_str);
                    }
                } else {
                    path_to_copy = Some(hyper_str);
                }
            } else {
                path_to_copy = Some(hyper_str);
            }
        } else {
            let (match_opt, _tag, _) = term_for_menu.check_match_at(x, y);
            if let Some(matched) = match_opt {
                let matched_str = matched.to_string();

                if matched_str.starts_with("http://") || matched_str.starts_with("https://") {
                    path_to_copy = Some(matched_str);
                } else {
                    let resolved = matched_str;
                    let is_path = std::path::Path::new(&resolved).exists();
                    if is_path {
                        path_to_copy = Some(resolved);
                    }
                }
            }
        }

        let mut is_maximized = false;
        let mut current = term_for_menu.parent();
        while let Some(parent) = current {
            if parent.widget_name() == "maximized-container" {
                is_maximized = true;
                break;
            }
            current = parent.parent();
        }

        let rect = gdk::Rectangle::new(x as i32, y as i32, 1, 1);
        let ctx = AppMenuContext {
            is_maximized,
            path_to_copy,
            has_selection: term_for_menu.has_selection(),
        };

        app_menu_clone.show(rect, ctx);
    });

    terminal.set_allow_hyperlink(true);

    let scroll_ctrl = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let term_scroll = terminal.clone();
    let cb_scroll = callback.clone();
    scroll_ctrl.connect_scroll(move |scroll_ctrl, _, y| {
        let state = scroll_ctrl.current_event_state();
        let is_ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);

        // If Ctrl is held, we always zoom, even if mouse mode is active in the TUI.
        // This is standard behavior in most terminal emulators to allow zooming out of a TUI.
        if is_ctrl {
            if y < 0.0 {
                cb_scroll(PaneOutput::ZoomIn);
            } else if y > 0.0 {
                cb_scroll(PaneOutput::ZoomOut);
            }
            return gtk::glib::Propagation::Stop;
        }

        // Otherwise, if mouse mode is active, the TUI might want the scroll event.
        if term_scroll.is_mouse_mode() {
            return gtk::glib::Propagation::Proceed;
        }

        gtk::glib::Propagation::Proceed
    });
    terminal.add_controller(scroll_ctrl);

    let mouse_coords = Rc::new(RefCell::new((0.0, 0.0)));
    let motion_ctrl = gtk::EventControllerMotion::new();
    let mc_clone = mouse_coords.clone();
    motion_ctrl.connect_motion(move |_, x, y| {
        *mc_clone.borrow_mut() = (x, y);
    });
    terminal.add_controller(motion_ctrl);
}
