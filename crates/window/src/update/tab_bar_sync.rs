use gtk4::gdk;
use gtk4::prelude::*;

use crate::application_detector::{
    ApplicationCategory, BELL_GPA_RESOURCE_PATH, TabIndicatorKind,
};
use crate::state::AppWindowInner;

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

    sync_tab_colors(inner);
    sync_header_title(inner);
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

pub fn sync_tab_colors(inner: &mut AppWindowInner) {
    for tc in &inner.tabs {
        let page = inner.tab_view.page(tc.controller.widget());
        if let Some(tab) = inner.tab_bar.tab_for_page(&page) {
            tab.set_tab_color(tc.tab_color);
        }
    }
    sync_tab_running_icons(inner);
}

pub fn sync_tab_running_icons(inner: &AppWindowInner) {
    for tc in &inner.tabs {
        let page = inner.tab_view.page(tc.controller.widget());
        let Some(tab_widget) = inner.tab_bar.tab_for_page(&page) else {
            continue;
        };

        sync_category_gpa(inner, tc, &tab_widget);
        sync_indicator_gpa(inner, tc, &tab_widget);
        sync_bell_gpa(inner, tc, &tab_widget);
    }
}

fn sync_category_gpa(
    inner: &AppWindowInner,
    tc: &crate::init::TerminalController,
    tab_widget: &crate::widgets::tab_bar::BoxxyTab,
) {
    let category = tc.app_state.application_category;
    let icon_img = tab_widget.category_image();

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
            if let Some(clock) = icon_img
                .frame_clock()
                .or_else(|| inner.tab_bar.frame_clock())
            {
                svg.set_frame_clock(&clock);
            }
            svg.play();
            *tc.category_svg.borrow_mut() = Some((category, svg));
        }

        let current_svg = tc.category_svg.borrow().as_ref().map(|(_, s)| s.clone());
        if let Some(svg) = current_svg {
            let paintable: &gdk::Paintable = svg.upcast_ref();
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
                let svg_enter = svg;
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

                    if is_celestial_loader && empty_idx >= 8 {
                        let svg_cycle = svg_enter;
                        let anim_source_inner = anim_source_rc;
                        let category_svg_inner = category_svg_rc;
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
                                                *anim_source_for_tick.borrow_mut() = None;
                                                return gtk4::glib::ControlFlow::Break;
                                            }
                                            let next_state = (svg_cycle.state() + 1) % empty_idx;
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
                svg.play();
                svg.set_state(empty_idx);
                let category_svg_rc = tc.category_svg.clone();
                let svg_fade = svg;
                let img_weak = icon_img.downgrade();
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
                            if let Some(img) = img_weak.upgrade() {
                                // Keep `category_image` visible so its 16px slot before the
                                // centered title stays reserved without shifting any widgets.
                                img.set_paintable(None::<&gdk::Paintable>);
                            }
                        }
                    },
                );
            }
        } else if icon_img.paintable().is_some() {
            icon_img.set_paintable(None::<&gdk::Paintable>);
        }
    }
}

fn sync_indicator_gpa(
    inner: &AppWindowInner,
    tc: &crate::init::TerminalController,
    tab_widget: &crate::widgets::tab_bar::BoxxyTab,
) {
    let ind_kind = if tc.has_pending_tasks() {
        TabIndicatorKind::Timer
    } else if tc.controller.is_claw_active() {
        TabIndicatorKind::Claw
    } else {
        TabIndicatorKind::None
    };

    let ind_img = tab_widget.indicator_image();

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
            if let Some(clock) = ind_img
                .frame_clock()
                .or_else(|| inner.tab_bar.frame_clock())
            {
                svg.set_frame_clock(&clock);
            }
            svg.play();
            *tc.indicator_svg.borrow_mut() = Some((ind_kind, svg));
        }

        let current_svg = tc.indicator_svg.borrow().as_ref().map(|(_, s)| s.clone());
        if let Some(ind_svg) = current_svg {
            let paintable: &gdk::Paintable = ind_svg.upcast_ref();
            if ind_img.paintable().as_ref() != Some(paintable) {
                ind_img.set_paintable(Some(&ind_svg));
            }
            if !ind_img.is_visible() {
                ind_img.set_visible(true);
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
                let svg_enter = ind_svg;
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
                let svg_fade = svg;
                let img_weak = ind_img.downgrade();
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
                            if let Some(img) = img_weak.upgrade() {
                                img.set_visible(false);
                                img.set_paintable(None::<&gdk::Paintable>);
                            }
                        }
                    },
                );
            }
        } else if ind_img.is_visible() {
            ind_img.set_visible(false);
            ind_img.set_paintable(None::<&gdk::Paintable>);
        }
    }
}

fn sync_bell_gpa(
    inner: &AppWindowInner,
    tc: &crate::init::TerminalController,
    tab_widget: &crate::widgets::tab_bar::BoxxyTab,
) {
    let bell_img = tab_widget.bell_image();

    if tc.unread_bell {
        if tc.bell_svg.borrow().is_none() {
            let svg = gtk4::Svg::from_resource(BELL_GPA_RESOURCE_PATH);
            if let Some(clock) = bell_img
                .frame_clock()
                .or_else(|| inner.tab_bar.frame_clock())
            {
                svg.set_frame_clock(&clock);
            }
            *tc.bell_svg.borrow_mut() = Some(svg);
        }

        let current_svg = tc.bell_svg.borrow().clone();
        if let Some(bell_svg) = current_svg {
            let paintable: &gdk::Paintable = bell_svg.upcast_ref();
            if bell_img.paintable().as_ref() != Some(paintable) {
                bell_img.set_paintable(Some(&bell_svg));
            }
            if !bell_img.is_visible() {
                bell_img.set_visible(true);
            }

            if tc.bell_ringing.get() {
                if let Some(clock) = bell_img
                    .frame_clock()
                    .or_else(|| inner.tab_bar.frame_clock())
                {
                    bell_svg.set_frame_clock(&clock);
                }
                bell_svg.play();
                if bell_svg.state() != 1 {
                    bell_svg.set_state(1);
                }
                let svg_ring = bell_svg;
                let img_weak = bell_img.downgrade();
                let ringing_flag = tc.bell_ringing.clone();
                gtk4::glib::idle_add_local_once(move || {
                    if !ringing_flag.get() {
                        return;
                    }
                    if let Some(img) = img_weak.upgrade()
                        && let Some(clock) = img.frame_clock()
                    {
                        svg_ring.set_frame_clock(&clock);
                    }
                    svg_ring.play();
                    svg_ring.set_state(1);
                });
            } else if bell_svg.state() != 0 {
                bell_svg.set_state(0);
            }
        }
    } else {
        if let Some(svg) = tc.bell_svg.borrow_mut().take() {
            svg.pause();
        }
        if bell_img.is_visible() {
            bell_img.set_visible(false);
            bell_img.set_paintable(None::<&gdk::Paintable>);
        }
    }
}
