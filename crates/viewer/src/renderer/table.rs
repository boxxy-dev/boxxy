use crate::parser::blocks::{ContentBlock, TableAlignment, TableCell};
use crate::parser::markdown::strip_pango_markup;
use crate::renderer::image::build_table_image_card;
use crate::renderer::text::connect_label_links;
use crate::renderer::{BlockRenderer, RenderContext};
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, pango};

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TableContainer;

    #[glib::object_subclass]
    impl ObjectSubclass for TableContainer {
        const NAME: &'static str = "BoxxyViewerTableContainer";
        type Type = super::TableContainer;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for TableContainer {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for TableContainer {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().first_child() else {
                return (0, 0, -1, -1);
            };

            let (min_w, nat_w, _, _) = child.measure(gtk::Orientation::Horizontal, -1);

            match orientation {
                gtk::Orientation::Horizontal => {
                    let clamped_min = min_w.min(180);
                    let clamped_nat = nat_w.max(clamped_min);
                    (clamped_min, clamped_nat, -1, -1)
                }
                gtk::Orientation::Vertical => {
                    if for_size < 0 {
                        // Unconstrained vertical query (used by GtkBox(Horizontal) to determine
                        // minimum height): measure at unwrapped width (like GtkLabel does) rather
                        // than GtkGrid's 1-character minimum width, preventing vertical height
                        // inflation and blank gaps below tables at any window width.
                        let unconstrained_w = nat_w.max(4096);
                        let (min_h, _, _, _) =
                            child.measure(gtk::Orientation::Vertical, unconstrained_w);
                        let (_, nat_h, _, _) =
                            child.measure(gtk::Orientation::Vertical, nat_w.max(720));
                        (min_h, nat_h.max(min_h), -1, -1)
                    } else {
                        let target_w = for_size.max(min_w.min(240));
                        let (min_h, nat_h, _, _) =
                            child.measure(gtk::Orientation::Vertical, target_w);
                        (min_h, nat_h.max(min_h), -1, -1)
                    }
                }
                _ => (min_w, nat_w, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                let _ = child.measure(gtk::Orientation::Vertical, width);
                child.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct TableContainer(ObjectSubclass<imp::TableContainer>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TableContainer {
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let obj: Self = glib::Object::new();
        child.set_parent(&obj);
        obj
    }
}

pub struct TableRenderer;

impl BlockRenderer for TableRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Table { .. })
    }

    fn render(
        &self,
        block: &ContentBlock,
        registry: &crate::registry::ViewerRegistry,
    ) -> gtk::Widget {
        self.render_with_context(block, registry, &RenderContext::default())
    }

    fn render_with_context(
        &self,
        block: &ContentBlock,
        _registry: &crate::registry::ViewerRegistry,
        ctx: &RenderContext,
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

        // Defer TSV formatting until the user actually clicks Copy so streaming tables
        // and scrolled-past history tables skip TSV string work completely on render.
        let headers_for_copy: std::rc::Rc<[String]> = headers.as_slice().into();
        let rows_for_copy: std::rc::Rc<[Vec<TableCell>]> = rows.as_slice().into();
        let cached_tsv: std::rc::Rc<std::cell::OnceCell<String>> =
            std::rc::Rc::new(std::cell::OnceCell::new());
        copy_btn.connect_clicked(move |btn| {
            if let Some(display) = gdk::Display::default() {
                let tsv = cached_tsv
                    .get_or_init(|| format_table_tsv(&headers_for_copy, &rows_for_copy));
                display.clipboard().set_text(tsv);
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

        // Identify columns that only contain images/media so prose columns get horizontal expansion
        let col_is_media_only: Vec<bool> = (0..num_cols)
            .map(|col_idx| {
                let mut has_media = false;
                for row in rows {
                    if let Some(cell) = row.get(col_idx) {
                        if !cell.markup.is_empty() {
                            return false;
                        }
                        if !cell.images.is_empty() {
                            has_media = true;
                        }
                    }
                }
                has_media
            })
            .collect();

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
                let header_cell = TableCell::new(markup.to_string(), Vec::new());
                let hexpand = !col_is_media_only.get(col_idx).copied().unwrap_or(false);
                let cell = build_table_cell(&header_cell, align, true, false, hexpand, ctx);
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                grid.attach(&cell, col_idx as i32, current_grid_row, 1, 1);
            }
            current_grid_row += 1;
        }

        let empty_cell = TableCell::default();
        for (row_idx, row_cells) in rows.iter().enumerate() {
            let is_odd = row_idx % 2 == 1;
            for col_idx in 0..num_cols {
                let cell_data = row_cells.get(col_idx).unwrap_or(&empty_cell);
                let align = alignments
                    .get(col_idx)
                    .copied()
                    .unwrap_or(TableAlignment::Left);
                let hexpand = !col_is_media_only.get(col_idx).copied().unwrap_or(false);
                let cell = build_table_cell(cell_data, align, false, is_odd, hexpand, ctx);
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                grid.attach(&cell, col_idx as i32, current_grid_row, 1, 1);
            }
            current_grid_row += 1;
        }

        let table_container = TableContainer::new(&grid);
        table_container.set_hexpand(true);

        vbox.append(&table_container);
        frame.set_child(Some(&vbox));
        frame.upcast()
    }
}

fn build_table_cell(
    cell: &TableCell,
    align: TableAlignment,
    is_header: bool,
    is_odd_row: bool,
    hexpand: bool,
    ctx: &RenderContext,
) -> gtk::Box {
    let cell_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    cell_box.add_css_class("viewer-table-cell");
    cell_box.set_hexpand(hexpand);
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

    if is_header || !cell.markup.is_empty() || cell.images.is_empty() {
        let markup = &cell.markup;
        let label = gtk::Label::new(None);
        label.set_use_markup(true);
        label.set_selectable(true);
        label.set_xalign(xalign);
        label.set_yalign(0.5);
        label.set_halign(halign);
        label.set_hexpand(hexpand);
        label.set_vexpand(cell.images.is_empty());
        label.set_margin_start(12);
        label.set_margin_end(12);
        label.set_margin_top(7);
        label.set_margin_bottom(if cell.images.is_empty() { 7 } else { 2 });

        // Wrap long prose cells or cells with explicit line breaks (`<br>`)
        let visible_len = visible_char_count(markup);
        if !is_header && (visible_len > 36 || markup.contains('\n')) {
            label.set_wrap(true);
            label.set_wrap_mode(pango::WrapMode::WordChar);
            label.set_max_width_chars(68);
        }

        if is_header {
            label.set_markup(&format!("<b>{markup}</b>"));
        } else {
            label.set_markup(markup);
        }

        connect_label_links(&label);
        cell_box.append(&label);
    }

    if !cell.images.is_empty() {
        let media_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        media_box.set_valign(gtk::Align::Center);
        media_box.set_vexpand(true);
        media_box.set_margin_start(10);
        media_box.set_margin_end(10);
        media_box.set_margin_top(if cell.markup.is_empty() { 8 } else { 2 });
        media_box.set_margin_bottom(8);

        let card_halign = match align {
            TableAlignment::Center => gtk::Align::Center,
            TableAlignment::Right => gtk::Align::End,
            TableAlignment::None | TableAlignment::Left => gtk::Align::Start,
        };
        media_box.set_halign(card_halign);

        for img in &cell.images {
            let card = build_table_image_card(img, ctx);
            media_box.append(&card);
        }
        cell_box.append(&media_box);
    }

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

fn format_table_tsv(headers: &[String], rows: &[Vec<TableCell>]) -> String {
    let approx_cap = (headers.len() + rows.len() * headers.len().max(2)) * 24;
    let mut out = String::with_capacity(approx_cap);

    if !headers.is_empty() {
        for (idx, col) in headers.iter().enumerate() {
            if idx > 0 {
                out.push('\t');
            }
            out.push_str(&strip_pango_markup(col, true));
        }
    }

    for row in rows {
        if !out.is_empty() || !headers.is_empty() {
            out.push('\n');
        }
        for (idx, cell) in row.iter().enumerate() {
            if idx > 0 {
                out.push('\t');
            }
            let text = strip_pango_markup(&cell.markup, true);
            if cell.images.is_empty() {
                out.push_str(&text);
            } else if text.is_empty() {
                for (img_idx, img) in cell.images.iter().enumerate() {
                    if img_idx > 0 {
                        out.push(' ');
                    }
                    out.push_str(&img.url);
                }
            } else {
                out.push_str(&text);
                out.push_str(" (");
                for (img_idx, img) in cell.images.iter().enumerate() {
                    if img_idx > 0 {
                        out.push(' ');
                    }
                    out.push_str(&img.url);
                }
                out.push(')');
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::markdown::parse_markdown;
    use crate::registry::ViewerRegistry;

    #[test]
    fn table_tsv_export_includes_text_and_image_urls() {
        let input = "\
| App | Screenshot |
| :--- | :--- |
| **Omnidict**<br>Offline dictionary | ![Omnidict](https://example.com/omnidict.png) |";
        let blocks = parse_markdown(input);
        let ContentBlock::Table { headers, rows, .. } = &blocks[0] else {
            panic!("Expected Table");
        };
        let tsv = format_table_tsv(headers, rows);
        assert_eq!(
            tsv,
            "App\tScreenshot\nOmnidict Offline dictionary\thttps://example.com/omnidict.png"
        );
    }

    #[test]
    fn table_inside_horizontal_row_does_not_inflate_vertical_height() {
        crate::run_gtk_test(|| {
            let input = "\
Querying Flathub index for latest application registrations.

| Application & Description | Details |
| :--- | :--- |
| **Omnidict** (`com.ktechpit.omnidict`)<br>Offline multilingual dictionary and thesaurus. Operates local indices without telemetry. | Dictionary |
| **DiagNG** (`com.p1security.diagng`)<br>Qualcomm baseband diagnostic monitor client. Bridges raw SPI/USB interfaces to capture cellular air traces into PCAP. | Network |
| **Kaori** (`io.github.riyaniii.Kaori`)<br>Manga and comic reader frontend. Native Libadwaita interface with gesture navigation. | Utility |
| **CalibRaw** (`io.github.Duecki1.CalibRaw`)<br>Non-destructive RAW image processor. GPU-accelerated pipeline with local AI masking modules. | Graphics |";

            let blocks = parse_markdown(input);
            let registry = ViewerRegistry::new_with_defaults();

            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            let left_col = gtk::Box::new(gtk::Orientation::Vertical, 0);
            left_col.set_size_request(42, -1);
            hbox.append(&left_col);

            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
            vbox.set_hexpand(true);
            hbox.append(&vbox);

            for block in &blocks {
                if let Some(w) = registry.render_block(block) {
                    vbox.append(&w);
                }
            }

            let (_, vbox_nat_h, _, _) = vbox.measure(gtk::Orientation::Vertical, 942);
            let (hbox_min_h, hbox_nat_h, _, _) = hbox.measure(gtk::Orientation::Vertical, 1000);

            assert_eq!(hbox_nat_h, vbox_nat_h);
            assert!(hbox_min_h <= hbox_nat_h);

            let rich_input = "\
Querying Flathub index for latest application registrations.

| Application & Description | Screenshot |
| :--- | :--- |
| **Omnidict** (`com.ktechpit.omnidict`)<br>Offline multilingual dictionary and thesaurus. Operates local indices without telemetry. | ![Omnidict](https://example.com/omnidict.png) |
| **DiagNG** (`com.p1security.diagng`)<br>Qualcomm baseband diagnostic monitor client. Bridges raw SPI/USB interfaces to capture cellular air traces into PCAP. | ![DiagNG](https://example.com/diagng.png) |

![shot-1](https://example.com/shot-1.png)
![shot-2](https://example.com/shot-2.png)
![shot-3](https://example.com/shot-3.png)";

            let rich_blocks = parse_markdown(rich_input);
            assert_eq!(
                rich_blocks.len(),
                3,
                "Expected [Paragraph, Table, ImageGallery]"
            );

            let hbox2 = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            let left_col2 = gtk::Box::new(gtk::Orientation::Vertical, 0);
            left_col2.set_size_request(42, -1);
            hbox2.append(&left_col2);

            let vbox2 = gtk::Box::new(gtk::Orientation::Vertical, 2);
            vbox2.set_hexpand(true);
            hbox2.append(&vbox2);

            for block in &rich_blocks {
                if let Some(w) = registry.render_block(block) {
                    vbox2.append(&w);
                }
            }

            let (_, vbox2_nat_h, _, _) = vbox2.measure(gtk::Orientation::Vertical, 942);
            let (hbox2_min_h, hbox2_nat_h, _, _) = hbox2.measure(gtk::Orientation::Vertical, 1000);

            assert_eq!(hbox2_nat_h, vbox2_nat_h);
            assert!(hbox2_min_h <= hbox2_nat_h);
        });
    }
}
