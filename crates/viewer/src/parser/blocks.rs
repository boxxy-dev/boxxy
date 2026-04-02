/// The Abstract Syntax Tree (AST) representing structured content.
///
/// This enum represents a sequence of logical visual blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentBlock {
    /// A header element with a level (1-6) and its Pango-formatted markup.
    Heading { level: u8, markup: String },

    /// A standard paragraph of text, already formatted with Pango markup
    /// (e.g. `<b>`, `<i>`, `<tt>`). Escaping of raw text has already occurred.
    Paragraph(String),

    /// A blockquote containing Pango-formatted markup.
    Blockquote(String),

    /// A list, either ordered or unordered, containing items.
    /// Each item is a sequence of ContentBlocks (e.g., a paragraph and a nested list).
    List { ordered: bool, items: Vec<ListItem> },

    /// A horizontal rule.
    Rule,

    /// An image.
    Image { url: String, title: String, alt: String },

    /// A fenced code block. `code` is preserved *raw* (unescaped)
    /// to ensure copy-to-clipboard functionality works properly.
    Code { lang: String, code: String },

    /// Out-of-band structured data (like a tool execution result).
    /// `schema` defines what kind of data it is (e.g., "list_processes"),
    /// and `raw_payload` contains the stringified JSON or raw data.
    Custom { schema: String, raw_payload: String },
}

/// A single item in a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// The content of the item.
    pub blocks: Vec<ContentBlock>,
    /// Optional checkbox state (Some(true) for [x], Some(false) for [ ], None for normal).
    pub checked: Option<bool>,
}
