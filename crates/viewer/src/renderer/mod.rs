pub mod code;
pub mod diff;
pub mod image;
pub mod table;
pub mod text;

use crate::parser::blocks::ContentBlock;
use crate::registry::ViewerRegistry;
use gtk4 as gtk;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Contextual lifecycle state passed from `StructuredViewer` to `BlockRenderer`s.
#[derive(Debug, Clone)]
pub struct RenderContext {
    /// Shared generation counter incremented whenever the parent `StructuredViewer`
    /// is cleared, recycled (`GtkListView` unbind), or reassigned via `set_content`.
    pub generation: Arc<AtomicU64>,
    /// True when rendering an actively growing streaming Markdown buffer (`append_markdown_stream`).
    pub is_streaming: bool,
}

impl RenderContext {
    #[must_use]
    pub fn new(generation: Arc<AtomicU64>, is_streaming: bool) -> Self {
        Self {
            generation,
            is_streaming,
        }
    }

    #[must_use]
    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

impl Default for RenderContext {
    fn default() -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            is_streaming: false,
        }
    }
}

/// A trait for mapping an abstract `ContentBlock` into a native GTK Widget.
pub trait BlockRenderer {
    /// Returns true if this renderer knows how to handle the given block.
    fn can_render(&self, block: &ContentBlock) -> bool;

    /// Renders the block into a new GTK widget without streaming/generation context.
    fn render(&self, block: &ContentBlock, registry: &ViewerRegistry) -> gtk::Widget;

    /// Renders the block with lifecycle context (`generation` for async cancellation and `is_streaming`).
    fn render_with_context(
        &self,
        block: &ContentBlock,
        registry: &ViewerRegistry,
        _ctx: &RenderContext,
    ) -> gtk::Widget {
        self.render(block, registry)
    }

    /// Attempts to mutate `widget` in-place to represent `block`, avoiding GObject teardown
    /// and recreation during 60Hz streaming and `GtkListView` row recycling.
    /// Returns `true` if `widget` was updated in-place, or `false` if a fresh widget is needed.
    fn update(
        &self,
        _block: &ContentBlock,
        _widget: &gtk::Widget,
        _registry: &ViewerRegistry,
        _ctx: &RenderContext,
    ) -> bool {
        false
    }
}
