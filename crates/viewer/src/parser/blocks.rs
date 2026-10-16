/// Horizontal alignment for a Markdown table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAlignment {
    None,
    Left,
    Center,
    Right,
}

/// A single item inside a coalesced `ImageGallery` or embedded in a `TableCell`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GalleryItem {
    pub url: String,
    pub title: String,
    pub alt: String,
    /// Optional Pango-formatted caption extracted from the list item or adjacent paragraph.
    pub caption: Option<String>,
}

/// A single cell in a Markdown table, supporting Pango text markup and embedded images/media.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableCell {
    /// Pango-formatted text markup in the cell.
    pub markup: String,
    /// Images or media items embedded inside the cell (`![alt](url "title")`).
    pub images: Vec<GalleryItem>,
}

impl TableCell {
    #[must_use]
    pub fn new(markup: String, images: Vec<GalleryItem>) -> Self {
        Self { markup, images }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.markup.is_empty() && self.images.is_empty()
    }
}

impl PartialEq<&str> for TableCell {
    fn eq(&self, other: &&str) -> bool {
        self.images.is_empty() && self.markup == *other
    }
}

impl PartialEq<String> for TableCell {
    fn eq(&self, other: &String) -> bool {
        self.images.is_empty() && &self.markup == other
    }
}

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

    /// A single standalone image.
    Image {
        url: String,
        title: String,
        alt: String,
    },

    /// Multiple images coalesced into a responsive multi-column media gallery.
    ImageGallery { items: Vec<GalleryItem> },

    /// A GitHub-Flavored Markdown table.
    Table {
        alignments: Vec<TableAlignment>,
        headers: Vec<String>,
        rows: Vec<Vec<TableCell>>,
    },

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
