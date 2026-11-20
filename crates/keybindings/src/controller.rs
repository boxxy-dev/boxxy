use crate::action::{ActionScope, ShortcutId, TerminalInputMode};
use crate::defaults::spec_for;
use crate::trigger::Chord;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;

/// Describes where keyboard focus currently resides inside an `AdwApplicationWindow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusContext {
    /// Focus is directly on `TerminalWidget` in the given input mode (`Normal`, future `Vi*`).
    Terminal(TerminalInputMode),
    /// Focus is inside the terminal pane's `SearchBarComponent` (`search-bar-osd`).
    TerminalSearchBar,
    /// Focus is inside the Claw overlay drawer (`claw-drawer`).
    ClawOverlay { in_text_input: bool },
    /// Focus is inside an open `GtkPopover` (`CommandPalette`, `Autocomplete`, menu).
    Popover,
    /// Focus is elsewhere in the window chrome (Sidebar, `TabBar`, Bookmarks tab).
    WindowChrome { in_text_input: bool },
}

/// Host callbacks implemented by `AppWindow` so `boxxy-keybindings` stays decoupled from
/// `boxxy-window` and `boxxy-terminal` internals.
pub trait WindowShortcutHost: 'static {
    /// Evaluates whether a `performable:` conditional binding can act right now.
    fn can_perform(&self, id: ShortcutId) -> bool;

    /// Queries the focused terminal pane's `TerminalInputMode` (`Normal` today; Vi hook for Stage 2).
    fn terminal_input_mode(&self) -> TerminalInputMode {
        TerminalInputMode::Normal
    }

    /// Dispatches `id` synchronously during key-press handling so GDK Wayland retains the
    /// active input event serial for clipboard operations (`win.copy`, `win.paste`).
    fn activate(&self, window: &adw::ApplicationWindow, id: ShortcutId, focus: FocusContext);
}

/// Inspects `window.focus()` to determine the active `FocusContext`.
#[must_use]
pub fn classify_window_focus(
    window: &adw::ApplicationWindow,
    terminal_mode: TerminalInputMode,
) -> FocusContext {
    let Some(focused) = RootExt::focus(window) else {
        return FocusContext::WindowChrome {
            in_text_input: false,
        };
    };

    if focused.type_().name() == "BoxxyTerminalWidget"
        || focused.css_name() == "boxxyterminalwidget"
    {
        return FocusContext::Terminal(terminal_mode);
    }

    let in_text_input = is_editable_widget(&focused);

    let mut cur = Some(focused);
    let mut in_popover = false;
    let mut in_claw_drawer = false;
    let mut in_search_bar = false;
    let mut in_terminal_pane = false;

    while let Some(w) = cur {
        if w.is::<gtk::Popover>() {
            in_popover = true;
        }
        if w.has_css_class("claw-drawer") {
            in_claw_drawer = true;
        }
        if w.has_css_class("search-bar-osd") {
            in_search_bar = true;
        }
        if w.has_css_class("terminal-pane") {
            in_terminal_pane = true;
        }
        cur = w.parent();
    }

    // Popovers (Command Palette, @file Autocomplete, context menus) own their own navigation.
    if in_popover {
        return FocusContext::Popover;
    }
    if in_search_bar {
        return FocusContext::TerminalSearchBar;
    }
    if in_claw_drawer || in_terminal_pane {
        return FocusContext::ClawOverlay { in_text_input };
    }

    FocusContext::WindowChrome { in_text_input }
}

fn is_editable_widget(widget: &gtk::Widget) -> bool {
    widget.is::<gtk::Text>()
        || widget.is::<gtk::TextView>()
        || widget.is::<gtk::Editable>()
        || widget.ancestor(gtk::Editable::static_type()).is_some()
        || widget.ancestor(gtk::TextView::static_type()).is_some()
}

/// Checks whether a shortcut with `(id, scope, chord)` is allowed to fire in `focus`.
#[must_use]
pub fn is_binding_eligible(id: ShortcutId, scope: ActionScope, chord: Chord, focus: FocusContext) -> bool {
    match focus {
        FocusContext::Popover => {
            // While a popover (e.g. Command Palette or Autocomplete) is open, only allow
            // toggling the Command Palette itself; never hijack Up/Down/Enter/Tab/Ctrl+V.
            id == ShortcutId::CommandPalette
        }
        FocusContext::Terminal(mode) => match mode {
            TerminalInputMode::Normal => true,
            TerminalInputMode::Search => scope != ActionScope::TerminalFocused || id == ShortcutId::Search,
            // Reserved for Stage 2 Vi Mode: global/pane shortcuts still work unless unbound,
            // while Vi key-table handles unmodified single-key motions.
            TerminalInputMode::ViNormal | TerminalInputMode::ViVisual => {
                !chord.modifiers.is_empty() || chord.is_bare_function_key()
            }
        },
        FocusContext::TerminalSearchBar => {
            if id == ShortcutId::Search {
                return true;
            }
            if scope == ActionScope::TerminalFocused {
                return false;
            }
            !conflicts_with_text_entry(chord)
        }
        FocusContext::ClawOverlay { in_text_input } => {
            if scope == ActionScope::TerminalFocused {
                return false;
            }
            if in_text_input && conflicts_with_text_entry(chord) {
                return false;
            }
            true
        }
        FocusContext::WindowChrome { in_text_input } => {
            // Allow Zoom shortcuts in Bookmarks / window views when not typing normal text,
            // otherwise restrict strictly to `GlobalWindow`.
            let allowed_scope = scope == ActionScope::GlobalWindow
                || matches!(
                    id,
                    ShortcutId::ZoomIn | ShortcutId::ZoomOut | ShortcutId::ResetZoom
                );
            if !allowed_scope {
                return false;
            }
            if in_text_input && conflicts_with_text_entry(chord) {
                return false;
            }
            true
        }
    }
}

/// Protects standard GTK text-editing keys (`Ctrl+C/V/X/A/Z/Y`, unmodified keys, `Shift+<char>`,
/// navigation arrows, `Enter`, `Backspace`, `Tab`) when focus is inside a text input widget
/// (`MsgBar` `GtkTextView`, `SearchEntry`, etc.).
fn conflicts_with_text_entry(chord: Chord) -> bool {
    if !chord
        .modifiers
        .intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK)
    {
        // Unmodified or Shift-only key (letters, Enter, Backspace, Delete, Arrows, Home/End):
        // let the text widget handle typing and selection unless it's a bare function key.
        return !chord.is_bare_function_key();
    }

    if chord.modifiers == gdk::ModifierType::CONTROL_MASK {
        return matches!(
            chord.keyval,
            gdk::Key::c
                | gdk::Key::v
                | gdk::Key::x
                | gdk::Key::a
                | gdk::Key::z
                | gdk::Key::y
                | gdk::Key::BackSpace
                | gdk::Key::Delete
                | gdk::Key::Left
                | gdk::Key::Right
                | gdk::Key::Up
                | gdk::Key::Down
                | gdk::Key::Home
                | gdk::Key::End
        );
    }

    if chord.modifiers == (gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK) {
        return matches!(
            chord.keyval,
            gdk::Key::z
                | gdk::Key::Left
                | gdk::Key::Right
                | gdk::Key::Home
                | gdk::Key::End
        );
    }

    false
}

/// Installs the unified `PropagationPhase::Capture` shortcut controller on `window`.
///
/// Uses GTK4's native `gdk::KeyEvent::matches()` (supporting both active Latin layouts like
/// QWERTY/Dvorak/AZERTY via `KeyMatch::Exact` and non-Latin layouts like Greek/Russian via
/// XKB cross-group `KeyMatch::Partial`), reading the lock-free `KEYBINDINGS` `ArcSwap` directly
/// on each key press so custom shortcut edits apply immediately with zero controller rebuilds.
pub fn attach_window_controller(
    window: &adw::ApplicationWindow,
    host: Rc<dyn WindowShortcutHost>,
) {
    let key_ctrl = gtk::EventControllerKey::new();
    key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);

    let win_weak = window.downgrade();
    key_ctrl.connect_key_pressed(move |ctrl, _keyval, _keycode, _state| {
        let Some(win) = win_weak.upgrade() else {
            return glib::Propagation::Proceed;
        };

        // Suspend main-window shortcuts whenever any modal `AdwDialog` (Preferences,
        // ShortcutCaptureDialog, ModelSelection, Close Confirmation, etc.) is open.
        if win.visible_dialog().is_some() {
            return glib::Propagation::Proceed;
        }

        let Some(ev) = ctrl.current_event() else {
            return glib::Propagation::Proceed;
        };
        let Ok(key_ev) = ev.downcast::<gdk::KeyEvent>() else {
            return glib::Propagation::Proceed;
        };
        if key_ev.is_modifier() {
            return glib::Propagation::Proceed;
        }

        let focus = classify_window_focus(&win, host.terminal_input_mode());
        let keymap = crate::keymap();

        let mut partial_candidate: Option<ShortcutId> = None;

        for entry in keymap.dispatch_entries() {
            let spec = spec_for(entry.id);
            if !is_binding_eligible(entry.id, spec.scope, entry.chord, focus) {
                continue;
            }

            match key_ev.matches(entry.chord.keyval, entry.chord.modifiers) {
                gdk::KeyMatch::Exact => {
                    if !entry.chord.performable || host.can_perform(entry.id) {
                        host.activate(&win, entry.id, focus);
                        return glib::Propagation::Stop;
                    }
                }
                gdk::KeyMatch::Partial => {
                    if partial_candidate.is_none()
                        && (!entry.chord.performable || host.can_perform(entry.id))
                    {
                        partial_candidate = Some(entry.id);
                    }
                }
                gdk::KeyMatch::None | _ => {}
            }
        }

        if let Some(id) = partial_candidate {
            host.activate(&win, id, focus);
            return glib::Propagation::Stop;
        }

        glib::Propagation::Proceed
    });

    window.add_controller(key_ctrl);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_focused_shortcuts_eligible_in_terminal_and_blocked_in_overlays() {
        let copy = Chord::parse("Ctrl+Shift+C").unwrap();
        let paste = Chord::parse("Ctrl+Shift+V").unwrap();

        assert!(is_binding_eligible(
            ShortcutId::Copy,
            ActionScope::TerminalFocused,
            copy,
            FocusContext::Terminal(TerminalInputMode::Normal),
        ));
        assert!(is_binding_eligible(
            ShortcutId::Paste,
            ActionScope::TerminalFocused,
            paste,
            FocusContext::Terminal(TerminalInputMode::Normal),
        ));

        assert!(!is_binding_eligible(
            ShortcutId::Copy,
            ActionScope::TerminalFocused,
            copy,
            FocusContext::ClawOverlay {
                in_text_input: false
            },
        ));
        assert!(!is_binding_eligible(
            ShortcutId::Paste,
            ActionScope::TerminalFocused,
            paste,
            FocusContext::TerminalSearchBar,
        ));
    }
}
