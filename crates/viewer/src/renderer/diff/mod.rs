pub mod canvas;
pub mod drawer;
pub mod layout;
pub mod markup;

use crate::parser::blocks::ContentBlock;
use crate::registry::ViewerRegistry;
use crate::renderer::BlockRenderer;
use drawer::{build_created_view, build_drawer_view, build_split_view};
use gtk4 as gtk;
use gtk4::pango;
use gtk4::prelude::*;
use layout::{DiffLayoutMode, ResponsiveDiffContainer};
use markup::{DiffPayloadKind, PreparedDiff, get_or_prepare_diff, prepare_from_payload};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

pub struct DiffRenderer;

impl BlockRenderer for DiffRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(
            block,
            ContentBlock::Custom { schema, .. }
                if schema == "file_diff"
                    || schema == "file_diff_compact"
                    || schema == "file_diff_collapsed"
        )
    }

    fn render(&self, block: &ContentBlock, _registry: &ViewerRegistry) -> gtk::Widget {
        let ContentBlock::Custom {
            schema,
            raw_payload,
        } = block
        else {
            return gtk::Box::new(gtk::Orientation::Vertical, 0).upcast();
        };

        let is_dark = crate::cache::is_card_surface_dark();
        let Some(prepared) = get_or_prepare_diff(raw_payload, is_dark) else {
            let err_lbl = gtk::Label::new(Some("Failed to render file diff."));
            err_lbl.add_css_class("dim-label");
            return err_lbl.upcast();
        };

        let collapse_policy = match schema.as_str() {
            "file_diff_collapsed" => DiffCollapsePolicy::AlwaysCollapsed,
            "file_diff_compact" => DiffCollapsePolicy::CollapseLarge,
            _ => DiffCollapsePolicy::AlwaysExpanded,
        };
        build_diff_card_with_policy(&prepared, is_dark, collapse_policy).upcast()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffCollapsePolicy {
    AlwaysExpanded,
    CollapseLarge,
    AlwaysCollapsed,
}

#[must_use]
pub fn build_diff_card(prepared: &Arc<PreparedDiff>, is_dark: bool) -> ResponsiveDiffContainer {
    build_diff_card_with_policy(prepared, is_dark, DiffCollapsePolicy::AlwaysExpanded)
}

#[must_use]
pub fn build_diff_card_with_mode(
    prepared: &Arc<PreparedDiff>,
    is_dark: bool,
    allow_auto_collapse: bool,
) -> ResponsiveDiffContainer {
    build_diff_card_with_policy(
        prepared,
        is_dark,
        if allow_auto_collapse {
            DiffCollapsePolicy::CollapseLarge
        } else {
            DiffCollapsePolicy::AlwaysExpanded
        },
    )
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn build_diff_card_with_policy(
    prepared: &Arc<PreparedDiff>,
    is_dark: bool,
    policy: DiffCollapsePolicy,
) -> ResponsiveDiffContainer {
    let total_changed_lines = prepared.stats.added_lines + prepared.stats.removed_lines;
    let initial_expanded = match policy {
        DiffCollapsePolicy::AlwaysExpanded => true,
        DiffCollapsePolicy::CollapseLarge => total_changed_lines <= 12,
        DiffCollapsePolicy::AlwaysCollapsed => false,
    };

    let frame = gtk::Frame::new(None);
    frame.add_css_class("view");
    frame.add_css_class("viewer-diff-card");
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.set_margin_bottom(10);
    frame.set_hexpand(true);

    let card_vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card_vbox.set_hexpand(true);

    // -----------------------------------------------------------------------
    // 1. Top Header Bar (Path, Badge, +A/-R Stats, 5-Block Graphic, Actions)
    // -----------------------------------------------------------------------
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("code-header");
    header.add_css_class("diff-card-header");

    let file_icon = gtk::Image::from_icon_name("boxxy-document-edit-symbolic");
    file_icon.set_pixel_size(15);
    file_icon.add_css_class("dim-label");
    file_icon.set_margin_start(4);
    header.append(&file_icon);

    let path_lbl = gtk::Label::new(None);
    path_lbl.set_use_markup(true);
    path_lbl.set_ellipsize(pango::EllipsizeMode::Middle);
    path_lbl.set_halign(gtk::Align::Start);
    path_lbl.set_hexpand(true);
    path_lbl.set_tooltip_text(Some(&prepared.path));
    path_lbl.set_markup(&format!(
        "<span alpha=\"65%\">{}</span><b>{}</b>",
        crate::parser::markdown::escape_pango(&prepared.dir_prefix),
        crate::parser::markdown::escape_pango(&prepared.filename)
    ));
    header.append(&path_lbl);

    // Badge pill (MODIFIED / NEW FILE / BINARY / UNCHANGED)
    let (badge_text, badge_class) = match prepared.kind {
        DiffPayloadKind::Modified => ("MODIFIED", "diff-badge-modified"),
        DiffPayloadKind::Created => ("NEW FILE", "diff-badge-created"),
        DiffPayloadKind::Binary => ("BINARY", "diff-badge-binary"),
        DiffPayloadKind::Unchanged => ("UNCHANGED", "diff-badge-binary"),
    };
    let badge = gtk::Label::new(Some(badge_text));
    badge.add_css_class("diff-badge");
    badge.add_css_class(badge_class);
    badge.set_valign(gtk::Align::Center);
    header.append(&badge);

    // +Added / -Removed counters + 5-block visual ratio graphic
    if matches!(
        prepared.kind,
        DiffPayloadKind::Modified | DiffPayloadKind::Created
    ) {
        let stats_box = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        stats_box.set_valign(gtk::Align::Center);

        let add_lbl = gtk::Label::new(Some(&format!("+{}", prepared.stats.added_lines)));
        add_lbl.add_css_class("diff-stat-add");
        stats_box.append(&add_lbl);

        if prepared.kind == DiffPayloadKind::Modified || prepared.stats.removed_lines > 0 {
            let del_lbl = gtk::Label::new(Some(&format!("−{}", prepared.stats.removed_lines)));
            del_lbl.add_css_class("diff-stat-del");
            stats_box.append(&del_lbl);
        }

        stats_box.append(&build_stat_graphic_bar(
            prepared.stats.added_lines,
            prepared.stats.removed_lines,
        ));
        header.append(&stats_box);
    }

    // Drawer peek button (shown when in compact/drawer mode for Modified files)
    let peek_drawer_btn = gtk::Button::builder()
        .label("Original")
        .tooltip_text("Slide out the Original file column as a drawer")
        .css_classes(["flat", "diff-header-pill"])
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    header.append(&peek_drawer_btn);

    // Split <-> Drawer layout mode toggle button (for Modified files)
    let mode_toggle_btn = gtk::Button::builder()
        .icon_name("boxxy-split-horizontal-symbolic")
        .tooltip_text("Switch between Two-Column Split and Compact Drawer view")
        .css_classes(["flat", "circular"])
        .valign(gtk::Align::Center)
        .visible(prepared.kind == DiffPayloadKind::Modified && initial_expanded)
        .build();
    header.append(&mode_toggle_btn);

    // Copy unified patch button
    if !prepared.unified_patch.is_empty() {
        let copy_btn = gtk::Button::builder()
            .icon_name("boxxy-edit-copy-symbolic")
            .tooltip_text("Copy unified diff patch")
            .css_classes(["flat", "circular"])
            .valign(gtk::Align::Center)
            .build();
        let patch_clone = prepared.unified_patch.clone();
        copy_btn.connect_clicked(move |btn| {
            if let Some(display) = gtk::gdk::Display::default() {
                display.clipboard().set_text(&patch_clone);
                btn.set_icon_name("boxxy-object-select-symbolic");
                let b = btn.clone();
                gtk::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(1400),
                    move || {
                        b.set_icon_name("boxxy-edit-copy-symbolic");
                    },
                );
            }
        });
        header.append(&copy_btn);
    }

    // Expand / Collapse button
    let collapse_btn = gtk::Button::builder()
        .icon_name("boxxy-expand-vertical-symbolic")
        .tooltip_text(if initial_expanded {
            "Collapse diff"
        } else {
            "Expand diff"
        })
        .css_classes(["flat", "circular"])
        .valign(gtk::Align::Center)
        .build();
    header.append(&collapse_btn);

    card_vbox.append(&header);

    // -----------------------------------------------------------------------
    // 2. Body Viewport (Lazily mounted on demand if `initial_expanded == false`)
    // -----------------------------------------------------------------------
    let container = ResponsiveDiffContainer::new(&frame);
    let body_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(180)
        .reveal_child(initial_expanded)
        .hexpand(true)
        .build();

    let body_mounted = Rc::new(Cell::new(false));
    let mount_body: Rc<dyn Fn()> = {
        let body_mounted = body_mounted.clone();
        let prepared_initial = Arc::clone(prepared);
        let body_revealer = body_revealer.clone();
        let container_weak = container.downgrade();
        let peek_drawer_btn = peek_drawer_btn.clone();
        let mode_toggle_btn = mode_toggle_btn.clone();

        Rc::new(move || {
            if body_mounted.replace(true) {
                return;
            }
            let Some(container) = container_weak.upgrade() else {
                return;
            };

            let current_dark = crate::cache::is_card_surface_dark();
            let active_init = if current_dark == is_dark {
                Arc::clone(&prepared_initial)
            } else {
                Arc::new(prepare_from_payload(
                    (*prepared_initial.payload).clone(),
                    current_dark,
                ))
            };

            mount_diff_body_into_revealer(
                &active_init,
                current_dark,
                &container,
                &body_revealer,
                &peek_drawer_btn,
                &mode_toggle_btn,
            );
        })
    };

    if initial_expanded {
        mount_body();
    }

    let is_modified = prepared.kind == DiffPayloadKind::Modified;
    let body_rev_for_btn = body_revealer.clone();
    let mode_btn_for_collapse = mode_toggle_btn;
    let peek_btn_for_collapse = peek_drawer_btn;
    let container_for_collapse = container.downgrade();
    let mount_on_expand = mount_body;
    collapse_btn.connect_clicked(move |btn| {
        let next = !body_rev_for_btn.reveals_child();
        if next {
            mount_on_expand();
        }
        body_rev_for_btn.set_reveal_child(next);
        btn.set_tooltip_text(Some(if next {
            "Collapse diff"
        } else {
            "Expand diff"
        }));
        mode_btn_for_collapse.set_visible(is_modified && next);
        if !next {
            peek_btn_for_collapse.set_visible(false);
        } else if let Some(c) = container_for_collapse.upgrade()
            && is_modified
            && c.current_mode() == DiffLayoutMode::Drawer
        {
            peek_btn_for_collapse.set_visible(true);
        }
        if let Some(c) = container_for_collapse.upgrade() {
            c.queue_resize();
        }
    });

    // Also allow clicking the file path label in the header to toggle expand/collapse
    let collapse_btn_for_path = collapse_btn;
    let path_click = gtk::GestureClick::new();
    path_click.connect_released(move |_, _, _, _| {
        collapse_btn_for_path.emit_clicked();
    });
    path_lbl.add_controller(path_click);

    card_vbox.append(&body_revealer);
    frame.set_child(Some(&card_vbox));
    container
}

#[allow(clippy::too_many_lines)]
fn mount_diff_body_into_revealer(
    prepared: &Arc<PreparedDiff>,
    is_dark: bool,
    container: &ResponsiveDiffContainer,
    body_revealer: &gtk::Revealer,
    peek_drawer_btn: &gtk::Button,
    mode_toggle_btn: &gtk::Button,
) {
    match prepared.kind {
        DiffPayloadKind::Binary => {
            let notice = build_info_banner(
                "Binary or non-text file detected — line-by-line text diff skipped.",
            );
            body_revealer.set_child(Some(&notice));
        }
        DiffPayloadKind::Unchanged => {
            let notice = build_info_banner("No textual changes between original and new file.");
            body_revealer.set_child(Some(&notice));
        }
        DiffPayloadKind::Created => {
            let created_box =
                build_created_view(&prepared.hunks, prepared.stats.omitted_lines, is_dark);
            body_revealer.set_child(Some(&created_box));

            let body_rev_weak = body_revealer.downgrade();
            let payload_arc = Arc::clone(&prepared.payload);
            let last_dark = Rc::new(Cell::new(is_dark));
            let update_created: Rc<dyn Fn(bool)> = Rc::new(move |new_dark: bool| {
                if last_dark.replace(new_dark) == new_dark {
                    return;
                }
                if let Some(body_rev) = body_rev_weak.upgrade() {
                    let re_prepared = prepare_from_payload((*payload_arc).clone(), new_dark);
                    let next_box = build_created_view(
                        &re_prepared.hunks,
                        re_prepared.stats.omitted_lines,
                        new_dark,
                    );
                    body_rev.set_child(Some(&next_box));
                }
            });
            let update_for_card = update_created.clone();
            container.connect_card_dark_changed(is_dark, move |new_dark| {
                update_for_card(new_dark);
            });
            let sm = libadwaita::StyleManager::default();
            let handler_id = sm.connect_dark_notify(move |_| {
                update_created(crate::cache::is_card_surface_dark());
            });
            let handler_cell = RefCell::new(Some(handler_id));
            container.connect_destroy(move |_| {
                if let Some(id) = handler_cell.borrow_mut().take() {
                    libadwaita::StyleManager::default().disconnect(id);
                }
            });
        }
        DiffPayloadKind::Modified => {
            let stack = gtk::Stack::builder()
                .transition_type(gtk::StackTransitionType::Crossfade)
                .transition_duration(150)
                .hhomogeneous(false)
                .vhomogeneous(false)
                .hexpand(true)
                .build();

            let split_box = build_split_view(prepared, is_dark);
            stack.add_named(&split_box, Some("split"));
            stack.set_visible_child_name("split");

            let drawer_rev_slot: Rc<RefCell<Option<gtk::Revealer>>> = Rc::new(RefCell::new(None));
            let active_dark = Rc::new(Cell::new(is_dark));
            let active_prepared = Rc::new(RefCell::new(Arc::clone(prepared)));

            // Wire the "Original" drawer button in the header
            let rev_slot_for_btn = drawer_rev_slot.clone();
            peek_drawer_btn.connect_clicked(move |btn| {
                if let Some(ref rev) = *rev_slot_for_btn.borrow() {
                    let next = !rev.reveals_child();
                    if next {
                        rev.set_visible(true);
                    }
                    rev.set_reveal_child(next);
                    btn.set_label(if next { "Hide Original" } else { "Original" });
                }
            });

            // Wire manual Split <-> Drawer mode toggle button
            let container_weak = container.downgrade();
            mode_toggle_btn.connect_clicked(move |_| {
                if let Some(c) = container_weak.upgrade() {
                    let next = match c.current_mode() {
                        DiffLayoutMode::Split => DiffLayoutMode::Drawer,
                        DiffLayoutMode::Drawer => DiffLayoutMode::Split,
                    };
                    c.set_user_override(Some(next));
                }
            });

            // React to automatic width breakpoint or manual override changes
            let stack_for_mode = stack.clone();
            let peek_btn_for_mode = peek_drawer_btn.clone();
            let rev_slot_for_mode = drawer_rev_slot.clone();
            let toggle_for_mode = mode_toggle_btn.clone();
            let body_rev_for_mode = body_revealer.clone();
            let container_for_resize = container.downgrade();
            let prepared_for_drawer = active_prepared.clone();
            let dark_for_drawer = active_dark.clone();
            let apply_mode: Rc<dyn Fn(DiffLayoutMode, bool)> =
                Rc::new(move |mode: DiffLayoutMode, _is_narrow: bool| {
                    let is_expanded = body_rev_for_mode.reveals_child();
                    match mode {
                        DiffLayoutMode::Split => {
                            if let Some(ref rev) = *rev_slot_for_mode.borrow() {
                                rev.set_reveal_child(false);
                                rev.set_visible(false);
                            }
                            peek_btn_for_mode.set_visible(false);
                            stack_for_mode.set_visible_child_name("split");
                            toggle_for_mode.set_tooltip_text(Some(
                                "Two-Column Split View active (click for Compact Drawer View)",
                            ));
                        }
                        DiffLayoutMode::Drawer => {
                            if rev_slot_for_mode.borrow().is_none() {
                                let (drawer_overlay, drawer_revealer, drawer_panel) =
                                    build_drawer_view(
                                        &prepared_for_drawer.borrow(),
                                        dark_for_drawer.get(),
                                    );
                                if let Some(c) = container_for_resize.upgrade() {
                                    c.set_drawer_panel(&drawer_panel);
                                }
                                let peek_btn_for_notify = peek_btn_for_mode.clone();
                                drawer_revealer.connect_reveal_child_notify(move |rev| {
                                    peek_btn_for_notify.set_label(if rev.reveals_child() {
                                        "Hide Original"
                                    } else {
                                        "Original"
                                    });
                                });
                                stack_for_mode.add_named(&drawer_overlay, Some("drawer"));
                                *rev_slot_for_mode.borrow_mut() = Some(drawer_revealer);
                            }
                            peek_btn_for_mode.set_visible(is_expanded);
                            stack_for_mode.set_visible_child_name("drawer");
                            toggle_for_mode.set_tooltip_text(Some(
                                "Compact Drawer View active (click for Two-Column Split View)",
                            ));
                        }
                    }
                    if let Some(c) = container_for_resize.upgrade() {
                        c.queue_resize();
                    }
                });

            let apply_mode_for_cb = apply_mode.clone();
            container.connect_mode_changed(move |mode, is_narrow| {
                apply_mode_for_cb(mode, is_narrow);
            });

            // Re-render syntax highlighting and diff canvas colors when `@card_bg_color` darkness changes
            let stack_weak = stack.downgrade();
            let container_for_theme = container.downgrade();
            let peek_btn_for_theme = peek_drawer_btn.clone();
            let rev_slot_for_theme = drawer_rev_slot;
            let payload_arc = Arc::clone(&prepared.payload);
            let update_modified: Rc<dyn Fn(bool)> = Rc::new(move |new_dark: bool| {
                if active_dark.replace(new_dark) == new_dark {
                    return;
                }
                let Some(stk) = stack_weak.upgrade() else {
                    return;
                };
                let re_prepared = Arc::new(prepare_from_payload((*payload_arc).clone(), new_dark));
                *active_prepared.borrow_mut() = Arc::clone(&re_prepared);

                let current_child_name = stk
                    .visible_child_name()
                    .map_or_else(|| "split".to_string(), |s| s.to_string());

                if let Some(old_split) = stk.child_by_name("split") {
                    stk.remove(&old_split);
                }
                let next_split = build_split_view(&re_prepared, new_dark);
                stk.add_named(&next_split, Some("split"));

                if let Some(old_drawer) = stk.child_by_name("drawer") {
                    stk.remove(&old_drawer);
                }
                *rev_slot_for_theme.borrow_mut() = None;

                if current_child_name == "drawer" {
                    let (drawer_overlay, drawer_revealer, drawer_panel) =
                        build_drawer_view(&re_prepared, new_dark);
                    if let Some(c) = container_for_theme.upgrade() {
                        c.set_drawer_panel(&drawer_panel);
                    }
                    let peek_btn_for_notify = peek_btn_for_theme.clone();
                    drawer_revealer.connect_reveal_child_notify(move |rev| {
                        peek_btn_for_notify.set_label(if rev.reveals_child() {
                            "Hide Original"
                        } else {
                            "Original"
                        });
                    });
                    stk.add_named(&drawer_overlay, Some("drawer"));
                    *rev_slot_for_theme.borrow_mut() = Some(drawer_revealer);
                    stk.set_visible_child_name("drawer");
                } else {
                    stk.set_visible_child_name("split");
                }
            });
            let update_for_card = update_modified.clone();
            container.connect_card_dark_changed(is_dark, move |new_dark| {
                update_for_card(new_dark);
            });
            let sm = libadwaita::StyleManager::default();
            let handler_id = sm.connect_dark_notify(move |_| {
                update_modified(crate::cache::is_card_surface_dark());
            });
            let handler_cell = RefCell::new(Some(handler_id));
            container.connect_destroy(move |_| {
                if let Some(id) = handler_cell.borrow_mut().take() {
                    libadwaita::StyleManager::default().disconnect(id);
                }
            });

            body_revealer.set_child(Some(&stack));

            // If the card was already allocated in narrow/Drawer mode while collapsed, sync immediately
            if container.current_mode() == DiffLayoutMode::Drawer {
                apply_mode(DiffLayoutMode::Drawer, true);
            }
        }
    }
}

/// Builds a 5-segment graphical bar (like GitHub / OpenCode PR diff bars)
/// showing the ratio of additions (green) to deletions (red).
fn build_stat_graphic_bar(added: usize, removed: usize) -> gtk::Box {
    const BLOCKS: usize = 5;
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    bar.set_valign(gtk::Align::Center);
    bar.set_margin_start(2);
    bar.set_margin_end(2);

    let total = added + removed;
    let (green_count, red_count) = if total == 0 {
        (0, 0)
    } else if added > 0 && removed == 0 {
        (BLOCKS, 0)
    } else if removed > 0 && added == 0 {
        (0, BLOCKS)
    } else {
        let mut g = ((added * BLOCKS) + (total / 2)) / total;
        g = g.clamp(1, BLOCKS - 1);
        (g, BLOCKS - g)
    };

    for i in 0..BLOCKS {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.set_size_request(7, 7);
        dot.add_css_class("diff-ratio-block");
        if i < green_count {
            dot.add_css_class("diff-ratio-add");
        } else if i < green_count + red_count {
            dot.add_css_class("diff-ratio-del");
        } else {
            dot.add_css_class("diff-ratio-neutral");
        }
        bar.append(&dot);
    }

    bar
}

fn build_info_banner(text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(12);
    row.set_margin_bottom(12);
    row.set_margin_start(14);
    row.set_margin_end(14);

    let lbl = gtk::Label::new(Some(text));
    lbl.add_css_class("dim-label");
    lbl.set_halign(gtk::Align::Start);
    lbl.set_wrap(true);
    row.append(&lbl);

    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_diff_lazily_instantiates_drawer_view_on_mode_switch() {
        crate::run_gtk_test(|| {
            let json = r#"{
                "path": "src/module.rs",
                "language": "rs",
                "kind": "Modified",
                "stats": {
                    "added_lines": 1,
                    "removed_lines": 1,
                    "hunk_count": 1,
                    "old_total_lines": 4,
                    "new_total_lines": 4,
                    "truncated": false,
                    "omitted_lines": 0
                },
                "hunks": [{
                    "header": "@@ -1,2 +1,2 @@",
                    "old_start": 1,
                    "old_count": 2,
                    "new_start": 1,
                    "new_count": 2,
                    "rows": [{
                        "left": {
                            "line_no": 1,
                            "kind": "Delete",
                            "spans": [{"emphasized": true, "text": "let x = 1;"}]
                        },
                        "right": {
                            "line_no": 1,
                            "kind": "Insert",
                            "spans": [{"emphasized": true, "text": "let x = 2;"}]
                        }
                    }]
                }]
            }"#;

            let prepared = get_or_prepare_diff(json, true).expect("valid diff json");
            let card = build_diff_card(&prepared, true);
            assert_eq!(card.current_mode(), DiffLayoutMode::Split);

            let frame = card
                .first_child()
                .and_then(|w| w.downcast::<gtk::Frame>().ok())
                .expect("frame");
            let vbox = frame
                .child()
                .and_then(|w| w.downcast::<gtk::Box>().ok())
                .expect("vbox");
            let revealer = vbox
                .last_child()
                .and_then(|w| w.downcast::<gtk::Revealer>().ok())
                .expect("body revealer");
            let stack = revealer
                .child()
                .and_then(|w| w.downcast::<gtk::Stack>().ok())
                .expect("mode stack");

            assert!(stack.child_by_name("split").is_some());
            assert!(stack.child_by_name("drawer").is_none());
            assert_eq!(stack.visible_child_name().as_deref(), Some("split"));

            card.set_user_override(Some(DiffLayoutMode::Drawer));
            assert_eq!(card.current_mode(), DiffLayoutMode::Drawer);
            let drawer_child = stack
                .child_by_name("drawer")
                .expect("drawer built on demand");
            assert_eq!(stack.visible_child_name().as_deref(), Some("drawer"));

            card.set_user_override(Some(DiffLayoutMode::Split));
            assert_eq!(stack.visible_child_name().as_deref(), Some("split"));
            card.set_user_override(Some(DiffLayoutMode::Drawer));
            assert_eq!(stack.child_by_name("drawer").as_ref(), Some(&drawer_child));
        });
    }

    #[test]
    fn compact_large_diff_defers_body_mount_until_expanded() {
        crate::run_gtk_test(|| {
            let json = r#"{
                "path": "src/large_module.rs",
                "language": "rs",
                "kind": "Created",
                "stats": {
                    "added_lines": 25,
                    "removed_lines": 0,
                    "hunk_count": 1,
                    "old_total_lines": 0,
                    "new_total_lines": 25,
                    "truncated": false,
                    "omitted_lines": 0
                },
                "hunks": [{
                    "header": "@@ -0,0 +1,2 @@",
                    "old_start": 0,
                    "old_count": 0,
                    "new_start": 1,
                    "new_count": 2,
                    "rows": [{
                        "left": { "line_no": null, "kind": "Empty", "spans": [] },
                        "right": { "line_no": 1, "kind": "Insert", "spans": [{"emphasized": false, "text": "pub fn f() {}"}] }
                    }]
                }]
            }"#;

            let prepared = get_or_prepare_diff(json, true).expect("valid diff json");
            let card = build_diff_card_with_mode(&prepared, true, true);

            let frame = card
                .first_child()
                .and_then(|w| w.downcast::<gtk::Frame>().ok())
                .expect("frame");
            let vbox = frame
                .child()
                .and_then(|w| w.downcast::<gtk::Box>().ok())
                .expect("vbox");
            let header = vbox
                .first_child()
                .and_then(|w| w.downcast::<gtk::Box>().ok())
                .expect("header");
            let collapse_btn = header
                .last_child()
                .and_then(|w| w.downcast::<gtk::Button>().ok())
                .expect("collapse button");
            let revealer = vbox
                .last_child()
                .and_then(|w| w.downcast::<gtk::Revealer>().ok())
                .expect("body revealer");

            assert!(!revealer.reveals_child());
            assert!(revealer.child().is_none());

            collapse_btn.emit_clicked();
            assert!(revealer.reveals_child());
            let mounted_body = revealer.child().expect("diff body mounted on first expand");

            collapse_btn.emit_clicked();
            assert!(!revealer.reveals_child());
            assert_eq!(revealer.child().as_ref(), Some(&mounted_body));
        });
    }
}
