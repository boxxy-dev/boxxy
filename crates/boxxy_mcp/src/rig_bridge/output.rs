use rig::message::{ImageMediaType, ToolResultContent};
use rig::tool::{ToolExecutionError, ToolOutput};
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents};
use serde_json::Value;

const EMPTY_ERROR_FALLBACK: &str = "MCP tool reported an execution error";

/// Normalizes an MCP MIME type (stripping `; ...` parameters, handling case-insensitivity
/// and `image/jpg` aliases) into a `rig::message::ImageMediaType`.
fn parse_image_media_type(raw_mime: &str) -> Option<ImageMediaType> {
    let essence = raw_mime.split(';').next().unwrap_or(raw_mime).trim();
    if essence.eq_ignore_ascii_case("image/jpeg") || essence.eq_ignore_ascii_case("image/jpg") {
        Some(ImageMediaType::JPEG)
    } else if essence.eq_ignore_ascii_case("image/png") {
        Some(ImageMediaType::PNG)
    } else if essence.eq_ignore_ascii_case("image/gif") {
        Some(ImageMediaType::GIF)
    } else if essence.eq_ignore_ascii_case("image/webp") {
        Some(ImageMediaType::WEBP)
    } else if essence.eq_ignore_ascii_case("image/heic") {
        Some(ImageMediaType::HEIC)
    } else if essence.eq_ignore_ascii_case("image/heif") {
        Some(ImageMediaType::HEIF)
    } else if essence.eq_ignore_ascii_case("image/svg+xml") {
        Some(ImageMediaType::SVG)
    } else {
        None
    }
}

/// Builds both the compact observer summary string and the `rig::tool::ToolOutput`.
///
/// Preserves multimodal images and SEP-2106 structured JSON natively while supporting
/// `rmcp 3.11+`'s contentless `CallToolResult`s.
///
/// # Errors
///
/// Returns [`ToolExecutionError`] if Rig rejects assembling a multimodal content payload.
pub fn build_tool_outputs(
    result: &CallToolResult,
) -> Result<(String, ToolOutput), ToolExecutionError> {
    let is_error = result.is_error == Some(true);
    let mut summary = format_call_tool_result(result);
    if summary.is_empty() && is_error {
        summary = EMPTY_ERROR_FALLBACK.to_string();
    }

    if has_supported_image_block(&result.content) {
        let multimodal = build_multimodal_output(result)?;
        return Ok((summary, multimodal));
    }

    if let Some(structured) = &result.structured_content {
        if structured.is_null() {
            if result.content.is_empty() && !is_error {
                return Ok((summary, ToolOutput::json(Value::Null)));
            }
            return Ok((summary.clone(), ToolOutput::text(summary)));
        }

        let rendered_blocks: Vec<String> = result
            .content
            .iter()
            .map(format_content_block)
            .filter(|s| !s.is_empty())
            .collect();
        let text_body = rendered_blocks.join("\n\n");

        if text_body.is_empty() {
            return Ok((summary, value_to_tool_output(structured)));
        }

        if let Some(inner_str) = unwrap_single_result_field(structured).and_then(Value::as_str)
            && text_body.trim() == inner_str.trim()
        {
            return Ok((summary, ToolOutput::text(text_body)));
        }

        let structured_str =
            serde_json::to_string(structured).unwrap_or_else(|_| structured.to_string());
        if is_redundant_text_representation(
            &rendered_blocks,
            &text_body,
            structured,
            &structured_str,
        ) {
            return Ok((summary, value_to_tool_output(structured)));
        }

        let combined = ToolOutput::content(vec![
            ToolResultContent::text(text_body),
            ToolResultContent::json(structured.clone()),
        ])?;
        return Ok((summary, combined));
    }

    Ok((summary.clone(), ToolOutput::text(summary)))
}

fn value_to_tool_output(val: &Value) -> ToolOutput {
    match val {
        Value::String(s) => ToolOutput::text(s.clone()),
        other => ToolOutput::json(other.clone()),
    }
}

fn has_supported_image_block(blocks: &[ContentBlock]) -> bool {
    blocks.iter().any(|block| match block {
        ContentBlock::Image(img) => parse_image_media_type(&img.mime_type).is_some(),
        ContentBlock::Resource(embedded) => match &embedded.resource {
            ResourceContents::BlobResourceContents {
                mime_type: Some(mime),
                ..
            } => parse_image_media_type(mime).is_some(),
            _ => false,
        },
        _ => false,
    })
}

fn build_multimodal_output(result: &CallToolResult) -> Result<ToolOutput, ToolExecutionError> {
    let mut parts: Vec<ToolResultContent> = Vec::with_capacity(result.content.len() + 1);
    let canonical_structured = result
        .structured_content
        .as_ref()
        .filter(|v| !v.is_null())
        .map(Value::to_string);
    let mut emitted_structured = false;

    for block in &result.content {
        match block {
            ContentBlock::Image(img) => {
                if let Some(media_type) = parse_image_media_type(&img.mime_type) {
                    parts.push(ToolResultContent::image_base64(
                        img.data.clone(),
                        Some(media_type),
                        None,
                    ));
                } else {
                    parts.push(ToolResultContent::text(format_content_block(block)));
                }
            }
            ContentBlock::Resource(embedded) => match &embedded.resource {
                ResourceContents::BlobResourceContents {
                    mime_type: Some(mime),
                    blob,
                    ..
                } if parse_image_media_type(mime).is_some() => {
                    let media_type = parse_image_media_type(mime);
                    parts.push(ToolResultContent::image_base64(
                        blob.clone(),
                        media_type,
                        None,
                    ));
                }
                _ => {
                    let text = format_content_block(block);
                    if !text.is_empty() {
                        parts.push(ToolResultContent::text(text));
                    }
                }
            },
            ContentBlock::Text(text) => {
                if !emitted_structured
                    && let (Some(canonical), Some(structured)) = (
                        canonical_structured.as_deref(),
                        result.structured_content.as_ref(),
                    )
                    && text.text.trim() == canonical.trim()
                {
                    parts.push(ToolResultContent::json(structured.clone()));
                    emitted_structured = true;
                } else if !text.text.is_empty() {
                    parts.push(ToolResultContent::text(text.text.clone()));
                }
            }
            _ => {
                let text = format_content_block(block);
                if !text.is_empty() {
                    parts.push(ToolResultContent::text(text));
                }
            }
        }
    }

    if let Some(structured) = result.structured_content.as_ref()
        && !structured.is_null()
        && !emitted_structured
    {
        parts.insert(0, ToolResultContent::json(structured.clone()));
    }

    if parts.is_empty() {
        return Ok(ToolOutput::text(""));
    }

    ToolOutput::content(parts)
}

/// Extracts a clean, human- and LLM-readable string from an MCP `CallToolResult`.
///
/// Handles SEP-2106 `structured_content` (any JSON value, including primitives/arrays/null)
/// and avoids wrapping plain text or `FastMCP` blocks in redundant JSON envelopes.
#[must_use]
pub fn format_call_tool_result(result: &CallToolResult) -> String {
    let rendered_blocks: Vec<String> = result
        .content
        .iter()
        .map(format_content_block)
        .filter(|s| !s.is_empty())
        .collect();

    let text_body = rendered_blocks.join("\n\n");

    match (&result.structured_content, text_body.is_empty()) {
        (Some(structured), true) => structured.as_str().map_or_else(
            || serde_json::to_string(structured).unwrap_or_else(|_| structured.to_string()),
            str::to_string,
        ),
        (Some(structured), false) if !structured.is_null() => {
            // If `structured` is FastMCP's single-key `{"result": "<string>"}` wrapper around
            // the exact same plain text block, return the plain text directly.
            if let Some(inner_str) = unwrap_single_result_field(structured).and_then(Value::as_str)
                && text_body.trim() == inner_str.trim()
            {
                return text_body;
            }

            let structured_str =
                serde_json::to_string(structured).unwrap_or_else(|_| structured.to_string());
            if is_redundant_text_representation(
                &rendered_blocks,
                &text_body,
                structured,
                &structured_str,
            ) {
                structured_str
            } else {
                serde_json::json!({
                    "content": text_body,
                    "structured_content": structured
                })
                .to_string()
            }
        }
        _ => text_body,
    }
}

fn unwrap_single_result_field(val: &Value) -> Option<&Value> {
    let obj = val.as_object()?;
    if obj.len() == 1 {
        obj.get("result")
    } else {
        None
    }
}

/// Checks whether the MCP `content` text blocks are merely a serialized mirror of `structured_content`
/// (covering standard JSON objects, `FastMCP` pretty-printed JSON, and `FastMCP` `{"result": [...]}` list blocks).
fn is_redundant_text_representation(
    rendered_blocks: &[String],
    text_body: &str,
    structured: &Value,
    structured_str: &str,
) -> bool {
    let trimmed = text_body.trim();
    if trimmed == structured_str.trim() {
        return true;
    }

    if looks_like_json(trimmed)
        && let Ok(parsed) = serde_json::from_str::<Value>(trimmed)
    {
        if &parsed == structured {
            return true;
        }
        if let Some(inner) = unwrap_single_result_field(structured)
            && &parsed == inner
        {
            return true;
        }
    }

    // FastMCP serializes `list[T]` return values as N separate TextContent JSON blocks
    // alongside `structured_content: {"result": [item_0, item_1, ...]}`.
    if let Some(Value::Array(arr)) = unwrap_single_result_field(structured)
        && arr.len() == rendered_blocks.len()
        && !arr.is_empty()
        && arr
            .iter()
            .zip(rendered_blocks.iter())
            .all(|(expected, block_str)| {
                let b = block_str.trim();
                looks_like_json(b)
                    && serde_json::from_str::<Value>(b).ok().as_ref() == Some(expected)
            })
    {
        return true;
    }

    false
}

const fn looks_like_json(s: &str) -> bool {
    matches!(
        s.as_bytes().first(),
        Some(b'{' | b'[' | b'"' | b't' | b'f' | b'n' | b'-' | b'0'..=b'9')
    )
}

fn format_content_block(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text.clone(),
        ContentBlock::Image(img) => {
            format!(
                "[Image ({}, {} bytes base64)]",
                img.mime_type,
                img.data.len()
            )
        }
        ContentBlock::Audio(audio) => {
            format!(
                "[Audio ({}, {} bytes base64)]",
                audio.mime_type,
                audio.data.len()
            )
        }
        ContentBlock::Resource(embedded) => match &embedded.resource {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                ..
            } => {
                let mime = mime_type.as_deref().unwrap_or("text/plain");
                format!("[Resource {uri} ({mime})]\n{text}")
            }
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => {
                let mime = mime_type.as_deref().unwrap_or("application/octet-stream");
                format!(
                    "[Binary Resource {uri} ({mime}, {} bytes base64)]",
                    blob.len()
                )
            }
            _ => serde_json::to_string(&embedded.resource).unwrap_or_default(),
        },
        ContentBlock::ResourceLink(resource) => {
            let desc = resource
                .description
                .as_deref()
                .map_or_else(String::new, |d| format!(" — {d}"));
            format!(
                "[Resource Link: {} ({}){desc}]",
                resource.name, resource.uri
            )
        }
        _ => serde_json::to_string(block).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn format_call_tool_result_extracts_plain_text() {
        let res = CallToolResult::success(vec![
            ContentBlock::text("First line"),
            ContentBlock::text("Second line"),
        ]);
        assert_eq!(format_call_tool_result(&res), "First line\n\nSecond line");
    }

    #[test]
    fn format_call_tool_result_formats_errors_cleanly() {
        let res = CallToolResult::error(vec![ContentBlock::text("Permission denied: /etc/shadow")]);
        assert_eq!(
            format_call_tool_result(&res),
            "Permission denied: /etc/shadow"
        );
    }

    #[test]
    fn format_call_tool_result_handles_structured_content_and_preserves_json_output() {
        let mut res = CallToolResult::success(vec![]);
        let payload = json!({"count": 42, "items": ["a", "b"]});
        res.structured_content = Some(payload.clone());
        assert_eq!(
            format_call_tool_result(&res),
            r#"{"count":42,"items":["a","b"]}"#
        );

        let (summary, output) = build_tool_outputs(&res).expect("valid structured output");
        assert_eq!(summary, r#"{"count":42,"items":["a","b"]}"#);
        assert_eq!(output.as_json(), Some(&payload));
    }

    #[test]
    fn build_tool_outputs_handles_contentless_results_and_empty_errors() {
        let empty_ok = CallToolResult::default();
        let (summary, output) = build_tool_outputs(&empty_ok).expect("contentless ok result");
        assert_eq!(summary, "");
        assert_eq!(output.as_text(), Some(""));

        let empty_err = CallToolResult::error(vec![]);
        let (err_summary, err_output) = build_tool_outputs(&empty_err).expect("empty error result");
        assert_eq!(err_summary, EMPTY_ERROR_FALLBACK);
        assert_eq!(err_output.as_text(), Some(EMPTY_ERROR_FALLBACK));
    }

    #[test]
    fn format_call_tool_result_deduplicates_matching_text_and_structured() {
        let payload = json!({"status": "ok", "items": [1, 2, 3]});
        let pretty_text = serde_json::to_string_pretty(&payload).unwrap();
        let mut res = CallToolResult::success(vec![ContentBlock::text(pretty_text)]);
        res.structured_content = Some(payload.clone());
        assert_eq!(
            format_call_tool_result(&res),
            r#"{"status":"ok","items":[1,2,3]}"#
        );
        let (_, dedup_output) = build_tool_outputs(&res).unwrap();
        assert_eq!(dedup_output.as_json(), Some(&payload));

        let item1 = json!({"a": 1});
        let item2 = json!({"b": 2});
        let mut list_res = CallToolResult::success(vec![
            ContentBlock::text(serde_json::to_string_pretty(&item1).unwrap()),
            ContentBlock::text(serde_json::to_string_pretty(&item2).unwrap()),
        ]);
        list_res.structured_content = Some(json!({"result": [item1, item2]}));
        assert_eq!(
            format_call_tool_result(&list_res),
            r#"{"result":[{"a":1},{"b":2}]}"#
        );

        let mut str_res = CallToolResult::success(vec![ContentBlock::text("hello world")]);
        str_res.structured_content = Some(json!({"result": "hello world"}));
        assert_eq!(format_call_tool_result(&str_res), "hello world");
    }

    #[test]
    fn build_tool_outputs_preserves_multimodal_images_and_normalizes_mime_variants() {
        let res = CallToolResult::success(vec![
            ContentBlock::text("Captured viewport"),
            ContentBlock::image("iVBORw0KGgoAAAANSUhEUg==", "IMAGE/JPG; charset=binary"),
        ]);
        let (summary, output) = build_tool_outputs(&res).expect("valid multimodal output");
        assert!(summary.contains("[Image (IMAGE/JPG; charset=binary, 24 bytes base64)]"));
        assert!(!summary.contains("iVBORw0KGgoAAAANSUhEUg=="));

        let parts = output.into_content();
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], ToolResultContent::Text(t) if t.text == "Captured viewport"));
        assert!(matches!(&parts[1], ToolResultContent::Image(_)));
    }
}
