use crate::renderer::diff::canvas::DiffHunkCanvas;
use crate::renderer::diff::markup::{DiffPayloadKind, PreparedDiff, PreparedHunk};
use gtk4 as gtk;
use gtk4::prelude::*;

/// Builds the two-column side-by-side view (`Original` on Left 50%, `Changes` on Right 50%).
#[must_use]
pub fn build_split_view(prepared: &PreparedDiff, is_dark: bool) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_hexpand(true);

    for (idx, hunk) in prepared.hunks.iter().enumerate() {
        if idx > 0 {
            root.append(&build_hunk_divider());
        }

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

    for (idx, hunk) in prepared.hunks.iter().enumerate() {
        if idx > 0 {
            base_box.append(&build_hunk_divider());
        }
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
        if idx > 0 {
            drawer_panel.append(&build_hunk_divider());
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

fn build_hunk_divider() -> gtk::Separator {
    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.add_css_class("diff-hunk-divider");
    sep
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
