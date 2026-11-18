use std::cell::RefCell;
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::OnceLock;

use gtk4::gio;
use gtk4::prelude::*;

use crate::state::TabColor;
use crate::widgets::tab_bar::BoxxyTabBar;

const SWATCH_COLORS: [(TabColor, &str); 10] = [
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

fn ensure_swatch_css_registered() {
    static SWATCH_CSS: OnceLock<()> = OnceLock::new();
    SWATCH_CSS.get_or_init(|| {
        let Some(display) = gtk4::gdk::Display::default() else {
            return;
        };
        let mut css = String::with_capacity(2048);
        for (idx, (_, hex)) in SWATCH_COLORS.iter().enumerate() {
            let _ = writeln!(
                css,
                "button.tab-color-swatch-{idx} {{ background-color: {hex}; min-width: 22px; min-height: 22px; padding: 0; margin: 2px; border-radius: 9999px; border: 2px solid transparent; transition: border-color 0.15s; opacity: 1.0; }} \
                 button.tab-color-swatch-{idx}:hover {{ border-color: rgba(255,255,255,0.5); }}"
            );
        }
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(&css);
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_USER + 50,
        );
    });
}

pub type TabPageLookup = Box<dyn Fn(&libadwaita::TabPage) -> Option<String>>;

pub struct TabContextCallbacks {
    pub on_close_page: Box<dyn Fn(libadwaita::TabPage)>,
    pub on_move_to_new_window: Box<dyn Fn(libadwaita::TabPage)>,
    pub on_set_color: Box<dyn Fn(libadwaita::TabPage, TabColor)>,
    pub on_set_title: Box<dyn Fn(libadwaita::TabPage, Option<String>)>,
    pub get_custom_title: TabPageLookup,
    pub get_cwd: TabPageLookup,
    pub on_menu_closed: Box<dyn Fn()>,
}

pub struct TabContextMenu {
    popover: gtk4::Popover,
}

impl TabContextMenu {
    pub fn new(
        tab_bar: &BoxxyTabBar,
        tab_view: &libadwaita::TabView,
        callbacks: TabContextCallbacks,
    ) -> Self {
        ensure_swatch_css_registered();

        let TabContextCallbacks {
            on_close_page,
            on_move_to_new_window,
            on_set_color,
            on_set_title,
            get_custom_title,
            get_cwd,
            on_menu_closed,
        } = callbacks;

        let current_page: Rc<RefCell<Option<libadwaita::TabPage>>> = Rc::new(RefCell::new(None));

        tab_view.set_menu_model(None::<&gio::MenuModel>);

        let popover = gtk4::Popover::builder()
            .position(gtk4::PositionType::Bottom)
            .has_arrow(false)
            .build();

        popover.add_css_class("menu");
        popover.set_parent(tab_bar);

        popover.connect_closed(move |_| {
            on_menu_closed();
        });

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
        if let Some(lbl) = close_btn.child().and_downcast::<gtk4::Label>() {
            lbl.set_xalign(0.0);
        }

        let move_btn = gtk4::Button::builder()
            .label("Move to New Window")
            .css_classes(["flat"])
            .halign(gtk4::Align::Fill)
            .build();
        if let Some(lbl) = move_btn.child().and_downcast::<gtk4::Label>() {
            lbl.set_xalign(0.0);
        }

        let title_entry = gtk4::Entry::builder()
            .placeholder_text("Set Title")
            .secondary_icon_name("boxxy-edit-clear-symbolic")
            .margin_top(4)
            .margin_bottom(4)
            .build();

        let cp_title = current_page.clone();
        let on_set_title = Rc::new(on_set_title);
        let pop_clone_title = popover.clone();

        let on_set_title_activate = on_set_title.clone();
        title_entry.connect_activate(move |entry| {
            if let Some(page) = cp_title.borrow().clone() {
                let text = entry.text().to_string();
                let title = (!text.is_empty()).then_some(text);
                on_set_title_activate(page, title);
            }
            pop_clone_title.popdown();
        });

        let cp_title_clear = current_page.clone();
        let on_set_title_clear = on_set_title;
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

        let cp = current_page.clone();
        let on_set_color = Rc::new(on_set_color);
        let pop_clone_color = popover.clone();

        for (idx, (color, _)) in SWATCH_COLORS.into_iter().enumerate() {
            let class_name = format!("tab-color-swatch-{idx}");
            let btn = gtk4::Button::builder()
                .width_request(22)
                .height_request(22)
                .css_classes(["flat", class_name.as_str()])
                .build();

            if color == TabColor::Default {
                btn.set_icon_name("boxxy-edit-clear-symbolic");
            }

            let cp_inner = cp.clone();
            let on_set_color_inner = on_set_color.clone();
            let pop_inner = pop_clone_color.clone();
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

        let cp_move = cp;
        let pop_clone2 = popover.clone();
        move_btn.connect_clicked(move |_| {
            if let Some(page) = cp_move.borrow().clone() {
                on_move_to_new_window(page);
            }
            pop_clone2.popdown();
        });

        let pop_for_ctx = popover.clone();
        let tb_weak = tab_bar.downgrade();
        let tv_weak = tab_view.downgrade();
        tab_bar.set_on_context_menu(move |tab_widget, page| {
            let (Some(tb), Some(tv)) = (tb_weak.upgrade(), tv_weak.upgrade()) else {
                return;
            };
            *current_page.borrow_mut() = Some(page.clone());

            let is_pinned = page.is_pinned();
            let is_terminal =
                !page.child().has_css_class("non-terminal-tab") && page.title() != "Bookmarks";
            let can_move = !is_pinned && (tv.n_pages() - tv.n_pinned_pages()) > 1;

            close_btn.set_sensitive(true);
            move_btn.set_sensitive(can_move);
            title_entry.set_sensitive(is_terminal);
            color_box.set_sensitive(is_terminal);

            if let Some(custom) = get_custom_title(page) {
                title_entry.set_text(&custom);
            } else {
                title_entry.set_text("");
            }
            let cwd = get_cwd(page);
            title_entry.set_tooltip_text(cwd.as_deref());

            let rect = if let Some(bounds) = tab_widget.compute_bounds(&tb) {
                gtk4::gdk::Rectangle::new(
                    (bounds.x() + bounds.width() * 0.5) as i32,
                    (bounds.y() + bounds.height()) as i32,
                    1,
                    1,
                )
            } else {
                gtk4::gdk::Rectangle::new(0, tb.height(), 1, 1)
            };
            pop_for_ctx.set_pointing_to(Some(&rect));
            pop_for_ctx.popup();
        });

        Self { popover }
    }

    pub fn unparent(&self) {
        if self.popover.parent().is_some() {
            self.popover.unparent();
        }
    }
}
