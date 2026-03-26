use std::cell::RefCell;
use std::rc::Rc;

use gtk4::gio;
use gtk4::prelude::*;

use crate::state::TabColor;

// ---------------------------------------------------------------------------
// TabContextMenu
// ---------------------------------------------------------------------------

pub struct TabContextMenu {
    _popover: gtk4::Popover,
    current_page: Rc<RefCell<Option<libadwaita::TabPage>>>,
}

fn find_tab_nodes(widget: &gtk4::Widget, tabs: &mut Vec<gtk4::Widget>) {
    if widget.css_name() == "tab" {
        tabs.push(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        find_tab_nodes(&c, tabs);
        child = c.next_sibling();
    }
}

impl TabContextMenu {
    pub fn new(
        tab_bar: &libadwaita::TabBar,
        tab_view: &libadwaita::TabView,
        on_close_page: impl Fn(libadwaita::TabPage) + 'static,
        on_move_to_new_window: impl Fn(libadwaita::TabPage) + 'static,
        on_set_color: impl Fn(libadwaita::TabPage, TabColor) + 'static,
        on_set_title: impl Fn(libadwaita::TabPage, Option<String>) + 'static,
        get_custom_title: impl Fn(&libadwaita::TabPage) -> Option<String> + 'static,
    ) -> Self {
        let current_page: Rc<RefCell<Option<libadwaita::TabPage>>> = Rc::new(RefCell::new(None));

        // Make sure native menu is disabled by setting it to None.
        tab_view.set_menu_model(None::<&gio::MenuModel>);

        let popover = gtk4::Popover::builder()
            .position(gtk4::PositionType::Bottom)
            .has_arrow(false)
            .build();

        popover.add_css_class("menu");
        popover.set_parent(tab_bar);

        let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        vbox.set_margin_top(6);
        vbox.set_margin_bottom(6);
        vbox.set_margin_start(6);
        vbox.set_margin_end(6);

        let close_btn = gtk4::Button::builder()
            .label("Close Tab")
            .css_classes(["flat"])
            .halign(gtk4::Align::Fill)
            .build();
        close_btn
            .child()
            .unwrap()
            .downcast::<gtk4::Label>()
            .unwrap()
            .set_xalign(0.0);

        let move_btn = gtk4::Button::builder()
            .label("Move to New Window")
            .css_classes(["flat"])
            .halign(gtk4::Align::Fill)
            .build();
        move_btn
            .child()
            .unwrap()
            .downcast::<gtk4::Label>()
            .unwrap()
            .set_xalign(0.0);

        let title_entry = gtk4::Entry::builder()
            .placeholder_text("Set Title")
            .secondary_icon_name("boxxy-edit-clear-symbolic")
            .margin_top(4)
            .margin_bottom(4)
            .build();

        let cp_title = current_page.clone();
        let on_set_title = Rc::new(on_set_title);
        let pop_clone_title = popover.clone();

        // Handle enter key to apply title
        let on_set_title_activate = on_set_title.clone();
        title_entry.connect_activate(move |entry| {
            if let Some(page) = cp_title.borrow().clone() {
                let text = entry.text().to_string();
                let title = if text.is_empty() { None } else { Some(text) };
                on_set_title_activate(page, title);
            }
            pop_clone_title.popdown();
        });

        // Handle clear icon click
        let cp_title_clear = current_page.clone();
        let on_set_title_clear = on_set_title.clone();
        title_entry.connect_icon_press(move |entry, pos| {
            if pos == gtk4::EntryIconPosition::Secondary {
                entry.set_text("");
                if let Some(page) = cp_title_clear.borrow().clone() {
                    on_set_title_clear(page, None);
                }
            }
        });

        let color_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        color_box.set_halign(gtk4::Align::Center);
        color_box.set_margin_top(4);
        color_box.set_margin_bottom(4);

        let colors = [
            (TabColor::Default, "rgba(255,255,255,0.1)"),
            (TabColor::Blue, "#3584e4"),
            (TabColor::Teal, "#2190a4"),
            (TabColor::Green, "#3a944a"),
            (TabColor::Yellow, "#c88800"),
            (TabColor::Orange, "#ed5b00"),
            (TabColor::Red, "#e62d42"),
            (TabColor::Pink, "#d56199"),
            (TabColor::Purple, "#9141ac"),
            (TabColor::Slate, "#6f8396"),
        ];

        let cp = current_page.clone();
        let on_set_color = Rc::new(on_set_color);
        let pop_clone_color = popover.clone();

        for (c, hex) in colors {
            let btn = gtk4::Button::builder()
                .width_request(22)
                .height_request(22)
                .css_classes(["flat"])
                .build();

            let css = format!(
                "button {{ background-color: {}; min-width: 22px; min-height: 22px; padding: 0; margin: 2px; border-radius: 6px; border: 2px solid transparent; transition: border-color 0.15s; opacity: 1.0; }} button:hover {{ border-color: rgba(255,255,255,0.5); }}",
                hex
            );
            let provider = gtk4::CssProvider::new();
            provider.load_from_string(&css);

            #[allow(deprecated)]
            btn.style_context()
                .add_provider(&provider, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);

            if c == TabColor::Default {
                btn.set_icon_name("boxxy-edit-clear-symbolic");
            }

            let cp_inner = cp.clone();
            let on_set_color_inner = on_set_color.clone();
            let pop_inner = pop_clone_color.clone();
            let color = c;
            btn.connect_clicked(move |_| {
                if let Some(page) = cp_inner.borrow().clone() {
                    on_set_color_inner(page, color);
                }
                pop_inner.popdown();
            });
            color_box.append(&btn);
        }

        vbox.append(&close_btn);
        vbox.append(&move_btn);
        vbox.append(&title_entry);
        vbox.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        vbox.append(&color_box);

        popover.set_child(Some(&vbox));

        let cp_close = cp.clone();
        let pop_clone = popover.clone();
        close_btn.connect_clicked(move |_| {
            if let Some(page) = cp_close.borrow().clone() {
                on_close_page(page);
            }
            pop_clone.popdown();
        });

        let cp_move = cp.clone();
        let pop_clone2 = popover.clone();
        move_btn.connect_clicked(move |_| {
            if let Some(page) = cp_move.borrow().clone() {
                on_move_to_new_window(page);
            }
            pop_clone2.popdown();
        });

        let gesture = gtk4::GestureClick::new();
        gesture.set_button(gtk4::gdk::BUTTON_SECONDARY);
        // Capture phase to intercept right-clicks before AdwTabBar handles them natively
        gesture.set_propagation_phase(gtk4::PropagationPhase::Capture);

        let pop_clone3 = popover.clone();
        let tv_clone = tab_view.clone();
        let tb_clone = tab_bar.clone();
        let cp_gesture = current_page.clone();

        gesture.connect_pressed(move |gesture, _, x, y| {
            let mut tabs = Vec::new();
            find_tab_nodes(tb_clone.upcast_ref(), &mut tabs);

            let mut clicked_index = None;
            for (i, tab) in tabs.iter().enumerate() {
                #[allow(deprecated)]
                if let Some((tab_x, tab_y)) = tb_clone.translate_coordinates(tab, x, y)
                    && tab_x >= 0.0
                    && tab_x < tab.width() as f64
                    && tab_y >= 0.0
                    && tab_y < tab.height() as f64
                {
                    clicked_index = Some(i);
                    break;
                }
            }

            let idx = if let Some(i) = clicked_index {
                i
            } else {
                let mut sel_idx = 0;
                for i in 0..tv_clone.n_pages() {
                    let p = tv_clone.nth_page(i);
                    if Some(p) == tv_clone.selected_page() {
                        sel_idx = i as usize;
                        break;
                    }
                }
                sel_idx
            };

            if idx < tv_clone.n_pages() as usize {
                let page = tv_clone.nth_page(idx as i32);
                *cp_gesture.borrow_mut() = Some(page.clone());

                let is_pinned = page.is_pinned();
                let _unpinned_count = tv_clone.n_pages() - tv_clone.n_pinned_pages();
                let is_terminal =
                    !page.child().has_css_class("non-terminal-tab") && page.title() != "Bookmarks";

                let (can_close, can_move) = if is_pinned {
                    (true, false)
                } else {
                    (true, tv_clone.n_pages() > 1)
                };

                close_btn.set_sensitive(can_close);
                move_btn.set_sensitive(can_move);
                title_entry.set_sensitive(is_terminal);
                color_box.set_sensitive(is_terminal);

                if let Some(custom) = get_custom_title(&page) {
                    title_entry.set_text(&custom);
                } else {
                    title_entry.set_text("");
                }

                let rect = gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
                pop_clone3.set_pointing_to(Some(&rect));
                pop_clone3.popup();

                // Crucial: Claim the event so AdwTabBar doesn't try to open its native menu
                gesture.set_state(gtk4::EventSequenceState::Claimed);
            }
        });

        tab_bar.add_controller(gesture);

        Self {
            _popover: popover,
            current_page,
        }
    }

    pub fn current_page(&self) -> Option<libadwaita::TabPage> {
        self.current_page.borrow().clone()
    }
}
