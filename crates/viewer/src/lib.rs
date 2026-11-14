pub mod cache;
pub mod parser;
pub mod registry;
pub mod renderer;
pub mod widget;

pub use parser::blocks::{ContentBlock, GalleryItem, ListItem, TableAlignment, TableCell};
pub use parser::markdown::{
    escape_pango, escape_pango_attr, normalize_fence_language, parse_markdown, strip_pango_markup,
    write_escaped_pango, write_escaped_pango_attr,
};
pub use registry::ViewerRegistry;
pub use renderer::code::CodeBlockWidget;
pub use renderer::{BlockRenderer, RenderContext};
pub use widget::StructuredViewer;
