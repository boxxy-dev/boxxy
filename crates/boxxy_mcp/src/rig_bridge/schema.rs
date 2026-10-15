use serde_json::{Value, json};

/// Translates an MCP JSON Schema (Draft 7 / 2020-12) into a Rig-compatible JSON schema format.
///
/// Normalizes top-level schemas so strict LLM function-calling APIs (OpenAI, Anthropic, Gemini)
/// do not reject parameterless or dialect-tagged MCP tools:
/// 1. Strips top-level `$schema` dialect URI.
/// 2. Ensures top-level object schemas always include `"type": "object"` and `"properties": {}`.
pub fn translate_schema(schema: Value) -> Value {
    let mut clean_schema = schema;

    if let Some(obj) = clean_schema.as_object_mut() {
        // Many LLM function-calling APIs choke on the standard JSON Schema "$schema" key.
        obj.remove("$schema");

        let is_object_schema = match obj.get("type").and_then(Value::as_str) {
            Some("object") => true,
            None => {
                obj.insert("type".to_string(), json!("object"));
                true
            }
            Some(_) => false,
        };

        // Strict providers (OpenAI, Gemini, Anthropic) require `properties` to be present
        // when `type` is `"object"`, even for tools that accept zero parameters.
        if is_object_schema && !obj.contains_key("properties") {
            obj.insert("properties".to_string(), json!({}));
        }
    } else {
        clean_schema = json!({
            "type": "object",
            "properties": {}
        });
    }

    clean_schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_schema_dialect_and_preserves_properties() {
        let input = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        });

        let translated = translate_schema(input);
        assert!(translated.get("$schema").is_none());
        assert_eq!(translated["type"], "object");
        assert_eq!(translated["properties"]["query"]["type"], "string");
        assert_eq!(translated["required"], json!(["query"]));
    }

    #[test]
    fn injects_properties_for_empty_object_schema() {
        let input = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object"
        });

        let translated = translate_schema(input);
        assert!(translated.get("$schema").is_none());
        assert_eq!(translated["type"], "object");
        assert_eq!(translated["properties"], json!({}));
    }

    #[test]
    fn normalizes_completely_empty_map() {
        let input = json!({});
        let translated = translate_schema(input);
        assert_eq!(translated["type"], "object");
        assert_eq!(translated["properties"], json!({}));
    }
}
