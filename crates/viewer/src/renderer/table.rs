use crate::parser::blocks::{ContentBlock, TableAlignment};
use crate::renderer::BlockRenderer;
use crate::renderer::text::connect_label_links;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::{gdk, pango};

pub struct TableRenderer;

impl BlockRenderer for TableRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Table { .. })
    }

    fn render(
        &self,
        block: &ContentBlock,
        _registry: &crate::registry::ViewerRegistry,
    ) -> gtk::Widget {
        let ContentBlock::Table {
            alignments,
            headers,
            rows,
        } = block
        else {
            unreachable!()
        };

        let frame = gtk::Frame::new(None);
        frame.add_css_class("view");
        frame.add_css_class("viewer-table-frame");
        frame.set_overflow(gtk::Overflow::Hidden);
        frame.set_margin_bottom(12);
        frame.set_hexpand(true);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox.set_hexpand(true);

        // Top toolbar with row count + Copy TSV button
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("code-header");
        toolbar.add_css_class("viewer-table-toolbar");

        let count_text = if rows.len() == 1 {
            "1 row".to_string()
        } else {
            format!("{} rows", rows.len())
        };
        let info_label = gtk::Label::new(Some(&count_text));
        info_label.add_css_class("dim-label");
        info_label.add_css_class("caption");
        info_label.set_hexpand(true);
        info_label.set_halign(gtk::Align::Start);
        info_label.set_margin_start(4);
        toolbar.append(&info_label);

        let copy_btn = gtk::Button::builder()
            .icon_name("boxxy-edit-copy-symbolic")
            .css_classes(["flat", "circular"])
            .tooltip_text("Copy table to clipboard")
            .valign(gtk::Align::Center)
            .build();

        let tsv_payload = format_table_tsv(headers, rows);
        copy_btn.connect_clicked(move |btn| {
            if let Some(display) = gdk::Display::default() {
                display.clipboard().set_text(&tsv_payload);
                btn.set_icon_name("boxxy-object-select-symbolic");
                let b = btn.clone();
                gtk::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(1500),
                    move || {
                        b.set_icon_name("boxxy-edit-copy-symbolic");
                    },
                );
            }
        });
        toolbar.append(&copy_btn);
        vbox.append(&toolbar);

        // Determine total column count across headers and body rows
        let num_cols = headers
            .len()
            .max(rows.iter().map(Vec::len).max().unwrap_or(0));

        let grid = gtk::Grid::new();
        grid.add_css_class("viewer-table-grid");
        grid.set_column_spacing(0);
        grid.set_row_spacing(0);
        grid.set_hexpand(true);

        let mut current_grid_row = 0_i32;

        if !headers.is_empty() {
            for col_idx in 0..num_cols {
                let markup = headers.get(col_idx).map_or("", String::as_str);
                let align = alignments
                    .get(col_idx)
                    .copied()
                    .unwrap_or(TableAlignment::Left);
                let cell = build_table_cell(markup, align, true, false);
                #[allow(clippy::cast_possible_wrap)]
                grid.attach(&cell, col_idx as i32, current_grid_row, 1, 1);
            }
            current_grid_row += 1;
        }

        for (row_idx, row_cells) in rows.iter().enumerate() {
            let is_odd = row_idx % 2 == 1;
            for col_idx in 0..num_cols {
                let markup = row_cells.get(col_idx).map_or("", String::as_str);
                let align = alignments
                    .get(col_idx)
                    .copied()
                    .unwrap_or(TableAlignment::Left);
                let cell = build_table_cell(markup, align, false, is_odd);
                #[allow(clippy::cast_possible_wrap)]
                grid.attach(&cell, col_idx as i32, current_grid_row, 1, 1);
            }
            current_grid_row += 1;
        }

        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .child(&grid)
            .build();

        vbox.append(&scroll);
        frame.set_child(Some(&vbox));
        frame.upcast()
    }
}

fn build_table_cell(
    markup: &str,
    align: TableAlignment,
    is_header: bool,
    is_odd_row: bool,
) -> gtk::Box {
    let cell_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    cell_box.add_css_class("viewer-table-cell");
    if is_header {
        cell_box.add_css_class("viewer-table-header-cell");
    } else if is_odd_row {
        cell_box.add_css_class("viewer-table-row-odd");
    } else {
        cell_box.add_css_class("viewer-table-row-even");
    }

    let (xalign, halign) = match align {
        TableAlignment::None | TableAlignment::Left => (0.0, gtk::Align::Fill),
        TableAlignment::Center => (0.5, gtk::Align::Fill),
        TableAlignment::Right => (1.0, gtk::Align::Fill),
    };

    let label = gtk::Label::new(None);
    label.set_use_markup(true);
    label.set_selectable(true);
    label.set_xalign(xalign);
    label.set_yalign(0.5);
    label.set_halign(halign);
    label.set_hexpand(true);
    label.set_margin_start(12);
    label.set_margin_end(12);
    label.set_margin_top(7);
    label.set_margin_bottom(7);

    // Wrap long prose cells (like window titles or descriptions) while keeping
    // compact cells (IDs, states, numbers) single-line.
    let visible_len = visible_char_count(markup);
    if !is_header && visible_len > 36 {
        label.set_wrap(true);
        label.set_wrap_mode(pango::WrapMode::WordChar);
        label.set_max_width_chars(46);
    }

    if is_header {
        label.set_markup(&format!("<b>{markup}</b>"));
    } else {
        label.set_markup(markup);
    }

    connect_label_links(&label);
    cell_box.append(&label);
    cell_box
}

fn visible_char_count(markup: &str) -> usize {
    let mut count = 0;
    let mut in_tag = false;
    for ch in markup.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => count += 1,
            _ => {}
        }
    }
    count
}

fn strip_tags_for_copy(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len());
    let mut in_tag = false;
    for ch in markup.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn format_table_tsv(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut lines = Vec::with_capacity(1 + rows.len());
    if !headers.is_empty() {
        let h: Vec<String> = headers.iter().map(|c| strip_tags_for_copy(c)).collect();
        lines.push(h.join("\t"));
    }
    for row in rows {
        let r: Vec<String> = row.iter().map(|c| strip_tags_for_copy(c)).collect();
        lines.push(r.join("\t"));
    }
    lines.join("\n")
}
