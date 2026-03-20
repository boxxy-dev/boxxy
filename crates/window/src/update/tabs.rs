use gtk4::gio;
use libadwaita::prelude::*;

use boxxy_terminal::{TerminalComponent, TerminalInit};
use boxxy_themes::load_palette;

use crate::init::AppInit;
use crate::init::ORPHAN_TABS;
use crate::state::AppWindowInner;
use crate::ui::AppWindow;

pub fn sync_header_title(inner: &AppWindowInner) {
    if inner.tab_view.n_pages() <= 1 && !inner.current_settings.always_show_tabs {
        if let Some(page) = inner.tab_view.selected_page() {
            inner.single_tab_title.set_title(&page.title());
        } else {
            inner.single_tab_title.set_title("Terminal");
        }
        inner.header_title_stack.set_visible_child_name("title");
    } else {
        inner.header_title_stack.set_visible_child_name("tabs");
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
    controller.set_claw_active(inner.claw.is_active());

    let mode = if inner.claw_proactive {
        boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
    } else {
        boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
    };
    controller.update_diagnosis_mode(&mode);
    controller.update_terminal_suggestions(inner.claw_terminal_suggestions);

    let widget = controller.widget().clone();

    let page = inner.tab_view.append(&widget);
    page.set_title("Terminal");

    let tc = crate::init::TerminalController {
        controller,
        id,
        cwd: None,
    };
    inner.tabs.push(tc);

    inner.tab_view.set_selected_page(&page);
}

pub fn close_tab_request(inner: &mut AppWindowInner, key: usize) {
    let page = (0..inner.tab_view.n_pages())
        .map(|i| inner.tab_view.nth_page(i))
        .find(|p| p.child().as_ptr() as usize == key);
    let Some(page) = page else {
        return;
    };

    if let Some(boxxy_apps_page) = &inner.boxxy_apps_page
        && *boxxy_apps_page == page
    {
        inner.boxxy_apps_controller = None;
        inner.boxxy_apps_page = None;
        inner.tab_view.close_page_finish(&page, true);
        return;
    }

    if let Some(bookmarks_page) = &inner.bookmarks_page
        && *bookmarks_page == page
    {
        inner.bookmarks_controller = None;
        inner.bookmarks_page = None;
        inner.tab_view.close_page_finish(&page, true);
        return;
    }

    if inner.tab_view.n_pages() <= 1 {
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
    if inner.tab_view.n_pages() <= 1 {
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

            if tc.controller.is_claw_active() && !inner.claw_active {
                let _ = inner
                    .tx
                    .send_blocking(crate::state::AppInput::SetClawActive(true));
            } else {
                tc.controller.set_claw_active(inner.claw_active);
            }

            let mode = if inner.claw_proactive {
                boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
            } else {
                boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
            };
            tc.controller.update_diagnosis_mode(&mode);
            tc.controller
                .update_terminal_suggestions(inner.claw_terminal_suggestions);

            tc.controller.grab_focus();
            inner.tabs.push(tc);
        }
    }
}

pub fn tab_page_detached(inner: &mut AppWindowInner, key: usize) {
    if let Some(pos) = inner
        .tabs
        .iter()
        .position(|c| c.controller.widget().as_ptr() as usize == key)
    {
        let tc = inner.tabs.remove(pos);
        ORPHAN_TABS.with(|pool| pool.borrow_mut().insert(key.to_string(), tc));
    } else if let Some(boxxy_apps_page) = &inner.boxxy_apps_page
        && boxxy_apps_page.child().as_ptr() as usize == key
    {
        inner.boxxy_apps_controller = None;
        inner.boxxy_apps_page = None;
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

        if tc.controller.is_claw_active() && !inner.claw_active {
            let _ = inner
                .tx
                .send_blocking(crate::state::AppInput::SetClawActive(true));
        } else {
            tc.controller.set_claw_active(inner.claw_active);
        }

        let mode = if inner.claw_proactive {
            boxxy_preferences::config::ClawAutoDiagnosisMode::Proactive
        } else {
            boxxy_preferences::config::ClawAutoDiagnosisMode::Lazy
        };
        tc.controller.update_diagnosis_mode(&mode);
        tc.controller
            .update_terminal_suggestions(inner.claw_terminal_suggestions);

        tc.controller.grab_focus();
        inner.tabs.push(tc);
    }
}

pub fn focus_active_terminal(inner: &mut AppWindowInner) {
    inner.bell_indicator.set_visible(false);
    if let Some(page) = inner.tab_view.selected_page() {
        page.set_indicator_icon(None::<&gio::Icon>);
        page.set_indicator_activatable(false);
        let child = page.child();
        let is_boxxy_apps = inner.boxxy_apps_page.as_ref() == Some(&page);
        let is_bookmarks = inner.bookmarks_page.as_ref() == Some(&page);

        if is_boxxy_apps || is_bookmarks {
            inner.content_header.remove_css_class("terminal-header");
        } else {
            inner.content_header.add_css_class("terminal-header");
        }

        if let Some(pos) = inner
            .tabs
            .iter()
            .position(|c| c.controller.widget() == &child)
        {
            let tc = &inner.tabs[pos];
            tc.controller.grab_focus();
            inner
                .claw
                .set_history_widget(&tc.controller.claw_history_widget());
        }
    }
    sync_header_title(inner);
}

pub fn open_boxxy_apps(inner: &mut AppWindowInner) {
    if let Some(page) = &inner.boxxy_apps_page {
        inner.tab_view.set_selected_page(page);
    } else {
        let controller = boxxy_apps::BoxxyAppsComponent::new();

        let widget = controller.widget().clone();

        let page = inner.tab_view.append(&widget);
        page.set_title("Boxxy Apps");
        page.set_icon(Some(&gio::ThemedIcon::new(
            "application-x-sharedlib-symbolic",
        )));

        inner.boxxy_apps_controller = Some(controller);
        inner.boxxy_apps_page = Some(page.clone());
        inner.tab_view.set_selected_page(&page);
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
        page.set_icon(Some(&gio::ThemedIcon::new("user-bookmarks-symbolic")));

        inner.bookmarks_controller = Some(controller);
        inner.bookmarks_page = Some(page.clone());
        inner.tab_view.set_selected_page(&page);
    }
    sync_header_title(inner);
}
