use super::blocks::{ContentBlock, GalleryItem, ListItem, TableAlignment, TableCell};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Parser, Tag, TagEnd};

/// Appends `text` to `out`, escaping `&`, `<`, and `>` for safe inclusion in Pango markup body text.
/// Uses bulk slice copying so clean ASCII/UTF-8 runs require zero per-character branching.
pub fn write_escaped_pango(out: &mut String, text: &str) {
    let bytes = text.as_bytes();
    let mut last = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let replacement = match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            _ => continue,
        };
        if last < i {
            out.push_str(&text[last..i]);
        }
        out.push_str(replacement);
        last = i + 1;
    }
    if last < text.len() {
        out.push_str(&text[last..]);
    }
}

/// Escapes text for safe inclusion in Pango markup body text.
#[must_use]
pub fn escape_pango(text: &str) -> String {
    if !text
        .as_bytes()
        .iter()
        .any(|&b| matches!(b, b'&' | b'<' | b'>'))
    {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 16);
    write_escaped_pango(&mut out, text);
    out
}

/// Appends `text` to `out`, escaping XML attribute special characters for double-quoted Pango attributes.
pub fn write_escaped_pango_attr(out: &mut String, text: &str) {
    let bytes = text.as_bytes();
    let mut last = 0;
    for (i, &b) in bytes.iter().enumerate() {
        let replacement = match b {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' => "&quot;",
            b'\'' => "&apos;",
            _ => continue,
        };
        if last < i {
            out.push_str(&text[last..i]);
        }
        out.push_str(replacement);
        last = i + 1;
    }
    if last < text.len() {
        out.push_str(&text[last..]);
    }
}

/// Escapes text for safe inclusion inside a double-quoted Pango XML attribute (e.g. `href="..."`).
#[must_use]
pub fn escape_pango_attr(text: &str) -> String {
    if !text
        .as_bytes()
        .iter()
        .any(|&b| matches!(b, b'&' | b'<' | b'>' | b'"' | b'\''))
    {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 16);
    write_escaped_pango_attr(&mut out, text);
    out
}

/// Strips Pango XML tags and decodes XML character entities in a single pass.
/// When `newlines_as_spaces` is true, `\n` outside tags is converted to `' '` (useful for TSV export).
#[must_use]
pub fn strip_pango_markup(markup: &str, newlines_as_spaces: bool) -> String {
    let mut out = String::with_capacity(markup.len());
    let bytes = markup.as_bytes();
    let mut i = 0;
    let mut in_tag = false;

    while i < bytes.len() {
        let b = bytes[i];
        if in_tag {
            if b == b'>' {
                in_tag = false;
            }
            i += 1;
            continue;
        }

        match b {
            b'<' => {
                in_tag = true;
                i += 1;
            }
            b'\n' if newlines_as_spaces => {
                out.push(' ');
                i += 1;
            }
            b'&' => {
                let rest = &markup[i..];
                if rest.starts_with("&amp;") {
                    out.push('&');
                    i += 5;
                } else if rest.starts_with("&lt;") {
                    out.push('<');
                    i += 4;
                } else if rest.starts_with("&gt;") {
                    out.push('>');
                    i += 4;
                } else if rest.starts_with("&quot;") {
                    out.push('"');
                    i += 6;
                } else if rest.starts_with("&apos;") {
                    out.push('\'');
                    i += 6;
                } else if rest.starts_with("&#39;") {
                    out.push('\'');
                    i += 5;
                } else {
                    out.push('&');
                    i += 1;
                }
            }
            _ => {
                // Copy contiguous plain UTF-8 run until next `<`, `&`, or `\n`
                let start = i;
                i += 1;
                while i < bytes.len() {
                    let nb = bytes[i];
                    if nb == b'<' || nb == b'&' || (newlines_as_spaces && nb == b'\n') {
                        break;
                    }
                    i += 1;
                }
                out.push_str(&markup[start..i]);
            }
        }
    }

    out
}

/// Normalizes a fenced code block's info string using OpenCode's rule:
/// extracts the first word (delimited by whitespace, `,`, `{`, or `:`), strips optional
/// Pandoc/Quarto `.` class prefixes, and lowercases ASCII.
///
/// Examples:
/// - `"CSV title=\"Q3\""` -> `"csv"`
/// - `"rust,no_run"` -> `"rust"`
/// - `"{.python}"` -> `"python"`
/// - `"ts:src/index.ts"` -> `"ts"`
#[must_use]
pub fn normalize_fence_language(info: &str) -> String {
    let trimmed = info.trim().trim_start_matches('{').trim_start_matches('.');
    let token = trimmed
        .split(|c: char| c.is_ascii_whitespace() || matches!(c, ',' | '{' | '}' | ':' | ';'))
        .next()
        .unwrap_or_default();
    token
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '#' | '.' | '-'))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[derive(Debug, Clone)]
enum InlineStyle {
    Strong,
    Emphasis,
    Strikethrough,
    Link(String), // Already escaped for Pango attribute
}

impl InlineStyle {
    fn write_open(&self, out: &mut String) {
        match self {
            Self::Strong => out.push_str("<b>"),
            Self::Emphasis => out.push_str("<i>"),
            Self::Strikethrough => out.push_str("<s>"),
            Self::Link(escaped_url) => {
                out.push_str("<a href=\"");
                out.push_str(escaped_url);
                out.push_str("\">");
            }
        }
    }

    fn close_tag(&self) -> &'static str {
        match self {
            Self::Strong => "</b>",
            Self::Emphasis => "</i>",
            Self::Strikethrough => "</s>",
            Self::Link(_) => "</a>",
        }
    }
}

#[derive(Debug)]
enum ParseContainer {
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
    },
    Item {
        blocks: Vec<ContentBlock>,
        checked: Option<bool>,
    },
    Paragraph(String, bool), // markup, is_implicit
    Heading(u8, String),
    BlockQuote {
        blocks: Vec<ContentBlock>,
    },
    CodeBlock(String, String),
    Image {
        url: String,
        title: String,
        alt: String,
    },
    Table {
        alignments: Vec<TableAlignment>,
        headers: Vec<String>,
        rows: Vec<Vec<TableCell>>,
        in_head: bool,
        current_row: Vec<TableCell>,
    },
    TableCell {
        markup: String,
        images: Vec<GalleryItem>,
    },
}

/// Parses a Markdown string into a sequence of `ContentBlock`s, including GFM tables,
/// clickable links, nested blockquotes, and automatic multi-image gallery coalescing.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn parse_markdown(input: &str) -> Vec<ContentBlock> {
    let parser = Parser::new_ext(input, pulldown_cmark::Options::all());

    let mut root_blocks = Vec::new();
    let mut stack: Vec<ParseContainer> = Vec::new();
    let mut inline_styles: Vec<InlineStyle> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => {
                if is_block_tag(&tag) {
                    maybe_close_implicit_paragraph(&mut stack, &inline_styles, &mut root_blocks);
                }
                match tag {
                    Tag::Paragraph => {
                        stack.push(ParseContainer::Paragraph(String::new(), false));
                    }
                    Tag::Heading { level, .. } => {
                        stack.push(ParseContainer::Heading(level as u8, String::new()));
                    }
                    Tag::BlockQuote(_) => {
                        stack.push(ParseContainer::BlockQuote { blocks: Vec::new() });
                    }
                    Tag::CodeBlock(kind) => {
                        let lang = if let CodeBlockKind::Fenced(info) = kind {
                            normalize_fence_language(&info)
                        } else {
                            String::new()
                        };
                        stack.push(ParseContainer::CodeBlock(lang, String::new()));
                    }
                    Tag::List(first_item_number) => stack.push(ParseContainer::List {
                        ordered: first_item_number.is_some(),
                        start: first_item_number.unwrap_or(1),
                        items: Vec::new(),
                    }),
                    Tag::Item => stack.push(ParseContainer::Item {
                        blocks: Vec::new(),
                        checked: None,
                    }),
                    Tag::Image {
                        dest_url, title, ..
                    } => {
                        // If an image appears inside a paragraph with visible preceding text,
                        // cleanly close active inline tags on the preceding text, emit it in
                        // document order, and re-open active inline tags for any trailing text.
                        flush_non_empty_top_paragraph(&mut stack, &inline_styles, &mut root_blocks);
                        stack.push(ParseContainer::Image {
                            url: dest_url.into_string(),
                            title: title.into_string(),
                            alt: String::new(),
                        });
                    }
                    Tag::Table(alignments) => {
                        let mapped = alignments
                            .into_iter()
                            .map(|a| match a {
                                Alignment::None => TableAlignment::None,
                                Alignment::Left => TableAlignment::Left,
                                Alignment::Center => TableAlignment::Center,
                                Alignment::Right => TableAlignment::Right,
                            })
                            .collect();
                        stack.push(ParseContainer::Table {
                            alignments: mapped,
                            headers: Vec::new(),
                            rows: Vec::new(),
                            in_head: false,
                            current_row: Vec::new(),
                        });
                    }
                    Tag::TableHead => {
                        if let Some(ParseContainer::Table {
                            in_head,
                            current_row,
                            ..
                        }) = find_enclosing_table(&mut stack)
                        {
                            *in_head = true;
                            current_row.clear();
                        }
                    }
                    Tag::TableRow => {
                        if let Some(ParseContainer::Table {
                            in_head,
                            current_row,
                            ..
                        }) = find_enclosing_table(&mut stack)
                        {
                            *in_head = false;
                            current_row.clear();
                        }
                    }
                    Tag::TableCell => {
                        stack.push(ParseContainer::TableCell {
                            markup: String::new(),
                            images: Vec::new(),
                        });
                    }

                    // Inline styles & links
                    Tag::Strong if !is_in_image(&stack) => {
                        inline_styles.push(InlineStyle::Strong);
                        append_raw_text(&mut stack, &inline_styles, "<b>");
                    }
                    Tag::Emphasis if !is_in_image(&stack) => {
                        inline_styles.push(InlineStyle::Emphasis);
                        append_raw_text(&mut stack, &inline_styles, "<i>");
                    }
                    Tag::Strikethrough if !is_in_image(&stack) => {
                        inline_styles.push(InlineStyle::Strikethrough);
                        append_raw_text(&mut stack, &inline_styles, "<s>");
                    }
                    Tag::Link { dest_url, .. } if !is_in_image(&stack) => {
                        let escaped_url = escape_pango_attr(&dest_url);
                        let style = InlineStyle::Link(escaped_url);
                        append_inline_open(&mut stack, &inline_styles, &style);
                        inline_styles.push(style);
                    }
                    _ => {}
                }
            }
            Event::End(tag) => {
                if matches!(tag, TagEnd::Item | TagEnd::List(_) | TagEnd::BlockQuote(_)) {
                    maybe_close_implicit_paragraph(&mut stack, &inline_styles, &mut root_blocks);
                }

                match tag {
                    TagEnd::TableCell => {
                        if let Some(ParseContainer::TableCell { mut markup, images }) = stack.pop()
                            && let Some(ParseContainer::Table { current_row, .. }) =
                                find_enclosing_table(&mut stack)
                        {
                            close_inline_tags(&mut markup, &inline_styles);
                            inline_styles.clear();
                            let cleaned = if has_visible_text_outside_tags(&markup) {
                                markup.trim().to_string()
                            } else {
                                String::new()
                            };
                            current_row.push(TableCell {
                                markup: cleaned,
                                images,
                            });
                        }
                    }
                    TagEnd::TableHead => {
                        if let Some(ParseContainer::Table {
                            headers,
                            in_head,
                            current_row,
                            ..
                        }) = find_enclosing_table(&mut stack)
                        {
                            *headers = std::mem::take(current_row)
                                .into_iter()
                                .map(|c| c.markup)
                                .collect();
                            *in_head = false;
                        }
                    }
                    TagEnd::TableRow => {
                        if let Some(ParseContainer::Table {
                            rows, current_row, ..
                        }) = find_enclosing_table(&mut stack)
                        {
                            let row = std::mem::take(current_row);
                            if !row.is_empty() {
                                rows.push(row);
                            }
                        }
                    }
                    _ if is_block_tag_end(&tag) => {
                        if let Some(container) = stack.pop() {
                            let block = match (tag, container) {
                                (TagEnd::Paragraph, ParseContainer::Paragraph(mut markup, _)) => {
                                    close_inline_tags(&mut markup, &inline_styles);
                                    inline_styles.clear();
                                    finalize_paragraph(markup)
                                }
                                (TagEnd::Heading(_), ParseContainer::Heading(level, mut markup)) => {
                                    close_inline_tags(&mut markup, &inline_styles);
                                    inline_styles.clear();
                                    Some(ContentBlock::Heading { level, markup })
                                }
                                (
                                    TagEnd::BlockQuote(_),
                                    ParseContainer::BlockQuote { blocks },
                                ) => {
                                    if blocks.is_empty() {
                                        None
                                    } else {
                                        Some(ContentBlock::Blockquote(blocks))
                                    }
                                }
                                (TagEnd::CodeBlock, ParseContainer::CodeBlock(lang, code)) => {
                                    Some(ContentBlock::Code { lang, code })
                                }
                                (TagEnd::Item, ParseContainer::Item { blocks, checked }) => {
                                    if let Some(ParseContainer::List { items, .. }) =
                                        stack.last_mut()
                                    {
                                        items.push(ListItem { blocks, checked });
                                    }
                                    None
                                }
                                (
                                    TagEnd::List(_),
                                    ParseContainer::List {
                                        ordered,
                                        start,
                                        items,
                                    },
                                ) => Some(ContentBlock::List {
                                    ordered,
                                    start,
                                    items,
                                }),
                                (TagEnd::Image, ParseContainer::Image { url, title, alt }) => {
                                    if let Some(ParseContainer::TableCell { images, .. }) =
                                        find_enclosing_table_cell(&mut stack)
                                    {
                                        images.push(GalleryItem {
                                            url,
                                            title,
                                            alt,
                                            caption: None,
                                        });
                                        None
                                    } else {
                                        Some(ContentBlock::Image { url, title, alt })
                                    }
                                }
                                (
                                    TagEnd::Table,
                                    ParseContainer::Table {
                                        alignments,
                                        headers,
                                        rows,
                                        ..
                                    },
                                ) => Some(ContentBlock::Table {
                                    alignments,
                                    headers,
                                    rows,
                                }),
                                _ => None,
                            };

                            if let Some(b) = block {
                                push_block(&mut root_blocks, &mut stack, b);
                            }
                        }
                    }
                    TagEnd::Strong
                        if !is_in_image(&stack)
                            && pop_matching_inline(&mut inline_styles, InlineKind::Strong) =>
                    {
                        append_raw_text(&mut stack, &inline_styles, "</b>");
                    }
                    TagEnd::Emphasis
                        if !is_in_image(&stack)
                            && pop_matching_inline(&mut inline_styles, InlineKind::Emphasis) =>
                    {
                        append_raw_text(&mut stack, &inline_styles, "</i>");
                    }
                    TagEnd::Strikethrough
                        if !is_in_image(&stack)
                            && pop_matching_inline(
                                &mut inline_styles,
                                InlineKind::Strikethrough,
                            ) =>
                    {
                        append_raw_text(&mut stack, &inline_styles, "</s>");
                    }
                    TagEnd::Link
                        if !is_in_image(&stack)
                            && pop_matching_inline(&mut inline_styles, InlineKind::Link) =>
                    {
                        append_raw_text(&mut stack, &inline_styles, "</a>");
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if is_in_code_block(&stack) || is_in_image(&stack) {
                    append_raw_text(&mut stack, &inline_styles, &t);
                } else {
                    append_escaped_text(&mut stack, &inline_styles, &t);
                }
            }
            Event::Code(c) => {
                if is_in_image(&stack) {
                    append_raw_text(&mut stack, &inline_styles, &c);
                } else {
                    append_inline_code(&mut stack, &inline_styles, &c);
                }
            }
            Event::SoftBreak => {
                if is_in_table_cell(&stack) {
                    append_raw_text(&mut stack, &inline_styles, " ");
                } else {
                    append_raw_text(&mut stack, &inline_styles, "\n");
                }
            }
            Event::HardBreak => {
                append_raw_text(&mut stack, &inline_styles, "\n");
            }
            Event::Rule => {
                maybe_close_implicit_paragraph(&mut stack, &inline_styles, &mut root_blocks);
                push_block(&mut root_blocks, &mut stack, ContentBlock::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(ParseContainer::Item { checked: c, .. }) = stack.last_mut() {
                    *c = Some(checked);
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                if is_html_line_break(&html) {
                    append_raw_text(&mut stack, &inline_styles, "\n");
                } else {
                    append_escaped_text(&mut stack, &inline_styles, &html);
                }
            }
            _ => {}
        }
    }

    // Flush unfinished containers for streaming support (closing any open inline tags)
    while let Some(container) = stack.pop() {
        let block = match container {
            ParseContainer::Paragraph(mut markup, _) => {
                close_inline_tags(&mut markup, &inline_styles);
                inline_styles.clear();
                finalize_paragraph(markup)
            }
            ParseContainer::Heading(level, mut markup) => {
                close_inline_tags(&mut markup, &inline_styles);
                inline_styles.clear();
                Some(ContentBlock::Heading { level, markup })
            }
            ParseContainer::BlockQuote { blocks } => {
                if blocks.is_empty() {
                    None
                } else {
                    Some(ContentBlock::Blockquote(blocks))
                }
            }
            ParseContainer::CodeBlock(lang, code) => Some(ContentBlock::Code { lang, code }),
            ParseContainer::Item { blocks, checked } => {
                if let Some(ParseContainer::List { items, .. }) = stack.last_mut() {
                    items.push(ListItem { blocks, checked });
                }
                None
            }
            ParseContainer::List {
                ordered,
                start,
                items,
            } => Some(ContentBlock::List {
                ordered,
                start,
                items,
            }),
            ParseContainer::Image { url, title, alt } => {
                if let Some(ParseContainer::TableCell { images, .. }) =
                    find_enclosing_table_cell(&mut stack)
                {
                    images.push(GalleryItem {
                        url,
                        title,
                        alt,
                        caption: None,
                    });
                    None
                } else {
                    Some(ContentBlock::Image { url, title, alt })
                }
            }
            ParseContainer::TableCell { mut markup, images } => {
                close_inline_tags(&mut markup, &inline_styles);
                inline_styles.clear();
                if let Some(ParseContainer::Table { current_row, .. }) =
                    find_enclosing_table(&mut stack)
                {
                    let cleaned = if has_visible_text_outside_tags(&markup) {
                        markup.trim().to_string()
                    } else {
                        String::new()
                    };
                    current_row.push(TableCell {
                        markup: cleaned,
                        images,
                    });
                }
                None
            }
            ParseContainer::Table {
                alignments,
                mut headers,
                mut rows,
                in_head,
                current_row,
            } => {
                if !current_row.is_empty() {
                    if in_head && headers.is_empty() {
                        headers = current_row.into_iter().map(|c| c.markup).collect();
                    } else {
                        rows.push(current_row);
                    }
                }
                Some(ContentBlock::Table {
                    alignments,
                    headers,
                    rows,
                })
            }
        };

        if let Some(b) = block {
            push_block(&mut root_blocks, &mut stack, b);
        }
    }

    if !may_have_gallery_candidates(&root_blocks) {
        return root_blocks;
    }

    coalesce_image_galleries(root_blocks)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineKind {
    Strong,
    Emphasis,
    Strikethrough,
    Link,
}

fn pop_matching_inline(styles: &mut Vec<InlineStyle>, expected: InlineKind) -> bool {
    let matches_kind = |s: &InlineStyle| {
        matches!(
            (s, expected),
            (InlineStyle::Strong, InlineKind::Strong)
                | (InlineStyle::Emphasis, InlineKind::Emphasis)
                | (InlineStyle::Strikethrough, InlineKind::Strikethrough)
                | (InlineStyle::Link(_), InlineKind::Link)
        )
    };
    if let Some(pos) = styles.iter().rposition(matches_kind) {
        styles.remove(pos);
        true
    } else {
        false
    }
}

fn close_inline_tags(out: &mut String, styles: &[InlineStyle]) {
    for style in styles.iter().rev() {
        out.push_str(style.close_tag());
    }
}

fn reopen_inline_tags(styles: &[InlineStyle]) -> String {
    if styles.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(styles.len() * 8);
    for style in styles {
        style.write_open(&mut out);
    }
    out
}

/// Returns true if `markup` contains at least one non-whitespace character outside Pango `<...>` tags.
fn has_visible_text_outside_tags(markup: &str) -> bool {
    let mut in_tag = false;
    for ch in markup.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag && !ch.is_whitespace() => return true,
            _ => {}
        }
    }
    false
}

fn finalize_paragraph(markup: String) -> Option<ContentBlock> {
    if !has_visible_text_outside_tags(&markup) {
        return None;
    }
    let trimmed = markup.trim();
    if trimmed.len() == markup.len() {
        Some(ContentBlock::Paragraph(markup))
    } else {
        Some(ContentBlock::Paragraph(trimmed.to_string()))
    }
}

/// Fast-path guard: returns `false` in O(blocks) without allocating or cloning when
/// the parsed document has no `Image`, `ImageGallery`, or media-list candidates (99%+ of turns).
fn may_have_gallery_candidates(blocks: &[ContentBlock]) -> bool {
    for block in blocks {
        match block {
            ContentBlock::Image { .. } | ContentBlock::ImageGallery { .. } => return true,
            ContentBlock::Blockquote(inner) => {
                if may_have_gallery_candidates(inner) {
                    return true;
                }
            }
            ContentBlock::List { items, .. } => {
                for item in items {
                    if may_have_gallery_candidates(&item.blocks) || item_may_contain_media_path(item)
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn item_may_contain_media_path(item: &ListItem) -> bool {
    if item.checked.is_some() || item.blocks.is_empty() || item.blocks.len() > 3 {
        return false;
    }
    item.blocks.iter().any(|b| match b {
        ContentBlock::Paragraph(m) | ContentBlock::Heading { markup: m, .. } => {
            m.len() <= 240 && (m.contains('/') || m.contains("file://"))
        }
        _ => false,
    })
}

/// Post-processes the parsed block tree to coalesce:
/// 1. Lists whose items are images (with optional short heading/caption paragraphs) into `ContentBlock::ImageGallery`.
/// 2. Top-level sequences of images—including `(Heading/Paragraph caption + Image [+ filename Paragraph])`
///    patterns commonly emitted by LLMs—into a single responsive `ContentBlock::ImageGallery`.
fn coalesce_image_galleries(blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
    let mut pass1 = Vec::with_capacity(blocks.len());

    for block in blocks {
        match block {
            ContentBlock::List {
                ordered,
                start,
                items,
            } => {
                if let Some(gallery_items) = try_extract_gallery_from_list(&items) {
                    pass1.push(ContentBlock::ImageGallery {
                        items: gallery_items,
                    });
                } else {
                    let recurse_items = items
                        .into_iter()
                        .map(|item| ListItem {
                            blocks: if may_have_gallery_candidates(&item.blocks) {
                                coalesce_image_galleries(item.blocks)
                            } else {
                                item.blocks
                            },
                            checked: item.checked,
                        })
                        .collect();
                    pass1.push(ContentBlock::List {
                        ordered,
                        start,
                        items: recurse_items,
                    });
                }
            }
            ContentBlock::Blockquote(inner) => {
                let processed = if may_have_gallery_candidates(&inner) {
                    coalesce_image_galleries(inner)
                } else {
                    inner
                };
                pass1.push(ContentBlock::Blockquote(processed));
            }
            other => pass1.push(other),
        }
    }

    coalesce_top_level_image_runs(pass1)
}

/// Scans a flat sequence of blocks and merges consecutive image entries
/// (where each entry is `[optional label Heading/Paragraph] + Image + [optional filename Paragraph]`)
/// into `ContentBlock::ImageGallery`.
fn coalesce_top_level_image_runs(blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
    let has_top_level_image = blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. } | ContentBlock::ImageGallery { .. }));
    if !has_top_level_image {
        return blocks;
    }

    let mut out: Vec<ContentBlock> = Vec::with_capacity(blocks.len());
    let mut i = 0;

    while i < blocks.len() {
        if let Some((first_items, next_idx)) = try_consume_image_entry(&blocks, i) {
            let mut gallery_items = first_items;
            let mut cursor = next_idx;

            while cursor < blocks.len() {
                if let Some((mut more_items, adv_idx)) = try_consume_image_entry(&blocks, cursor) {
                    gallery_items.append(&mut more_items);
                    cursor = adv_idx;
                } else {
                    break;
                }
            }

            if gallery_items.len() >= 2 {
                out.push(ContentBlock::ImageGallery {
                    items: gallery_items,
                });
                i = cursor;
                continue;
            } else if gallery_items.len() == 1 {
                let item = gallery_items.into_iter().next().unwrap();
                if item.caption.is_some() {
                    out.push(ContentBlock::ImageGallery { items: vec![item] });
                } else {
                    out.push(ContentBlock::Image {
                        url: item.url,
                        title: item.title,
                        alt: item.alt,
                    });
                }
                i = cursor;
                continue;
            }
        }

        out.push(blocks[i].clone());
        i += 1;
    }

    out
}

/// Attempts to consume one image entry starting at `blocks[idx]`.
fn try_consume_image_entry(
    blocks: &[ContentBlock],
    idx: usize,
) -> Option<(Vec<GalleryItem>, usize)> {
    match blocks.get(idx)? {
        ContentBlock::ImageGallery { items } => Some((items.clone(), idx + 1)),
        ContentBlock::Image { url, title, alt } => {
            let mut next = idx + 1;
            let mut caption = None;
            if let Some(ContentBlock::Paragraph(p)) = blocks.get(next)
                && is_short_image_label(p)
                && (caption_matches_image(p, url, alt)
                    || !matches!(blocks.get(next + 1), Some(ContentBlock::Image { .. })))
            {
                caption = Some(p.clone());
                next += 1;
            }
            Some((
                vec![GalleryItem {
                    url: url.clone(),
                    title: title.clone(),
                    alt: alt.clone(),
                    caption,
                }],
                next,
            ))
        }
        ContentBlock::Heading { level, markup } => {
            if !is_short_image_label(markup) {
                return None;
            }
            match blocks.get(idx + 1)? {
                ContentBlock::ImageGallery { items } if items.len() == 1 => {
                    let mut cloned = items.clone();
                    if cloned[0].caption.is_none() {
                        cloned[0].caption = Some(markup.clone());
                    }
                    Some((cloned, idx + 2))
                }
                ContentBlock::Image { url, title, alt } => {
                    let mut next = idx + 2;
                    let mut trailing_caption = None;
                    if let Some(ContentBlock::Paragraph(p)) = blocks.get(next)
                        && is_short_image_label(p)
                        && caption_matches_image(p, url, alt)
                    {
                        trailing_caption = Some(p.clone());
                        next += 1;
                    }

                    let another_image_follows = is_followed_by_image_entry(blocks, next);

                    if *level >= 2 || caption_matches_image(markup, url, alt) || another_image_follows
                    {
                        let chosen_caption = trailing_caption.or_else(|| Some(markup.clone()));
                        Some((
                            vec![GalleryItem {
                                url: url.clone(),
                                title: title.clone(),
                                alt: alt.clone(),
                                caption: chosen_caption,
                            }],
                            next,
                        ))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        ContentBlock::Paragraph(markup) => {
            if !is_short_image_label(markup) {
                return None;
            }
            match blocks.get(idx + 1)? {
                ContentBlock::ImageGallery { items } if items.len() == 1 => {
                    let mut cloned = items.clone();
                    if cloned[0].caption.is_none() {
                        cloned[0].caption = Some(markup.clone());
                    }
                    Some((cloned, idx + 2))
                }
                ContentBlock::Image { url, title, alt } => {
                    let mut next = idx + 2;
                    let mut trailing_caption = None;
                    if let Some(ContentBlock::Paragraph(p)) = blocks.get(next)
                        && is_short_image_label(p)
                        && caption_matches_image(p, url, alt)
                    {
                        trailing_caption = Some(p.clone());
                        next += 1;
                    }
                    let labeled_pair_follows = is_followed_by_labeled_image_entry(blocks, next)
                        && !markup.trim_end().ends_with('.');

                    if caption_matches_image(markup, url, alt) || labeled_pair_follows {
                        let chosen_caption = trailing_caption.or_else(|| Some(markup.clone()));
                        Some((
                            vec![GalleryItem {
                                url: url.clone(),
                                title: title.clone(),
                                alt: alt.clone(),
                                caption: chosen_caption,
                            }],
                            next,
                        ))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_followed_by_image_entry(blocks: &[ContentBlock], next: usize) -> bool {
    matches!(
        blocks.get(next),
        Some(ContentBlock::Image { .. } | ContentBlock::ImageGallery { .. })
    ) || is_followed_by_labeled_image_entry(blocks, next)
}

fn is_followed_by_labeled_image_entry(blocks: &[ContentBlock], next: usize) -> bool {
    matches!(
        blocks.get(next),
        Some(ContentBlock::Heading { .. } | ContentBlock::Paragraph(_))
    ) && matches!(
        blocks.get(next + 1),
        Some(ContentBlock::Image { .. } | ContentBlock::ImageGallery { .. })
    )
}

fn is_short_image_label(markup: &str) -> bool {
    let trimmed = markup.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 160
        && !trimmed.ends_with(':')
        && !trimmed.contains('\n')
}

fn caption_matches_image(label_markup: &str, url: &str, alt: &str) -> bool {
    let stripped = strip_pango_markup(label_markup, false);
    let clean_label = stripped.trim().to_lowercase();
    if clean_label.is_empty() {
        return false;
    }
    let url_lower = url.to_lowercase();
    if url_lower.ends_with(&clean_label) || url_lower.contains(&format!("/{clean_label}.")) {
        return true;
    }
    let alt_lower = alt.trim().to_lowercase();
    !alt_lower.is_empty() && (clean_label == alt_lower || alt_lower.starts_with(&clean_label))
}

/// Checks if a `List` is a media-listing list (each bullet has an image/video/audio embed
/// or a direct link/absolute path to a media file, plus optional short caption blocks).
fn try_extract_gallery_from_list(items: &[ListItem]) -> Option<Vec<GalleryItem>> {
    if items.is_empty() {
        return None;
    }

    let mut gallery = Vec::with_capacity(items.len());

    for item in items {
        if item.checked.is_some() || item.blocks.is_empty() || item.blocks.len() > 3 {
            return None;
        }

        let mut found_img: Option<(String, String, String)> = None;
        let mut caption_parts: Vec<String> = Vec::new();

        for b in &item.blocks {
            match b {
                ContentBlock::Image { url, title, alt } => {
                    if found_img.is_some() {
                        return None;
                    }
                    found_img = Some((url.clone(), title.clone(), alt.clone()));
                }
                ContentBlock::Paragraph(markup) | ContentBlock::Heading { markup, .. } => {
                    if markup.len() > 240 {
                        return None;
                    }
                    if found_img.is_none()
                        && let Some((media_url, label_opt)) =
                            try_extract_media_path_from_markup(markup)
                    {
                        found_img = Some((media_url, String::new(), String::new()));
                        if let Some(lbl) = label_opt {
                            caption_parts.push(lbl);
                        }
                        continue;
                    }
                    let t = markup.trim().to_string();
                    if !t.is_empty() && !caption_parts.contains(&t) {
                        caption_parts.push(t);
                    }
                }
                _ => return None,
            }
        }

        let (url, title, alt) = found_img?;
        let caption = caption_parts.last().cloned();

        gallery.push(GalleryItem {
            url,
            title,
            alt,
            caption,
        });
    }

    Some(gallery)
}

fn has_media_extension(path_or_url: &str) -> bool {
    let trimmed = path_or_url.trim();
    const MEDIA_EXTS: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".svg", ".mp4", ".webm", ".mkv",
        ".mov", ".avi", ".mp3", ".flac", ".wav", ".ogg", ".m4a", ".opus", ".aac",
    ];
    MEDIA_EXTS.iter().any(|ext| {
        trimmed.len() >= ext.len()
            && trimmed[trimmed.len() - ext.len()..].eq_ignore_ascii_case(ext)
    })
}

fn try_extract_media_path_from_markup(markup: &str) -> Option<(String, Option<String>)> {
    let trimmed = markup.trim();
    if let Some(rest) = trimmed.strip_prefix("<a href=\"")
        && let Some(end_quote) = rest.find("\">")
        && let Some(inner) = rest[end_quote + 2..].strip_suffix("</a>")
    {
        let href = strip_pango_markup(&rest[..end_quote], false);
        if (href.starts_with('/') || href.starts_with("file://")) && has_media_extension(&href) {
            return Some((href, Some(inner.to_string())));
        }
    }

    let clean = trimmed
        .strip_prefix("<tt>")
        .and_then(|s| s.strip_suffix("</tt>"))
        .unwrap_or(trimmed)
        .trim();
    if (clean.starts_with('/') || clean.starts_with("file://"))
        && !clean.contains('\n')
        && has_media_extension(clean)
    {
        return Some((clean.to_string(), None));
    }
    None
}

fn is_html_line_break(html: &str) -> bool {
    let trimmed = html.trim();
    trimmed.eq_ignore_ascii_case("<br>")
        || trimmed.eq_ignore_ascii_case("<br/>")
        || trimmed.eq_ignore_ascii_case("<br />")
}

fn find_enclosing_table_cell(stack: &mut [ParseContainer]) -> Option<&mut ParseContainer> {
    stack
        .iter_mut()
        .rev()
        .find(|c| matches!(c, ParseContainer::TableCell { .. }))
}

fn find_enclosing_table(stack: &mut [ParseContainer]) -> Option<&mut ParseContainer> {
    stack
        .iter_mut()
        .rev()
        .find(|c| matches!(c, ParseContainer::Table { .. }))
}

fn flush_non_empty_top_paragraph(
    stack: &mut Vec<ParseContainer>,
    inline_styles: &[InlineStyle],
    root_blocks: &mut Vec<ContentBlock>,
) {
    let should_flush = matches!(
        stack.last(),
        Some(ParseContainer::Paragraph(m, _)) if has_visible_text_outside_tags(m)
    );
    if should_flush && let Some(ParseContainer::Paragraph(mut markup, implicit)) = stack.pop() {
        let trimmed_len = markup.trim_end().len();
        markup.truncate(trimmed_len);
        close_inline_tags(&mut markup, inline_styles);
        if let Some(block) = finalize_paragraph(markup) {
            push_block(root_blocks, stack, block);
        }
        if !implicit {
            stack.push(ParseContainer::Paragraph(
                reopen_inline_tags(inline_styles),
                false,
            ));
        }
    }
}

fn is_block_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::List(_)
            | Tag::Item
            | Tag::Image { .. }
            | Tag::Table(_)
    )
}

fn is_block_tag_end(tag: &TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::Image
            | TagEnd::Table
    )
}

fn is_in_code_block(stack: &[ParseContainer]) -> bool {
    matches!(stack.last(), Some(ParseContainer::CodeBlock(_, _)))
}

fn is_in_image(stack: &[ParseContainer]) -> bool {
    matches!(stack.last(), Some(ParseContainer::Image { .. }))
}

fn is_in_table_cell(stack: &[ParseContainer]) -> bool {
    stack
        .iter()
        .rev()
        .any(|c| matches!(c, ParseContainer::TableCell { .. }))
}

fn ensure_text_target<'a>(
    stack: &'a mut Vec<ParseContainer>,
    inline_styles: &[InlineStyle],
) -> Option<&'a mut String> {
    let needs_paragraph = matches!(
        stack.last(),
        Some(
            ParseContainer::Item { .. }
                | ParseContainer::List { .. }
                | ParseContainer::BlockQuote { .. }
        ) | None
    );

    if needs_paragraph {
        stack.push(ParseContainer::Paragraph(
            reopen_inline_tags(inline_styles),
            true,
        ));
    }

    match stack.last_mut()? {
        ParseContainer::Paragraph(s, _)
        | ParseContainer::Heading(_, s)
        | ParseContainer::CodeBlock(_, s)
        | ParseContainer::TableCell { markup: s, .. } => Some(s),
        ParseContainer::Image { alt, .. } => Some(alt),
        _ => None,
    }
}

fn append_raw_text(stack: &mut Vec<ParseContainer>, inline_styles: &[InlineStyle], text: &str) {
    if let Some(target) = ensure_text_target(stack, inline_styles) {
        target.push_str(text);
    }
}

fn append_escaped_text(stack: &mut Vec<ParseContainer>, inline_styles: &[InlineStyle], text: &str) {
    if let Some(target) = ensure_text_target(stack, inline_styles) {
        write_escaped_pango(target, text);
    }
}

fn append_inline_code(stack: &mut Vec<ParseContainer>, inline_styles: &[InlineStyle], code: &str) {
    if let Some(target) = ensure_text_target(stack, inline_styles) {
        target.push_str("<tt>");
        write_escaped_pango(target, code);
        target.push_str("</tt>");
    }
}

fn append_inline_open(
    stack: &mut Vec<ParseContainer>,
    inline_styles: &[InlineStyle],
    style: &InlineStyle,
) {
    if let Some(target) = ensure_text_target(stack, inline_styles) {
        style.write_open(target);
    }
}

fn maybe_close_implicit_paragraph(
    stack: &mut Vec<ParseContainer>,
    inline_styles: &[InlineStyle],
    root_blocks: &mut Vec<ContentBlock>,
) {
    let is_implicit = matches!(stack.last(), Some(ParseContainer::Paragraph(_, true)));

    if is_implicit && let Some(ParseContainer::Paragraph(mut markup, _)) = stack.pop() {
        close_inline_tags(&mut markup, inline_styles);
        if let Some(block) = finalize_paragraph(markup) {
            push_block(root_blocks, stack, block);
        }
    }
}

fn push_block(
    root_blocks: &mut Vec<ContentBlock>,
    stack: &mut [ParseContainer],
    block: ContentBlock,
) {
    // Find the nearest container that supports nested blocks (ListItem or BlockQuote)
    for container in stack.iter_mut().rev() {
        match container {
            ParseContainer::Item { blocks, .. } | ParseContainer::BlockQuote { blocks } => {
                blocks.push(block);
                return;
            }
            _ => {}
        }
    }
    root_blocks.push(block);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_list() {
        let input = "- Item 1\n- Item 2";
        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::List {
            ordered,
            start,
            items,
        } = &blocks[0]
        {
            assert!(!ordered);
            assert_eq!(*start, 1);
            assert_eq!(items.len(), 2);
            match &items[0].blocks[0] {
                ContentBlock::Paragraph(m) => assert_eq!(m, "Item 1"),
                _ => panic!("Expected Paragraph"),
            }
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_ordered_list_preserves_custom_start_number() {
        let input = "3. Step three\n4. Step four";
        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::List {
            ordered,
            start,
            items,
        } = &blocks[0]
        {
            assert!(*ordered);
            assert_eq!(*start, 3);
            assert_eq!(items.len(), 2);
        } else {
            panic!("Expected ordered List");
        }
    }

    #[test]
    fn test_nested_list() {
        let input = "1. Item 1\n   - Subitem A\n   - Subitem B\n2. Item 2";
        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::List { items, .. } = &blocks[0] {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].blocks.len(), 2);
            match &items[0].blocks[1] {
                ContentBlock::List {
                    items: sub_items, ..
                } => assert_eq!(sub_items.len(), 2),
                _ => panic!("Expected nested List"),
            }
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_task_list() {
        let input = "- [ ] Task 1\n- [x] Task 2";
        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::List { items, .. } = &blocks[0] {
            assert_eq!(items[0].checked, Some(false));
            assert_eq!(items[1].checked, Some(true));
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_inline_tag_splitting() {
        let input = "- foo **bar** baz";
        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::List { items, .. } = &blocks[0] {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].blocks.len(), 1);
            match &items[0].blocks[0] {
                ContentBlock::Paragraph(m) => assert_eq!(m, "foo <b>bar</b> baz"),
                _ => panic!("Expected Paragraph"),
            }
        }
    }

    #[test]
    fn test_link_and_strikethrough_markup() {
        let input = "- /path/to/[link](https://example.com?a=1&b=2): ~~old~~ 10GB";
        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::List { items, .. } = &blocks[0] {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].blocks.len(), 1);
            match &items[0].blocks[0] {
                ContentBlock::Paragraph(m) => {
                    assert_eq!(
                        m,
                        "/path/to/<a href=\"https://example.com?a=1&amp;b=2\">link</a>: <s>old</s> 10GB"
                    );
                }
                _ => panic!("Expected Paragraph"),
            }
        }
    }

    #[test]
    fn test_clickable_badge_image_does_not_emit_broken_pango_tags() {
        let input = "[![Build Status](https://example.com/badge.svg)](https://example.com/actions)";
        let blocks = parse_markdown(input);
        assert_eq!(
            blocks,
            vec![ContentBlock::Image {
                url: "https://example.com/badge.svg".to_string(),
                title: String::new(),
                alt: "Build Status".to_string(),
            }]
        );
    }

    #[test]
    fn test_image_inside_bold_paragraph_balances_pango_tags() {
        let input = "**Before ![diagram](https://example.com/a.png) After**";
        let blocks = parse_markdown(input);
        // Note: `**Before**` followed by `Image` and `**After**` may coalesce caption if matching,
        // here "Before" doesn't match "a.png" or "diagram", so we get [Paragraph("<b>Before</b>"), Image/Gallery].
        assert_eq!(blocks[0], ContentBlock::Paragraph("<b>Before</b>".to_string()));
        for block in &blocks {
            if let ContentBlock::Paragraph(m) = block {
                assert!(
                    gtk4::pango::parse_markup(m, '\0').is_ok(),
                    "Invalid Pango markup emitted: {m}"
                );
            }
        }
    }

    #[test]
    fn test_fenced_code_info_string_extracts_first_lowercased_word() {
        assert_eq!(normalize_fence_language("CSV title=\"Q3\""), "csv");
        assert_eq!(normalize_fence_language("Rust,no_run"), "rust");
        assert_eq!(normalize_fence_language("{.Python}"), "python");
        assert_eq!(normalize_fence_language("ts:src/module.ts"), "ts");
        assert_eq!(normalize_fence_language("c++"), "c++");

        let blocks = parse_markdown("```Rust,no_run title=\"src/lib.rs\"\nfn demo() {}\n```");
        assert_eq!(
            blocks,
            vec![ContentBlock::Code {
                lang: "rust".to_string(),
                code: "fn demo() {}\n".to_string(),
            }]
        );
    }

    #[test]
    fn test_single_image() {
        let input = "![Alt text](https://example.com/image.png \"Title\")";
        let blocks = parse_markdown(input);
        if let ContentBlock::Image { url, title, alt } = &blocks[0] {
            assert_eq!(url, "https://example.com/image.png");
            assert_eq!(title, "Title");
            assert_eq!(alt, "Alt text");
        } else {
            panic!("Expected Image");
        }
    }

    #[test]
    fn test_wallpaper_list_coalesces_into_gallery() {
        let input = "\
# Wallpapers

* ![img-1](https://example.com/wall-1.jpg)
  wall-1.jpg
* ![img-2](https://example.com/wall-2.jpeg)
  wall-2.jpeg
* ![img-3](https://example.com/wall-3.png)
  wall-3.png";

        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], ContentBlock::Heading { level: 1, .. }));
        if let ContentBlock::ImageGallery { items } = &blocks[1] {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0].url, "https://example.com/wall-1.jpg");
            assert_eq!(items[0].caption.as_deref(), Some("wall-1.jpg"));
            assert_eq!(items[1].url, "https://example.com/wall-2.jpeg");
            assert_eq!(items[1].caption.as_deref(), Some("wall-2.jpeg"));
        } else {
            panic!("Expected ImageGallery, got {:?}", blocks[1]);
        }
    }

    #[test]
    fn test_heading_and_image_pairs_coalesce_into_gallery() {
        let input = "\
### wall-1
![wall-1.jpeg](https://example.com/wall-1.jpeg)

### wall-2
![wall-2.jpeg](https://example.com/wall-2.jpeg)

### wall-3
![wall-3.jpeg](https://example.com/wall-3.jpeg)";

        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::ImageGallery { items } = &blocks[0] {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0].url, "https://example.com/wall-1.jpeg");
            assert_eq!(items[1].url, "https://example.com/wall-2.jpeg");
            assert_eq!(items[2].url, "https://example.com/wall-3.jpeg");
        } else {
            panic!("Expected ImageGallery, got {:?}", blocks[0]);
        }
    }

    #[test]
    fn test_gfm_table_parsing() {
        let input = "\
| ID | Title | Class | Workspace | State |
| :--- | :--- | :---: | ---: | :--- |
| 101 | **App One** | `org.example.AppOne` | 1 | Focused |
| 102 | App Two | `org.example.AppTwo` | 2 | Normal |";

        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::Table {
            alignments,
            headers,
            rows,
        } = &blocks[0]
        {
            assert_eq!(
                alignments,
                &vec![
                    TableAlignment::Left,
                    TableAlignment::Left,
                    TableAlignment::Center,
                    TableAlignment::Right,
                    TableAlignment::Left,
                ]
            );
            assert_eq!(
                headers,
                &vec!["ID", "Title", "Class", "Workspace", "State"]
            );
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0][0], "101");
            assert_eq!(rows[0][1], "<b>App One</b>");
            assert_eq!(rows[0][2], "<tt>org.example.AppOne</tt>");
            assert_eq!(rows[0][3], "1");
            assert_eq!(rows[0][4], "Focused");
        } else {
            panic!("Expected Table, got {:?}", blocks[0]);
        }
    }

    #[test]
    fn test_blockquote_preserves_nested_blocks_in_order() {
        let input = "\
> First line
>
> - Bullet A
> - Bullet B
>
> ```rs
> let x = 1;
> ```
>
> **Last** line";
        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::Blockquote(inner) = &blocks[0] {
            assert_eq!(inner.len(), 4);
            assert_eq!(inner[0], ContentBlock::Paragraph("First line".to_string()));
            assert!(matches!(inner[1], ContentBlock::List { .. }));
            assert!(matches!(inner[2], ContentBlock::Code { ref lang, .. } if lang == "rs"));
            assert_eq!(
                inner[3],
                ContentBlock::Paragraph("<b>Last</b> line".to_string())
            );
        } else {
            panic!("Expected Blockquote, got {:?}", blocks[0]);
        }
    }

    #[test]
    fn test_heading_and_bullet_list_pairs_coalesce_into_gallery() {
        let input = "\
### wall-1
* ![wall-1](https://example.com/wall-1.jpeg)
  wall-1.jpeg

### wall-2
* ![wall-2](https://example.com/wall-2.jpeg)
  wall-2.jpeg";

        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::ImageGallery { items } = &blocks[0] {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].url, "https://example.com/wall-1.jpeg");
            assert_eq!(items[0].caption.as_deref(), Some("wall-1.jpeg"));
            assert_eq!(items[1].url, "https://example.com/wall-2.jpeg");
            assert_eq!(items[1].caption.as_deref(), Some("wall-2.jpeg"));
        } else {
            panic!("Expected ImageGallery, got {:?}", blocks[0]);
        }
    }

    #[test]
    fn test_table_with_embedded_images_and_br_tags() {
        let input = "\
Querying Flathub index for latest application registrations.

| Application & Description | Screenshot |
| :--- | :--- |
| **Omnidict** (`com.ktechpit.omnidict`)<br>Offline multilingual dictionary and thesaurus. | ![Omnidict](https://example.com/omnidict.png) |
| **DiagNG** (`com.p1security.diagng`)<br/>Qualcomm baseband diagnostic monitor client. | ![DiagNG](https://example.com/diagng.png) |";

        let blocks = parse_markdown(input);
        assert_eq!(
            blocks.len(),
            2,
            "Expected [Paragraph, Table], got {blocks:?}"
        );
        assert_eq!(
            blocks[0],
            ContentBlock::Paragraph(
                "Querying Flathub index for latest application registrations.".to_string()
            )
        );

        if let ContentBlock::Table { headers, rows, .. } = &blocks[1] {
            assert_eq!(
                headers,
                &vec!["Application &amp; Description", "Screenshot"]
            );
            assert_eq!(rows.len(), 2);
            assert_eq!(
                rows[0][0].markup,
                "<b>Omnidict</b> (<tt>com.ktechpit.omnidict</tt>)\nOffline multilingual dictionary and thesaurus."
            );
            assert!(rows[0][0].images.is_empty());
            assert!(rows[0][1].markup.is_empty());
            assert_eq!(rows[0][1].images.len(), 1);
            assert_eq!(rows[0][1].images[0].url, "https://example.com/omnidict.png");
            assert_eq!(rows[0][1].images[0].alt, "Omnidict");

            assert_eq!(
                rows[1][0].markup,
                "<b>DiagNG</b> (<tt>com.p1security.diagng</tt>)\nQualcomm baseband diagnostic monitor client."
            );
            assert_eq!(rows[1][1].images.len(), 1);
            assert_eq!(rows[1][1].images[0].url, "https://example.com/diagng.png");
            assert_eq!(rows[1][1].images[0].alt, "DiagNG");
        } else {
            panic!("Expected Table, got {:?}", blocks[1]);
        }
    }

    #[test]
    fn test_intro_paragraph_not_swallowed_into_gallery() {
        let input = "\
Here are the latest screenshots from the build pipeline.

![shot-1](https://example.com/shot-1.png)
![shot-2](https://example.com/shot-2.png)";

        let blocks = parse_markdown(input);
        assert_eq!(
            blocks.len(),
            2,
            "Expected [Paragraph, ImageGallery], got {blocks:?}"
        );
        assert_eq!(
            blocks[0],
            ContentBlock::Paragraph(
                "Here are the latest screenshots from the build pipeline.".to_string()
            )
        );
        assert!(matches!(blocks[1], ContentBlock::ImageGallery { .. }));
    }

    #[test]
    fn test_strip_pango_markup_single_pass() {
        assert_eq!(
            strip_pango_markup(
                "<b>App &amp; Tools</b>\n<tt>&lt;v1&gt; &quot;ok&quot; &apos;s&#39;</tt>",
                true
            ),
            "App & Tools <v1> \"ok\" 's'"
        );
    }

    #[test]
    fn test_streaming_partial_chunks_always_emit_valid_pango_markup() {
        let full = "Check **[Docs](https://example.com?x=1&y=2)** and [![CI](https://example.com/ci.svg)](https://example.com) with ~~strikethrough~~!";
        for end in 1..=full.len() {
            if !full.is_char_boundary(end) {
                continue;
            }
            let slice = &full[..end];
            let blocks = parse_markdown(slice);
            for block in blocks {
                if let ContentBlock::Paragraph(m) | ContentBlock::Heading { markup: m, .. } = block
                {
                    // `<a>` is a GtkLabel markup extension on top of Pango; rewrite `<a href="...">`
                    // to `<u>` so `pango::parse_markup` validates XML balance + entity escaping.
                    let normalized = replace_anchor_tags_for_pango_check(&m);
                    assert!(
                        gtk4::pango::parse_markup(&normalized, '\0').is_ok(),
                        "Broken Pango markup at slice {slice:?} -> {m:?}"
                    );
                }
            }
        }
    }

    fn replace_anchor_tags_for_pango_check(markup: &str) -> String {
        let mut out = String::with_capacity(markup.len());
        let mut rest = markup;
        while let Some(pos) = rest.find("<a href=\"") {
            out.push_str(&rest[..pos]);
            let after_open = &rest[pos + 9..];
            let end_quote = after_open
                .find("\">")
                .expect("<a href=\" must have closing \">");
            let attr_val = &after_open[..end_quote];
            assert!(
                !attr_val.contains('"') && !attr_val.contains('<') && !attr_val.contains('>'),
                "Unescaped attribute char in href: {attr_val}"
            );
            // Also verify attribute entities via Pango by putting attr_val inside a dummy tag text
            assert!(
                gtk4::pango::parse_markup(&format!("<span>{attr_val}</span>"), '\0').is_ok(),
                "Invalid XML entities in href: {attr_val}"
            );
            out.push_str("<u>");
            rest = &after_open[end_quote + 2..];
        }
        out.push_str(rest);
        out.replace("</a>", "</u>")
    }
}
