use super::blocks::{ContentBlock, ListItem};
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
}

/// Parses a Markdown string into a sequence of ContentBlocks.
pub fn parse_markdown(input: &str) -> Vec<ContentBlock> {
    let parser = Parser::new_ext(input, pulldown_cmark::Options::all());

    let mut root_blocks = Vec::new();
    let mut stack: Vec<ParseContainer> = Vec::new();

    for event in parser {
        match event {
            Event::Start(tag) => {
                maybe_close_implicit_paragraph(&mut stack, &mut root_blocks);
                match tag {
                    Tag::Paragraph => stack.push(ParseContainer::Paragraph(String::new(), false)),
                    Tag::Heading { level, .. } => {
                        stack.push(ParseContainer::Heading(level as u8, String::new()))
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
                        stack.push(ParseContainer::Image {
                            url: dest_url.to_string(),
                            title: title.to_string(),
                            alt: String::new(),
                        });
                    }

                    // Inline styles
                    Tag::Strong => append_text(&mut stack, "<b>"),
                    Tag::Emphasis => append_text(&mut stack, "<i>"),
                    Tag::Strikethrough => append_text(&mut stack, "<s>"),
                    _ => {}
                }
            }
            Event::End(tag) => {
                if matches!(tag, TagEnd::Item | TagEnd::List(_) | TagEnd::BlockQuote(_)) {
                    maybe_close_implicit_paragraph(&mut stack, &mut root_blocks);
                }

                if let Some(container) = stack.pop() {
                    let block = match (tag, container) {
                        (TagEnd::Paragraph, ParseContainer::Paragraph(markup, _)) => {
                            if markup.trim().is_empty() {
                                None // Skip empty paragraphs (often created around images)
                            } else {
                                Some(ContentBlock::Paragraph(markup))
                            }
                        }
                        (TagEnd::Heading(_), ParseContainer::Heading(level, markup)) => {
                            Some(ContentBlock::Heading { level, markup })
                        }
                        (TagEnd::BlockQuote(_), ParseContainer::BlockQuote(markup)) => {
                            Some(ContentBlock::Blockquote(markup))
                        }
                        (TagEnd::CodeBlock, ParseContainer::CodeBlock(lang, code)) => {
                            Some(ContentBlock::Code { lang, code })
                        }
                        (TagEnd::Item, ParseContainer::Item { blocks, checked }) => {
                            if let Some(ParseContainer::List { items, .. }) = stack.last_mut() {
                                items.push(ListItem { blocks, checked });
                            }
                            None
                        }
                        (TagEnd::List(_), ParseContainer::List { ordered, items }) => {
                            Some(ContentBlock::List { ordered, items })
                        }
                        (TagEnd::Image, ParseContainer::Image { url, title, alt }) => {
                            Some(ContentBlock::Image { url, title, alt })
                        }
                        _ => None,
                    };

                    if let Some(b) = block {
                        push_block(&mut root_blocks, &mut stack, b);
                    }
                }
            }
            Event::Text(t) => {
                let escaped = if is_in_code_block(&stack) {
                    t.to_string()
                } else {
                    escape_pango(&t)
                };
                append_text(&mut stack, &escaped);
            }
            Event::Code(c) => {
                append_text(&mut stack, "<tt>");
                append_text(&mut stack, &escape_pango(&c));
                append_text(&mut stack, "</tt>");
            }
            Event::SoftBreak | Event::HardBreak => {
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
            Event::Html(html) => {
                append_text(&mut stack, &escape_pango(&html));
            }
            _ => {}
        }
    }

    // Flush unfinished containers for streaming support
    while let Some(container) = stack.pop() {
        let block = match container {
            ParseContainer::Paragraph(markup, _) => {
                if markup.trim().is_empty() {
                    None
                } else {
                    Some(ContentBlock::Paragraph(markup))
                }
            }
            ParseContainer::Heading(level, markup) => Some(ContentBlock::Heading { level, markup }),
            ParseContainer::BlockQuote(markup) => Some(ContentBlock::Blockquote(markup)),
            ParseContainer::CodeBlock(lang, code) => Some(ContentBlock::Code { lang, code }),
            ParseContainer::Item { blocks, checked } => {
                if let Some(ParseContainer::List { items, .. }) = stack.last_mut() {
                    items.push(ListItem { blocks, checked });
                }
                None
            }
            ParseContainer::List { ordered, items } => Some(ContentBlock::List { ordered, items }),
            ParseContainer::Image { url, title, alt } => {
                Some(ContentBlock::Image { url, title, alt })
            }
        };

        if let Some(b) = block {
            push_block(&mut root_blocks, &mut stack, b);
        }
    }

    root_blocks
}

fn is_in_code_block(stack: &[ParseContainer]) -> bool {
    matches!(stack.last(), Some(ParseContainer::CodeBlock(_, _)))
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
            ParseContainer::Paragraph(s, _) => s.push_str(text),
            ParseContainer::Heading(_, s) => s.push_str(text),
            ParseContainer::BlockQuote(s) => s.push_str(text),
            ParseContainer::CodeBlock(_, s) => s.push_str(text),
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

    if is_implicit {
        if let Some(ParseContainer::Paragraph(markup, _)) = stack.pop() {
            if !markup.trim().is_empty() {
                push_block(root_blocks, stack, ContentBlock::Paragraph(markup));
            }
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
        }
    }

    #[test]
    fn test_image() {
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
}
