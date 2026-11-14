use crate::renderer::diff::markup::DiffRowTone;
use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gdk, glib, graphene, pango};
use std::cell::{Cell, RefCell};
use std::sync::Arc;

const V_PADDING: i32 = 6;

/// Contiguous vertical run of identical non-Context diff rows, coalescing adjacent
/// additions/deletions/fillers into a single GSK color rectangle + accent bar.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CoalescedToneRun {
    tone: DiffRowTone,
    y: f32,
    h: f32,
}

fn push_coalesced_run(runs: &mut Vec<CoalescedToneRun>, tone: DiffRowTone, y: f32, h: f32) {
    if tone == DiffRowTone::Context {
        return;
    }
    if let Some(last) = runs.last_mut()
        && last.tone == tone
        && (y - (last.y + last.h)).abs() <= 1.5
    {
        last.h = (y + h) - last.y;
        return;
    }
    runs.push(CoalescedToneRun { tone, y, h });
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct DiffHunkCanvas {
        pub(super) tones: RefCell<Arc<[DiffRowTone]>>,
        pub(super) code_label: RefCell<Option<gtk::Label>>,
        pub(super) is_dark: Cell<bool>,
        pub(super) cached_geom: Cell<(i32, i32)>,
        pub(super) cached_runs: RefCell<Vec<CoalescedToneRun>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DiffHunkCanvas {
        const NAME: &'static str = "BoxxyViewerDiffHunkCanvas";
        type Type = super::DiffHunkCanvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for DiffHunkCanvas {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for DiffHunkCanvas {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().first_child() else {
                return (0, 0, -1, -1);
            };
            match orientation {
                gtk::Orientation::Horizontal => {
                    let (min_w, nat_w, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
                    (min_w.min(120), nat_w.max(160), -1, -1)
                }
                gtk::Orientation::Vertical => {
                    let target_w = if for_size < 0 { 600 } else { for_size.max(120) };
                    let (min_h, nat_h, _, _) =
                        child.measure(gtk::Orientation::Vertical, target_w);
                    let full_h = nat_h.max(min_h);
                    (full_h, full_h, -1, -1)
                }
                _ => child.measure(orientation, for_size),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().first_child() {
                let _ = child.measure(gtk::Orientation::Vertical, width);
                child.allocate(width, height, baseline, None);
            }
        }

        #[allow(clippy::cast_precision_loss)]
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let w_i32 = self.obj().width();
            let h_i32 = self.obj().height();
            let tones = self.tones.borrow();

            if w_i32 > 0 && h_i32 > 0 && !tones.is_empty() {
                if self.cached_geom.get() != (w_i32, h_i32) {
                    let mut runs = self.cached_runs.borrow_mut();
                    runs.clear();

                    let top_pad = V_PADDING as f32;
                    let height = h_i32 as f32;
                    let n = tones.len();

                    if let Some(ref lbl) = *self.code_label.borrow() {
                        let layout = lbl.layout();
                        let mut iter = layout.iter();
                        let mut has_line = true;

                        for (i, &tone) in tones.iter().enumerate() {
                            let (y, h) = if has_line {
                                let (y0, y1) = iter.line_yrange();
                                let ly = top_pad + (y0 as f32 / pango::SCALE as f32);
                                let lh = ((y1 - y0) as f32 / pango::SCALE as f32).max(1.0);
                                has_line = iter.next_line();
                                (ly, lh)
                            } else {
                                let avail_h = (height - 2.0 * top_pad).max(16.0);
                                let line_h = avail_h / (n as f32);
                                (top_pad + (i as f32) * line_h, line_h)
                            };
                            push_coalesced_run(&mut runs, tone, y, h);
                        }
                    } else {
                        let avail_h = (height - 2.0 * top_pad).max(16.0);
                        let line_h = avail_h / (n as f32);
                        for (i, &tone) in tones.iter().enumerate() {
                            push_coalesced_run(
                                &mut runs,
                                tone,
                                top_pad + (i as f32) * line_h,
                                line_h,
                            );
                        }
                    }

                    self.cached_geom.set((w_i32, h_i32));
                }

                let width = w_i32 as f32;
                let is_dark = self.is_dark.get();
                let (del_bg, del_bar, ins_bg, ins_bar, empty_bg) = if is_dark {
                    (
                        gdk::RGBA::new(0.878, 0.106, 0.141, 0.14),
                        gdk::RGBA::new(0.878, 0.106, 0.141, 0.88),
                        gdk::RGBA::new(0.180, 0.761, 0.494, 0.14),
                        gdk::RGBA::new(0.180, 0.761, 0.494, 0.88),
                        gdk::RGBA::new(0.50, 0.52, 0.56, 0.07),
                    )
                } else {
                    (
                        gdk::RGBA::new(0.812, 0.133, 0.180, 0.11),
                        gdk::RGBA::new(0.812, 0.133, 0.180, 0.85),
                        gdk::RGBA::new(0.102, 0.498, 0.216, 0.11),
                        gdk::RGBA::new(0.102, 0.498, 0.216, 0.85),
                        gdk::RGBA::new(0.45, 0.48, 0.52, 0.06),
                    )
                };

                for run in self.cached_runs.borrow().iter().copied() {
                    match run.tone {
                        DiffRowTone::Delete => {
                            snapshot.append_color(
                                &del_bg,
                                &graphene::Rect::new(0.0, run.y, width, run.h),
                            );
                            snapshot.append_color(
                                &del_bar,
                                &graphene::Rect::new(0.0, run.y, 3.0, run.h),
                            );
                        }
                        DiffRowTone::Insert => {
                            snapshot.append_color(
                                &ins_bg,
                                &graphene::Rect::new(0.0, run.y, width, run.h),
                            );
                            snapshot.append_color(
                                &ins_bar,
                                &graphene::Rect::new(0.0, run.y, 3.0, run.h),
                            );
                        }
                        DiffRowTone::Empty => {
                            snapshot.append_color(
                                &empty_bg,
                                &graphene::Rect::new(0.0, run.y, width, run.h),
                            );
                        }
                        DiffRowTone::Context => {}
                    }
                }
            }

            self.parent_snapshot(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct DiffHunkCanvas(ObjectSubclass<imp::DiffHunkCanvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl DiffHunkCanvas {
    #[must_use]
    pub fn new(
        gutter_markup: &str,
        code_markup: &str,
        tones: Arc<[DiffRowTone]>,
        is_dark: bool,
    ) -> Self {
        let obj: Self = glib::Object::new();
        obj.add_css_class("diff-hunk-canvas");
        obj.set_hexpand(true);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hbox.set_hexpand(true);

        let gutter_lbl = gtk::Label::new(None);
        gutter_lbl.set_use_markup(true);
        gutter_lbl.set_selectable(false);
        gutter_lbl.set_xalign(1.0);
        gutter_lbl.set_yalign(0.0);
        gutter_lbl.set_valign(gtk::Align::Start);
        gutter_lbl.set_margin_start(8);
        gutter_lbl.set_margin_end(8);
        gutter_lbl.set_margin_top(V_PADDING);
        gutter_lbl.set_margin_bottom(V_PADDING);
        gutter_lbl.add_css_class("diff-gutter-label");
        gutter_lbl.set_markup(gutter_markup);
        hbox.append(&gutter_lbl);

        let sep = gtk::Separator::new(gtk::Orientation::Vertical);
        sep.add_css_class("diff-gutter-sep");
        hbox.append(&sep);

        let code_lbl = gtk::Label::new(None);
        code_lbl.set_use_markup(true);
        code_lbl.set_selectable(true);
        code_lbl.set_wrap(false);
        code_lbl.set_xalign(0.0);
        code_lbl.set_yalign(0.0);
        code_lbl.set_valign(gtk::Align::Start);
        code_lbl.set_hexpand(true);
        code_lbl.set_margin_start(10);
        code_lbl.set_margin_end(12);
        code_lbl.set_margin_top(V_PADDING);
        code_lbl.set_margin_bottom(V_PADDING);
        code_lbl.add_css_class("diff-code-label");
        code_lbl.set_markup(code_markup);

        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .child(&code_lbl)
            .build();
        hbox.append(&scroll);

        hbox.set_parent(&obj);

        let imp = obj.imp();
        *imp.tones.borrow_mut() = tones;
        *imp.code_label.borrow_mut() = Some(code_lbl);
        imp.is_dark.set(is_dark);

        obj
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_adjacent_same_tone_rows_into_single_run() {
        let mut runs = Vec::new();
        push_coalesced_run(&mut runs, DiffRowTone::Context, 0.0, 20.0);
        push_coalesced_run(&mut runs, DiffRowTone::Insert, 20.0, 20.0);
        push_coalesced_run(&mut runs, DiffRowTone::Insert, 40.0, 20.0);
        push_coalesced_run(&mut runs, DiffRowTone::Insert, 60.0, 20.0);
        push_coalesced_run(&mut runs, DiffRowTone::Delete, 80.0, 20.0);

        assert_eq!(runs.len(), 2);
        assert_eq!(
            runs[0],
            CoalescedToneRun {
                tone: DiffRowTone::Insert,
                y: 20.0,
                h: 60.0
            }
        );
        assert_eq!(
            runs[1],
            CoalescedToneRun {
                tone: DiffRowTone::Delete,
                y: 80.0,
                h: 20.0
            }
        );
    }
}
