use crate::cache::get_or_parse_markdown;
use crate::parser::blocks::ContentBlock;
use crate::parser::markdown::parse_markdown;
use crate::registry::ViewerRegistry;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Internal state, wrapped in Rc to easily share with glib timeouts
pub(crate) struct ViewerState {
    pub(crate) container: gtk::Box,
    pub(crate) registry: Rc<ViewerRegistry>,

    // Main buffer holding all processed text in the current markdown segment
    pub(crate) buffer: RefCell<String>,
    // Rendered blocks for the current markdown segment, enabling incremental AST diffing
    // (including gallery coalescing where adjacent blocks merge).
    pub(crate) rendered_blocks: RefCell<Vec<(ContentBlock, gtk::Widget)>>,

    // Debouncing state
    pub(crate) pending_text: RefCell<String>,
    pub(crate) update_queued: Cell<bool>,

    // Generation counter to cancel async tasks when the viewer is cleared or recycled.
    // Atomic so it can be shared with background threads (e.g., syntax highlighters).
    pub(crate) generation: Arc<AtomicU64>,
}

impl ViewerState {
    /// Diffs `blocks` against `self.rendered_blocks`, preserving unchanged prefix widgets
    /// and replacing only the modified or newly coalesced tail blocks.
    fn apply_blocks(&self, blocks: &[ContentBlock], is_streaming: bool) {
        if blocks.is_empty() {
            return;
        }

        let mut rendered = self.rendered_blocks.borrow_mut();

        // Find longest unchanged prefix
        let mut common_len = 0;
        let max_common = rendered.len().min(blocks.len());
        while common_len < max_common && rendered[common_len].0 == blocks[common_len] {
            common_len += 1;
        }

        // Remove streaming-active class from the last preserved widget if it was previously active
        if common_len > 0 {
            rendered[common_len - 1]
                .1
                .remove_css_class("streaming-active");
        }

        // Remove diverged tail widgets from the GTK container
        for (_, old_widget) in rendered.drain(common_len..) {
            self.container.remove(&old_widget);
        }

        // Render and append new/updated blocks
        let last_idx = blocks.len() - 1;
        for (i, block) in blocks.iter().enumerate().skip(common_len) {
            if let Some(widget) = self.registry.render_block(block) {
                if is_streaming && i == last_idx {
                    widget.add_css_class("streaming-active");
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
    /// Creates a new StructuredViewer with the given registry.
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
                pending_text: RefCell::new(String::new()),
                update_queued: Cell::new(false),
                generation: Arc::new(AtomicU64::new(0)),
            }),
        }
    }

    /// Returns the current generation of the viewer. Incremented on clear/recycle.
    pub fn generation(&self) -> u64 {
        self.state.generation.load(Ordering::Relaxed)
    }

    /// Returns the shared generation atomic for background tasks.
    pub fn generation_handle(&self) -> Arc<AtomicU64> {
        self.state.generation.clone()
    }

    /// Returns the underlying GTK container.
    pub fn widget(&self) -> &gtk::Box {
        &self.state.container
    }

    /// Immediately flushes any pending stream data. Useful before custom blocks or at the end of a stream.
    pub fn flush(&self) {
        self.state.flush();
    }

    /// Completely replaces the content. Uses the parsed AST LRU cache so `GtkListView`
    /// row recycling (`connect_bind`) avoids re-parsing static history messages.
    pub fn set_content(&self, raw_text: &str) {
        self.clear();
        if raw_text.is_empty() {
            return;
        }
        let blocks = get_or_parse_markdown(raw_text);
        self.state.buffer.borrow_mut().push_str(raw_text);
        self.state.apply_blocks(&blocks, false);
    }

    /// Clears the viewer container and streaming state.
    pub fn clear(&self) {
        // Increment generation to cancel pending async tasks (e.g., highlighting)
        self.state.generation.fetch_add(1, Ordering::SeqCst);

        self.state.buffer.borrow_mut().clear();
        self.state.pending_text.borrow_mut().clear();
        self.state.rendered_blocks.borrow_mut().clear();

        while let Some(child) = self.state.container.first_child() {
            self.state.container.remove(&child);
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

    /// Appends out-of-band structured data (e.g., ToolResult, ProposeFileWrite)
    /// This bypasses the Markdown parser entirely.
    pub fn append_custom_block(&self, schema: &str, payload: &str) {
        // Force flush any pending markdown first so ordering is preserved
        self.flush();

        // Seal the current markdown segment so custom blocks stay in document order
        if let Some((_, last_widget)) = self.state.rendered_blocks.borrow().last() {
            last_widget.remove_css_class("streaming-active");
        }
        self.state.rendered_blocks.borrow_mut().clear();
        self.state.buffer.borrow_mut().clear();

        let block = ContentBlock::Custom {
            schema: schema.to_string(),
            raw_payload: payload.to_string(),
        };

        if let Some(widget) = self.state.registry.render_block(&block) {
            widget.set_hexpand(true);
            widget.set_halign(gtk::Align::Fill);
            self.state.container.append(&widget);
        }
    }
}
