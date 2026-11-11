//! Custom `StructuredViewer` block renderer for `list_processes` tool payloads.

use boxxy_viewer::{BlockRenderer, ContentBlock, ViewerRegistry};
use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::OnceCell;
use std::rc::Rc;

/// Renders the `list_processes` custom block into a compact columnar list.
/// Two payload schemas are accepted — the object form (new tool output) and
/// the tuple form (legacy persisted rows) — so sessions restored from the DB
/// don't silently fail to render.
pub struct ProcessListRenderer;

impl BlockRenderer for ProcessListRenderer {
    fn can_render(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Custom { schema, .. } if schema == "list_processes")
    }

    fn render(&self, block: &ContentBlock, _registry: &ViewerRegistry) -> gtk::Widget {
        #[derive(serde::Deserialize)]
        struct ProcessListOutput {
            processes: Vec<boxxy_claw_protocol::ProcessSample>,
        }

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);

        let ContentBlock::Custom { raw_payload, .. } = block else {
            return vbox.upcast();
        };

        let list_box = gtk::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk::SelectionMode::None);

        #[allow(clippy::too_many_arguments)]
        let append_row = |list_box: &gtk::ListBox,
                          pid: u32,
                          name: &str,
                          cpu: f64,
                          mem: u64,
                          read: u64,
                          write: u64,
                          gpu_usage: Option<f32>,
                          gpu_mem: Option<u64>| {
            let row = gtk::ListBoxRow::new();
            let item_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            item_box.set_margin_top(4);
            item_box.set_margin_bottom(4);
            item_box.set_margin_start(4);
            item_box.set_margin_end(4);

            let pid_lbl = gtk::Label::new(Some(&format!("{pid}")));
            pid_lbl.set_width_chars(6);
            item_box.append(&pid_lbl);

            let name_lbl = gtk::Label::new(Some(name));
            name_lbl.set_hexpand(true);
            name_lbl.set_halign(gtk::Align::Start);
            name_lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
            item_box.append(&name_lbl);

            // Show GPU metrics only when active for this process
            if gpu_usage.is_some_and(|u| u > 0.0) || gpu_mem.is_some_and(|m| m > 0) {
                let gpu_str = match (gpu_usage.filter(|u| *u > 0.0), gpu_mem.filter(|m| *m > 0)) {
                    (Some(u), Some(m)) => format!("GPU:{u:.0}% {}MB", m / (1024 * 1024)),
                    (Some(u), None) => format!("GPU:{u:.0}%"),
                    (None, Some(m)) => format!("VRAM:{}MB", m / (1024 * 1024)),
                    (None, None) => String::new(),
                };
                if !gpu_str.is_empty() {
                    let gpu_lbl = gtk::Label::new(Some(&gpu_str));
                    gpu_lbl.add_css_class("caption");
                    gpu_lbl.add_css_class("dim-label");
                    gpu_lbl.set_margin_end(6);
                    item_box.append(&gpu_lbl);
                }
            }

            // Only show disk I/O when there's actual activity; quiet
            // processes get a cleaner row.
            if read > 0 || write > 0 {
                let io_str = format!("R:{}MB W:{}MB", read / 1_048_576, write / 1_048_576);
                let io_lbl = gtk::Label::new(Some(&io_str));
                io_lbl.add_css_class("caption");
                io_lbl.add_css_class("dim-label");
                io_lbl.set_margin_end(6);
                item_box.append(&io_lbl);
            }

            let cpu_lbl = gtk::Label::new(Some(&format!("{cpu:.1}%")));
            cpu_lbl.add_css_class("caption");
            cpu_lbl.add_css_class("dim-label");
            item_box.append(&cpu_lbl);

            let mem_lbl = gtk::Label::new(Some(&format!("{}MB", mem / (1024 * 1024))));
            mem_lbl.add_css_class("caption");
            mem_lbl.add_css_class("dim-label");
            item_box.append(&mem_lbl);

            row.set_child(Some(&item_box));
            list_box.append(&row);
        };

        if let Ok(output) = serde_json::from_str::<ProcessListOutput>(raw_payload) {
            for info in output.processes {
                append_row(
                    &list_box,
                    info.pid,
                    &info.name,
                    info.cpu_usage,
                    info.memory_bytes,
                    info.read_bytes,
                    info.written_bytes,
                    info.gpu_usage,
                    info.gpu_memory_bytes,
                );
            }
            vbox.append(&list_box);
        } else if let Ok(processes) =
            serde_json::from_str::<Vec<(u32, String, f64, u64, u64, u64)>>(raw_payload)
        {
            // Legacy tuple schema — keep parsing so old persisted sessions
            // restore without a "failed to parse" error.
            for (pid, name, cpu, mem, read, write) in processes {
                append_row(&list_box, pid, &name, cpu, mem, read, write, None, None);
            }
            vbox.append(&list_box);
        } else {
            let error_lbl = gtk::Label::new(Some("Failed to parse process list."));
            error_lbl.add_css_class("error");
            vbox.append(&error_lbl);
        }

        vbox.upcast()
    }
}

/// Returns the shared registry used by every Claw `StructuredViewer`:
/// the standard text/code/image renderers plus the custom `list_processes`
/// renderer. Cached per UI thread so recycled `ListView` rows share one `Rc`.
#[must_use]
pub fn get_claw_viewer_registry() -> Rc<ViewerRegistry> {
    thread_local! {
        static REGISTRY: OnceCell<Rc<ViewerRegistry>> = const { OnceCell::new() };
    }

    REGISTRY.with(|cell| {
        cell.get_or_init(|| {
            let mut registry = ViewerRegistry::new_with_defaults();
            registry.register(Box::new(ProcessListRenderer));
            Rc::new(registry)
        })
        .clone()
    })
}
