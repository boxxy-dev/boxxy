use super::app_menu::{AppMenuComponent, AppMenuContext};
use crate::PaneOutput;
use crate::search_bar::SearchBarComponent;
use boxxy_vte::terminal::TerminalWidget;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) fn export_and_open_scrollback(terminal: &TerminalWidget) {
    let selected = terminal.selected_text();
    let term_weak = terminal.downgrade();
    glib::spawn_future_local(async move {
        let Some(term) = term_weak.upgrade() else {
            return;
        };
        let text = match selected {
            Some(sel) if !sel.is_empty() => sel,
            _ => term.dump_buffer(true).await.unwrap_or_default(),
        };
        let path_res = boxxy_sys_utils::runtime()
            .spawn_blocking(move || -> std::io::Result<std::path::PathBuf> {
                let mut path = std::env::temp_dir();
                path.push(format!("boxxy-scrollback-{}.txt", uuid::Uuid::new_v4()));
                std::fs::write(&path, text.as_bytes())?;
                Ok(path)
            })
            .await;

        if let Ok(Ok(path)) = path_res
            && let Some(term) = term_weak.upgrade()
        {
            if let Some(path_str) = path.to_str() {
                term.display().clipboard().set_text(path_str);
            }
            let file = gio::File::for_path(&path);
            let launcher = gtk::FileLauncher::new(Some(&file));
            let root_win = term
                .root()
                .and_then(|r| r.downcast::<gtk::Window>().ok());
            launcher.launch(root_win.as_ref(), gio::Cancellable::NONE, |_| {});
        }
    });
}

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
    let term_copy = terminal.downgrade();
    copy_action.connect_activate(move |_, _| {
        if let Some(term) = term_copy.upgrade() {
            term.copy_clipboard();
        }
    });
    action_group.add_action(&copy_action);

    let paste_action = gio::SimpleAction::new("paste", None);
    let term_paste = terminal.downgrade();
    paste_action.connect_activate(move |_, _| {
        if let Some(term) = term_paste.upgrade() {
            term.paste_clipboard();
        }
    });
    action_group.add_action(&paste_action);

    let select_all_action = gio::SimpleAction::new("select-all", None);
    let term_select = terminal.downgrade();
    select_all_action.connect_activate(move |_, _| {
        if let Some(term) = term_select.upgrade() {
            term.select_all();
        }
    });
    action_group.add_action(&select_all_action);

    let copy_last_output_action = gio::SimpleAction::new("copy-last-output", None);
    let term_copy_last = terminal.downgrade();
    copy_last_output_action.connect_activate(move |_, _| {
        if let Some(term) = term_copy_last.upgrade() {
            term.copy_last_command_output();
        }
    });
    action_group.add_action(&copy_last_output_action);

    let open_scrollback_action = gio::SimpleAction::new("open-scrollback", None);
    let term_open_scrollback = terminal.downgrade();
    open_scrollback_action.connect_activate(move |_, _| {
        if let Some(term) = term_open_scrollback.upgrade() {
            export_and_open_scrollback(&term);
        }
    });
    action_group.add_action(&open_scrollback_action);

    let clear_scrollback_action = gio::SimpleAction::new("clear-scrollback", None);
    let term_clear_sb = terminal.downgrade();
    clear_scrollback_action.connect_activate(move |_, _| {
        if let Some(term) = term_clear_sb.upgrade() {
            term.clear_scrollback();
        }
    });
    action_group.add_action(&clear_scrollback_action);

    let reset_terminal_action = gio::SimpleAction::new("reset-terminal", None);
    let term_reset = terminal.downgrade();
    reset_terminal_action.connect_activate(move |_, _| {
        if let Some(term) = term_reset.upgrade() {
            term.reset_terminal();
        }
    });
    action_group.add_action(&reset_terminal_action);

    let copy_path_action = gio::SimpleAction::new("copy-path", Some(glib::VariantTy::STRING));
    let term_copy_path = terminal.downgrade();
    copy_path_action.connect_activate(move |_, param: Option<&glib::Variant>| {
        if let Some(path) = param.and_then(|v| v.get::<String>())
            && let Some(term) = term_copy_path.upgrade()
        {
            term.display().clipboard().set_text(&path);
        }
    });
    action_group.add_action(&copy_path_action);

    terminal.insert_action_group("term", Some(&action_group));

    let search_bar_clone_ev = search_bar.clone();
    let term_for_sb_ev = terminal.downgrade();
    let ev_ctrl_search = gtk::EventControllerKey::new();
    ev_ctrl_search.set_propagation_phase(gtk::PropagationPhase::Capture);
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
        } else if (keyval == gtk::gdk::Key::j || keyval == gtk::gdk::Key::J)
            && state.contains(
                gtk::gdk::ModifierType::CONTROL_MASK
                    | gtk::gdk::ModifierType::ALT_MASK
                    | gtk::gdk::ModifierType::SHIFT_MASK,
            )
        {
            if let Some(term) = term_for_sb_ev.upgrade() {
                export_and_open_scrollback(&term);
            }
            gtk::glib::Propagation::Stop
        } else if keyval == gtk::gdk::Key::Escape && search_bar_clone_ev.is_visible() {
            search_bar_clone_ev.hide();
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

    let term_for_middle_click = terminal.downgrade();
    middle_gesture.connect_pressed(move |gesture: &gtk::GestureClick, _n_press, _x, _y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        if let Some(term) = term_for_middle_click.upgrade() {
            term.paste_primary();
        }
    });
    terminal.add_controller(middle_gesture);

    // Right-click context menu is driven by the terminal engine's ownership
    // decision (inside boxxy-vte's click_gesture).  When the engine decides
    // the terminal owns the right-click (no mouse-reporting mode active, or
    // Shift is held), it calls this callback instead of forwarding to the PTY.
    let term_for_menu = terminal.downgrade();
    let app_menu_clone = app_menu.clone();
    terminal.on_context_menu(move |x, y| {
        let Some(term_for_menu) = term_for_menu.upgrade() else {
            return;
        };
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
    let term_scroll = terminal.downgrade();
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
        if let Some(term) = term_scroll.upgrade()
            && term.is_mouse_mode()
        {
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
