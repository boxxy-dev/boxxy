//! Multimodal user-message construction with cursor-ordered `[Text, Image, Text, Image]` interleaving.

use super::{ATTACHMENTS_CONTEXT_MARKER, extract_user_authored_prompt};
use boxxy_claw_protocol::ImageAttachmentMeta;

pub(crate) fn sniff_base64_image_media_type(b64: &str) -> rig::message::ImageMediaType {
    use base64::Engine as _;

    let trimmed = b64.trim();
    // Decode up to 16 base64 chars (12 raw bytes — covers PNG, JPEG, GIF, and RIFF/WEBP headers)
    // onto the stack without heap allocation.
    let take = trimmed.len().min(16) & !3;
    let mut buf = [0_u8; 12];
    if take >= 4
        && let Ok(n) = base64::engine::general_purpose::STANDARD
            .decode_slice(&trimmed.as_bytes()[..take], &mut buf)
        && let Some(media) = rig::message::ImageMediaType::sniff(&buf[..n])
    {
        return media;
    }
    rig::message::ImageMediaType::PNG
}

fn is_valid_at_mention_boundary(line: &str, byte_start: usize, byte_end: usize) -> bool {
    let before_ok = if byte_start == 0 {
        true
    } else {
        line[..byte_start]
            .chars()
            .next_back()
            .is_none_or(|c| {
                c.is_whitespace() || matches!(c, '(' | '[' | ']' | '{' | '}' | '"' | '\'' | ',' | ';')
            })
    };
    if !before_ok {
        return false;
    }

    if byte_end >= line.len() {
        return true;
    }
    let rest = &line[byte_end..];
    let Some(next_ch) = rest.chars().next() else {
        return true;
    };
    if next_ch.is_whitespace() || matches!(next_ch, ',' | ';' | '!' | '?' | ')' | ']' | '}' | '"' | '\'') {
        return true;
    }
    if next_ch == '.' {
        let after_dot = &rest[1..];
        return after_dot.is_empty()
            || after_dot
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace() || matches!(c, ',' | ';' | '!' | '?' | ')' | ']' | '}'));
    }
    false
}

/// Finds all non-overlapping `(byte_start, byte_end, image_idx)` token occurrences in `user_head`
/// outside fenced code blocks (` ``` `), prioritizing longer tokens first.
fn find_inline_image_matches(
    user_head: &str,
    image_meta: &[ImageAttachmentMeta],
    images_len: usize,
) -> Vec<(usize, usize, usize)> {
    let mut candidates: Vec<(usize, &str)> = image_meta
        .iter()
        .take(images_len)
        .enumerate()
        .filter(|(_, m)| !m.token.is_empty())
        .map(|(idx, m)| (idx, m.token.as_str()))
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }

    candidates.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

    let mut matches: Vec<(usize, usize, usize)> = Vec::new();
    let mut active_fence_ticks: Option<usize> = None;
    let mut line_byte_offset = 0usize;

    for raw_line in user_head.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        if let Some(fence_len) = active_fence_ticks {
            let trimmed = line.trim();
            let ticks = trimmed.bytes().take_while(|&b| b == b'`').count();
            if ticks >= fence_len && trimmed.len() == ticks {
                active_fence_ticks = None;
            }
            line_byte_offset += raw_line.len();
            continue;
        }
        let trimmed_start = line.trim_start();
        let ticks = trimmed_start.bytes().take_while(|&b| b == b'`').count();
        if ticks >= 3 {
            active_fence_ticks = Some(ticks);
            line_byte_offset += raw_line.len();
            continue;
        }

        if !line.is_empty() {
            for &(img_idx, tok) in &candidates {
                let is_at_mention = tok.starts_with('@');
                let mut from = 0usize;
                while let Some(rel) = line[from..].find(tok) {
                    let local_start = from + rel;
                    let local_end = local_start + tok.len();
                    if is_at_mention && !is_valid_at_mention_boundary(line, local_start, local_end)
                    {
                        from = local_start + 1;
                        continue;
                    }
                    let abs_start = line_byte_offset + local_start;
                    let abs_end = line_byte_offset + local_end;
                    let overlaps = matches
                        .iter()
                        .any(|&(s, e, _)| abs_start < e && abs_end > s);
                    if !overlaps {
                        matches.push((abs_start, abs_end, img_idx));
                    }
                    from = local_end;
                }
            }
        }

        line_byte_offset += raw_line.len();
    }

    matches.sort_unstable_by_key(|&(s, _, _)| s);
    matches
}

#[must_use]
fn format_annotated_image_tag(meta: &ImageAttachmentMeta, one_based_idx: usize) -> String {
    if meta.token.starts_with('@') {
        return meta.token.clone();
    }

    if !meta.token.is_empty() {
        let inner = meta.token.trim_matches(['[', ']']);
        if meta.label.is_empty()
            || meta.label == "Pasted Image"
            || inner.eq_ignore_ascii_case(&meta.label)
            || inner.contains(&meta.label)
        {
            return meta.token.clone();
        }
        let head = inner.split_once(" · ").map_or(inner, |(h, _)| h);
        return format!("[{head}: {}]", meta.label);
    }

    if meta.label.is_empty() || meta.label == "Pasted Image" {
        format!("[Attached Image {one_based_idx}]")
    } else {
        format!("[Attached Image {one_based_idx}: {}]", meta.label)
    }
}

/// Merges msgbar image attachments with `@image.png` workspace references and orders them
/// by their first appearance in the user-authored prompt so UI thumbnails, `current_turn_user_images`,
/// and multimodal `UserContent` blocks all share the exact reading order of the sentence.
#[must_use]
pub(crate) fn merge_and_order_turn_images(
    prompt: &str,
    images: Vec<String>,
    mut meta: Vec<ImageAttachmentMeta>,
    extra_images: Vec<String>,
    extra_meta: &[ImageAttachmentMeta],
) -> (Vec<String>, Vec<ImageAttachmentMeta>) {
    if meta.len() < images.len() {
        meta.resize(images.len(), ImageAttachmentMeta::default());
    } else {
        meta.truncate(images.len());
    }

    let mut pairs: Vec<(String, ImageAttachmentMeta)> = images.into_iter().zip(meta).collect();

    for (idx, b64) in extra_images.into_iter().enumerate() {
        let m = extra_meta.get(idx).cloned().unwrap_or_default();
        if let Some(existing) = pairs.iter_mut().find(|(existing_b64, _)| *existing_b64 == b64) {
            if existing.1.token.is_empty() && !m.token.is_empty() {
                existing.1 = m;
            }
        } else {
            pairs.push((b64, m));
        }
    }

    if pairs.len() <= 1 {
        return pairs.into_iter().unzip();
    }

    let user_head = extract_user_authored_prompt(prompt);
    let current_meta: Vec<ImageAttachmentMeta> = pairs.iter().map(|(_, m)| m.clone()).collect();
    let matches = find_inline_image_matches(user_head, &current_meta, pairs.len());

    let mut first_pos = vec![usize::MAX; pairs.len()];
    for (byte_start, _, img_idx) in matches {
        if first_pos[img_idx] == usize::MAX {
            first_pos[img_idx] = byte_start;
        }
    }

    let mut indexed: Vec<(usize, usize, String, ImageAttachmentMeta)> = pairs
        .into_iter()
        .enumerate()
        .map(|(orig_idx, (b64, m))| (first_pos[orig_idx], orig_idx, b64, m))
        .collect();
    indexed.sort_by_key(|&(pos, orig_idx, _, _)| (pos, orig_idx));

    let mut out_images = Vec::with_capacity(indexed.len());
    let mut out_meta = Vec::with_capacity(indexed.len());
    for (_, _, b64, m) in indexed {
        out_images.push(b64);
        out_meta.push(m);
    }
    (out_images, out_meta)
}

/// Annotates inline `[Image N]` tokens with filenames (e.g. `[Image 1: diagram.png]`) for
/// `SessionState.history` text persistence so subsequent turns retain image filenames and positions
/// without re-sending multi-megabyte base64 payloads on every future turn.
#[must_use]
pub(crate) fn annotate_prompt_for_history(
    prompt_with_attachments: &str,
    images_len: usize,
    image_meta: &[ImageAttachmentMeta],
) -> String {
    if images_len == 0 {
        return prompt_with_attachments.to_string();
    }

    let (user_head, attachments_tail) = prompt_with_attachments
        .find(ATTACHMENTS_CONTEXT_MARKER)
        .map_or((prompt_with_attachments, ""), |idx| {
            (
                &prompt_with_attachments[..idx],
                &prompt_with_attachments[idx..],
            )
        });

    let matches = find_inline_image_matches(user_head, image_meta, images_len);
    let mut mentioned = vec![false; images_len];
    let mut out = String::with_capacity(prompt_with_attachments.len() + 64);
    let mut cursor = 0usize;

    for (b_start, b_end, img_idx) in matches {
        out.push_str(&user_head[cursor..b_start]);
        let meta = image_meta.get(img_idx).cloned().unwrap_or_default();
        out.push_str(&format_annotated_image_tag(&meta, img_idx + 1));
        mentioned[img_idx] = true;
        cursor = b_end;
    }
    out.push_str(&user_head[cursor..]);

    for (img_idx, was_mentioned) in mentioned.into_iter().enumerate() {
        if !was_mentioned {
            let meta = image_meta.get(img_idx).cloned().unwrap_or_default();
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&format_annotated_image_tag(&meta, img_idx + 1));
        }
    }

    out.push_str(attachments_tail);
    out
}

/// Builds a multimodal `rig::message::Message::User` that interleaves `UserContent::Text` and
/// `UserContent::Image` blocks at the exact cursor positions of `[Image N]` and `@image.png`
/// tokens, keeping `--- ATTACHMENTS ---` and `## CURRENT TURN CONTEXT` cleanly after the user's
/// visual prompt.
#[must_use]
pub(crate) fn build_interleaved_user_message(
    prompt_with_attachments: &str,
    turn_tail: &str,
    images: Vec<String>,
    image_meta: &[ImageAttachmentMeta],
) -> rig::message::Message {
    if images.is_empty() {
        return rig::message::Message::user(format!("{prompt_with_attachments}{turn_tail}"));
    }

    let (user_head, attachments_tail) = prompt_with_attachments
        .find(ATTACHMENTS_CONTEXT_MARKER)
        .map_or((prompt_with_attachments, ""), |idx| {
            (
                &prompt_with_attachments[..idx],
                &prompt_with_attachments[idx..],
            )
        });

    let matches = find_inline_image_matches(user_head, image_meta, images.len());
    let mut emitted = vec![false; images.len()];
    let mut contents: Vec<rig::message::UserContent> = Vec::with_capacity(images.len() * 2 + 1);
    let mut text_buf = String::new();
    let mut cursor = 0usize;

    for (b_start, b_end, img_idx) in matches {
        text_buf.push_str(&user_head[cursor..b_start]);
        let meta = image_meta.get(img_idx).cloned().unwrap_or_default();
        let tag = format_annotated_image_tag(&meta, img_idx + 1);
        text_buf.push_str(&tag);
        cursor = b_end;

        // Emit the heavy base64 image payload at its first inline anchor; subsequent references
        // to the same token keep `[Image N: filename]` text without duplicating vision tokens.
        if !emitted[img_idx] {
            emitted[img_idx] = true;
            if !text_buf.is_empty() {
                contents.push(rig::message::UserContent::text(std::mem::take(&mut text_buf)));
            }
            let b64 = images[img_idx].clone();
            let media_type = sniff_base64_image_media_type(&b64);
            contents.push(rig::message::UserContent::image_base64(
                b64,
                Some(media_type),
                None,
            ));
        }
    }

    text_buf.push_str(&user_head[cursor..]);

    // Fallback: any image without an inline token in `user_head` (e.g. legacy history recall or
    // programmatic submission) is attached immediately after the user prompt and BEFORE turn context.
    for (img_idx, b64) in images.into_iter().enumerate() {
        if !emitted[img_idx] {
            let meta = image_meta.get(img_idx).cloned().unwrap_or_default();
            let tag = format_annotated_image_tag(&meta, img_idx + 1);
            if !text_buf.is_empty() {
                text_buf.push_str("\n\n");
            }
            text_buf.push_str(&tag);
            contents.push(rig::message::UserContent::text(std::mem::take(&mut text_buf)));
            let media_type = sniff_base64_image_media_type(&b64);
            contents.push(rig::message::UserContent::image_base64(
                b64,
                Some(media_type),
                None,
            ));
        }
    }

    text_buf.push_str(attachments_tail);
    text_buf.push_str(turn_tail);
    if !text_buf.is_empty() {
        contents.push(rig::message::UserContent::text(text_buf));
    }

    rig::message::Message::User { content: contents }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_part_summary(msg: &rig::message::Message) -> Vec<String> {
        let rig::message::Message::User { content } = msg else {
            panic!("expected User message");
        };
        content
            .iter()
            .map(|part| match part {
                rig::message::UserContent::Text(t) => format!("TEXT({})", t.text),
                rig::message::UserContent::Image(img) => {
                    let b64 = match &img.data {
                        rig::message::DocumentSourceKind::Base64(s) => s.as_str(),
                        _ => "other",
                    };
                    format!("IMAGE({b64},{:?})", img.media_type)
                }
                _ => "OTHER".to_string(),
            })
            .collect()
    }

    #[test]
    fn sniffs_jpeg_webp_gif_and_png_base64_prefixes() {
        assert_eq!(
            sniff_base64_image_media_type("/9j/4AAQSkZJRgABAQ"),
            rig::message::ImageMediaType::JPEG
        );
        assert_eq!(
            sniff_base64_image_media_type("UklGRh4AAABXRUJQVlA4"),
            rig::message::ImageMediaType::WEBP
        );
        assert_eq!(
            sniff_base64_image_media_type("R0lGODlhAQABAIAAAP"),
            rig::message::ImageMediaType::GIF
        );
        assert_eq!(
            sniff_base64_image_media_type("iVBORw0KGgoAAAANSUhEUg"),
            rig::message::ImageMediaType::PNG
        );
    }

    #[test]
    fn interleaves_multiple_inline_images_before_turn_context_and_deduplicates_repeats() {
        let jpg_a = "/9j/4AAQSkZJRgABAQ".to_string();
        let png_b = "iVBORw0KGgoAAAANSUhEUg".to_string();
        let meta = vec![
            ImageAttachmentMeta {
                token: "[Image 1 · chart_a.jpg]".to_string(),
                label: "chart_a.jpg".to_string(),
            },
            ImageAttachmentMeta {
                token: "[Image 2 · diagram_b.png]".to_string(),
                label: "diagram_b.png".to_string(),
            },
        ];

        let msg = build_interleaved_user_message(
            "what is this? [Image 1 · chart_a.jpg] and what is that? [Image 2 · diagram_b.png] vs [Image 1 · chart_a.jpg]",
            "\n\n## CURRENT TURN CONTEXT\nCWD: `/workspace`",
            vec![jpg_a.clone(), png_b.clone()],
            &meta,
        );

        let parts = collect_part_summary(&msg);
        assert_eq!(
            parts,
            vec![
                "TEXT(what is this? [Image 1 · chart_a.jpg])".to_string(),
                format!("IMAGE({jpg_a},Some(JPEG))"),
                "TEXT( and what is that? [Image 2 · diagram_b.png])".to_string(),
                format!("IMAGE({png_b},Some(PNG))"),
                "TEXT( vs [Image 1 · chart_a.jpg]\n\n## CURRENT TURN CONTEXT\nCWD: `/workspace`)"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn handles_mixed_at_file_mentions_adjacent_tokens_and_code_fences_without_empty_text_parts() {
        let png_1 = "iVBORw0KGgoAAAANSUhEUg".to_string();
        let webp_2 = "UklGRh4AAABXRUJQVlA4".to_string();
        let prompt = "```md\n[Image 1]\n```\n[Image 1]@assets/mockup.webp done";

        let (ordered_imgs, ordered_meta) = merge_and_order_turn_images(
            prompt,
            vec![png_1.clone()],
            vec![ImageAttachmentMeta {
                token: "[Image 1]".to_string(),
                label: "Pasted Image".to_string(),
            }],
            vec![webp_2.clone()],
            &[ImageAttachmentMeta {
                token: "@assets/mockup.webp".to_string(),
                label: "mockup.webp".to_string(),
            }],
        );

        let msg = build_interleaved_user_message(prompt, "", ordered_imgs, &ordered_meta);
        let parts = collect_part_summary(&msg);
        assert_eq!(
            parts,
            vec![
                "TEXT(```md\n[Image 1]\n```\n[Image 1])".to_string(),
                format!("IMAGE({png_1},Some(PNG))"),
                "TEXT(@assets/mockup.webp)".to_string(),
                format!("IMAGE({webp_2},Some(WEBP))"),
                "TEXT( done)".to_string(),
            ]
        );
    }

    #[test]
    fn places_mentionless_fallback_images_before_attachments_and_turn_context() {
        let png = "iVBORw0KGgoAAAANSUhEUg".to_string();
        let msg = build_interleaved_user_message(
            "inspect this\n\n--- ATTACHMENTS ---\n\n**[File 1] · app.rs**\n```text\nfn main() {}\n```",
            "\n\nTerminal Snapshot:\n```\n$ ls\n```",
            vec![png.clone()],
            &[ImageAttachmentMeta {
                token: String::new(),
                label: "capture.png".to_string(),
            }],
        );

        let parts = collect_part_summary(&msg);
        assert_eq!(
            parts,
            vec![
                "TEXT(inspect this\n\n[Attached Image 1: capture.png])".to_string(),
                format!("IMAGE({png},Some(PNG))"),
                "TEXT(\n\n--- ATTACHMENTS ---\n\n**[File 1] · app.rs**\n```text\nfn main() {}\n```\n\nTerminal Snapshot:\n```\n$ ls\n```)".to_string(),
            ]
        );
    }
}
