use crate::renderer::diff::canvas::DiffHunkCanvas;
use crate::renderer::diff::markup::{DiffPayloadKind, PreparedDiff, PreparedHunk};
use gtk4 as gtk;
use gtk4::prelude::*;

/// Builds the two-column side-by-side view (`Original` on Left 50%, `Changes` on Right 50%).
#[must_use]
pub fn build_split_view(prepared: &PreparedDiff, is_dark: bool) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_hexpand(true);

    // Column titles bar: ORIGINAL | CHANGES
    let col_headers = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    col_headers.add_css_class("diff-column-headers");
    col_headers.set_homogeneous(true);

    let left_hdr = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    left_hdr.set_margin_start(12);
    left_hdr.set_margin_end(12);
    left_hdr.set_margin_top(5);
    left_hdr.set_margin_bottom(5);
    let left_dot = gtk::Label::new(Some("−"));
    left_dot.add_css_class("diff-stat-del");
    let left_title = gtk::Label::new(Some("ORIGINAL"));
    left_title.add_css_class("dim-label");
    left_title.add_css_class("diff-col-title");
    left_hdr.append(&left_dot);
    left_hdr.append(&left_title);

    let right_hdr = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    right_hdr.set_margin_start(12);
    right_hdr.set_margin_end(12);
    right_hdr.set_margin_top(5);
    right_hdr.set_margin_bottom(5);
    let right_dot = gtk::Label::new(Some("+"));
    right_dot.add_css_class("diff-stat-add");
    let right_title = gtk::Label::new(Some("CHANGES"));
    right_title.add_css_class("dim-label");
    right_title.add_css_class("diff-col-title");
    right_hdr.append(&right_dot);
    right_hdr.append(&right_title);

    col_headers.append(&left_hdr);
    col_headers.append(&right_hdr);
    root.append(&col_headers);

    for hunk in &prepared.hunks {
        root.append(&build_hunk_header_bar(&hunk.header));

        let pair_grid = gtk::Grid::new();
        pair_grid.set_column_homogeneous(true);
        pair_grid.set_hexpand(true);
        pair_grid.add_css_class("diff-split-grid");

        let left_canvas = DiffHunkCanvas::new(
            &hunk.left_gutter_markup,
            &hunk.left_code_markup,
            hunk.left_tones.clone(),
            is_dark,
        );
        left_canvas.add_css_class("diff-col-left");

        let right_canvas = DiffHunkCanvas::new(
            &hunk.right_gutter_markup,
            &hunk.right_code_markup,
            hunk.right_tones.clone(),
            is_dark,
        );
        right_canvas.add_css_class("diff-col-right");

        pair_grid.attach(&left_canvas, 0, 0, 1, 1);
        pair_grid.attach(&right_canvas, 1, 0, 1, 1);
        root.append(&pair_grid);
    }

    if prepared.stats.truncated && prepared.stats.omitted_lines > 0 {
        root.append(&build_truncation_banner(prepared.stats.omitted_lines));
    }

    root
}

/// Builds the responsive Drawer view for narrow/non-maximized windows and split panes.
///
/// The primary layer displays the `Changes` (or unified) view at 100% width for clean
/// readability, while a slide-over `GtkRevealer` drawer inside a `GtkOverlay` allows the
/// user to slide out the `Original` column (or `Changes` column) on demand.
#[must_use]
pub fn build_drawer_view(
    prepared: &PreparedDiff,
    is_dark: bool,
) -> (gtk::Overlay, gtk::Revealer, gtk::Box) {
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);

    // Base surface (100% width): Unified/Changes view so all additions and removals are clear
    let base_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    base_box.set_hexpand(true);

    for hunk in &prepared.hunks {
        base_box.append(&build_hunk_header_bar(&hunk.header));
        let canvas = if prepared.kind == DiffPayloadKind::Created {
            DiffHunkCanvas::new(
                &hunk.right_gutter_markup,
                &hunk.right_code_markup,
                hunk.right_tones.clone(),
                is_dark,
            )
        } else {
            DiffHunkCanvas::new(
                &hunk.unified_gutter_markup,
                &hunk.unified_code_markup,
                hunk.unified_tones.clone(),
                is_dark,
            )
        };
        base_box.append(&canvas);
    }

    if prepared.stats.truncated && prepared.stats.omitted_lines > 0 {
        base_box.append(&build_truncation_banner(prepared.stats.omitted_lines));
    }

    overlay.set_child(Some(&base_box));

    // Slide-over drawer holding the isolated ORIGINAL column for side-by-side comparison
    // inside narrow panes without cramping the main view.
    let drawer_revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideLeft)
        .transition_duration(200)
        .reveal_child(false)
        .visible(false)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Fill)
        .can_target(false)
        .build();

    let drawer_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    drawer_panel.add_css_class("diff-slide-drawer");
    drawer_panel.set_hexpand(true);

    drawer_revealer.connect_reveal_child_notify(move |rev| {
        let revealed = rev.reveals_child();
        rev.set_can_target(revealed);
        if !revealed {
            rev.set_visible(false);
        }
    });

    for (idx, hunk) in prepared.hunks.iter().enumerate() {
        if idx == 0 {
            // For the first hunk, combine the drawer title ("ORIGINAL") and hunk header into a
            // single top bar so lines in the drawer align horizontally with the base view and
            // never get pushed down off the bottom edge.
            let drawer_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            drawer_header.add_css_class("diff-drawer-header");

            let drawer_title = gtk::Label::new(Some(&format!("ORIGINAL  {}", hunk.header)));
            drawer_title.add_css_class("diff-col-title");
            drawer_title.add_css_class("dim-label");
            drawer_title.set_hexpand(true);
            drawer_title.set_halign(gtk::Align::Start);
            drawer_header.append(&drawer_title);

            let close_btn = gtk::Button::builder()
                .icon_name("boxxy-cross-small-symbolic")
                .tooltip_text("Close Original Drawer")
                .css_classes(["flat", "circular", "diff-drawer-close"])
                .valign(gtk::Align::Center)
                .build();
            let rev_for_close = drawer_revealer.clone();
            close_btn.connect_clicked(move |_| {
                rev_for_close.set_reveal_child(false);
            });
            drawer_header.append(&close_btn);
            drawer_panel.append(&drawer_header);
        } else {
            drawer_panel.append(&build_hunk_header_bar(&hunk.header));
        }

        let orig_canvas = DiffHunkCanvas::new(
            &hunk.left_gutter_markup,
            &hunk.left_code_markup,
            hunk.left_tones.clone(),
            is_dark,
        );
        drawer_panel.append(&orig_canvas);
    }

    drawer_revealer.set_child(Some(&drawer_panel));
    overlay.add_overlay(&drawer_revealer);
    overlay.set_measure_overlay(&drawer_revealer, true);

    (overlay, drawer_revealer, drawer_panel)
}

/// Builds a clean single-column all-additions view for brand-new files (`DiffPayloadKind::Created`).
#[must_use]
pub fn build_created_view(hunks: &[PreparedHunk], omitted_lines: usize, is_dark: bool) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_hexpand(true);

    for hunk in hunks {
        let canvas = DiffHunkCanvas::new(
            &hunk.right_gutter_markup,
            &hunk.right_code_markup,
            hunk.right_tones.clone(),
            is_dark,
        );
        root.append(&canvas);
    }

    if omitted_lines > 0 {
        root.append(&build_truncation_banner(omitted_lines));
    }

    root
}

fn build_hunk_header_bar(header_text: &str) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bar.add_css_class("diff-hunk-header");

    let icon_lbl = gtk::Label::new(Some("⋯"));
    icon_lbl.add_css_class("dim-label");
    bar.append(&icon_lbl);

    let lbl = gtk::Label::new(Some(header_text));
    lbl.add_css_class("dim-label");
    lbl.set_halign(gtk::Align::Start);
    lbl.set_hexpand(true);
    bar.append(&lbl);

    bar
}

fn build_truncation_banner(omitted_lines: usize) -> gtk::Box {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bar.add_css_class("diff-truncation-banner");
    bar.set_halign(gtk::Align::Center);
    bar.set_margin_top(6);
    bar.set_margin_bottom(6);

    let lbl = gtk::Label::new(Some(&format!(
        "⋯ {omitted_lines} additional lines omitted for responsiveness ⋯"
    )));
    lbl.add_css_class("caption");
    lbl.add_css_class("dim-label");
    bar.append(&lbl);

    bar
}
