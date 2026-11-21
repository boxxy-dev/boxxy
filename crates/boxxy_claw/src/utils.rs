use log::warn;

const CLAW_MD: &str = include_str!("../../../resources/prompts/claw.md");
const MEMORY_FLUSH_MD: &str = include_str!("../../../resources/prompts/memory_flush.md");
const BOOKMARK_GENERATOR_MD: &str =
    include_str!("../../../resources/prompts/bookmark_generator.md");
const AI_CHAT_MD: &str = include_str!("../../../resources/prompts/ai_chat.md");
const PRIVACY_POLICY_MD: &str = include_str!("../../../resources/prompts/privacy_policy.md");

#[must_use]
pub fn load_prompt_fallback(_resource_path: &str, filename: &str) -> String {
    match filename {
        "claw.md" => CLAW_MD.to_string(),
        "memory_flush.md" => MEMORY_FLUSH_MD.to_string(),
        "bookmark_generator.md" => BOOKMARK_GENERATOR_MD.to_string(),
        "ai_chat.md" => AI_CHAT_MD.to_string(),
        "privacy_policy.md" => PRIVACY_POLICY_MD.to_string(),
        _ => {
            warn!("Unknown prompt file requested: {filename}");
            format!("Fallback content for {filename}")
        }
    }
}

/// Extracts and deserializes a JSON object of type `T` from raw LLM text.
///
/// Handles markdown code fences (` ```json ... ``` ` or ` ``` ... ``` `) even when
/// preceded by bracketed prose like `Processed [Session: 1]:`, as well as unfenced JSON
/// surrounded by leading or trailing braces/brackets.
pub(crate) fn parse_llm_json<T: serde::de::DeserializeOwned>(
    raw: &str,
) -> Result<T, serde_json::Error> {
    let trimmed = raw.trim();

    // 1. Inspect fenced code blocks first so leading prose containing `[` or `{`
    //    (e.g. `Processed [Session: 1]:\n```json\n{...}\n````) never misleads `json_in_text`.
    let mut cursor = trimmed;
    while let Some(open_idx) = cursor.find("```") {
        let after_open = &cursor[open_idx + 3..];
        let Some(close_rel) = after_open.find("```") else {
            break;
        };
        let block = &after_open[..close_rel];
        let body = block.find('\n').map_or_else(
            || block.trim_start().strip_prefix("json").unwrap_or(block),
            |nl| {
                let tag = block[..nl].trim();
                if tag.is_empty()
                    || tag
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    &block[nl + 1..]
                } else {
                    block
                }
            },
        );

        if let Ok(parsed) = rig::structured_output::json_in_text::<T>(body) {
            return Ok(parsed);
        }
        cursor = &after_open[close_rel + 3..];
    }

    // 2. Direct / first-delimiter extraction via `rig::structured_output::json_in_text`.
    let primary_err = match rig::structured_output::json_in_text::<T>(trimmed) {
        Ok(parsed) => return Ok(parsed),
        Err(err) => err,
    };

    // 3. Fallback: scan every `{` offset with a streaming `serde_json::Deserializer` so
    //    leading `[tag]` or `{note}` prose before an unfenced JSON object is skipped cleanly
    //    and trailing `{note}` prose after the object is ignored without slicing `rfind('}')`.
    for (idx, _) in trimmed.match_indices('{') {
        if let Some(Ok(parsed)) = serde_json::Deserializer::from_str(&trimmed[idx..])
            .into_iter::<T>()
            .next()
        {
            return Ok(parsed);
        }
    }

    Err(primary_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Eq, Deserialize)]
    struct SamplePayload {
        key: String,
        count: u32,
    }

    #[test]
    fn parse_llm_json_handles_leading_brackets_fences_and_trailing_braces() {
        let fenced = "Processed [Session: 1] {phase: alpha}:\n```json\n{\"key\": \"ok\", \"count\": 3}\n```\nDone {extra_note}";
        assert_eq!(
            parse_llm_json::<SamplePayload>(fenced).unwrap(),
            SamplePayload {
                key: "ok".to_string(),
                count: 3,
            }
        );

        let unfenced = "Summary [ID: 7] {draft}: {\"key\": \"clean\", \"count\": 9} trailing {brace}";
        assert_eq!(
            parse_llm_json::<SamplePayload>(unfenced).unwrap(),
            SamplePayload {
                key: "clean".to_string(),
                count: 9,
            }
        );
    }
}
