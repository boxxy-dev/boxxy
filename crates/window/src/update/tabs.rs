use gtk4::gio;
use libadwaita::prelude::*;

use boxxy_terminal::{TerminalComponent, TerminalInit};
use boxxy_themes::load_palette;

use crate::init::AppInit;
use crate::init::ORPHAN_TABS;
use crate::state::AppWindowInner;
use crate::ui::AppWindow;

pub fn sync_header_title(inner: &AppWindowInner) {
    let unpinned_pages = inner.tab_view.n_pages() - inner.tab_view.n_pinned_pages();
    if unpinned_pages <= 1 {
        inner.tab_bar.set_expand_tabs(false);
    } else {
        inner
            .tab_bar
            .set_expand_tabs(!inner.current_settings.fixed_width_tabs);
    }
}

pub fn find_tab_nodes(widget: &gtk4::Widget, tabs: &mut Vec<gtk4::Widget>) {
    // Only collect widgets with CSS name "tab" that are actually visible and managed by the tab bar
    if widget.css_name() == "tab" && widget.is_visible() {
        tabs.push(widget.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        find_tab_nodes(&c, tabs);
        child = c.next_sibling();
    }
}

pub fn sync_tab_colors(inner: &mut AppWindowInner) {
    let mut tab_nodes = Vec::new();
    find_tab_nodes(inner.tab_bar.upcast_ref(), &mut tab_nodes);

    let n_pages = inner.tab_view.n_pages();

    for i in 0..n_pages {
        let page = inner.tab_view.nth_page(i);
        // We look for the corresponding tab widget.
        // AdwTabBar might have internal layout widgets, but find_tab_nodes filters for "tab".
        if let Some(tab_widget) = tab_nodes.get(i as usize) {
            let mut color_to_apply = crate::state::TabColor::Default;
            if let Some(tc) = inner
                .tabs
                .iter()
                .find(|t| t.controller.widget() == &page.child())
            {
                color_to_apply = tc.tab_color;
            }

            for c in crate::state::TabColor::all() {
                if let Some(cls) = c.as_css_class() {
                    tab_widget.remove_css_class(cls);
                }
            }
            if let Some(cls) = color_to_apply.as_css_class() {
                tab_widget.add_css_class(cls);
            }
        }
    }
}

pub fn new_tab(inner: &mut AppWindowInner) {
    new_tab_with_intent(inner, None);
}

pub fn new_tab_with_intent(inner: &mut AppWindowInner, intent: Option<String>) {
    let id = uuid::Uuid::new_v4().to_string();

    let working_dir = if inner.current_settings.preserve_working_dir {
        inner
            .tab_view
            .selected_page()
            .and_then(|page| {
                let child = page.child();
                inner
                    .tabs
                    .iter()
                    .find(|c| c.controller.widget() == &child)
                    .and_then(|tc| tc.cwd.clone())
            })
            .or(inner.initial_working_dir.take())
    } else {
        None
    };

    let controller = TerminalComponent::new(TerminalInit {
        id: id.clone(),
        working_dir,
        spawn_intent: intent,
    });

    let parsed = load_palette(inner.current_settings.theme.as_str());
    let is_dark = libadwaita::StyleManager::default().is_dark();
    let palette = parsed
        .as_ref()
        .map(|p| if is_dark { p.dark } else { p.light });

    controller.update_settings(inner.current_settings.clone(), palette);
    controller.set_claw_active(inner.current_settings.claw_on_by_default);

    let mode = if inner.current_settings.claw_auto_diagnosis_mode
        == boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
    {
        boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
    } else {
        boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
    };
    controller.update_diagnosis_mode(&mode);

    let widget = controller.widget().clone();

    let page = inner.tab_view.append(&widget);
    page.set_title("Terminal");

    // Auto-coloring
    let mut tab_color = crate::state::TabColor::Default;
    if inner.current_settings.colored_tabs {
        let colors = crate::state::TabColor::all();
        let mut usage = std::collections::HashMap::new();

        for t in &inner.tabs {
            *usage.entry(t.tab_color).or_insert(0) += 1;
        }

        let min_usage = colors
            .iter()
            .map(|c| usage.get(c).cloned().unwrap_or(0))
            .min()
            .unwrap_or(0);
        let candidates: Vec<_> = colors
            .iter()
            .filter(|c| usage.get(*c).cloned().unwrap_or(0) == min_usage)
            .collect();

        use rand::seq::IndexedRandom;
        if let Some(color) = candidates.choose(&mut rand::rng()) {
            tab_color = **color;
        }
    }

    let tc = crate::init::TerminalController {
        controller,
        id,
        cwd: None,
        tab_color,
        custom_title: None,
    };
    inner.tabs.push(tc);

    inner.tab_view.set_selected_page(&page);

    let tx = inner.tx.clone();
    gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
        let _ = tx.send_blocking(crate::state::AppInput::SyncTabColors);
    });
}

pub fn set_tab_color(inner: &mut AppWindowInner, key: usize, color: crate::state::TabColor) {
    if let Some(pos) = inner
        .tabs
        .iter()
        .position(|t| t.controller.widget().as_ptr() as usize == key)
    {
        inner.tabs[pos].tab_color = color;
    }
    sync_tab_colors(inner);
}

pub fn set_tab_title(inner: &mut AppWindowInner, key: usize, title: Option<String>) {
    if let Some(pos) = inner
        .tabs
        .iter()
        .position(|t| t.controller.widget().as_ptr() as usize == key)
    {
        inner.tabs[pos].custom_title = title.clone();

        let page = (0..inner.tab_view.n_pages())
            .map(|i| inner.tab_view.nth_page(i))
            .find(|p| p.child().as_ptr() as usize == key);

        if let Some(page) = page {
            if let Some(t) = title {
                page.set_title(&t);
            } else {
                let default_title = inner.tabs[pos].cwd.as_deref().unwrap_or("Terminal");
                page.set_title(default_title);
            }
            sync_header_title(inner);
        }
    }
}

pub fn close_tab_request(inner: &mut AppWindowInner, key: usize) {
    let page = (0..inner.tab_view.n_pages())
        .map(|i| inner.tab_view.nth_page(i))
        .find(|p| p.child().as_ptr() as usize == key);
    let Some(page) = page else {
        return;
    };

    if let Some(bookmarks_page) = &inner.bookmarks_page
        && *bookmarks_page == page
    {
        inner.bookmarks_controller = None;
        inner.bookmarks_page = None;
        inner.tab_view.close_page_finish(&page, true);
        return;
    }

    let unpinned_count = inner.tab_view.n_pages() - inner.tab_view.n_pinned_pages();
    if unpinned_count <= 1 && !page.is_pinned() {
        inner.window.close();
        return;
    }

    if let Some(pos) = inner
        .tabs
        .iter()
        .position(|c| c.controller.widget().as_ptr() as usize == key)
    {
        inner.tabs.remove(pos);
    }
    inner.tab_view.close_page_finish(&page, true);
}

pub fn close_tab(inner: &mut AppWindowInner, id: String) {
    let unpinned_count = inner.tab_view.n_pages() - inner.tab_view.n_pinned_pages();
    if unpinned_count <= 1 {
        inner.window.close();
        return;
    }
    if let Some(pos) = inner.tabs.iter().position(|c| c.id == id) {
        let controller = &inner.tabs[pos];
        let widget = controller.controller.widget();
        let page = inner.tab_view.page(widget);
        inner.tab_view.close_page(&page);
    }
}

pub fn move_tab_to_new_window_request(inner: &mut AppWindowInner, key: usize) {
    let page = (0..inner.tab_view.n_pages())
        .map(|i| inner.tab_view.nth_page(i))
        .find(|p| p.child().as_ptr() as usize == key);
    let Some(page) = page else {
        return;
    };

    if let Some(app) =
        gio::Application::default().and_then(|a| a.downcast::<libadwaita::Application>().ok())
    {
        let new_tab_view = libadwaita::TabView::new();
        AppWindow::new(
            &app,
            AppInit {
                incoming_tab_view: Some(new_tab_view.clone()),
                working_dir: None,
            },
        );

        // Detach manually before the transfer to guarantee it's in ORPHAN_TABS
        // before the new window receives its async TabPageAttached event.
        tab_page_detached(inner, key);

        inner.tab_view.transfer_page(&page, &new_tab_view, 0);
    }
}

pub fn adopt_orphan_tabs(inner: &mut AppWindowInner) {
    let n = inner.tab_view.n_pages();
    for i in 0..n {
        let page = inner.tab_view.nth_page(i);
        let key = page.child().as_ptr() as usize;
        if let Some(tc) = ORPHAN_TABS.with(|pool| pool.borrow_mut().remove(&key.to_string())) {
            let parsed = load_palette(inner.current_settings.theme.as_str());
            let is_dark = libadwaita::StyleManager::default().is_dark();
            let palette = parsed
                .as_ref()
                .map(|p| if is_dark { p.dark } else { p.light });
            tc.controller
                .update_settings(inner.current_settings.clone(), palette);

            if tc.controller.is_claw_active() {
                page.set_icon(Some(&gio::ThemedIcon::new("boxxyclaw")));
            } else {
                page.set_icon(None::<&gio::Icon>);
            }

            tc.controller.grab_focus();
            inner.tabs.push(tc);
        }
    }
    let tx = inner.tx.clone();
    gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
        let _ = tx.send_blocking(crate::state::AppInput::SyncTabColors);
    });
}

pub fn tab_page_detached(inner: &mut AppWindowInner, key: usize) {
    if let Some(pos) = inner
        .tabs
        .iter()
        .position(|c| c.controller.widget().as_ptr() as usize == key)
    {
        let tc = inner.tabs.remove(pos);
        ORPHAN_TABS.with(|pool| pool.borrow_mut().insert(key.to_string(), tc));
    } else if let Some(bookmarks_page) = &inner.bookmarks_page
        && bookmarks_page.child().as_ptr() as usize == key
    {
        inner.bookmarks_controller = None;
        inner.bookmarks_page = None;
    }
}

pub fn tab_page_attached(inner: &mut AppWindowInner, key: usize) {
    if let Some(tc) = ORPHAN_TABS.with(|pool| pool.borrow_mut().remove(&key.to_string())) {
        let parsed = load_palette(inner.current_settings.theme.as_str());
        let is_dark = libadwaita::StyleManager::default().is_dark();
        let palette = parsed
            .as_ref()
            .map(|p| if is_dark { p.dark } else { p.light });
        tc.controller
            .update_settings(inner.current_settings.clone(), palette);

        let widget = tc.controller.widget();
        let page = inner.tab_view.page(widget);
        if tc.controller.is_claw_active() {
            page.set_icon(Some(&gio::ThemedIcon::new("boxxyclaw")));
        } else {
            page.set_icon(None::<&gio::Icon>);
        }

        tc.controller.grab_focus();
        inner.tabs.push(tc);
    }
    let tx = inner.tx.clone();
    gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
        let _ = tx.send_blocking(crate::state::AppInput::SyncTabColors);
    });
}

pub fn focus_active_terminal(inner: &mut AppWindowInner) {
    inner.bell_indicator.set_visible(false);
    sync_tab_colors(inner);
    if let Some(page) = inner.tab_view.selected_page() {
        page.set_indicator_icon(None::<&gio::Icon>);
        page.set_indicator_activatable(false);
        let child = page.child();
        let is_terminal = inner.tabs.iter().any(|c| c.controller.widget() == &child);

        // Reach the AdwToolbarView that wraps the header + tab-view.
        // Adding "non-terminal-toolbar" gives it a 2-class selector that wins
        // over the single-class `.terminal-toolbar { background: transparent }`
        // rule, restoring an opaque background for non-terminal pages.
        let toolbar_opt = inner
            .content_header
            .ancestor(libadwaita::ToolbarView::static_type());

        if !is_terminal {
            inner.content_header.remove_css_class("terminal-header");
            inner.tab_view.add_css_class("non-terminal-tab");
            if let Some(ref toolbar) = toolbar_opt {
                toolbar.add_css_class("non-terminal-toolbar");
            }
        } else {
            inner.content_header.add_css_class("terminal-header");
            inner.tab_view.remove_css_class("non-terminal-tab");
            if let Some(ref toolbar) = toolbar_opt {
                toolbar.remove_css_class("non-terminal-toolbar");
            }
        }

        if let Some(pos) = inner
            .tabs
            .iter()
            .position(|c| c.controller.widget() == &child)
        {
            let tc = &inner.tabs[pos];
            tc.controller.grab_focus();

            // Sync Claw UI state
            let tab_is_claw_active = tc.controller.is_claw_active();
            inner.claw_active = tab_is_claw_active;
            if tab_is_claw_active {
                page.set_icon(Some(&gio::ThemedIcon::new("boxxyclaw")));
            } else {
                page.set_icon(None::<&gio::Icon>);
            }

            let tab_is_proactive = tc.controller.is_proactive();
            inner.claw_proactive = tab_is_proactive;

            inner
                .claw
                .update_ui(inner.claw_active, inner.claw_proactive);

            inner
                .claw
                .set_history_widget(&tc.controller.claw_history_widget());
            inner.claw.set_token_usage(tc.controller.get_total_tokens());
        }
    }
    sync_header_title(inner);
}

pub fn open_bookmarks(inner: &mut AppWindowInner) {
    if let Some(page) = &inner.bookmarks_page {
        inner.tab_view.set_selected_page(page);
    } else {
        let tx_run = inner.tx.clone();
        let controller =
            boxxy_bookmarks::tab::BookmarksTabComponent::new(move |name, filename, script| {
                let _ = tx_run.send_blocking(crate::state::AppInput::ExecuteInNewTab(
                    name, filename, script,
                ));
            });

        let widget = controller.widget().clone();

        let page = inner.tab_view.insert(&widget, 0);
        page.set_title("Bookmarks");
        inner.tab_view.set_page_pinned(&page, true);
        page.set_icon(Some(&gio::ThemedIcon::new("boxxy-user-bookmarks-symbolic")));

        inner.bookmarks_controller = Some(controller);
        inner.bookmarks_page = Some(page.clone());
        inner.tab_view.set_selected_page(&page);
    }
    sync_header_title(inner);
}
