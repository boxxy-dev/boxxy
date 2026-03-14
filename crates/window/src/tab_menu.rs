// Context-menu engine for AdwTabView tabs.
//
// AdwTabView shows the context menu as a GtkPopoverMenu parented to the window,
// so action lookup walks window → application — NOT through the tab view.
// The action group must therefore be registered on the window (or another
// ancestor), not on the tab view itself.
//
// Public API:
//   TabContextMenu::new(tab_view, action_host, on_close_page)
//
// `action_host` is typically the ApplicationWindow. The "tab" action group is
// inserted there so the popup can resolve "tab.close-tab".

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gio;
use gtk4::prelude::*;

// ---------------------------------------------------------------------------
// TabContextMenu
// ---------------------------------------------------------------------------

/// Attaches a right-click context menu to an `AdwTabView`.
///
/// `action_host` must be an ancestor widget (usually the `ApplicationWindow`)
/// so that the popup can resolve actions via GTK's widget-tree lookup.
///
/// Current actions:
/// - **Close Tab** — disabled on the last remaining tab; calls `on_close_page`.
///
/// Extend via [`TabContextMenu::menu`] and [`TabContextMenu::action_group`].
pub struct TabContextMenu {
    /// The `gio::Menu` backing the popover — append items here for new actions.
    pub menu: gio::Menu,
    /// The action group registered as `"tab"` on the action host widget.
    pub action_group: gio::SimpleActionGroup,
    current_page: Rc<RefCell<Option<libadwaita::TabPage>>>,
}

impl TabContextMenu {
    pub fn new(
        tab_view: &libadwaita::TabView,
        action_host: &impl IsA<gtk4::Widget>,
        on_close_page: impl Fn(libadwaita::TabPage) + 'static,
        on_move_to_new_window: impl Fn(libadwaita::TabPage) + 'static,
    ) -> Self {
        let current_page: Rc<RefCell<Option<libadwaita::TabPage>>> =
            Rc::new(RefCell::new(None));

        // Build the close action.
        let close_action = gio::SimpleAction::new("close-tab", None);
        close_action.set_enabled(false); // enabled dynamically in setup-menu

        let cp = current_page.clone();
        close_action.connect_activate(move |_, _| {
            if let Some(page) = cp.borrow().clone() {
                on_close_page(page);
            }
        });

        // Build the move to new window action.
        let move_action = gio::SimpleAction::new("move-to-new-window", None);
        move_action.set_enabled(false); // enabled dynamically in setup-menu
        
        let cp2 = current_page.clone();
        move_action.connect_activate(move |_, _| {
            if let Some(page) = cp2.borrow().clone() {
                on_move_to_new_window(page);
            }
        });

        // Track which page was right-clicked and toggle close availability.
        let close_action_ref = close_action.clone();
        let move_action_ref = move_action.clone();
        let cp = current_page.clone();
        tab_view.connect_setup_menu(move |tv, page| {
            *cp.borrow_mut() = page.cloned();
            
            let (can_close, can_move) = match page {
                None => (false, false),
                Some(p) => {
                    let is_pinned = p.is_pinned();
                    let unpinned_count = tv.n_pages() - tv.n_pinned_pages();
                    
                    if is_pinned {
                        // Pinned tabs can be closed, but cannot be moved to a new window
                        (true, false)
                    } else {
                        // Unpinned tabs can be closed and moved if there's at least one other unpinned tab
                        let allowed = unpinned_count > 1;
                        (allowed, allowed)
                    }
                }
            };
            
            close_action_ref.set_enabled(can_close);
            move_action_ref.set_enabled(can_move);
        });

        // Register on the host widget (window) so the popup can find it.
        let action_group = gio::SimpleActionGroup::new();
        action_group.add_action(&close_action);
        action_group.add_action(&move_action);
        action_host.insert_action_group("tab", Some(&action_group));

        // Hand the menu model to AdwTabView — it shows it on secondary click.
        let menu = gio::Menu::new();
        menu.append(Some("Close Tab"), Some("tab.close-tab"));
        menu.append(Some("Move to New Window"), Some("tab.move-to-new-window"));
        tab_view.set_menu_model(Some(&menu));

        Self { menu, action_group, current_page }
    }

    /// The page the context menu is currently (or was last) open for.
    pub fn current_page(&self) -> Option<libadwaita::TabPage> {
        self.current_page.borrow().clone()
    }
}
