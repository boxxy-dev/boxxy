use serde_json::{Map, Value, json};

/// Translates an MCP JSON Schema (Draft 7 / 2020-12) into a Rig-compatible JSON schema format.
///
/// Normalizes both root and nested subschemas so strict LLM function-calling APIs
/// (`OpenAI`, Anthropic, Gemini) do not reject parameterless objects, dialect tags,
/// nullable type arrays (`["object", "null"]`), or nested Pydantic/Zod/`schemars` `$defs`:
/// 1. Strips `$schema` dialect URIs recursively from schema nodes (without touching
///    any user tool property literally named `"$schema"` inside `properties`).
/// 2. Ensures the root schema is an `"object"` with `"properties": {}`.
/// 3. Ensures nested `"type": "object"` or `"type": ["object", "null"]` schemas without
///    `properties`, `additionalProperties`, `patternProperties`, or `unevaluatedProperties`
///    carry `"properties": {}`, while preserving Draft-07 property-name `dependencies` arrays (`rmcp 3.11`).
#[must_use]
pub fn translate_schema(mut schema: Value) -> Value {
    if !schema.is_object() {
        return json!({
            "type": "object",
            "properties": {}
        });
    }
    normalize_schema_node(&mut schema, true);
    schema
}

fn normalize_schema_node(node: &mut Value, is_root: bool) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };

    // Many LLM function-calling APIs reject the JSON Schema "$schema" dialect key.
    obj.remove("$schema");

    let is_object_schema = match obj.get("type") {
        Some(Value::String(s)) => s == "object",
        Some(Value::Array(arr)) => arr.iter().any(|v| v.as_str() == Some("object")),
        None if is_root => {
            obj.insert("type".to_string(), json!("object"));
            true
        }
        _ => false,
    };

    if is_object_schema {
        let has_shape_keywords = obj.contains_key("properties")
            || (!is_root
                && (obj.contains_key("additionalProperties")
                    || obj.contains_key("patternProperties")
                    || obj.contains_key("unevaluatedProperties")));
        if !has_shape_keywords {
            obj.insert("properties".to_string(), json!({}));
        }
    }

    // Maps whose values are subschema definitions (keys are property/definition names, NOT keywords).
    for map_key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(Value::Object(sub_map)) = obj.get_mut(map_key) {
            normalize_schema_map(sub_map);
        }
    }

    // Draft-07 `dependencies`: values may be either a subschema object OR a string array of property names.
    if let Some(Value::Object(deps_map)) = obj.get_mut("dependencies") {
        for dep_val in deps_map.values_mut() {
            if dep_val.is_object() {
                normalize_schema_node(dep_val, false);
            }
        }
    }

    // Keywords whose value is a single subschema (or boolean).
    for single_key in [
        "additionalProperties",
        "unevaluatedProperties",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
        "contains",
    ] {
        if let Some(sub) = obj.get_mut(single_key)
            && sub.is_object()
        {
            normalize_schema_node(sub, false);
        }
    }

    // Keywords whose value is either a single subschema or an array of subschemas.
    for list_or_single_key in ["items", "prefixItems", "anyOf", "oneOf", "allOf"] {
        match obj.get_mut(list_or_single_key) {
            Some(Value::Array(arr)) => {
                for item in arr {
                    normalize_schema_node(item, false);
                }
            }
            Some(sub) if sub.is_object() => {
                normalize_schema_node(sub, false);
            }
            _ => {}
        }
    }
}

fn normalize_schema_map(map: &mut Map<String, Value>) {
    for subschema in map.values_mut() {
        normalize_schema_node(subschema, false);
    }
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
    fn normalizes_completely_empty_or_scalar_schema() {
        for input in [json!({}), json!(true), json!(null)] {
            let translated = translate_schema(input);
            assert_eq!(translated["type"], "object");
            assert_eq!(translated["properties"], json!({}));
        }
    }

    #[test]
    fn normalizes_nested_defs_nullable_objects_dict_maps_and_dependency_arrays() {
        let input = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "$schema": { "type": "string" },
                "empty_nested": {
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "type": "object"
                },
                "nullable_empty": {
                    "type": ["object", "null"]
                },
                "freeform_headers": {
                    "type": "object",
                    "additionalProperties": { "type": "string" }
                },
                "unevaluated_map": {
                    "type": "object",
                    "unevaluatedProperties": {
                        "$schema": "https://json-schema.org/draft/2020-12/schema",
                        "type": "string"
                    }
                }
            },
            "$defs": {
                "Filter": {
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object"
                }
            },
            "dependencies": {
                "credit_card": ["billing_address"],
                "advanced": {
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "type": "object"
                }
            }
        });

        let translated = translate_schema(input);
        // A user parameter literally named "$schema" inside `properties` must be preserved!
        assert_eq!(translated["properties"]["$schema"]["type"], "string");
        // Nested `$schema` dialect tags are stripped and bare objects (including `["object", "null"]`) receive `"properties": {}`.
        assert!(
            translated["properties"]["empty_nested"]
                .get("$schema")
                .is_none()
        );
        assert_eq!(
            translated["properties"]["empty_nested"]["properties"],
            json!({})
        );
        assert_eq!(
            translated["properties"]["nullable_empty"]["properties"],
            json!({})
        );
        // Dictionary maps with `additionalProperties` / `unevaluatedProperties` keep their open-map semantics.
        assert!(
            translated["properties"]["freeform_headers"]
                .get("properties")
                .is_none()
        );
        assert!(
            translated["properties"]["unevaluated_map"]
                .get("properties")
                .is_none()
        );
        assert!(
            translated["properties"]["unevaluated_map"]["unevaluatedProperties"]
                .get("$schema")
                .is_none()
        );
        // `$defs` and Draft-07 `dependencies` (both array and subschema forms) are handled cleanly.
        assert!(translated["$defs"]["Filter"].get("$schema").is_none());
        assert_eq!(translated["$defs"]["Filter"]["properties"], json!({}));
        assert_eq!(
            translated["dependencies"]["credit_card"],
            json!(["billing_address"])
        );
        assert_eq!(
            translated["dependencies"]["advanced"]["properties"],
            json!({})
        );
    }
}
