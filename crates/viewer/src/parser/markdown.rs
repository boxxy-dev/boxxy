use super::blocks::ContentBlock;
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

/// Escapes text for safe inclusion in Pango markup.
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

/// Parses a Markdown string into a sequence of ContentBlocks.
pub fn parse_markdown(input: &str) -> Vec<ContentBlock> {
    let parser = Parser::new_ext(input, pulldown_cmark::Options::all());

    let mut blocks = Vec::new();
    let mut current_text = String::new();

    // Track state to handle incomplete streams
    let mut current_block_type = None;

    // For lists
    let mut in_list = false;
    let mut is_ordered = false;
    let mut current_list_items = Vec::new();

    // For code blocks
    let mut current_lang = String::new();
    let mut in_code_block = false;

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    current_block_type = Some(Tag::Paragraph);
                    current_text.clear();
                }
                Tag::Heading { level, .. } => {
                    current_block_type = Some(Tag::Heading {
                        level,
                        id: None,
                        classes: vec![],
                        attrs: vec![],
                    });
                    current_text.clear();
                }
                Tag::BlockQuote(k) => {
                    current_block_type = Some(Tag::BlockQuote(k));
                    current_text.clear();
                }
                Tag::CodeBlock(kind) => {
                    in_code_block = true;
                    current_block_type = Some(Tag::CodeBlock(kind.clone()));
                    current_text.clear();
                    if let CodeBlockKind::Fenced(lang) = kind {
                        current_lang = lang.to_string();
                    } else {
                        current_lang.clear();
                    }
                }
                Tag::List(first_item_number) => {
                    in_list = true;
                    is_ordered = first_item_number.is_some();
                    current_list_items.clear();
                    current_block_type = Some(Tag::List(first_item_number));
                }
                Tag::Item => {
                    current_block_type = Some(Tag::Item);
                    current_text.clear();
                }

                // Inline styles
                Tag::Strong => current_text.push_str("<b>"),
                Tag::Emphasis => current_text.push_str("<i>"),
                Tag::Strikethrough => current_text.push_str("<s>"),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if in_list {
                        current_text.push('\n');
                    } else {
                        blocks.push(ContentBlock::Paragraph(std::mem::take(&mut current_text)));
                        current_block_type = None;
                    }
                }
                TagEnd::Heading(level) => {
                    blocks.push(ContentBlock::Heading {
                        level: level as u8,
                        markup: std::mem::take(&mut current_text),
                    });
                    current_block_type = None;
                }
                TagEnd::BlockQuote(_) => {
                    blocks.push(ContentBlock::Blockquote(std::mem::take(&mut current_text)));
                    current_block_type = None;
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    blocks.push(ContentBlock::Code {
                        lang: std::mem::take(&mut current_lang),
                        code: std::mem::take(&mut current_text),
                    });
                    current_block_type = None;
                }
                TagEnd::Item => {
                    if current_text.ends_with('\n') {
                        current_text.pop();
                    }
                    current_list_items.push(std::mem::take(&mut current_text));
                }
                TagEnd::List(_) => {
                    in_list = false;
                    blocks.push(ContentBlock::List {
                        ordered: is_ordered,
                        items: std::mem::take(&mut current_list_items),
                    });
                    current_block_type = None;
                }

                // Inline styles
                TagEnd::Strong => current_text.push_str("</b>"),
                TagEnd::Emphasis => current_text.push_str("</i>"),
                TagEnd::Strikethrough => current_text.push_str("</s>"),
                _ => {}
            },
            Event::Text(t) => {
                if in_code_block {
                    current_text.push_str(&t);
                } else {
                    current_text.push_str(&escape_pango(&t));
                }
            }
            Event::Code(c) => {
                current_text.push_str("<tt>");
                current_text.push_str(&escape_pango(&c));
                current_text.push_str("</tt>");
            }
            Event::SoftBreak => {
                if in_code_block {
                    current_text.push('\n');
                } else {
                    current_text.push(' ');
                }
            }
            Event::HardBreak => {
                if in_code_block {
                    current_text.push('\n');
                } else {
                    current_text.push('\n');
                }
            }
            Event::Html(html) => {
                current_text.push_str(&escape_pango(&html));
            }
            _ => {}
        }
    }

    // Flush any unfinished block (essential for streaming)
    if let Some(tag) = current_block_type {
        match tag {
            Tag::Paragraph => blocks.push(ContentBlock::Paragraph(current_text)),
            Tag::Heading { level, .. } => blocks.push(ContentBlock::Heading {
                level: level as u8,
                markup: current_text,
            }),
            Tag::BlockQuote(_) => blocks.push(ContentBlock::Blockquote(current_text)),
            Tag::CodeBlock(_) => {
                blocks.push(ContentBlock::Code {
                    lang: current_lang,
                    code: current_text,
                });
            }
            Tag::List(_) => {
                blocks.push(ContentBlock::List {
                    ordered: is_ordered,
                    items: current_list_items,
                });
            }
            Tag::Item => {
                if current_text.ends_with('\n') {
                    current_text.pop();
                }
                current_list_items.push(current_text);
                blocks.push(ContentBlock::List {
                    ordered: is_ordered,
                    items: current_list_items,
                });
            }
            _ => {}
        }
    } else if !current_text.is_empty() && !in_list {
        // Fallback for raw text not wrapped in any element
        blocks.push(ContentBlock::Paragraph(current_text));
    }

    blocks
}
