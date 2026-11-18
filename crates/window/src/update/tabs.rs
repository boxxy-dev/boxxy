use gtk4::gio;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use boxxy_terminal::{TerminalComponent, TerminalInit};
use boxxy_themes::load_palette;

use super::close_dialog::{CloseDecision, prompt_close_active_work};
use super::tab_bar_sync::{sync_header_title, sync_tab_colors, sync_tab_titles};
use crate::init::{AppInit, ORPHAN_TABS, TabLaunchSpec, TerminalController};
use crate::state::{AppInput, AppWindowInner, TabColor};
use crate::ui::AppWindow;

type SplitSessionEntry = (usize, bool, boxxy_terminal::DetachedSessionInfo);

pub fn new_tab(inner: &mut AppWindowInner) {
    new_tab_with_intent(inner, None);
}

pub fn new_tab_launch(inner: &mut AppWindowInner, spec: TabLaunchSpec) {
    let _ = new_tab_internal(inner, None, spec, None);
}

pub fn new_tab_with_intent(
    inner: &mut AppWindowInner,
    intent: Option<String>,
) -> TerminalComponent {
    new_tab_internal(inner, intent, TabLaunchSpec::default(), None)
}

pub fn new_tab_reattach(
    inner: &mut AppWindowInner,
    pid: u32,
    working_dir: Option<String>,
) -> TerminalComponent {
    new_tab_internal(
        inner,
        None,
        TabLaunchSpec {
            working_dir,
            ..Default::default()
        },
        Some(pid),
    )
}

pub fn new_tab_with_character(
    inner: &mut AppWindowInner,
    character_id: String,
    working_dir: Option<String>,
) -> TerminalComponent {
    let controller = new_tab_internal(
        inner,
        None,
        TabLaunchSpec {
            working_dir,
            ..Default::default()
        },
        None,
    );
    let _ = controller.activate_character(character_id);
    inner.window.present();
    controller
}

/// Restores a batch of detached sessions, grouping sessions that originated from
/// splits within the same tab (`{tab_id}::{idx}::{v|h}`) back into a single tab's
/// split layout while opening unrelated tabs separately.
pub fn reattach_sessions_grouped(
    inner: &mut AppWindowInner,
    sessions: Vec<boxxy_terminal::DetachedSessionInfo>,
) {
    let mut groups: Vec<(String, Vec<SplitSessionEntry>)> = Vec::new();

    for s in sessions {
        let (tab_key, idx, is_vertical) = parse_layout_tag(&s.pane_id);
        if let Some((_, list)) = groups.iter_mut().find(|(k, _)| k == tab_key) {
            list.push((idx, is_vertical, s));
        } else {
            groups.push((tab_key.to_string(), vec![(idx, is_vertical, s)]));
        }
    }

    for (_, mut list) in groups {
        list.sort_by_key(|(idx, _, _)| *idx);
        let mut iter = list.into_iter();
        let Some((_, _, first)) = iter.next() else {
            continue;
        };
        let first_cwd = (!first.cwd.is_empty()).then_some(first.cwd);
        let controller = new_tab_reattach(inner, first.pid, first_cwd);
        for (_, is_vertical, next) in iter {
            let next_cwd = (!next.cwd.is_empty()).then_some(next.cwd);
            controller.split_reattach(is_vertical, next.pid, next_cwd);
        }
    }
}

fn parse_layout_tag(tag: &str) -> (&str, usize, bool) {
    let mut parts = tag.split("::");
    let tab_id = parts.next().unwrap_or(tag);
    let idx = parts
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);
    let is_vertical = parts.next() != Some("h");
    (tab_id, idx, is_vertical)
}

fn new_tab_internal(
    inner: &mut AppWindowInner,
    intent: Option<String>,
    explicit_launch: TabLaunchSpec,
    reattach_pid: Option<u32>,
) -> TerminalComponent {
    let id = uuid::Uuid::new_v4().to_string();
    let initial = inner.initial_launch.take().unwrap_or_default();

    let working_dir = explicit_launch
        .working_dir
        .or(initial.working_dir)
        .or_else(|| {
            inner
                .current_settings
                .preserve_working_dir
                .then(|| inner.active_tab().and_then(|tc| tc.cwd.clone()))
                .flatten()
        });

    let command = explicit_launch.command.or(initial.command);
    let hold = explicit_launch.hold || initial.hold;
    let custom_title = explicit_launch.title.or(initial.title);

    let controller = TerminalComponent::new(TerminalInit {
        id: id.clone(),
        working_dir: working_dir.clone(),
        spawn_intent: intent,
        reattach_pid,
        command,
        hold,
    });

    let parsed = load_palette(inner.current_settings.theme.as_str());
    let is_dark = libadwaita::StyleManager::default().is_dark();
    let palette = parsed
        .as_ref()
        .map(|p| if is_dark { p.dark } else { p.light });
    controller.update_settings(inner.current_settings.clone(), palette);

    let widget = controller.widget();
    let page = inner.tab_view.append(widget);
    page.set_title("Terminal");

    // Auto-coloring
    let mut tab_color = TabColor::Default;
    if inner.current_settings.colored_tabs {
        let colors = TabColor::all();
        let mut usage = HashMap::new();
        for t in &inner.tabs {
            *usage.entry(t.tab_color).or_insert(0usize) += 1;
        }

        let min_usage = colors
            .iter()
            .map(|c| usage.get(c).copied().unwrap_or(0))
            .min()
            .unwrap_or(0);
        let candidates: Vec<_> = colors
            .iter()
            .filter(|c| usage.get(*c).copied().unwrap_or(0) == min_usage)
            .collect();

        use rand::seq::IndexedRandom;
        if let Some(color) = candidates.choose(&mut rand::rng()) {
            tab_color = **color;
        }
    }

    let tc = TerminalController {
        controller: controller.clone(),
        id,
        cwd: working_dir,
        raw_title: None,
        foreground_process: None,
        has_running_app: false,
        unread_bell: false,
        bell_ringing: Rc::new(std::cell::Cell::new(false)),
        bell_svg: Rc::new(RefCell::new(None)),
        bell_timer_source: Rc::new(RefCell::new(None)),
        command_started_at: None,
        command_used_alt_screen: false,
        app_state: crate::application_detector::TerminalAppState::default(),
        category_svg: Rc::new(RefCell::new(None)),
        indicator_svg: Rc::new(RefCell::new(None)),
        working_anim_source: Rc::new(RefCell::new(None)),
        tab_color,
        custom_title,
    };
    inner.tabs.push(tc);
    sync_tab_titles(inner);

    inner.tab_view.set_selected_page(&page);

    controller
}

pub fn set_tab_color(inner: &mut AppWindowInner, key: usize, color: TabColor) {
    if let Some(pos) = inner.tab_pos_by_widget_key(key) {
        inner.tabs[pos].tab_color = color;
    }
    sync_tab_colors(inner);
}

pub fn set_tab_title(inner: &mut AppWindowInner, key: usize, title: Option<String>) {
    if let Some(pos) = inner.tab_pos_by_widget_key(key) {
        inner.tabs[pos].custom_title = title;
        sync_tab_titles(inner);
    }
}

fn remove_tab_by_widget_key(inner: &mut AppWindowInner, key: usize) {
    if let Some(pos) = inner.tab_pos_by_widget_key(key) {
        let removed = inner.tabs.remove(pos);
        removed.stop_working_anim();
    }
    sync_tab_titles(inner);
}

pub fn close_tab_request(
    inner_ref: &Rc<RefCell<AppWindowInner>>,
    inner: &mut AppWindowInner,
    key: usize,
) {
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

    let Some(pos) = inner.tab_pos_by_widget_key(key) else {
        inner.tab_view.close_page_finish(&page, true);
        return;
    };

    let unpinned_count = inner.tab_view.n_pages() - inner.tab_view.n_pinned_pages();
    if unpinned_count <= 1 && !page.is_pinned() {
        inner.tab_view.close_page_finish(&page, false);
        inner.window.close();
        return;
    }

    let pids = inner.tabs[pos].controller.get_pids();
    let pane_ids: HashSet<String> = inner.tabs[pos]
        .controller
        .get_pane_ids()
        .into_iter()
        .collect();
    let active_claw_panes = inner.tabs[pos].controller.get_active_claw_panes();
    let inner_clone = inner_ref.clone();
    gtk4::glib::spawn_future_local(async move {
        let agent = boxxy_terminal::get_agent().await;
        let daemon_alive = agent.is_daemon_alive().await;
        let mut running_apps = Vec::new();
        if daemon_alive {
            for pid in &pids {
                if let Ok(Ok(mut procs)) = tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    agent.get_running_processes(*pid),
                )
                .await
                {
                    running_apps.append(&mut procs);
                }
            }
        }

        let tab_pending_tasks: Vec<_> = if daemon_alive {
            agent
                .get_pending_tasks()
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|t| pane_ids.contains(&t.pane_id))
                .collect()
        } else {
            Vec::new()
        };

        // Idle shell prompt with no active processes, reminders, or AI sessions:
        // terminate cleanly even when `pty_persistence` is on.
        if !daemon_alive
            || (running_apps.is_empty()
                && tab_pending_tasks.is_empty()
                && active_claw_panes.is_empty())
        {
            let controller_opt = inner_clone
                .borrow()
                .tab_by_widget_key(key)
                .map(|c| c.controller.clone());
            if let Some(controller) = controller_opt
                && daemon_alive
            {
                controller.terminate_on_close_sync().await;
            }
            remove_tab_by_widget_key(&mut inner_clone.borrow_mut(), key);
            inner_clone.borrow().tab_view.close_page_finish(&page, true);
            return;
        }

        if boxxy_preferences::Settings::load().pty_persistence {
            let (controller_opt, tx) = {
                let inner = inner_clone.borrow();
                (
                    inner.tab_by_widget_key(key).map(|c| c.controller.clone()),
                    inner.tx.clone(),
                )
            };
            if let Some(controller) = controller_opt {
                controller.detach_on_close_sync().await;
            }
            remove_tab_by_widget_key(&mut inner_clone.borrow_mut(), key);
            inner_clone.borrow().tab_view.close_page_finish(&page, true);
            let _ = tx.send(AppInput::RefreshDetachedSessions).await;
            return;
        }

        let window = inner_clone.borrow().window.clone();
        let decision = prompt_close_active_work(
            &window,
            "Close Tab?",
            "Put this session in the background to continue it later without needing to resume, or terminate it.",
            "Terminate",
            running_apps,
            tab_pending_tasks,
            active_claw_panes,
        )
        .await;

        match decision {
            CloseDecision::RunInBackground => {
                let (controller_opt, tx) = {
                    let inner = inner_clone.borrow();
                    (
                        inner.tab_by_widget_key(key).map(|c| c.controller.clone()),
                        inner.tx.clone(),
                    )
                };
                if let Some(controller) = controller_opt {
                    controller.detach_on_close_sync().await;
                }
                remove_tab_by_widget_key(&mut inner_clone.borrow_mut(), key);
                inner_clone.borrow().tab_view.close_page_finish(&page, true);
                let _ = tx.send(AppInput::RefreshDetachedSessions).await;
            }
            CloseDecision::Terminate => {
                let controller_opt = inner_clone
                    .borrow()
                    .tab_by_widget_key(key)
                    .map(|c| c.controller.clone());
                if let Some(controller) = controller_opt {
                    controller.terminate_on_close_sync().await;
                }
                remove_tab_by_widget_key(&mut inner_clone.borrow_mut(), key);
                inner_clone.borrow().tab_view.close_page_finish(&page, true);
            }
            CloseDecision::Cancel => {
                let tx = {
                    let inner = inner_clone.borrow();
                    inner.tab_view.close_page_finish(&page, false);
                    inner.tx.clone()
                };
                let _ = tx.send_blocking(AppInput::FocusActiveTerminal);
            }
        }
    });
}

pub fn close_tab(inner: &mut AppWindowInner, id: String) {
    let Some(pos) = inner.tabs.iter().position(|c| c.id == id) else {
        return;
    };

    let removed = inner.tabs.remove(pos);
    removed.stop_working_anim();

    // If no terminal tabs remain at all, close the window immediately.
    if inner.tabs.is_empty() {
        inner.force_close.set(true);
        inner.window.close();
        inner.window.destroy();
        return;
    }

    let page = inner.tab_view.page(removed.controller.widget());
    sync_tab_titles(inner);
    inner.tab_view.close_page(&page);
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
                ..AppInit::new()
            },
        );

        // Detach manually before transfer so `ORPHAN_TABS` is populated before
        // the target window processes its `page-attached` signal.
        tab_page_detached(inner, key);

        inner.tab_view.transfer_page(&page, &new_tab_view, 0);
    }
}

fn attach_transferred_controller(inner: &mut AppWindowInner, tc: TerminalController) {
    let parsed = load_palette(inner.current_settings.theme.as_str());
    let is_dark = libadwaita::StyleManager::default().is_dark();
    let palette = parsed
        .as_ref()
        .map(|p| if is_dark { p.dark } else { p.light });
    tc.controller
        .update_settings(inner.current_settings.clone(), palette);

    tc.controller.grab_focus();
    inner.tabs.push(tc);
}

pub fn adopt_orphan_tabs(inner: &mut AppWindowInner) {
    let n = inner.tab_view.n_pages();
    for i in 0..n {
        let page = inner.tab_view.nth_page(i);
        let key = page.child().as_ptr() as usize;
        if let Some(tc) = ORPHAN_TABS.with(|pool| pool.borrow_mut().remove(&key)) {
            attach_transferred_controller(inner, tc);
        }
    }
    sync_tab_titles(inner);
}

pub fn tab_page_detached(inner: &mut AppWindowInner, key: usize) {
    if let Some(pos) = inner.tab_pos_by_widget_key(key) {
        let tc = inner.tabs.remove(pos);
        ORPHAN_TABS.with(|pool| pool.borrow_mut().insert(key, tc));
        sync_tab_titles(inner);
    } else if let Some(bookmarks_page) = &inner.bookmarks_page
        && bookmarks_page.child().as_ptr() as usize == key
    {
        inner.bookmarks_controller = None;
        inner.bookmarks_page = None;
    }
}

pub fn tab_page_attached(inner: &mut AppWindowInner, key: usize) {
    if let Some(tc) = ORPHAN_TABS.with(|pool| pool.borrow_mut().remove(&key)) {
        attach_transferred_controller(inner, tc);
        sync_tab_titles(inner);
    }
}

pub fn focus_active_terminal(inner: &mut AppWindowInner) {
    if inner.tab_view.selected_page().is_some() {
        let active_pos = inner.active_tab_pos();
        if let Some(pos) = active_pos
            && inner.tabs[pos].unread_bell
        {
            inner.tabs[pos].clear_bell();
        }
        let is_terminal = active_pos.is_some();

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
                if let Ok(tv) = toolbar.clone().downcast::<libadwaita::ToolbarView>() {
                    tv.set_top_bar_style(libadwaita::ToolbarStyle::Raised);
                }
            }
        } else {
            inner.content_header.add_css_class("terminal-header");
            inner.tab_view.remove_css_class("non-terminal-tab");
            if let Some(ref toolbar) = toolbar_opt {
                toolbar.remove_css_class("non-terminal-toolbar");
                if let Ok(tv) = toolbar.clone().downcast::<libadwaita::ToolbarView>() {
                    tv.set_top_bar_style(libadwaita::ToolbarStyle::Flat);
                }
            }
        }

        if let Some(pos) = active_pos {
            let modal_open = inner.has_modal_or_popover_open();
            let tc = &inner.tabs[pos];
            if !modal_open {
                tc.controller.grab_focus();
            }

            let tab_is_claw_active = tc.controller.is_claw_active();
            inner.claw_active = tab_is_claw_active;

            inner.claw.set_pane_state(
                &tc.controller.agent_name(),
                tc.controller.is_sleep(),
                tc.controller.is_pinned(),
                tc.controller.is_web_search(),
                tc.controller.is_vibe(),
            );
            inner
                .claw
                .set_session_usage(tc.controller.get_session_usage());
            inner.claw.update_tasks(tc.get_pending_tasks());
        }
    }
    sync_tab_colors(inner);
    sync_header_title(inner);
}

pub fn open_bookmarks(inner: &mut AppWindowInner) {
    if let Some(page) = &inner.bookmarks_page {
        inner.tab_view.set_selected_page(page);
    } else {
        let tx_run = inner.tx.clone();
        let controller =
            boxxy_bookmarks::tab::BookmarksTabComponent::new(move |name, filename, script| {
                let _ = tx_run.send_blocking(AppInput::ExecuteInNewTab(name, filename, script));
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
