use crate::parser::blocks::ContentBlock;
use crate::renderer::{BlockRenderer, RenderContext};
use gtk4 as gtk;

const BUILTIN_RENDERER_COUNT: usize = 5;

/// Controls which hyperlink schemes and media sources a [`ViewerRegistry`] permits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkSafetyPolicy {
    /// Standard local terminal/Claw mode: allows `http://`, `https://`, `mailto:`, `file://`,
    /// local `/abs/path` launches, and local `ffmpeg` media thumbnails.
    #[default]
    LocalWorkspace,
    /// Hardened remote-content mode (used by `Boxxy News`): allows strictly `https://` links
    /// via `GtkUriLauncher`, disables `GtkFileLauncher` and `ffmpeg`, and restricts images to
    /// allowlisted Boxxy HTTPS origins.
    RemoteHttpsOnly,
}

pub struct ViewerRegistry {
    renderers: Vec<Box<dyn BlockRenderer>>,
    link_safety: LinkSafetyPolicy,
}

impl ViewerRegistry {
    /// Creates a new registry equipped with the standard text, table, code, image, and diff renderers.
    #[must_use]
    pub fn new_with_defaults() -> Self {
        Self::with_policy(LinkSafetyPolicy::LocalWorkspace)
    }

    /// Creates a hardened registry for untrusted/remote Markdown (`LinkSafetyPolicy::RemoteHttpsOnly`).
    #[must_use]
    pub fn new_remote_safe() -> Self {
        Self::with_policy(LinkSafetyPolicy::RemoteHttpsOnly)
    }

    #[must_use]
    pub fn with_policy(link_safety: LinkSafetyPolicy) -> Self {
        let mut registry = Self {
            renderers: Vec::with_capacity(BUILTIN_RENDERER_COUNT + 2),
            link_safety,
        };

        // Standard renderers
        registry.register(Box::new(crate::renderer::text::TextRenderer));
        registry.register(Box::new(crate::renderer::table::TableRenderer));
        registry.register(Box::new(crate::renderer::code::CodeRenderer));
        registry.register(Box::new(crate::renderer::image::ImageRenderer));
        registry.register(Box::new(crate::renderer::diff::DiffRenderer));

        registry
    }

    #[must_use]
    pub const fn link_safety(&self) -> LinkSafetyPolicy {
        self.link_safety
    }

    /// Registers a custom renderer. Custom renderers take precedence over defaults
    /// because we iterate backwards.
    pub fn register(&mut self, renderer: Box<dyn BlockRenderer>) {
        self.renderers.push(renderer);
    }

    /// Returns true if no custom renderer registered after the built-in defaults claims `block`.
    #[must_use]
    pub(crate) fn uses_builtin_renderer(&self, block: &ContentBlock) -> bool {
        if self.renderers.len() <= BUILTIN_RENDERER_COUNT {
            return true;
        }
        !self.renderers[BUILTIN_RENDERER_COUNT..]
            .iter()
            .rev()
            .any(|r| r.can_render(block))
    }

    /// Finds the first registered renderer capable of rendering the given block.
    #[must_use]
    pub fn render_block(&self, block: &ContentBlock) -> Option<gtk::Widget> {
        self.render_block_with_context(block, &RenderContext::default())
    }

    /// Finds the first registered renderer capable of rendering the given block with lifecycle context.
    #[must_use]
    pub fn render_block_with_context(
        &self,
        block: &ContentBlock,
        ctx: &RenderContext,
    ) -> Option<gtk::Widget> {
        for renderer in self.renderers.iter().rev() {
            if renderer.can_render(block) {
                return Some(renderer.render_with_context(block, self, ctx));
            }
        }
        None
    }

    /// Attempts to update `widget` in-place to match `block` using the matching renderer.
    #[must_use]
    pub fn try_update_block(
        &self,
        block: &ContentBlock,
        widget: &gtk::Widget,
        ctx: &RenderContext,
    ) -> bool {
        for renderer in self.renderers.iter().rev() {
            if renderer.can_render(block) {
                return renderer.update(block, widget, self, ctx);
            }
        }
        false
    }
}
