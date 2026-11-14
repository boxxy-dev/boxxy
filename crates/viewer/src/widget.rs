use crate::cache::get_or_parse_markdown;
use crate::parser::blocks::ContentBlock;
use crate::parser::markdown::parse_markdown;
use crate::registry::ViewerRegistry;
use crate::renderer::RenderContext;
use crate::renderer::code::CodeBlockWidget;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const POOL_MAX_PARAGRAPHS: usize = 16;
const POOL_MAX_HEADINGS: usize = 8;
const POOL_MAX_RULES: usize = 4;
const POOL_MAX_CODE_BLOCKS: usize = 8;

/// Per-viewer bounded recycling pool for common leaf/card GTK widgets.
/// Eliminates GObject allocation, CSS node creation, and controller setup during
/// `GtkListView` row recycling (`connect_unbind` -> `connect_bind`).
#[derive(Default)]
pub(crate) struct WidgetPool {
    paragraphs: Vec<gtk::Label>,
    headings: Vec<gtk::Label>,
    rules: Vec<gtk::Separator>,
    codes: Vec<CodeBlockWidget>,
}

impl WidgetPool {
    fn park(&mut self, widget: gtk::Widget) {
        widget.remove_css_class("streaming-active");

        if let Ok(code) = widget.clone().downcast::<CodeBlockWidget>() {
            if self.codes.len() < POOL_MAX_CODE_BLOCKS {
                code.reset_for_pool();
                self.codes.push(code);
            }
            return;
        }

        if widget.has_css_class("viewer-paragraph") {
            if self.paragraphs.len() < POOL_MAX_PARAGRAPHS
                && let Ok(lbl) = widget.downcast::<gtk::Label>()
            {
                lbl.select_region(0, 0);
                lbl.set_margin_bottom(8);
                self.paragraphs.push(lbl);
            }
            return;
        }

        if widget.has_css_class("viewer-heading") {
            if self.headings.len() < POOL_MAX_HEADINGS
                && let Ok(lbl) = widget.downcast::<gtk::Label>()
            {
                lbl.select_region(0, 0);
                lbl.set_margin_top(12);
                lbl.set_margin_bottom(8);
                self.headings.push(lbl);
            }
            return;
        }

        if widget.has_css_class("viewer-rule")
            && self.rules.len() < POOL_MAX_RULES
            && let Ok(sep) = widget.downcast::<gtk::Separator>()
        {
            sep.set_margin_top(12);
            sep.set_margin_bottom(12);
            self.rules.push(sep);
        }
    }

    fn take_and_update(
        &mut self,
        block: &ContentBlock,
        registry: &ViewerRegistry,
        ctx: &RenderContext,
    ) -> Option<gtk::Widget> {
        if !registry.uses_builtin_renderer(block) {
            return None;
        }
        let candidate: gtk::Widget = match block {
            ContentBlock::Paragraph(_) => self.paragraphs.pop()?.upcast(),
            ContentBlock::Heading { .. } => self.headings.pop()?.upcast(),
            ContentBlock::Rule => self.rules.pop()?.upcast(),
            ContentBlock::Code { .. } => self.codes.pop()?.upcast(),
            _ => return None,
        };
        if registry.try_update_block(block, &candidate, ctx) {
            Some(candidate)
        } else {
            None
        }
    }

    #[cfg(test)]
    fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.paragraphs.len(),
            self.headings.len(),
            self.rules.len(),
            self.codes.len(),
        )
    }
}

/// Internal state, wrapped in Rc to easily share with glib timeouts
pub(crate) struct ViewerState {
    pub(crate) container: gtk::Box,
    pub(crate) registry: Rc<ViewerRegistry>,

    // Main buffer holding all processed text in the current markdown segment
    pub(crate) buffer: RefCell<String>,
    // Rendered blocks for the current markdown segment, enabling incremental AST diffing
    // and in-place tail mutation.
    pub(crate) rendered_blocks: RefCell<Vec<(ContentBlock, gtk::Widget)>>,
    pub(crate) pool: RefCell<WidgetPool>,

    // Debouncing state
    pub(crate) pending_text: RefCell<String>,
    pub(crate) update_queued: Cell<bool>,

    // Generation counter to cancel async tasks when the viewer is cleared or recycled.
    pub(crate) generation: Arc<AtomicU64>,
}

impl ViewerState {
    fn drain_rendered_to_pool(&self, rendered: &mut Vec<(ContentBlock, gtk::Widget)>, from: usize) {
        if from >= rendered.len() {
            return;
        }
        let mut pool = self.pool.borrow_mut();
        for (_, old_widget) in rendered.drain(from..) {
            self.container.remove(&old_widget);
            pool.park(old_widget);
        }
    }

    /// Diffs `blocks` against `self.rendered_blocks`, preserving unchanged prefix widgets,
    /// mutating compatible diverged blocks in-place, and recycling any surplus widgets.
    fn apply_blocks(&self, blocks: &[ContentBlock], is_streaming: bool) {
        let mut rendered = self.rendered_blocks.borrow_mut();

        if blocks.is_empty() {
            self.drain_rendered_to_pool(&mut rendered, 0);
            return;
        }

        let last_idx = blocks.len() - 1;

        // 1. Find longest exact unchanged prefix
        let mut common_len = 0;
        let max_common = rendered.len().min(blocks.len());
        while common_len < max_common && rendered[common_len].0 == blocks[common_len] {
            common_len += 1;
        }

        // If the last preserved block was previously the active streaming tail and is now
        // settled (either stream completed or a subsequent block started after it), finalize it
        // in-place so `StreamCodeLexer` commits its spans into `HIGHLIGHT_CACHE`.
        if common_len > 0 {
            let prev_tail_idx = common_len - 1;
            let was_streaming_tail = rendered[prev_tail_idx].1.has_css_class("streaming-active");
            let still_streaming_tail = is_streaming && prev_tail_idx == last_idx;
            if was_streaming_tail && !still_streaming_tail {
                rendered[prev_tail_idx]
                    .1
                    .remove_css_class("streaming-active");
                let settled_ctx = RenderContext::new(self.generation.clone(), false);
                let _ = self.registry.try_update_block(
                    &blocks[prev_tail_idx],
                    &rendered[prev_tail_idx].1,
                    &settled_ctx,
                );
            }
        }

        // 2. Attempt in-place updates for compatible diverged blocks at matching indices
        let mut reuse_end = common_len;
        while reuse_end < max_common {
            let block_is_tail = is_streaming && reuse_end == last_idx;
            let block_ctx = RenderContext::new(self.generation.clone(), block_is_tail);
            if self.registry.try_update_block(
                &blocks[reuse_end],
                &rendered[reuse_end].1,
                &block_ctx,
            ) {
                if block_is_tail {
                    rendered[reuse_end].1.add_css_class("streaming-active");
                } else {
                    rendered[reuse_end].1.remove_css_class("streaming-active");
                }
                rendered[reuse_end].0 = blocks[reuse_end].clone();
                reuse_end += 1;
            } else {
                break;
            }
        }

        // 3. Remove and park any incompatible or surplus tail widgets
        self.drain_rendered_to_pool(&mut rendered, reuse_end);

        // 4. Render (or pull from recycling pool) and append remaining blocks
        for (i, block) in blocks.iter().enumerate().skip(reuse_end) {
            let block_is_tail = is_streaming && i == last_idx;
            let block_ctx = RenderContext::new(self.generation.clone(), block_is_tail);

            let widget_opt = self
                .pool
                .borrow_mut()
                .take_and_update(block, &self.registry, &block_ctx)
                .or_else(|| self.registry.render_block_with_context(block, &block_ctx));

            if let Some(widget) = widget_opt {
                if block_is_tail {
                    widget.add_css_class("streaming-active");
                } else {
                    widget.remove_css_class("streaming-active");
                }
                if widget.halign() == gtk::Align::Fill {
                    widget.set_hexpand(true);
                }
                self.container.append(&widget);
                rendered.push((block.clone(), widget));
            }
        }
    }

    /// Executes the incremental DOM update for streaming markdown
    fn update_dom(&self) {
        let blocks = parse_markdown(&self.buffer.borrow());
        self.apply_blocks(&blocks, true);
    }

    /// Flushes any pending text to the main buffer and forces a DOM update immediately.
    fn flush(&self) {
        let mut pending = self.pending_text.borrow_mut();
        if pending.is_empty() {
            return;
        }
        self.buffer.borrow_mut().push_str(&pending);
        pending.clear();
        self.update_dom();
    }
}

/// A unified GTK widget for rendering structured Markdown and out-of-band JSON data.
#[derive(Clone)]
pub struct StructuredViewer {
    pub(crate) state: Rc<ViewerState>,
}

impl StructuredViewer {
    /// Creates a new `StructuredViewer` with the given registry.
    #[must_use]
    pub fn new(registry: Rc<ViewerRegistry>) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.set_hexpand(true);
        container.set_halign(gtk::Align::Fill);

        Self {
            state: Rc::new(ViewerState {
                container,
                registry,
                buffer: RefCell::new(String::new()),
                rendered_blocks: RefCell::new(Vec::new()),
                pool: RefCell::new(WidgetPool::default()),
                pending_text: RefCell::new(String::new()),
                update_queued: Cell::new(false),
                generation: Arc::new(AtomicU64::new(0)),
            }),
        }
    }

    /// Returns the current generation of the viewer. Incremented on clear/recycle/set_content.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.state.generation.load(Ordering::Relaxed)
    }

    /// Returns the shared generation atomic for background tasks.
    #[must_use]
    pub fn generation_handle(&self) -> Arc<AtomicU64> {
        self.state.generation.clone()
    }

    /// Returns the underlying GTK container.
    #[must_use]
    pub fn widget(&self) -> &gtk::Box {
        &self.state.container
    }

    /// Immediately flushes any pending stream data. Useful before custom blocks or at the end of a stream.
    pub fn flush(&self) {
        self.state.flush();
    }

    /// Replaces the viewer content using the parsed AST LRU cache and in-place widget reuse.
    /// `GtkListView` row recycling (`connect_bind`) updates existing `GtkLabel` / `CodeBlockWidget`
    /// instances in-place or pulls from the slot's recycling pool in 0 allocations.
    pub fn set_content(&self, raw_text: &str) {
        self.state.generation.fetch_add(1, Ordering::SeqCst);
        self.state.pending_text.borrow_mut().clear();
        {
            let mut buf = self.state.buffer.borrow_mut();
            buf.clear();
            buf.push_str(raw_text);
        }

        // Remove any non-markdown custom blocks that may sit in `container` outside `rendered_blocks`
        if self.state.rendered_blocks.borrow().is_empty() && self.state.container.first_child().is_some()
        {
            while let Some(child) = self.state.container.first_child() {
                self.state.container.remove(&child);
                self.state.pool.borrow_mut().park(child);
            }
        }

        if raw_text.is_empty() {
            let mut rendered = self.state.rendered_blocks.borrow_mut();
            self.state.drain_rendered_to_pool(&mut rendered, 0);
            return;
        }

        let blocks = get_or_parse_markdown(raw_text);
        self.state.apply_blocks(&blocks, false);
    }

    /// Incrementally renders `partial_text` without polluting `AST_CACHE` or `HIGHLIGHT_CACHE`
    /// while `is_complete` is `false` (ideal for typewriter reveals and live delta previews).
    /// When `is_complete` is `true`, commits to `AST_CACHE` and finalizes active code lexers.
    pub fn set_streaming_content(&self, partial_text: &str, is_complete: bool) {
        self.state.pending_text.borrow_mut().clear();
        {
            let mut buf = self.state.buffer.borrow_mut();
            buf.clear();
            buf.push_str(partial_text);
        }

        if partial_text.is_empty() {
            let mut rendered = self.state.rendered_blocks.borrow_mut();
            self.state.drain_rendered_to_pool(&mut rendered, 0);
            return;
        }

        if is_complete {
            let blocks = get_or_parse_markdown(partial_text);
            self.state.apply_blocks(&blocks, false);
        } else {
            let blocks = parse_markdown(partial_text);
            self.state.apply_blocks(&blocks, true);
        }
    }

    /// Parks reusable child widgets into this viewer's bounded recycling pool and cancels
    /// any pending background tasks (`GtkListView` `connect_unbind` fast path).
    pub fn park_for_recycle(&self) {
        self.clear();
    }

    /// Clears the viewer container and streaming state, parking reusable widgets into the pool.
    pub fn clear(&self) {
        // Increment generation to cancel pending async tasks (e.g., highlighting)
        self.state.generation.fetch_add(1, Ordering::SeqCst);

        self.state.buffer.borrow_mut().clear();
        self.state.pending_text.borrow_mut().clear();

        {
            let mut rendered = self.state.rendered_blocks.borrow_mut();
            self.state.drain_rendered_to_pool(&mut rendered, 0);
        }

        // Remove any remaining out-of-band custom widgets
        while let Some(child) = self.state.container.first_child() {
            self.state.container.remove(&child);
            self.state.pool.borrow_mut().park(child);
        }
    }

    /// Appends text to the continuous Markdown stream with 60Hz debouncing.
    /// Fast LLM generation will be batched into ~16ms chunks to prevent GTK starvation.
    pub fn append_markdown_stream(&self, new_text: &str) {
        self.state.pending_text.borrow_mut().push_str(new_text);

        if !self.state.update_queued.get() {
            self.state.update_queued.set(true);

            let state_clone = self.state.clone();

            // ~60 FPS update limit (16ms)
            gtk::glib::timeout_add_local_once(Duration::from_millis(16), move || {
                state_clone.update_queued.set(false);
                state_clone.flush();
            });
        }
    }

    /// Appends out-of-band structured data (e.g., ToolResult, ProposeFileWrite).
    /// This bypasses the Markdown parser entirely.
    pub fn append_custom_block(&self, schema: &str, payload: &str) {
        // Force flush any pending markdown first so ordering is preserved
        self.flush();

        // Seal the current markdown segment so custom blocks stay in document order
        {
            let rendered = self.state.rendered_blocks.borrow();
            if let Some((last_block, last_widget)) = rendered.last()
                && last_widget.has_css_class("streaming-active")
            {
                last_widget.remove_css_class("streaming-active");
                let settled_ctx = RenderContext::new(self.state.generation.clone(), false);
                let _ = self
                    .state
                    .registry
                    .try_update_block(last_block, last_widget, &settled_ctx);
            }
        }
        self.state.rendered_blocks.borrow_mut().clear();
        self.state.buffer.borrow_mut().clear();

        let block = ContentBlock::Custom {
            schema: schema.to_string(),
            raw_payload: payload.to_string(),
        };

        let ctx = RenderContext::new(self.state.generation.clone(), false);
        if let Some(widget) = self.state.registry.render_block_with_context(&block, &ctx) {
            widget.set_hexpand(true);
            widget.set_halign(gtk::Align::Fill);
            self.state.container.append(&widget);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_mutates_tail_widgets_in_place_and_park_recycles_widgets() {
        crate::run_gtk_test(|| {
            let viewer = StructuredViewer::new(Rc::new(ViewerRegistry::new_with_defaults()));
            viewer.set_streaming_content("Hello ", false);
            let first_para = viewer.widget().first_child().expect("first paragraph");

            viewer.set_streaming_content("Hello **world**!", false);
            let updated_para = viewer.widget().first_child().expect("same paragraph");
            assert_eq!(first_para, updated_para);

            viewer.set_streaming_content("Hello **world**!\n\n```rust\nlet x = 1;", false);
            let code_w1 = viewer.widget().last_child().expect("code block");
            viewer.set_streaming_content(
                "Hello **world**!\n\n```rust\nlet x = 1;\nlet y = 2;\n```",
                true,
            );
            let code_w2 = viewer.widget().last_child().expect("settled code block");
            assert_eq!(code_w1, code_w2);

            viewer.park_for_recycle();
            assert!(viewer.widget().first_child().is_none());
            let (p_cnt, _, _, c_cnt) = viewer.state.pool.borrow().counts();
            assert_eq!(p_cnt, 1);
            assert_eq!(c_cnt, 1);

            viewer.set_content("Rebound paragraph\n\n```rust\nfn rebound() {}\n```");
            assert_eq!(viewer.widget().first_child().as_ref(), Some(&first_para));
            assert_eq!(viewer.widget().last_child().as_ref(), Some(&code_w1));
        });
    }
}
