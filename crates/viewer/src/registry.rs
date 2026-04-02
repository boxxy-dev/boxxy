use crate::parser::blocks::ContentBlock;
use crate::renderer::BlockRenderer;
use gtk4 as gtk;

pub struct ViewerRegistry {
    renderers: Vec<Box<dyn BlockRenderer>>,
}

impl ViewerRegistry {
    /// Creates a new registry equipped with the standard text and code renderers.
    pub fn new_with_defaults() -> Self {
        let mut registry = Self {
            renderers: Vec::new(),
        };

        // Standard renderers
        registry.register(Box::new(crate::renderer::text::TextRenderer));
        registry.register(Box::new(crate::renderer::code::CodeRenderer));

        registry
    }

    /// Registers a custom renderer. Custom renderers take precedence over defaults
    /// because we push them to the front (or iterate backwards).
    pub fn register(&mut self, renderer: Box<dyn BlockRenderer>) {
        self.renderers.push(renderer);
    }

    /// Finds the first registered renderer capable of rendering the given block.
    pub fn render_block(&self, block: &ContentBlock) -> Option<gtk::Widget> {
        // Iterate backwards so that custom renderers can override defaults
        for renderer in self.renderers.iter().rev() {
            if renderer.can_render(block) {
                return Some(renderer.render(block, self));
            }
        }
        None
    }
}
