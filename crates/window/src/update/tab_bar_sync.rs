use gtk4::gio;
use gtk4::prelude::*;

use crate::application_detector::{ApplicationCategory, TabIndicatorKind};
use crate::state::{AppWindowInner, TabColor};

pub fn sync_header_title(inner: &AppWindowInner) {
    let unpinned_pages = inner.tab_view.n_pages() - inner.tab_view.n_pinned_pages();
    let should_expand = unpinned_pages > 1 && !inner.current_settings.fixed_width_tabs;
    if inner.tab_bar.expands_tabs() != should_expand {
        inner.tab_bar.set_expand_tabs(should_expand);
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
    let mut tab_nodes = Vec::new();
    find_tab_nodes(inner.tab_bar.upcast_ref(), &mut tab_nodes);
    sync_tab_running_icons_with_nodes(inner, &tab_nodes);
}

fn sync_tab_running_icons_with_nodes(inner: &AppWindowInner, tab_nodes: &[gtk4::Widget]) {
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

    let n_pages = inner.tab_view.n_pages();
    for i in 0..n_pages {
        let page = inner.tab_view.nth_page(i);
        let child = page.child();
        let Some(tc) = inner.tabs.iter().find(|t| t.controller.widget() == &child) else {
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
                                            let anim_source_for_tick = anim_source_inner.clone();
                                            let source = gtk4::glib::timeout_add_local(
                                                std::time::Duration::from_millis(175),
                                                move || {
                                                    if svg_cycle.state() >= empty_idx {
                                                        // Clear the stored SourceId before returning Break so
                                                        // `stop_working_anim()` never removes an expired source.
                                                        *anim_source_for_tick.borrow_mut() = None;
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
        if let Some(tab_widget) = tab_nodes.get(i as usize) {
            ensure_tab_label_halign(tab_widget);

            let child = page.child();
            let target_cls = inner
                .tabs
                .iter()
                .find(|t| t.controller.widget() == &child)
                .and_then(|tc| tc.tab_color.as_css_class());

            // Diff-only CSS class mutations avoid dirtying GTK style nodes on no-op focus events.
            for c in TabColor::all() {
                if let Some(cls) = c.as_css_class()
                    && Some(cls) != target_cls
                    && tab_widget.has_css_class(cls)
                {
                    tab_widget.remove_css_class(cls);
                }
            }
            if let Some(cls) = target_cls
                && !tab_widget.has_css_class(cls)
            {
                tab_widget.add_css_class(cls);
            }
        }
    }

    sync_tab_running_icons_with_nodes(inner, &tab_nodes);
}
