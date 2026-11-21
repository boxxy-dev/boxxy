//! Provider-hosted native web search configuration, URL citation formatting, and search telemetry.

use boxxy_model_selection::ModelProvider;
use rig::completion::ProviderToolDefinition;
use rig::message::{AssistantContent, Message, SourceLocation};
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::Write as _;

/// Returns the typed [`ProviderToolDefinition`] for built-in server-side web search.
///
/// Applies to providers whose hosted search tool uses a `"type"` discriminator
/// (`OpenAI` Responses `web_search` and Anthropic `web_search_20250305`).
#[must_use]
pub fn native_search_provider_tool(provider: &ModelProvider) -> Option<ProviderToolDefinition> {
    match provider {
        ModelProvider::ChatGpt(_, _) | ModelProvider::OpenAi(_, _) => {
            Some(ProviderToolDefinition::new("web_search"))
        }
        ModelProvider::Anthropic(_, _) => Some(
            ProviderToolDefinition::new("web_search_20250305")
                .with_config("name", serde_json::json!("web_search")),
        ),
        ModelProvider::Gemini(_, _)
        | ModelProvider::Ollama(_)
        | ModelProvider::OpenRouter(_)
        | ModelProvider::DeepSeek(_)
        | ModelProvider::Moonshot(_, _)
        | ModelProvider::Xai(_, _)
        | ModelProvider::Zai(_, _)
        | ModelProvider::Mistral(_, _)
        | ModelProvider::MiniMax(_, _)
        | ModelProvider::Groq(_, _)
        | ModelProvider::OpenCodeZen(_, _)
        | ModelProvider::OpenCodeGo(_, _)
        | ModelProvider::OpenAiCompatible { .. } => None,
    }
}

/// Returns provider-specific `additional_params` JSON required for built-in web search.
///
/// Used when the wire protocol requires non-`ProviderToolDefinition` fields
/// (Gemini `googleSearch` + `toolConfig.includeServerSideToolInvocations`).
#[must_use]
pub fn native_search_additional_params(provider: &ModelProvider) -> Option<serde_json::Value> {
    match provider {
        ModelProvider::Gemini(_, _) => Some(serde_json::json!({
            "tools": [{ "googleSearch": {} }],
            "toolConfig": {
                "includeServerSideToolInvocations": true
            }
        })),
        ModelProvider::ChatGpt(_, _)
        | ModelProvider::OpenAi(_, _)
        | ModelProvider::Anthropic(_, _)
        | ModelProvider::Ollama(_)
        | ModelProvider::OpenRouter(_)
        | ModelProvider::DeepSeek(_)
        | ModelProvider::Moonshot(_, _)
        | ModelProvider::Xai(_, _)
        | ModelProvider::Zai(_, _)
        | ModelProvider::Mistral(_, _)
        | ModelProvider::MiniMax(_, _)
        | ModelProvider::Groq(_, _)
        | ModelProvider::OpenCodeZen(_, _)
        | ModelProvider::OpenCodeGo(_, _)
        | ModelProvider::OpenAiCompatible { .. } => None,
    }
}

/// Attaches built-in server-side web search to `builder` for `provider`.
///
/// Uses [`rig::agent::AgentBuilder::provider_tool`] (`OpenAI`, Anthropic) or
/// `additional_params` (Gemini).
#[must_use]
pub fn attach_native_search(
    mut builder: rig::agent::AgentBuilder,
    provider: &ModelProvider,
) -> rig::agent::AgentBuilder {
    if let Some(tool) = native_search_provider_tool(provider) {
        builder = builder.provider_tool(tool);
    }
    if let Some(params) = native_search_additional_params(provider) {
        builder = builder.additional_params(params);
    }
    builder
}

fn is_safe_http_url(url: &str) -> bool {
    let has_http_scheme = (url.len() > 8 && url[..8].eq_ignore_ascii_case("https://"))
        || (url.len() > 7 && url[..7].eq_ignore_ascii_case("http://"));
    has_http_scheme && !url.bytes().any(|b| b < 0x20 || b == 0x7f)
}

fn encode_markdown_url(url: &str) -> Cow<'_, str> {
    if !url.bytes().any(|b| matches!(b, b'(' | b')' | b' ')) {
        return Cow::Borrowed(url);
    }

    let mut encoded = String::with_capacity(url.len() + 8);
    for ch in url.chars() {
        match ch {
            '(' => encoded.push_str("%28"),
            ')' => encoded.push_str("%29"),
            ' ' => encoded.push_str("%20"),
            _ => encoded.push(ch),
        }
    }
    Cow::Owned(encoded)
}

fn sanitize_markdown_label(raw: &str) -> Cow<'_, str> {
    if !raw
        .chars()
        .any(|c| c.is_control() || matches!(c, '[' | ']' | '\\'))
    {
        return Cow::Borrowed(raw);
    }

    let mut out = String::with_capacity(raw.len() + 4);
    let mut prev_ws = false;
    for ch in raw.chars() {
        if ch.is_control() {
            if !prev_ws && !out.is_empty() {
                out.push(' ');
                prev_ws = true;
            }
            continue;
        }
        prev_ws = ch == ' ';
        match ch {
            '[' => out.push_str("\\["),
            ']' => out.push_str("\\]"),
            '\\' => out.push_str("\\\\"),
            _ => out.push(ch),
        }
    }
    Cow::Owned(out.trim().to_owned())
}

/// Collects unique `(title, url)` pairs from `Text::citations()` across assistant turns in `messages`.
///
/// Only `http://` and `https://` URLs free of control characters are retained.
#[must_use]
pub fn collect_url_citations(messages: &[Message]) -> Vec<(Option<String>, String)> {
    let mut seen_urls: HashSet<&str> = HashSet::new();
    let mut sources = Vec::new();

    for msg in messages {
        if let Message::Assistant(turn) = msg {
            for item in &turn.content {
                if let AssistantContent::Text(text) = item {
                    for citation in text.citations() {
                        for source in &citation.sources {
                            if let SourceLocation::Url { url } = &source.location {
                                let trimmed_url = url.trim();
                                if is_safe_http_url(trimmed_url) && seen_urls.insert(trimmed_url) {
                                    let clean_title = source
                                        .title
                                        .as_deref()
                                        .map(str::trim)
                                        .filter(|t| !t.is_empty())
                                        .map(str::to_owned);
                                    sources.push((clean_title, trimmed_url.to_owned()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    sources
}

/// Appends a Markdown `**Sources:**` footer to `output` for unlinked URL citations in `messages`.
#[must_use]
pub fn append_citations_to_output(messages: &[Message], output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() || trimmed == "[SILENT_ACK]" {
        return output.to_owned();
    }

    let citations = collect_url_citations(messages);
    if citations.is_empty() {
        return output.to_owned();
    }

    let missing_sources: Vec<(Option<String>, String)> = citations
        .into_iter()
        .filter(|(_, url)| {
            let encoded = encode_markdown_url(url);
            !output.contains(url.as_str()) && !output.contains(encoded.as_ref())
        })
        .collect();

    if missing_sources.is_empty() {
        return output.to_owned();
    }

    let mut result = output.to_owned();
    result.push_str("\n\n**Sources:**\n");
    for (title, url) in missing_sources {
        let raw_label = title.as_deref().unwrap_or(url.as_str());
        let safe_label = sanitize_markdown_label(raw_label);
        let safe_url = encode_markdown_url(&url);
        let _ = writeln!(result, "- [{safe_label}]({safe_url})");
    }
    result.trim_end().to_owned()
}

fn push_unique_query(queries: &mut Vec<String>, raw_query: &str) {
    let trimmed = raw_query.trim();
    if !trimmed.is_empty() && !queries.iter().any(|existing| existing == trimmed) {
        queries.push(trimmed.to_owned());
    }
}

fn extract_queries_from_obj(queries: &mut Vec<String>, obj: &serde_json::Value) {
    if let Some(q) = obj.get("query").and_then(serde_json::Value::as_str) {
        push_unique_query(queries, q);
    }
    if let Some(arr) = obj.get("queries").and_then(serde_json::Value::as_array) {
        for q in arr.iter().filter_map(serde_json::Value::as_str) {
            push_unique_query(queries, q);
        }
    }
}

fn is_gemini_search_part(part_obj: &serde_json::Value) -> bool {
    // Older Gemini grounding payloads omit `toolType` and carry `args.query` / `args.queries`.
    part_obj
        .get("toolType")
        .and_then(serde_json::Value::as_str)
        .is_none_or(|tool_type| tool_type.to_ascii_lowercase().contains("search"))
}

/// Extracts a human-readable summary when provider-native web search ran during `messages`.
///
/// Recognizes `web_search_call` on `OpenAI`, `server_tool_use` / `web_search_tool_result`
/// on Anthropic, Gemini search `toolCall` / `toolResponse` parts, and grounding URL citations.
#[must_use]
pub fn extract_native_search_summary(messages: &[Message]) -> Option<String> {
    let mut queries = Vec::new();
    let mut search_invoked = false;

    for msg in messages {
        if let Message::Assistant(turn) = msg {
            for item in &turn.content {
                if let AssistantContent::Opaque(opaque) = item {
                    match opaque.kind() {
                        Some("web_search_call") => {
                            search_invoked = true;
                            if let Some(action) = opaque.item.get("action") {
                                extract_queries_from_obj(&mut queries, action);
                            }
                        }
                        Some("server_tool_use")
                            if opaque.item.get("name").and_then(serde_json::Value::as_str)
                                == Some("web_search") =>
                        {
                            search_invoked = true;
                            if let Some(input) = opaque.item.get("input") {
                                extract_queries_from_obj(&mut queries, input);
                            }
                        }
                        Some("web_search_tool_result") => {
                            search_invoked = true;
                        }
                        _ => {
                            // Gemini server-side tool invocation part (`toolCall` / `toolResponse`)
                            if let Some(tool_call) = opaque.item.get("toolCall") {
                                if is_gemini_search_part(tool_call) {
                                    search_invoked = true;
                                    if let Some(args) = tool_call.get("args") {
                                        extract_queries_from_obj(&mut queries, args);
                                    }
                                }
                            } else if let Some(tool_resp) = opaque.item.get("toolResponse")
                                && is_gemini_search_part(tool_resp)
                            {
                                search_invoked = true;
                            }
                        }
                    }
                }
            }
        }
    }

    let citations = collect_url_citations(messages);
    if !search_invoked && citations.is_empty() {
        return None;
    }

    let mut summary = String::new();
    if !queries.is_empty() {
        let _ = writeln!(summary, "Queries: {}", queries.join(", "));
    }
    if !citations.is_empty() {
        summary.push_str("Sources:\n");
        for (title, url) in &citations {
            let label = title.as_deref().unwrap_or(url.as_str());
            let _ = writeln!(summary, "- {label}: {url}");
        }
    }
    if summary.is_empty() {
        summary.push_str("Executed provider-native web search.");
    }

    Some(summary.trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::message::{AssistantMessage, Opaque, Text};

    #[test]
    fn extracts_anthropic_and_gemini_native_search_summaries() {
        let anthropic_msgs = vec![Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::Opaque(Opaque {
                item: serde_json::json!({
                    "type": "server_tool_use",
                    "id": "srvtoolu_1",
                    "name": "web_search",
                    "input": { "query": "gtk4 rust docs" }
                }),
                replay: true,
            }),
            AssistantContent::Opaque(Opaque {
                item: serde_json::json!({
                    "type": "web_search_tool_result",
                    "tool_use_id": "srvtoolu_1"
                }),
                replay: true,
            }),
            AssistantContent::Text(Text::new("Found docs.")),
        ]))];

        let summary =
            extract_native_search_summary(&anthropic_msgs).expect("anthropic search detected");
        assert_eq!(summary, "Queries: gtk4 rust docs");

        let gemini_msgs = vec![Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::Opaque(Opaque {
                item: serde_json::json!({
                    "toolCall": {
                        "toolType": "GOOGLE_SEARCH_WEB",
                        "args": { "queries": ["tokio runtime", "tokio runtime", "  "] }
                    }
                }),
                replay: true,
            }),
            AssistantContent::Text(Text::new("Result.")),
        ]))];

        let g_summary =
            extract_native_search_summary(&gemini_msgs).expect("gemini search detected");
        assert_eq!(g_summary, "Queries: tokio runtime");
    }

    #[test]
    fn silent_ack_and_empty_outputs_never_append_sources() {
        let citation_msg =
            vec![Message::Assistant(AssistantMessage::new(vec![
                AssistantContent::Text(Text::new("Ack").with_citations([
                    rig::message::Citation::new([rig::message::Source::new(SourceLocation::Url {
                        url: "https://example.com/doc".to_string(),
                    })]),
                ])),
            ]))];

        assert_eq!(
            append_citations_to_output(&citation_msg, "[SILENT_ACK]"),
            "[SILENT_ACK]"
        );
        assert_eq!(append_citations_to_output(&citation_msg, "   "), "   ");
    }
}
