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
use markup::{DiffPayloadKind, PreparedDiff, get_or_prepare_diff};
use std::sync::Once;

pub struct DiffRenderer;

impl BlockRenderer for DiffRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(
            block,
            ContentBlock::Custom { schema, .. }
                if schema == "file_diff" || schema == "file_diff_compact"
        )
    }

    fn render(&self, block: &ContentBlock, _registry: &ViewerRegistry) -> gtk::Widget {
        ensure_diff_css_loaded();

        let ContentBlock::Custom {
            schema,
            raw_payload,
        } = block
        else {
            return gtk::Box::new(gtk::Orientation::Vertical, 0).upcast();
        };

        let is_dark = libadwaita::StyleManager::default().is_dark();
        let Some(prepared) = get_or_prepare_diff(raw_payload, is_dark) else {
            let err_lbl = gtk::Label::new(Some("Failed to render file diff."));
            err_lbl.add_css_class("dim-label");
            return err_lbl.upcast();
        };

        let allow_auto_collapse = schema == "file_diff_compact";
        build_diff_card_with_mode(&prepared, is_dark, allow_auto_collapse).upcast()
    }
}

#[must_use]
pub fn build_diff_card(prepared: &PreparedDiff, is_dark: bool) -> ResponsiveDiffContainer {
    build_diff_card_with_mode(prepared, is_dark, false)
}

#[must_use]
pub fn build_diff_card_with_mode(
    prepared: &PreparedDiff,
    is_dark: bool,
    allow_auto_collapse: bool,
) -> ResponsiveDiffContainer {
    // Safeguard 2:
    // - Pending approval diffs (`allow_auto_collapse == false`) ALWAYS start expanded.
    // - Small diffs (`<= 12` changed lines) ALWAYS start expanded.
    // - Only auto-approved / history diffs with `> 12` changed lines start collapsed.
    let total_changed_lines = prepared.stats.added_lines + prepared.stats.removed_lines;
    let initial_expanded = !allow_auto_collapse || total_changed_lines <= 12;
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

    // Expand / Collapse chevron button
    let collapse_btn = gtk::Button::builder()
        .icon_name(if initial_expanded {
            "boxxy-pan-down-symbolic"
        } else {
            "boxxy-pan-end-symbolic"
        })
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
    // 2. Body Viewport (Modified 2-Col/Drawer Stack, Created, Binary, Unchanged)
    // -----------------------------------------------------------------------
    let container = ResponsiveDiffContainer::new(&frame);
    let body_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(180)
        .reveal_child(initial_expanded)
        .hexpand(true)
        .build();

    let is_modified = prepared.kind == DiffPayloadKind::Modified;
    let body_rev_for_btn = body_revealer.clone();
    let mode_btn_for_collapse = mode_toggle_btn.clone();
    let peek_btn_for_collapse = peek_drawer_btn.clone();
    let container_for_collapse = container.downgrade();
    collapse_btn.connect_clicked(move |btn| {
        let next = !body_rev_for_btn.reveals_child();
        body_rev_for_btn.set_reveal_child(next);
        btn.set_icon_name(if next {
            "boxxy-pan-down-symbolic"
        } else {
            "boxxy-pan-end-symbolic"
        });
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
    let collapse_btn_for_path = collapse_btn.clone();
    let path_click = gtk::GestureClick::new();
    path_click.connect_released(move |_, _, _, _| {
        collapse_btn_for_path.emit_clicked();
    });
    path_lbl.add_controller(path_click);

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
            let (drawer_overlay, drawer_revealer, drawer_panel) =
                build_drawer_view(prepared, is_dark);
            container.set_drawer_panel(&drawer_panel);

            stack.add_named(&split_box, Some("split"));
            stack.add_named(&drawer_overlay, Some("drawer"));
            stack.set_visible_child_name("split");

            // Wire the "Original" drawer button in the header
            let rev_for_btn = drawer_revealer.clone();
            peek_drawer_btn.connect_clicked(move |btn| {
                let next = !rev_for_btn.reveals_child();
                if next {
                    rev_for_btn.set_visible(true);
                }
                rev_for_btn.set_reveal_child(next);
                btn.set_label(if next { "Hide Original" } else { "Original" });
            });

            let peek_btn_for_notify = peek_drawer_btn.clone();
            drawer_revealer.connect_reveal_child_notify(move |rev| {
                peek_btn_for_notify.set_label(if rev.reveals_child() {
                    "Hide Original"
                } else {
                    "Original"
                });
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
            let peek_btn_for_mode = peek_drawer_btn;
            let rev_for_mode = drawer_revealer;
            let toggle_for_mode = mode_toggle_btn;
            let body_rev_for_mode = body_revealer.clone();
            let container_for_resize = container.downgrade();
            container.connect_mode_changed(move |mode, _is_narrow| {
                let is_expanded = body_rev_for_mode.reveals_child();
                match mode {
                    DiffLayoutMode::Split => {
                        rev_for_mode.set_reveal_child(false);
                        rev_for_mode.set_visible(false);
                        peek_btn_for_mode.set_visible(false);
                        stack_for_mode.set_visible_child_name("split");
                        toggle_for_mode.set_tooltip_text(Some(
                            "Two-Column Split View active (click for Compact Drawer View)",
                        ));
                    }
                    DiffLayoutMode::Drawer => {
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

            body_revealer.set_child(Some(&stack));
        }
    }

    card_vbox.append(&body_revealer);

    frame.set_child(Some(&card_vbox));
    container
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

fn ensure_diff_css_loaded() {
    static DIFF_CSS_ONCE: Once = Once::new();
    DIFF_CSS_ONCE.call_once(|| {
        if let Some(display) = gtk::gdk::Display::default() {
            let css = "
                .viewer-diff-card {
                    border-radius: 10px;
                    font-size: 1.05em;
                }
                .diff-card-header {
                    padding: 6px 10px;
                    border-bottom: 1px solid rgba(128, 128, 128, 0.16);
                }
                .diff-column-headers {
                    background-color: rgba(128, 128, 128, 0.05);
                    border-bottom: 1px solid rgba(128, 128, 128, 0.14);
                }
                .diff-col-title {
                    font-weight: 700;
                    font-size: 0.85em;
                    letter-spacing: 0.4px;
                }
                .diff-hunk-header {
                    background-color: rgba(56, 139, 253, 0.08);
                    border-top: 1px solid rgba(128, 128, 128, 0.12);
                    border-bottom: 1px solid rgba(128, 128, 128, 0.12);
                    padding: 3px 10px;
                }
                .diff-col-left {
                    border-right: 1px solid rgba(128, 128, 128, 0.18);
                }
                .diff-gutter-sep {
                    opacity: 0.35;
                }
                .diff-gutter-label {
                    font-size: 1em;
                    font-variant-numeric: tabular-nums;
                }
                .diff-code-label {
                    font-size: 1em;
                    font-variant-numeric: tabular-nums;
                }
                .diff-badge {
                    font-size: 0.70em;
                    font-weight: 700;
                    padding: 2px 7px;
                    border-radius: 99px;
                    letter-spacing: 0.4px;
                }
                .diff-badge-modified {
                    background-color: rgba(56, 139, 253, 0.18);
                    color: #58a6ff;
                }
                .diff-badge-created {
                    background-color: rgba(46, 194, 126, 0.20);
                    color: #3fb950;
                }
                .diff-badge-binary {
                    background-color: rgba(128, 128, 128, 0.18);
                }
                .diff-stat-add {
                    color: #2ec27e;
                    font-weight: 700;
                }
                .diff-stat-del {
                    color: #e01b24;
                    font-weight: 700;
                }
                .diff-ratio-block {
                    border-radius: 2px;
                }
                .diff-ratio-add {
                    background-color: #2ec27e;
                }
                .diff-ratio-del {
                    background-color: #e01b24;
                }
                .diff-ratio-neutral {
                    background-color: rgba(128, 128, 128, 0.28);
                }
                .diff-header-pill {
                    font-size: 0.78em;
                    font-weight: 600;
                    padding: 2px 8px;
                    border-radius: 6px;
                    background-color: rgba(128, 128, 128, 0.14);
                }
                .diff-slide-drawer {
                    background-color: @card_bg_color;
                    border-left: 2px solid @accent_bg_color;
                    box-shadow: -6px 0 18px rgba(0, 0, 0, 0.35);
                }
                .diff-drawer-header {
                    padding: 2px 10px;
                    background-color: rgba(128, 128, 128, 0.10);
                    border-bottom: 1px solid rgba(128, 128, 128, 0.18);
                }
                .diff-drawer-close {
                    min-height: 20px;
                    min-width: 20px;
                    padding: 0;
                }
                .diff-truncation-banner {
                    padding: 4px 12px;
                    border-top: 1px dashed rgba(128, 128, 128, 0.22);
                }
            ";
            let provider = gtk::CssProvider::new();
            provider.load_from_string(css);
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}
