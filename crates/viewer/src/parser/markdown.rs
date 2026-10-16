use super::blocks::{ContentBlock, GalleryItem, ListItem, TableAlignment, TableCell};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Parser, Tag, TagEnd};

/// Escapes text for safe inclusion in Pango markup body text.
#[must_use]
pub fn escape_pango(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escapes text for safe inclusion inside a double-quoted Pango XML attribute (e.g. `href="..."`).
#[must_use]
pub fn escape_pango_attr(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[derive(Debug)]
enum ParseContainer {
    List {
        ordered: bool,
        items: Vec<ListItem>,
    },
    Item {
        blocks: Vec<ContentBlock>,
        checked: Option<bool>,
    },
    Paragraph(String, bool), // markup, is_implicit
    Heading(u8, String),
    BlockQuote(String),
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
/// clickable links, and automatic multi-image gallery coalescing.
#[must_use]
pub fn parse_markdown(input: &str) -> Vec<ContentBlock> {
    let parser = Parser::new_ext(input, pulldown_cmark::Options::all());

    let mut root_blocks = Vec::new();
    let mut stack: Vec<ParseContainer> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => {
                if is_block_tag(&tag) {
                    maybe_close_implicit_paragraph(&mut stack, &mut root_blocks);
                }
                match tag {
                    Tag::Paragraph => {
                        stack.push(ParseContainer::Paragraph(String::new(), false));
                    }
                    Tag::Heading { level, .. } => {
                        stack.push(ParseContainer::Heading(level as u8, String::new()));
                    }
                    Tag::BlockQuote(_) => stack.push(ParseContainer::BlockQuote(String::new())),
                    Tag::CodeBlock(kind) => {
                        let lang = if let CodeBlockKind::Fenced(lang) = kind {
                            lang.to_string()
                        } else {
                            String::new()
                        };
                        stack.push(ParseContainer::CodeBlock(lang, String::new()));
                    }
                    Tag::List(first_item_number) => stack.push(ParseContainer::List {
                        ordered: first_item_number.is_some(),
                        items: Vec::new(),
                    }),
                    Tag::Item => stack.push(ParseContainer::Item {
                        blocks: Vec::new(),
                        checked: None,
                    }),
                    Tag::Image {
                        dest_url, title, ..
                    } => {
                        // If an image appears inside a non-empty paragraph, flush the
                        // preceding text first so captions/labels remain in document order.
                        flush_non_empty_top_paragraph(&mut stack, &mut root_blocks);
                        stack.push(ParseContainer::Image {
                            url: dest_url.to_string(),
                            title: title.to_string(),
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
                    Tag::Strong => append_text(&mut stack, "<b>"),
                    Tag::Emphasis => append_text(&mut stack, "<i>"),
                    Tag::Strikethrough => append_text(&mut stack, "<s>"),
                    Tag::Link { dest_url, .. } => {
                        if !is_in_image(&stack) {
                            let escaped_url = escape_pango_attr(&dest_url);
                            append_text(&mut stack, &format!("<a href=\"{escaped_url}\">"));
                        }
                    }
                    _ => {}
                }
            }
            Event::End(tag) => {
                if matches!(tag, TagEnd::Item | TagEnd::List(_) | TagEnd::BlockQuote(_)) {
                    maybe_close_implicit_paragraph(&mut stack, &mut root_blocks);
                }

                match tag {
                    TagEnd::TableCell => {
                        if let Some(ParseContainer::TableCell { markup, images }) = stack.pop()
                            && let Some(ParseContainer::Table { current_row, .. }) =
                                find_enclosing_table(&mut stack)
                        {
                            current_row.push(TableCell {
                                markup: markup.trim().to_string(),
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
                                (TagEnd::Paragraph, ParseContainer::Paragraph(markup, _)) => {
                                    let trimmed = markup.trim();
                                    if trimmed.is_empty() {
                                        None
                                    } else if let Some(bq) = find_enclosing_blockquote(&mut stack) {
                                        if !bq.is_empty() {
                                            bq.push_str("\n\n");
                                        }
                                        bq.push_str(trimmed);
                                        None
                                    } else {
                                        Some(ContentBlock::Paragraph(trimmed.to_string()))
                                    }
                                }
                                (TagEnd::Heading(_), ParseContainer::Heading(level, markup)) => {
                                    Some(ContentBlock::Heading { level, markup })
                                }
                                (TagEnd::BlockQuote(_), ParseContainer::BlockQuote(markup)) => {
                                    if markup.trim().is_empty() {
                                        None
                                    } else {
                                        Some(ContentBlock::Blockquote(markup))
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
                                (TagEnd::List(_), ParseContainer::List { ordered, items }) => {
                                    Some(ContentBlock::List { ordered, items })
                                }
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
                    TagEnd::Strong => append_text(&mut stack, "</b>"),
                    TagEnd::Emphasis => append_text(&mut stack, "</i>"),
                    TagEnd::Strikethrough => append_text(&mut stack, "</s>"),
                    TagEnd::Link => {
                        if !is_in_image(&stack) {
                            append_text(&mut stack, "</a>");
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                let escaped = if is_in_code_block(&stack) || is_in_image(&stack) {
                    t.to_string()
                } else {
                    escape_pango(&t)
                };
                append_text(&mut stack, &escaped);
            }
            Event::Code(c) => {
                if is_in_image(&stack) {
                    append_text(&mut stack, &c);
                } else {
                    append_text(&mut stack, "<tt>");
                    append_text(&mut stack, &escape_pango(&c));
                    append_text(&mut stack, "</tt>");
                }
            }
            Event::SoftBreak => {
                if !is_in_table_cell(&stack) {
                    append_text(&mut stack, "\n");
                } else {
                    append_text(&mut stack, " ");
                }
            }
            Event::HardBreak => {
                append_text(&mut stack, "\n");
            }
            Event::Rule => {
                maybe_close_implicit_paragraph(&mut stack, &mut root_blocks);
                push_block(&mut root_blocks, &mut stack, ContentBlock::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(ParseContainer::Item { checked: c, .. }) = stack.last_mut() {
                    *c = Some(checked);
                }
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                if is_html_line_break(&html) {
                    append_text(&mut stack, "\n");
                } else {
                    append_text(&mut stack, &escape_pango(&html));
                }
            }
            _ => {}
        }
    }

    // Flush unfinished containers for streaming support
    while let Some(container) = stack.pop() {
        let block = match container {
            ParseContainer::Paragraph(markup, _) => {
                let trimmed = markup.trim();
                if trimmed.is_empty() {
                    None
                } else if let Some(bq) = find_enclosing_blockquote(&mut stack) {
                    if !bq.is_empty() {
                        bq.push_str("\n\n");
                    }
                    bq.push_str(trimmed);
                    None
                } else {
                    Some(ContentBlock::Paragraph(trimmed.to_string()))
                }
            }
            ParseContainer::Heading(level, markup) => Some(ContentBlock::Heading { level, markup }),
            ParseContainer::BlockQuote(markup) => {
                if markup.trim().is_empty() {
                    None
                } else {
                    Some(ContentBlock::Blockquote(markup))
                }
            }
            ParseContainer::CodeBlock(lang, code) => Some(ContentBlock::Code { lang, code }),
            ParseContainer::Item { blocks, checked } => {
                if let Some(ParseContainer::List { items, .. }) = stack.last_mut() {
                    items.push(ListItem { blocks, checked });
                }
                None
            }
            ParseContainer::List { ordered, items } => Some(ContentBlock::List { ordered, items }),
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
            ParseContainer::TableCell { markup, images } => {
                if let Some(ParseContainer::Table { current_row, .. }) =
                    find_enclosing_table(&mut stack)
                {
                    current_row.push(TableCell {
                        markup: markup.trim().to_string(),
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

    coalesce_image_galleries(root_blocks)
}

/// Post-processes the parsed block tree to coalesce:
/// 1. Lists whose items are images (with optional short heading/caption paragraphs) into `ContentBlock::ImageGallery`.
/// 2. Top-level sequences of images—including `(Heading/Paragraph caption + Image [+ filename Paragraph])`
///    patterns commonly emitted by LLMs—into a single responsive `ContentBlock::ImageGallery`.
fn coalesce_image_galleries(blocks: Vec<ContentBlock>) -> Vec<ContentBlock> {
    let mut pass1 = Vec::with_capacity(blocks.len());

    for block in blocks {
        match block {
            ContentBlock::List { ordered, items } => {
                if let Some(gallery_items) = try_extract_gallery_from_list(&items) {
                    pass1.push(ContentBlock::ImageGallery {
                        items: gallery_items,
                    });
                } else {
                    let recurse_items = items
                        .into_iter()
                        .map(|item| ListItem {
                            blocks: coalesce_image_galleries(item.blocks),
                            checked: item.checked,
                        })
                        .collect();
                    pass1.push(ContentBlock::List {
                        ordered,
                        items: recurse_items,
                    });
                }
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
    let mut out: Vec<ContentBlock> = Vec::with_capacity(blocks.len());
    let mut i = 0;

    while i < blocks.len() {
        // Check if blocks[i..] starts an image run (either directly with Image/ImageGallery,
        // or with a short label Heading/Paragraph immediately followed by an Image).
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
                // Single image: if a redundant heading/caption was absorbed or it was already
                // a single-item gallery, emit a clean single-item ImageGallery (which renders
                // as a standalone card with caption) when a caption exists, or ContentBlock::Image.
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
/// Supported shapes:
/// - `ImageGallery { items }`
/// - `Image { .. } [+ optional redundant/filename Paragraph]`
/// - `Heading/Paragraph (short item label) + Image { .. } [+ optional redundant/filename Paragraph]`
fn try_consume_image_entry(
    blocks: &[ContentBlock],
    idx: usize,
) -> Option<(Vec<GalleryItem>, usize)> {
    match blocks.get(idx)? {
        ContentBlock::ImageGallery { items } => Some((items.clone(), idx + 1)),
        ContentBlock::Image { url, title, alt } => {
            let mut next = idx + 1;
            let mut caption = None;
            // Absorb a trailing short filename paragraph if it isn't the label for a subsequent image
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
    let clean_label = label_markup
        .replace("<b>", "")
        .replace("</b>", "")
        .replace("<tt>", "")
        .replace("</tt>", "")
        .trim()
        .to_lowercase();
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
                        return None; // More than one image in a single bullet item
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
        // Prefer the last caption part if one is a stem and the next is the full filename
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
    let lower = path_or_url.trim().to_lowercase();
    const MEDIA_EXTS: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".svg", ".mp4", ".webm", ".mkv",
        ".mov", ".avi", ".mp3", ".flac", ".wav", ".ogg", ".m4a", ".opus", ".aac",
    ];
    MEDIA_EXTS.iter().any(|ext| lower.ends_with(ext))
}

fn try_extract_media_path_from_markup(markup: &str) -> Option<(String, Option<String>)> {
    let trimmed = markup.trim();
    // Case 1: `<a href="/abs/path.mp4">label</a>`
    if let Some(rest) = trimmed.strip_prefix("<a href=\"")
        && let Some(end_quote) = rest.find("\">")
        && let Some(inner) = rest[end_quote + 2..].strip_suffix("</a>")
    {
        let href = rest[..end_quote]
            .replace("&amp;", "&")
            .replace("&quot;", "\"")
            .replace("&apos;", "'");
        if (href.starts_with('/') || href.starts_with("file://")) && has_media_extension(&href) {
            return Some((href, Some(inner.to_string())));
        }
    }

    // Case 2: `/abs/path.mp4` or `<tt>/abs/path.mp4</tt>`
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

fn find_enclosing_blockquote(stack: &mut [ParseContainer]) -> Option<&mut String> {
    for c in stack.iter_mut().rev() {
        if let ParseContainer::BlockQuote(s) = c {
            return Some(s);
        }
    }
    None
}

fn flush_non_empty_top_paragraph(
    stack: &mut Vec<ParseContainer>,
    root_blocks: &mut Vec<ContentBlock>,
) {
    let should_flush = matches!(
        stack.last(),
        Some(ParseContainer::Paragraph(m, _)) if !m.trim().is_empty()
    );
    if should_flush
        && let Some(ParseContainer::Paragraph(markup, implicit)) = stack.pop()
    {
        let trimmed = markup.trim().to_string();
        if !trimmed.is_empty() {
            push_block(root_blocks, stack, ContentBlock::Paragraph(trimmed));
        }
        if !implicit {
            stack.push(ParseContainer::Paragraph(String::new(), false));
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

fn append_text(stack: &mut Vec<ParseContainer>, text: &str) {
    let mut needs_paragraph = false;
    if let Some(container) = stack.last() {
        if matches!(
            container,
            ParseContainer::Item { .. } | ParseContainer::List { .. }
        ) {
            needs_paragraph = true;
        }
    } else {
        needs_paragraph = true;
    }

    if needs_paragraph {
        stack.push(ParseContainer::Paragraph(String::new(), true));
    }

    if let Some(container) = stack.last_mut() {
        match container {
            ParseContainer::Paragraph(s, _)
            | ParseContainer::Heading(_, s)
            | ParseContainer::BlockQuote(s)
            | ParseContainer::CodeBlock(_, s)
            | ParseContainer::TableCell { markup: s, .. } => s.push_str(text),
            ParseContainer::Image { alt, .. } => alt.push_str(text),
            _ => {}
        }
    }
}

fn maybe_close_implicit_paragraph(
    stack: &mut Vec<ParseContainer>,
    root_blocks: &mut Vec<ContentBlock>,
) {
    let is_implicit = if let Some(ParseContainer::Paragraph(_, implicit)) = stack.last() {
        *implicit
    } else {
        false
    };

    if is_implicit
        && let Some(ParseContainer::Paragraph(markup, _)) = stack.pop()
    {
        let trimmed = markup.trim();
        if !trimmed.is_empty() {
            push_block(
                root_blocks,
                stack,
                ContentBlock::Paragraph(trimmed.to_string()),
            );
        }
    }
}

fn push_block(
    root_blocks: &mut Vec<ContentBlock>,
    stack: &mut [ParseContainer],
    block: ContentBlock,
) {
    // Find the nearest container that supports nested blocks (Item)
    for container in stack.iter_mut().rev() {
        if let ParseContainer::Item { blocks, .. } = container {
            blocks.push(block);
            return;
        }
    }
    // Otherwise push to root
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
        if let ContentBlock::List { ordered, items } = &blocks[0] {
            assert!(!ordered);
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
    fn test_blockquote_preserves_content() {
        let input = "> First line\n>\n> **Second** line";
        let blocks = parse_markdown(input);
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::Blockquote(m) = &blocks[0] {
            assert_eq!(m, "First line\n\n<b>Second</b> line");
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
        assert_eq!(blocks.len(), 2, "Expected [Paragraph, Table], got {blocks:?}");
        assert_eq!(
            blocks[0],
            ContentBlock::Paragraph(
                "Querying Flathub index for latest application registrations.".to_string()
            )
        );

        if let ContentBlock::Table { headers, rows, .. } = &blocks[1] {
            assert_eq!(headers, &vec!["Application &amp; Description", "Screenshot"]);
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
        assert_eq!(blocks.len(), 2, "Expected [Paragraph, ImageGallery], got {blocks:?}");
        assert_eq!(
            blocks[0],
            ContentBlock::Paragraph(
                "Here are the latest screenshots from the build pipeline.".to_string()
            )
        );
        assert!(matches!(blocks[1], ContentBlock::ImageGallery { .. }));
    }
}
