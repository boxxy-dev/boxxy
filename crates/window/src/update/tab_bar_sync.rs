use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk4::gdk;
use gtk4::prelude::*;

use crate::application_detector::{
    ApplicationCategory, BELL_GPA_RESOURCE_PATH, TabIndicatorKind,
};
use crate::state::AppWindowInner;

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
}

/// Finds the index of the `"empty"` or `"hidden"` state in a GPA `gtk4::Svg`
/// (supports 2-state `outline empty` and 3-state `outline filled empty` icons).
fn empty_state_index(svg: &gtk4::Svg) -> u32 {
    let (names, n_states) = svg.state_names();
    names
        .iter()
        .position(|n| n.as_str() == "empty" || n.as_str() == "hidden")
        .map_or_else(|| n_states.saturating_sub(1), |pos| pos as u32)
}

/// Returns `true` when the window is mapped and not suspended by the compositor.
/// Unfocused visible windows (tiled or on a second monitor) stay animated, while
/// minimized, workspace-hidden, screen-locked, or occluded windows pause at 0 Hz.
fn is_window_animating(inner: &AppWindowInner) -> bool {
    inner.window.is_mapped() && !inner.window.is_suspended()
}

/// Loads a `.gpa` icon with `SvgFeatures::ANIMATIONS` stripped so `set_state(0)`
/// snaps instantaneously to its resting state in one static frame while suspended.
fn load_static_gpa_state_zero(gpa_path: &str) -> gtk4::Svg {
    let svg = gtk4::Svg::new();
    svg.set_features(svg.features() - gtk4::SvgFeatures::ANIMATIONS);
    svg.load_from_resource(gpa_path);
    svg.set_state(0);
    svg.pause();
    svg
}

fn start_celestial_phase_timer(
    window: &gtk4::Window,
    svg_cycle: gtk4::Svg,
    empty_idx: u32,
    anim_source_rc: &Rc<RefCell<Option<gtk4::glib::SourceId>>>,
    category_svg_rc: &Rc<RefCell<Option<(ApplicationCategory, gtk4::Svg)>>>,
) {
    if anim_source_rc.borrow().is_some() || empty_idx < 8 {
        return;
    }
    let win_weak = window.downgrade();
    let anim_tick = anim_source_rc.clone();
    let cat_tick = category_svg_rc.clone();
    let src = gtk4::glib::timeout_add_local(Duration::from_millis(175), move || {
        let visible = win_weak
            .upgrade()
            .is_some_and(|w| w.is_mapped() && !w.is_suspended());
        let active = cat_tick.borrow().as_ref().is_some_and(|(cat, cur)| {
            *cat == ApplicationCategory::RunningCommand
                && cur == &svg_cycle
                && svg_cycle.state() < empty_idx
        });
        if !visible || !active {
            svg_cycle.pause();
            *anim_tick.borrow_mut() = None;
            return gtk4::glib::ControlFlow::Break;
        }
        svg_cycle.set_state((svg_cycle.state() + 1) % empty_idx);
        gtk4::glib::ControlFlow::Continue
    });
    *anim_source_rc.borrow_mut() = Some(src);
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
    let can_animate = is_window_animating(inner);
    let win_upcast: gtk4::Window = inner.window.clone().upcast();
    let fallback_clock = inner.tab_bar.frame_clock();

    for tc in &inner.tabs {
        let page = inner.tab_view.page(tc.controller.widget());
        let Some(tab_widget) = inner.tab_bar.tab_for_page(&page) else {
            continue;
        };

        let category = tc.app_state.application_category;
        let is_loader = category == ApplicationCategory::RunningCommand;
        if (!is_loader || !can_animate)
            && let Some(source) = tc.working_anim_source.borrow_mut().take()
        {
            source.remove();
        }
        let anim_rc = tc.working_anim_source.clone();
        let cat_rc_for_loop = tc.category_svg.clone();
        let win_for_loop = win_upcast.clone();
        sync_slotted_gpa(
            &win_upcast,
            fallback_clock.as_ref(),
            tab_widget.category_image(),
            &tc.category_svg,
            category.gpa_resource_path().map(|p| (category, p)),
            ApplicationCategory::Idle,
            false,
            520,
            is_loader,
            can_animate,
            move |svg, empty_idx| {
                start_celestial_phase_timer(
                    &win_for_loop,
                    svg,
                    empty_idx,
                    &anim_rc,
                    &cat_rc_for_loop,
                );
            },
        );

        let ind_kind = if tc.has_pending_tasks() {
            TabIndicatorKind::Timer
        } else if tc.controller.is_claw_active() {
            TabIndicatorKind::Claw
        } else {
            TabIndicatorKind::None
        };
        sync_slotted_gpa(
            &win_upcast,
            fallback_clock.as_ref(),
            tab_widget.indicator_image(),
            &tc.indicator_svg,
            ind_kind.gpa_resource_path().map(|p| (ind_kind, p)),
            TabIndicatorKind::None,
            true,
            540,
            false,
            can_animate,
            |_, _| {},
        );

        sync_bell_gpa(inner, tc, &tab_widget, can_animate);
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_slotted_gpa<K: Copy + PartialEq + 'static>(
    window: &gtk4::Window,
    fallback_clock: Option<&gdk::FrameClock>,
    img: &gtk4::Image,
    slot: &Rc<RefCell<Option<(K, gtk4::Svg)>>>,
    active: Option<(K, &'static str)>,
    idle_key: K,
    hide_when_empty: bool,
    fade_ms: u64,
    continuous_loop: bool,
    can_animate: bool,
    on_loop_ready: impl Fn(gtk4::Svg, u32) + 'static,
) {
    if let Some((key, gpa_path)) = active {
        if hide_when_empty && !img.is_visible() {
            img.set_visible(true);
        }

        if !can_animate {
            let existing_active = slot.borrow().as_ref().and_then(|(k, s)| {
                (*k == key && s.state() < empty_state_index(s)).then(|| s.clone())
            });
            if let Some(svg) = existing_active {
                svg.pause();
                if img.paintable().as_ref() != Some(svg.upcast_ref::<gdk::Paintable>()) {
                    img.set_paintable(Some(&svg));
                }
            } else {
                if let Some((_, old)) = slot.borrow_mut().take() {
                    old.pause();
                }
                let svg = load_static_gpa_state_zero(gpa_path);
                img.set_paintable(Some(&svg));
                *slot.borrow_mut() = Some((key, svg));
            }
            return;
        }

        let needs_load = slot.borrow().as_ref().is_none_or(|(k, s)| {
            *k != key || (continuous_loop && !s.features().contains(gtk4::SvgFeatures::ANIMATIONS))
        });
        if needs_load {
            let prev = slot.borrow_mut().take();
            if let Some((_, old)) = prev {
                old.pause();
            }
            let svg = gtk4::Svg::from_resource(gpa_path);
            if let Some(clock) = img.frame_clock().or_else(|| fallback_clock.cloned()) {
                svg.set_frame_clock(&clock);
            }
            svg.play();
            *slot.borrow_mut() = Some((key, svg));
        }

        let current_svg = slot.borrow().as_ref().map(|(_, s)| s.clone());
        let Some(svg) = current_svg else {
            return;
        };
        if img.paintable().as_ref() != Some(svg.upcast_ref::<gdk::Paintable>()) {
            img.set_paintable(Some(&svg));
        }
        if let Some(clock) = img.frame_clock().or_else(|| fallback_clock.cloned()) {
            svg.set_frame_clock(&clock);
        }
        svg.play();

        let empty_idx = empty_state_index(&svg);
        let on_loop = Rc::new(on_loop_ready);
        if svg.state() == empty_idx {
            let img_weak = img.downgrade();
            let win_weak = window.downgrade();
            let slot_rc = slot.clone();
            let on_loop_enter = on_loop;
            gtk4::glib::idle_add_local_once(move || {
                let Some(win) = win_weak.upgrade() else {
                    svg.pause();
                    return;
                };
                let still_same = slot_rc
                    .borrow()
                    .as_ref()
                    .is_some_and(|(k, cur)| *k == key && cur == &svg);
                if !still_same || !win.is_mapped() || win.is_suspended() {
                    svg.pause();
                    return;
                }
                if let Some(w) = img_weak.upgrade()
                    && let Some(clock) = w.frame_clock()
                {
                    svg.set_frame_clock(&clock);
                }
                svg.play();
                svg.set_state(0);

                if continuous_loop {
                    let slot_inner = slot_rc;
                    let win_settle = win.downgrade();
                    gtk4::glib::timeout_add_local_once(Duration::from_millis(500), move || {
                        let valid = slot_inner.borrow().as_ref().is_some_and(|(k, cur)| {
                            *k == key && cur == &svg && svg.state() < empty_idx
                        });
                        if valid
                            && win_settle
                                .upgrade()
                                .is_some_and(|w| w.is_mapped() && !w.is_suspended())
                        {
                            on_loop_enter(svg, empty_idx);
                        }
                    });
                }
            });
        } else if continuous_loop {
            on_loop(svg, empty_idx);
        }
    } else {
        // Bind outside `if let` so the `RefCell` borrow is dropped before `borrow_mut()`.
        let existing = slot.borrow().clone();
        if let Some((prev_key, svg)) = existing {
            let clear_img = |target: &gtk4::Image| {
                if hide_when_empty {
                    target.set_visible(false);
                }
                target.set_paintable(None::<&gdk::Paintable>);
            };
            if !can_animate || !svg.features().contains(gtk4::SvgFeatures::ANIMATIONS) {
                svg.pause();
                *slot.borrow_mut() = None;
                clear_img(img);
            } else if prev_key != idle_key {
                let empty_idx = empty_state_index(&svg);
                *slot.borrow_mut() = Some((idle_key, svg.clone()));
                if svg.state() != empty_idx {
                    svg.play();
                    svg.set_state(empty_idx);
                    let slot_rc = slot.clone();
                    let img_weak = img.downgrade();
                    gtk4::glib::timeout_add_local_once(Duration::from_millis(fade_ms), move || {
                        let should_clear = slot_rc.borrow().as_ref().is_some_and(|(k, cur)| {
                            *k == idle_key && cur == &svg && svg.state() == empty_idx
                        });
                        if should_clear {
                            svg.pause();
                            *slot_rc.borrow_mut() = None;
                            if let Some(w) = img_weak.upgrade() {
                                if hide_when_empty {
                                    w.set_visible(false);
                                }
                                w.set_paintable(None::<&gdk::Paintable>);
                            }
                        }
                    });
                } else {
                    svg.pause();
                    *slot.borrow_mut() = None;
                    clear_img(img);
                }
            }
        } else if (!hide_when_empty && img.paintable().is_some())
            || (hide_when_empty && img.is_visible())
        {
            if hide_when_empty {
                img.set_visible(false);
            }
            img.set_paintable(None::<&gdk::Paintable>);
        }
    }
}

fn sync_bell_gpa(
    inner: &AppWindowInner,
    tc: &crate::init::TerminalController,
    tab_widget: &crate::widgets::tab_bar::BoxxyTab,
    can_animate: bool,
) {
    let bell_img = tab_widget.bell_image();

    if tc.unread_bell {
        if tc.bell_svg.borrow().is_none() {
            let svg = gtk4::Svg::from_resource(BELL_GPA_RESOURCE_PATH);
            svg.pause();
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
            if bell_img.paintable().as_ref() != Some(bell_svg.upcast_ref::<gdk::Paintable>()) {
                bell_img.set_paintable(Some(&bell_svg));
            }
            if !bell_img.is_visible() {
                bell_img.set_visible(true);
            }

            if tc.bell_ringing.get() && can_animate {
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
                let win_weak = inner.window.downgrade();
                let ringing_flag = tc.bell_ringing.clone();
                gtk4::glib::idle_add_local_once(move || {
                    if !ringing_flag.get()
                        || !win_weak
                            .upgrade()
                            .is_some_and(|w| w.is_mapped() && !w.is_suspended())
                    {
                        svg_ring.pause();
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
                bell_svg.pause();
                bell_svg.set_state(0);
            } else if !can_animate {
                bell_svg.pause();
            }
        }
    } else {
        let prev = tc.bell_svg.borrow_mut().take();
        if let Some(svg) = prev {
            svg.pause();
        }
        if bell_img.is_visible() {
            bell_img.set_visible(false);
            bell_img.set_paintable(None::<&gdk::Paintable>);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYNTHETIC_GPA: &[u8] = br#"<svg xmlns='http://www.w3.org/2000/svg'
        xmlns:gpa='https://www.gtk.org/grappa'
        width='16' height='16' gpa:version='1'
        gpa:state='1' gpa:state-names='outline empty'>
      <path d='M 2 2 L 14 14' fill='none' stroke='rgb(0,0,0)' stroke-width='2'
            gpa:states='outline' gpa:stroke='foreground'
            gpa:transition-type='fade' gpa:transition-duration='400ms'/>
    </svg>"#;

    #[gtk4::test]
    fn static_gpa_snap_renders_state_zero_without_frame_clock() {
        let bytes = gtk4::glib::Bytes::from_static(SYNTHETIC_GPA);

        let svg = gtk4::Svg::new();
        svg.set_features(svg.features() - gtk4::SvgFeatures::ANIMATIONS);
        svg.load_from_bytes(&bytes);
        assert_eq!(empty_state_index(&svg), 1);

        let snap_empty = gtk4::Snapshot::new();
        gdk::prelude::PaintableExt::snapshot(&svg, &snap_empty, 16.0, 16.0);
        assert!(snap_empty.to_node().is_none());

        svg.set_state(0);
        svg.pause();
        let snap_zero = gtk4::Snapshot::new();
        gdk::prelude::PaintableExt::snapshot(&svg, &snap_zero, 16.0, 16.0);
        assert!(
            snap_zero.to_node().is_some(),
            "static GPA state 0 must render visible geometry without frame clock ticks"
        );
    }

    #[gtk4::test]
    fn sync_slotted_gpa_active_to_idle_transitions_without_refcell_conflict() {
        let win = gtk4::Window::new();
        let img = gtk4::Image::new();
        let bytes = gtk4::glib::Bytes::from_static(SYNTHETIC_GPA);
        let initial_svg = gtk4::Svg::from_bytes(&bytes);
        initial_svg.set_state(0);

        let slot: Rc<RefCell<Option<(ApplicationCategory, gtk4::Svg)>>> =
            Rc::new(RefCell::new(Some((ApplicationCategory::Editor, initial_svg))));

        // Active -> Idle in animated mode (`can_animate = true`) must borrow cleanly
        // and transition slot key to `Idle` without panicking on `RefCell`.
        sync_slotted_gpa(
            &win,
            None,
            &img,
            &slot,
            None,
            ApplicationCategory::Idle,
            false,
            520,
            false,
            true,
            |_, _| {},
        );
        assert_eq!(slot.borrow().as_ref().map(|(k, _)| *k), Some(ApplicationCategory::Idle));

        // Suspended (`can_animate = false`) clears immediately to `None`.
        sync_slotted_gpa(
            &win,
            None,
            &img,
            &slot,
            None,
            ApplicationCategory::Idle,
            false,
            520,
            false,
            false,
            |_, _| {},
        );
        assert!(slot.borrow().is_none());
    }

    #[gtk4::test]
    fn suspend_and_resume_preserves_active_continuous_svg_without_reload() {
        const SYNTHETIC_LOOP_GPA: &[u8] = br#"<svg xmlns='http://www.w3.org/2000/svg'
            xmlns:gpa='https://www.gtk.org/grappa'
            width='16' height='16' gpa:version='1'
            gpa:state='4' gpa:state-names='p0 p1 p2 p3 hidden'>
          <circle cx='8' cy='8' r='6' fill='none' stroke='rgb(0,0,0)'
                  gpa:states='p0 p1 p2 p3' gpa:stroke='foreground'
                  gpa:animation-type='automatic' gpa:animation-direction='segment'
                  gpa:animation-duration='1000ms' gpa:animation-repeat='indefinite'/>
        </svg>"#;

        let win = gtk4::Window::new();
        let img = gtk4::Image::new();
        let bytes = gtk4::glib::Bytes::from_static(SYNTHETIC_LOOP_GPA);
        let active_svg = gtk4::Svg::from_bytes(&bytes);
        active_svg.play();
        active_svg.set_state(2);
        img.set_paintable(Some(&active_svg));

        let slot: Rc<RefCell<Option<(ApplicationCategory, gtk4::Svg)>>> = Rc::new(RefCell::new(
            Some((ApplicationCategory::RunningCommand, active_svg.clone())),
        ));

        // Suspending (`can_animate = false`) must pause the existing `Svg` in-place
        // rather than replacing it with a static reload.
        sync_slotted_gpa(
            &win,
            None,
            &img,
            &slot,
            Some((ApplicationCategory::RunningCommand, "/synthetic/unused.gpa")),
            ApplicationCategory::Idle,
            false,
            520,
            true,
            false,
            |_, _| {},
        );
        {
            let borrowed = slot.borrow();
            let (cat, cur_svg) = borrowed.as_ref().expect("slot must remain populated");
            assert_eq!(*cat, ApplicationCategory::RunningCommand);
            assert_eq!(cur_svg, &active_svg, "must keep the exact same Svg instance");
            assert_eq!(cur_svg.state(), 2, "must preserve active phase state");
            assert!(
                cur_svg.features().contains(gtk4::SvgFeatures::ANIMATIONS),
                "must preserve SvgFeatures::ANIMATIONS across suspend"
            );
            assert!(!cur_svg.is_playing(), "must pause playback while suspended");
        }

        // Resuming (`can_animate = true`) must restart playback and loop callback on the
        // same `Svg` without reloading or flashing through `hidden`.
        let loop_resumed = Rc::new(std::cell::Cell::new(false));
        let loop_flag = loop_resumed.clone();
        sync_slotted_gpa(
            &win,
            None,
            &img,
            &slot,
            Some((ApplicationCategory::RunningCommand, "/synthetic/unused.gpa")),
            ApplicationCategory::Idle,
            false,
            520,
            true,
            true,
            move |_, empty_idx| {
                assert_eq!(empty_idx, 4);
                loop_flag.set(true);
            },
        );
        {
            let borrowed = slot.borrow();
            let (_, cur_svg) = borrowed.as_ref().expect("slot must remain populated");
            assert_eq!(cur_svg, &active_svg, "must resume the same Svg instance");
            assert_eq!(cur_svg.state(), 2);
            assert!(cur_svg.is_playing(), "must resume playback when visible");
            assert!(loop_resumed.get(), "must restart phase loop callback");
        }
    }
}
