use crate::parser::blocks::ContentBlock;
use crate::parser::markdown::parse_markdown;
use crate::registry::ViewerRegistry;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Internal state, wrapped in Rc to easily share with glib timeouts
struct ViewerState {
    container: gtk::Box,
    registry: Rc<ViewerRegistry>,

    // Main buffer holding all processed text
    buffer: RefCell<String>,
    // The currently streaming, incomplete widget
    active_widget: RefCell<Option<gtk::Widget>>,

    // Debouncing state
    pending_text: RefCell<String>,
    update_queued: Cell<bool>,
}

impl ViewerState {
    /// Executes the core "Active Block" DOM update strategy
    fn update_dom(&self) {
        let blocks = parse_markdown(&self.buffer.borrow());

        if blocks.is_empty() {
            return;
        }

        // The last block is considered "active" (still being streamed to).
        // All previous blocks are considered "sealed" (finished).
        let active_block_index = blocks.len() - 1;
        let active_block = &blocks[active_block_index];

        // 1. Remove the current active widget from the DOM if it exists.
        if let Some(widget) = self.active_widget.borrow_mut().take() {
            self.container.remove(&widget);
        }

        // 2. Count sealed blocks currently in the DOM
        let mut sealed_count = 0;
        let mut child = self.container.first_child();
        while child.is_some() {
            sealed_count += 1;
            child = child.unwrap().next_sibling();
        }

        // 3. Render any newly sealed blocks that aren't in the DOM yet
        for i in sealed_count..active_block_index {
            if let Some(widget) = self.registry.render_block(&blocks[i]) {
                widget.set_hexpand(true);
                widget.set_halign(gtk::Align::Fill);
                self.container.append(&widget);
            }
        }

        // 4. Render the current active block and attach it
        if let Some(widget) = self.registry.render_block(active_block) {
            widget.add_css_class("streaming-active");
            widget.set_hexpand(true);
            widget.set_halign(gtk::Align::Fill);
            self.container.append(&widget);
            *self.active_widget.borrow_mut() = Some(widget);
        }
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
    state: Rc<ViewerState>,
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
                active_widget: RefCell::new(None),
                pending_text: RefCell::new(String::new()),
                update_queued: Cell::new(false),
            }),
        }
    }

    /// Returns the underlying GTK container.
    pub fn widget(&self) -> &gtk::Box {
        &self.state.container
    }

    /// Immediately flushes any pending stream data. Useful before custom blocks or at the end of a stream.
    pub fn flush(&self) {
        self.state.flush();
    }

    /// Completely replaces the content. Useful for static popovers or resetting.
    pub fn set_content(&self, raw_text: &str) {
        self.clear();
        self.append_markdown_stream(raw_text);
        self.flush(); // Ensure it renders immediately
    }

    /// Clears the viewer container and streaming state.
    pub fn clear(&self) {
        self.state.buffer.borrow_mut().clear();
        self.state.pending_text.borrow_mut().clear();
        *self.state.active_widget.borrow_mut() = None;

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

        // If there's an active markdown block being streamed, we should seal it first
        if let Some(widget) = self.state.active_widget.borrow_mut().take() {
            widget.remove_css_class("streaming-active");
        }

        // Reset the markdown buffer because we are interrupting the text stream
        // with a custom widget block.
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
