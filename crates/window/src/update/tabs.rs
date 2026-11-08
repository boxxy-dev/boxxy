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

pub fn sync_tab_titles(inner: &mut AppWindowInner) {
    let inputs: Vec<crate::tab_title::TabTitleInput> = inner
        .tabs
        .iter()
        .map(|tc| crate::tab_title::TabTitleInput {
            custom_title: tc.custom_title.clone(),
            cwd: tc.cwd.clone().or_else(|| tc.controller.working_dir()),
            raw_title: tc.raw_title.clone().or_else(|| tc.controller.last_title()),
            foreground_process: tc
                .foreground_process
                .clone()
                .or_else(|| tc.controller.foreground_process()),
            alternate_screen_active: tc.controller.is_alt_screen(),
        })
        .collect();

    let resolved = crate::tab_title::resolve_tab_titles_for_host(
        &inputs,
        inner.current_settings.smart_tab_naming,
    );

    for (tc, res) in inner.tabs.iter_mut().zip(resolved) {
        tc.has_running_app = res.has_running_app;
        tc.app_state = res.app_state;
        let widget = tc.controller.widget();
        let page = inner.tab_view.page(widget);
        if page.title() != res.title {
            page.set_title(&res.title);
        }
        let escaped_tooltip = gtk4::glib::markup_escape_text(&res.tooltip);
        if page.tooltip().as_deref() != Some(escaped_tooltip.as_str()) {
            page.set_tooltip(&escaped_tooltip);
        }
    }

    sync_tab_running_icons(inner);
    sync_header_title(inner);
}

fn find_tab_icon_image(widget: &gtk4::Widget) -> Option<gtk4::Image> {
    if let Some(img) = widget.downcast_ref::<gtk4::Image>() {
        return Some(img.clone());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        // Skip indicator and close buttons, which have their own GtkImage children
        if c.downcast_ref::<gtk4::Button>().is_none()
            && let Some(found) = find_tab_icon_image(&c)
        {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

fn find_tab_indicator_image(widget: &gtk4::Widget) -> Option<gtk4::Image> {
    if widget.has_css_class("tab-indicator") {
        let mut child = widget.first_child();
        while let Some(c) = child {
            if let Some(img) = c.downcast_ref::<gtk4::Image>() {
                return Some(img.clone());
            }
            child = c.next_sibling();
        }
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(found) = find_tab_indicator_image(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

fn sync_tab_title_centering(tab_widget: &gtk4::Widget, icon_visible: bool) {
    let mut child = tab_widget.first_child();
    while let Some(c) = child {
        if c.has_css_class("tab-title") {
            let target_margin_end = if icon_visible { 26 } else { 4 };
            if c.margin_end() != target_margin_end {
                c.set_margin_end(target_margin_end);
            }
            break;
        }
        child = c.next_sibling();
    }
}

/// Finds the index of the `"empty"` or `"hidden"` state in a GPA `gtk4::Svg`
/// (supports 2-state `outline empty` and 3-state `outline filled empty` icons).
fn empty_state_index(svg: &gtk4::Svg) -> u32 {
    let (names, n_states) = svg.state_names();
    if let Some(pos) = names
        .iter()
        .position(|n| n.as_str() == "empty" || n.as_str() == "hidden")
    {
        pos as u32
    } else {
        n_states.saturating_sub(1)
    }
}

pub fn sync_tab_running_icons(inner: &AppWindowInner) {
    use crate::application_detector::{ApplicationCategory, TabIndicatorKind};

    for tc in &inner.tabs {
        let page = inner.tab_view.page(tc.controller.widget());
        let category = tc.app_state.application_category;

        if let Some(gpa_path) = category.gpa_resource_path() {
            let needs_load = tc
                .category_svg
                .borrow()
                .as_ref()
                .is_none_or(|(active_cat, _)| *active_cat != category);
            if needs_load {
                if let Some(source) = tc.working_anim_source.borrow_mut().take() {
                    source.remove();
                }
                if let Some((_, old_svg)) = tc.category_svg.borrow_mut().take() {
                    old_svg.pause();
                }
                let svg = gtk4::Svg::from_resource(gpa_path);
                if let Some(clock) = inner.tab_bar.frame_clock() {
                    svg.set_frame_clock(&clock);
                }
                svg.play();
                *tc.category_svg.borrow_mut() = Some((category, svg));
            }
            if page.icon().is_none() {
                page.set_icon(Some(&gio::ThemedIcon::new("utilities-terminal-symbolic")));
            }
        } else {
            if let Some(source) = tc.working_anim_source.borrow_mut().take() {
                source.remove();
            }
        let existing = tc.category_svg.borrow().clone();
            if let Some((_, svg)) = existing {
                let empty_idx = empty_state_index(&svg);
                *tc.category_svg.borrow_mut() = Some((ApplicationCategory::Idle, svg.clone()));
                if svg.state() != empty_idx {
                    // Play the un-draw / disappear animation (`active` -> `empty/hidden`) before hiding the icon slot
                    svg.play();
                    svg.set_state(empty_idx);
                    let category_svg_rc = tc.category_svg.clone();
                    let svg_fade = svg.clone();
                    let page_weak = page.downgrade();
                    gtk4::glib::timeout_add_local_once(
                        std::time::Duration::from_millis(520),
                        move || {
                            let should_clear = category_svg_rc.borrow().as_ref().is_some_and(
                                |(cur_cat, current_svg)| {
                                    *cur_cat == ApplicationCategory::Idle
                                        && current_svg == &svg_fade
                                        && svg_fade.state() == empty_idx
                                },
                            );
                            if should_clear {
                                svg_fade.pause();
                                *category_svg_rc.borrow_mut() = None;
                                if let Some(p) = page_weak.upgrade() {
                                    p.set_icon(None::<&gio::Icon>);
                                }
                            }
                        },
                    );
                }
            } else if page.icon().is_some() {
                page.set_icon(None::<&gio::Icon>);
            }
        }

        // Sync tab indicator GPA (`boxxy-clock.gpa` / `boxxy-boxxyclaw.gpa` / `boxxy-bell.gpa`)
        let ind_kind = if tc.has_pending_tasks() {
            TabIndicatorKind::Timer
        } else if tc.controller.is_claw_active() {
            TabIndicatorKind::Claw
        } else if tc.unread_bell {
            TabIndicatorKind::Bell
        } else {
            TabIndicatorKind::None
        };

        if let Some(ind_gpa_path) = ind_kind.gpa_resource_path() {
            let needs_load = tc
                .indicator_svg
                .borrow()
                .as_ref()
                .is_none_or(|(active_kind, _)| *active_kind != ind_kind);
            if needs_load {
                if let Some((_, old_svg)) = tc.indicator_svg.borrow_mut().take() {
                    old_svg.pause();
                }
                let svg = gtk4::Svg::from_resource(ind_gpa_path);
                if let Some(clock) = inner.tab_bar.frame_clock() {
                    svg.set_frame_clock(&clock);
                }
                svg.play();
                *tc.indicator_svg.borrow_mut() = Some((ind_kind, svg));
            }
            if page.indicator_icon().is_none() {
                page.set_indicator_icon(Some(&gio::ThemedIcon::new(ind_kind.fallback_icon_name())));
                page.set_indicator_activatable(false);
            }
        } else {
            let existing = tc.indicator_svg.borrow().clone();
            if let Some((_, svg)) = existing {
                let empty_idx = empty_state_index(&svg);
                *tc.indicator_svg.borrow_mut() = Some((TabIndicatorKind::None, svg.clone()));
                if svg.state() != empty_idx {
                    svg.play();
                    svg.set_state(empty_idx);
                    let indicator_svg_rc = tc.indicator_svg.clone();
                    let svg_fade = svg.clone();
                    let page_weak = page.downgrade();
                    gtk4::glib::timeout_add_local_once(
                        std::time::Duration::from_millis(540),
                        move || {
                            let should_clear = indicator_svg_rc.borrow().as_ref().is_some_and(
                                |(cur_kind, current_svg)| {
                                    *cur_kind == TabIndicatorKind::None
                                        && current_svg == &svg_fade
                                        && svg_fade.state() == empty_idx
                                },
                            );
                            if should_clear {
                                svg_fade.pause();
                                *indicator_svg_rc.borrow_mut() = None;
                                if let Some(p) = page_weak.upgrade() {
                                    p.set_indicator_icon(None::<&gio::Icon>);
                                }
                            }
                        },
                    );
                }
            } else if page.indicator_icon().is_some() {
                page.set_indicator_icon(None::<&gio::Icon>);
            }
        }
    }

    let mut tab_nodes = Vec::new();
    find_tab_nodes(inner.tab_bar.upcast_ref(), &mut tab_nodes);

    let n_pages = inner.tab_view.n_pages();
    for i in 0..n_pages {
        let page = inner.tab_view.nth_page(i);
        let Some(tc) = inner
            .tabs
            .iter()
            .find(|t| t.controller.widget() == &page.child())
        else {
            continue;
        };

        let category = tc.app_state.application_category;

        if let Some(tab_widget) = tab_nodes.get(i as usize) {
            sync_tab_title_centering(tab_widget, page.icon().is_some());
            if let Some(icon_img) = find_tab_icon_image(tab_widget) {
                let current_svg_opt = tc.category_svg.borrow().as_ref().map(|(_, s)| s.clone());
                if let Some(svg) = current_svg_opt {
                    let paintable: &gtk4::gdk::Paintable = svg.upcast_ref();
                    if icon_img.paintable().as_ref() != Some(paintable) {
                        icon_img.set_paintable(Some(&svg));
                    }
                    if let Some(clock) = icon_img
                        .frame_clock()
                        .or_else(|| inner.tab_bar.frame_clock())
                    {
                        svg.set_frame_clock(&clock);
                    }
                    svg.play();
                    let empty_idx = empty_state_index(&svg);
                    if category.is_active() && svg.state() == empty_idx {
                        // Defer state transition `empty/hidden -> 0` by one idle tick
                        // so `GtkImage` is mapped and has rendered the initial `empty` state
                        // on the frame clock before starting the appear animation.
                        let svg_enter = svg.clone();
                        let img_weak = icon_img.downgrade();
                        let is_celestial_loader = category == ApplicationCategory::RunningCommand;
                        let anim_source_rc = tc.working_anim_source.clone();
                        let category_svg_rc = tc.category_svg.clone();
                        gtk4::glib::idle_add_local_once(move || {
                            if let Some(img) = img_weak.upgrade()
                                && let Some(clock) = img.frame_clock()
                            {
                                svg_enter.set_frame_clock(&clock);
                            }
                            svg_enter.play();
                            svg_enter.set_state(0);

                            // For `celestial-loader.gpa` (8 compass working states + state 8 `hidden`),
                            // let the 500ms appear animation complete, then cycle through the 8
                            // compass working states (`0..8`) in sync with the 1400ms comet orbit.
                            if is_celestial_loader && empty_idx >= 8 {
                                let svg_cycle = svg_enter.clone();
                                let anim_source_inner = anim_source_rc.clone();
                                let category_svg_inner = category_svg_rc.clone();
                                gtk4::glib::timeout_add_local_once(
                                    std::time::Duration::from_millis(500),
                                    move || {
                                        let still_active = category_svg_inner
                                            .borrow()
                                            .as_ref()
                                            .is_some_and(|(cat, cur)| {
                                                *cat == ApplicationCategory::RunningCommand
                                                    && cur == &svg_cycle
                                                    && svg_cycle.state() < empty_idx
                                            });
                                        if still_active && anim_source_inner.borrow().is_none() {
                                            let source = gtk4::glib::timeout_add_local(
                                                std::time::Duration::from_millis(175),
                                                move || {
                                                    if svg_cycle.state() >= empty_idx {
                                                        return gtk4::glib::ControlFlow::Break;
                                                    }
                                                    let next_state =
                                                        (svg_cycle.state() + 1) % empty_idx;
                                                    svg_cycle.set_state(next_state);
                                                    gtk4::glib::ControlFlow::Continue
                                                },
                                            );
                                            *anim_source_inner.borrow_mut() = Some(source);
                                        }
                                    },
                                );
                            }
                        });
                    }
                } else if !category.is_active() && page.icon().is_some() {
                    page.set_icon(None::<&gio::Icon>);
                    sync_tab_title_centering(tab_widget, false);
                }
            }

            if let Some(ind_img) = find_tab_indicator_image(tab_widget) {
                let current_ind_opt = tc
                    .indicator_svg
                    .borrow()
                    .as_ref()
                    .map(|(k, s)| (*k, s.clone()));
                if let Some((ind_kind, ind_svg)) = current_ind_opt {
                    let paintable: &gtk4::gdk::Paintable = ind_svg.upcast_ref();
                    if ind_img.paintable().as_ref() != Some(paintable) {
                        ind_img.set_paintable(Some(&ind_svg));
                    }
                    if let Some(clock) = ind_img
                        .frame_clock()
                        .or_else(|| inner.tab_bar.frame_clock())
                    {
                        ind_svg.set_frame_clock(&clock);
                    }
                    ind_svg.play();
                    let empty_idx = empty_state_index(&ind_svg);
                    if ind_kind.is_active() && ind_svg.state() == empty_idx {
                        let svg_enter = ind_svg.clone();
                        let img_weak = ind_img.downgrade();
                        gtk4::glib::idle_add_local_once(move || {
                            if let Some(img) = img_weak.upgrade()
                                && let Some(clock) = img.frame_clock()
                            {
                                svg_enter.set_frame_clock(&clock);
                            }
                            svg_enter.play();
                            svg_enter.set_state(0);
                        });
                    }
                } else if page.indicator_icon().is_some() {
                    page.set_indicator_icon(None::<&gio::Icon>);
                }
            }
        }
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

fn ensure_tab_label_halign(widget: &gtk4::Widget) {
    if let Some(label) = widget.downcast_ref::<gtk4::Label>()
        && label.halign() != gtk4::Align::Center
    {
        label.set_halign(gtk4::Align::Center);
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        ensure_tab_label_halign(&c);
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
            ensure_tab_label_halign(tab_widget);

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
    sync_tab_running_icons(inner);
}

pub fn new_tab(inner: &mut AppWindowInner) {
    new_tab_with_intent(inner, None);
}

pub fn new_tab_in_dir(inner: &mut AppWindowInner, working_dir: Option<String>) {
    let _ = new_tab_internal(inner, None, working_dir, None);
}

pub fn new_tab_with_intent(inner: &mut AppWindowInner, intent: Option<String>) -> TerminalComponent {
    new_tab_internal(inner, intent, None, None)
}

pub fn new_tab_reattach(
    inner: &mut AppWindowInner,
    pid: u32,
    working_dir: Option<String>,
) -> TerminalComponent {
    new_tab_internal(inner, None, working_dir, Some(pid))
}

/// Restores a batch of detached sessions, grouping sessions that originated from
/// splits within the same tab (`{tab_id}::{idx}::{v|h}`) back into a single tab's
/// split layout while opening unrelated tabs separately.
pub fn reattach_sessions_grouped(
    inner: &mut AppWindowInner,
    sessions: Vec<boxxy_terminal::DetachedSessionInfo>,
) {
    let mut groups: Vec<(
        String,
        Vec<(usize, bool, boxxy_terminal::DetachedSessionInfo)>,
    )> = Vec::new();

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
    explicit_working_dir: Option<String>,
    reattach_pid: Option<u32>,
) -> TerminalComponent {
    let id = uuid::Uuid::new_v4().to_string();

    let working_dir = explicit_working_dir
        .or_else(|| inner.initial_working_dir.take())
        .or_else(|| {
            if inner.current_settings.preserve_working_dir {
                inner.tab_view.selected_page().and_then(|page| {
                    let child = page.child();
                    inner
                        .tabs
                        .iter()
                        .find(|c| c.controller.widget() == &child)
                        .and_then(|tc| tc.cwd.clone())
                })
            } else {
                None
            }
        });

    let controller = TerminalComponent::new(TerminalInit {
        id: id.clone(),
        working_dir: working_dir.clone(),
        spawn_intent: intent,
        reattach_pid,
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

    let tx_icon = inner.tx.clone();
    page.connect_notify_local(Some("icon"), move |_, _| {
        let _ = tx_icon.send_blocking(crate::state::AppInput::SyncTabColors);
    });
    let tx_ind = inner.tx.clone();
    page.connect_notify_local(Some("indicator-icon"), move |_, _| {
        let _ = tx_ind.send_blocking(crate::state::AppInput::SyncTabColors);
    });

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
        controller: controller.clone(),
        id,
        cwd: working_dir,
        raw_title: None,
        foreground_process: None,
        has_running_app: false,
        unread_bell: false,
        app_state: crate::application_detector::TerminalAppState::default(),
        category_svg: std::rc::Rc::new(std::cell::RefCell::new(None)),
        indicator_svg: std::rc::Rc::new(std::cell::RefCell::new(None)),
        working_anim_source: std::rc::Rc::new(std::cell::RefCell::new(None)),
        tab_color,
        custom_title: None,
    };
    inner.tabs.push(tc);
    sync_tab_titles(inner);

    inner.tab_view.set_selected_page(&page);

    let tx = inner.tx.clone();
    gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
        let _ = tx.send_blocking(crate::state::AppInput::SyncTabColors);
    });

    controller
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
        inner.tabs[pos].custom_title = title;
        sync_tab_titles(inner);
    }
}

pub fn close_tab_request(
    inner_ref: &std::rc::Rc<std::cell::RefCell<AppWindowInner>>,
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

    let unpinned_count = inner.tab_view.n_pages() - inner.tab_view.n_pinned_pages();
    if unpinned_count <= 1 && !page.is_pinned() {
        inner.tab_view.close_page_finish(&page, false);
        inner.window.close();
        return;
    }

    let Some(pos) = inner
        .tabs
        .iter()
        .position(|c| c.controller.widget().as_ptr() as usize == key)
    else {
        inner.tab_view.close_page_finish(&page, true);
        return;
    };

    let pids = inner.tabs[pos].controller.get_pids();
    let pane_ids: std::collections::HashSet<String> =
        inner.tabs[pos].controller.get_pane_ids().into_iter().collect();
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

        // If the tab only has an idle shell prompt (no running child commands),
        // no pending AI reminders/tasks, and no active AI agent session (or the
        // daemon is stopped), terminate the shell cleanly even when `pty_persistence`
        // is on so explicitly closed tabs don't accumulate empty shells in the background.
        if !daemon_alive
            || (running_apps.is_empty()
                && tab_pending_tasks.is_empty()
                && active_claw_panes.is_empty())
        {
            let controller_opt = {
                let inner = inner_clone.borrow();
                inner
                    .tabs
                    .iter()
                    .find(|c| c.controller.widget().as_ptr() as usize == key)
                    .map(|c| c.controller.clone())
            };
            if let Some(controller) = controller_opt
                && daemon_alive
            {
                controller.terminate_on_close_sync().await;
            }
            {
                let mut inner = inner_clone.borrow_mut();
                if let Some(pos) = inner
                    .tabs
                    .iter()
                    .position(|c| c.controller.widget().as_ptr() as usize == key)
                {
                    let removed = inner.tabs.remove(pos);
                    removed.stop_working_anim();
                }
                sync_tab_titles(&mut inner);
            }
            inner_clone.borrow().tab_view.close_page_finish(&page, true);
            return;
        }

        // If child processes, AI tasks, or an AI agent session ARE active and
        // "Always Keep Running Processes in Background" (`pty_persistence`) is on,
        // detach the session into the background without nagging.
        if boxxy_preferences::Settings::load().pty_persistence {
            let (controller_opt, tx) = {
                let inner = inner_clone.borrow();
                let ctrl = inner
                    .tabs
                    .iter()
                    .find(|c| c.controller.widget().as_ptr() as usize == key)
                    .map(|c| c.controller.clone());
                (ctrl, inner.tx.clone())
            };
            if let Some(controller) = controller_opt {
                controller.detach_on_close_sync().await;
            }
            {
                let mut inner = inner_clone.borrow_mut();
                if let Some(pos) = inner
                    .tabs
                    .iter()
                    .position(|c| c.controller.widget().as_ptr() as usize == key)
                {
                    let removed = inner.tabs.remove(pos);
                    removed.stop_working_anim();
                }
                sync_tab_titles(&mut inner);
            }
            inner_clone.borrow().tab_view.close_page_finish(&page, true);
            let _ = tx.send(crate::state::AppInput::RefreshDetachedSessions).await;
            return;
        }

        let dialog = libadwaita::AlertDialog::builder()
            .heading("Close Tab?")
            .body("Put this session in the background to continue it later without needing to resume, or terminate it.")
            .build();

        let group = libadwaita::PreferencesGroup::new();
        for (pid, comm) in running_apps {
            let row = libadwaita::ActionRow::builder()
                .title(&comm)
                .subtitle(format!("Process {}", pid))
                .focusable(false)
                .build();
            group.add(&row);
        }

        let mut panes_with_tasks = std::collections::HashSet::new();
        for task in tab_pending_tasks {
            panes_with_tasks.insert(task.pane_id.clone());
            let type_str = match task.task_type {
                boxxy_claw_protocol::TaskType::Notification => "Reminder",
                boxxy_claw_protocol::TaskType::Command => "Scheduled Command",
                boxxy_claw_protocol::TaskType::Query => "Scheduled Query",
            };
            let agent_name = if task.character_name.is_empty() {
                "AI Agent"
            } else {
                &task.character_name
            };
            let row = libadwaita::ActionRow::builder()
                .title(&task.payload)
                .subtitle(format!("{} by {}", type_str, agent_name))
                .focusable(false)
                .build();
            group.add(&row);
        }

        for (pane_id, agent_name) in active_claw_panes {
            if !panes_with_tasks.contains(&pane_id) {
                let row = libadwaita::ActionRow::builder()
                    .title(&agent_name)
                    .subtitle("Active AI Agent Session")
                    .focusable(false)
                    .build();
                group.add(&row);
            }
        }

        let scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .max_content_height(250)
            .propagate_natural_height(true)
            .child(&group)
            .build();

        let extra_vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
        extra_vbox.set_width_request(440);
        extra_vbox.append(&scrolled);

        let actions_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 10);
        actions_row.set_homogeneous(true);

        let cancel_btn = gtk4::Button::builder()
            .label("Cancel")
            .height_request(38)
            .build();
        let terminate_btn = gtk4::Button::builder()
            .label("Terminate")
            .css_classes(["destructive-action"])
            .height_request(38)
            .build();
        let bg_btn = gtk4::Button::builder()
            .label("Run in Background")
            .css_classes(["suggested-action"])
            .height_request(38)
            .build();

        let chosen = std::rc::Rc::new(std::cell::Cell::new("cancel"));
        {
            let c = chosen.clone();
            let d = dialog.downgrade();
            cancel_btn.connect_clicked(move |_| {
                c.set("cancel");
                if let Some(dlg) = d.upgrade() {
                    dlg.close();
                }
            });
        }
        {
            let c = chosen.clone();
            let d = dialog.downgrade();
            terminate_btn.connect_clicked(move |_| {
                c.set("close");
                if let Some(dlg) = d.upgrade() {
                    dlg.close();
                }
            });
        }
        {
            let c = chosen.clone();
            let d = dialog.downgrade();
            bg_btn.connect_clicked(move |_| {
                c.set("background");
                if let Some(dlg) = d.upgrade() {
                    dlg.close();
                }
            });
        }

        actions_row.append(&cancel_btn);
        actions_row.append(&terminate_btn);
        actions_row.append(&bg_btn);
        extra_vbox.append(&actions_row);

        dialog.set_extra_child(Some(&extra_vbox));

        let window = inner_clone.borrow().window.clone();
        let _ = dialog.choose_future(Some(&window)).await;
        let response = chosen.get();
        if response == "background" {
            let (controller_opt, tx) = {
                let inner = inner_clone.borrow();
                let ctrl = inner
                    .tabs
                    .iter()
                    .find(|c| c.controller.widget().as_ptr() as usize == key)
                    .map(|c| c.controller.clone());
                (ctrl, inner.tx.clone())
            };
            if let Some(controller) = controller_opt {
                controller.detach_on_close_sync().await;
            }
            {
                let mut inner = inner_clone.borrow_mut();
                if let Some(pos) = inner
                    .tabs
                    .iter()
                    .position(|c| c.controller.widget().as_ptr() as usize == key)
                {
                    let removed = inner.tabs.remove(pos);
                    removed.stop_working_anim();
                }
                sync_tab_titles(&mut inner);
            }
            inner_clone.borrow().tab_view.close_page_finish(&page, true);
            let _ = tx.send(crate::state::AppInput::RefreshDetachedSessions).await;
        } else if response == "close" {
            let controller_opt = {
                let inner = inner_clone.borrow();
                inner
                    .tabs
                    .iter()
                    .find(|c| c.controller.widget().as_ptr() as usize == key)
                    .map(|c| c.controller.clone())
            };
            if let Some(controller) = controller_opt {
                controller.terminate_on_close_sync().await;
            }
            {
                let mut inner = inner_clone.borrow_mut();
                if let Some(pos) = inner
                    .tabs
                    .iter()
                    .position(|c| c.controller.widget().as_ptr() as usize == key)
                {
                    let removed = inner.tabs.remove(pos);
                    removed.stop_working_anim();
                }
                sync_tab_titles(&mut inner);
            }
            inner_clone.borrow().tab_view.close_page_finish(&page, true);
        } else {
            let tx = {
                let inner = inner_clone.borrow();
                inner.tab_view.close_page_finish(&page, false);
                inner.tx.clone()
            };
            let _ = tx.send_blocking(crate::state::AppInput::FocusActiveTerminal);
        }
    });
}

pub fn close_tab(inner: &mut AppWindowInner, id: String) {
    let unpinned_count = inner.tab_view.n_pages() - inner.tab_view.n_pinned_pages();
    if unpinned_count <= 1 {
        if let Some(pos) = inner.tabs.iter().position(|c| c.id == id) {
            inner.tabs.remove(pos);
        }
        if inner.tabs.is_empty() {
            inner.force_close.set(true);
            inner.window.close();
            inner.window.destroy();
            return;
        }
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
        if let Some(tc) = ORPHAN_TABS.with(
            |pool: &std::cell::RefCell<
                std::collections::HashMap<String, crate::init::TerminalController>,
            >| pool.borrow_mut().remove(&key.to_string()),
        ) {
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
    }
    sync_tab_titles(inner);
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
        ORPHAN_TABS.with(
            |pool: &std::cell::RefCell<
                std::collections::HashMap<String, crate::init::TerminalController>,
            >| pool.borrow_mut().insert(key.to_string(), tc),
        );
        sync_tab_titles(inner);
    } else if let Some(bookmarks_page) = &inner.bookmarks_page
        && bookmarks_page.child().as_ptr() as usize == key
    {
        inner.bookmarks_controller = None;
        inner.bookmarks_page = None;
    }
}

pub fn tab_page_attached(inner: &mut AppWindowInner, key: usize) {
    if let Some(tc) = ORPHAN_TABS.with(
        |pool: &std::cell::RefCell<
            std::collections::HashMap<String, crate::init::TerminalController>,
        >| pool.borrow_mut().remove(&key.to_string()),
    ) {
        let parsed = load_palette(inner.current_settings.theme.as_str());
        let is_dark = libadwaita::StyleManager::default().is_dark();
        let palette = parsed
            .as_ref()
            .map(|p| if is_dark { p.dark } else { p.light });
        tc.controller
            .update_settings(inner.current_settings.clone(), palette);

        let widget = tc.controller.widget();
        let page = inner.tab_view.page(widget);
        let tx_icon = inner.tx.clone();
        page.connect_notify_local(Some("icon"), move |_, _| {
            let _ = tx_icon.send_blocking(crate::state::AppInput::SyncTabColors);
        });
        let tx_ind = inner.tx.clone();
        page.connect_notify_local(Some("indicator-icon"), move |_, _| {
            let _ = tx_ind.send_blocking(crate::state::AppInput::SyncTabColors);
        });

        tc.controller.grab_focus();
        inner.tabs.push(tc);
        sync_tab_titles(inner);
    }
    let tx = inner.tx.clone();
    gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
        let _ = tx.send_blocking(crate::state::AppInput::SyncTabColors);
    });
}

pub fn focus_active_terminal(inner: &mut AppWindowInner) {
    inner.bell_indicator.set_visible(false);
    if let Some(page) = inner.tab_view.selected_page() {
        let child = page.child();
        if let Some(pos) = inner
            .tabs
            .iter()
            .position(|c| c.controller.widget() == &child)
        {
            inner.tabs[pos].unread_bell = false;
        }
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

        if let Some(pos) = inner
            .tabs
            .iter()
            .position(|c| c.controller.widget() == &child)
        {
            let modal_open = inner.has_modal_or_popover_open();
            let tc = &inner.tabs[pos];
            if !modal_open {
                tc.controller.grab_focus();
            }

            // Sync Claw UI state
            let tab_is_claw_active = tc.controller.is_claw_active();
            inner.claw_active = tab_is_claw_active;
            let tab_is_sleep = tc.controller.is_sleep();

            inner.claw.set_pane_state(
                &tc.controller.agent_name(),
                tab_is_sleep,
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
