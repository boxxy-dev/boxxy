//! Unified AI provider abstraction, catalog/cost resolution, native search, and agent runtime
//! built on `rig-core` (`0.47`).

mod agent;
mod catalog;
mod chatgpt_auth;
mod credentials;
mod hook;
mod providers;
mod reasoning;
mod search;

pub use agent::{
    AgentOptions, AgentTurnOutput, BoxxyAgent, create_agent, create_agent_with_config,
    create_agent_with_options,
};
pub use catalog::{
    enrich_usage_with_catalog_cost, enrich_usage_with_endpoints_cost,
    extract_response_context_tokens, model_context_window, model_context_window_with_endpoints,
    model_input_limit, model_input_limit_with_endpoints, model_output_limit,
    model_output_limit_with_endpoints, resolve_catalog_spec,
};
pub use chatgpt_auth::{
    CHATGPT_OAUTH_SENTINEL, ChatGptAuthStatus, ChatGptSignInMode, ChatGptSignInPrompt,
    build_chatgpt_model_spec, chatgpt_auth_dir_in, chatgpt_auth_status, inspect_chatgpt_auth_in,
    is_chatgpt_oauth_signed_in, refresh_chatgpt_account_models, refresh_chatgpt_auth_status,
    shared_chatgpt_authenticator, sign_in_chatgpt_oauth, sign_out_chatgpt_oauth,
};
pub use credentials::AiCredentials;
pub use hook::ModelContextHook;
pub use providers::{
    BuiltProviderModel, ProviderModelBackend, build_provider_model,
    build_provider_model_with_options, configure_agent_builder, fetch_openai_compatible_models,
    needs_relaxed_finish_reasons,
};
pub use reasoning::{
    map_thinking_level, recommended_max_tokens_for_reasoning, resolve_model_reasoning,
    resolve_model_reasoning_with_endpoints,
};
pub use search::{
    append_citations_to_output, attach_native_search, collect_url_citations,
    extract_native_search_summary, native_search_additional_params, native_search_provider_tool,
};

#[cfg(test)]
mod tests {
    use super::*;
    use boxxy_model_selection::{
        AnthropicModel, ChatGptModel, CustomModelConfig, DeepSeekModel, GeminiModel, GroqModel,
        MiniMaxModel, MistralModel, ModelProvider, MoonshotModel, OpenAiApiRoute,
        OpenAiCompatibleEndpoint, OpenAiModel, OpenCodeGoModel, OpenCodeZenModel, ThinkingLevel,
        XaiModel, ZaiModel,
    };
    use rig::completion::{Cost, Effort, ProviderToolDefinition, Reasoning, Usage};
    use rig::message::{
        AssistantContent, AssistantMessage, Citation, Message, Opaque, Source, SourceLocation, Text,
    };

    #[test]
    fn test_map_thinking_level() {
        assert_eq!(map_thinking_level(ThinkingLevel::None), Reasoning::Off);
        assert_eq!(
            map_thinking_level(ThinkingLevel::Minimal),
            Reasoning::Effort(Effort::Minimal)
        );
        assert_eq!(
            map_thinking_level(ThinkingLevel::Low),
            Reasoning::Effort(Effort::Low)
        );
        assert_eq!(
            map_thinking_level(ThinkingLevel::Medium),
            Reasoning::Effort(Effort::Medium)
        );
        assert_eq!(
            map_thinking_level(ThinkingLevel::High),
            Reasoning::Effort(Effort::High)
        );
        assert_eq!(
            map_thinking_level(ThinkingLevel::XHigh),
            Reasoning::Effort(Effort::XHigh)
        );
    }

    #[test]
    fn test_build_provider_model_reasoning() {
        let creds = AiCredentials::default();

        // Gemini with Low thinking -> Effort(Low)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::Gemini(GeminiModel::Flash3_8, Some(ThinkingLevel::Low)),
            &creds,
        );
        assert_eq!(provider, "Gemini");
        assert_eq!(model, "gemini-3.8-flash");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::Low)));

        // Gemini with None thinking -> None (Gemini 3 cannot disable thinking)
        let (_, _, _, reasoning_none) = build_provider_model(
            &ModelProvider::Gemini(GeminiModel::Flash3_8, Some(ThinkingLevel::None)),
            &creds,
        );
        assert_eq!(reasoning_none, None);

        // Anthropic Claude 5.5 with Medium -> adaptive Effort(Medium)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::Anthropic(AnthropicModel::ClaudeSonnet5, Some(ThinkingLevel::Medium)),
            &creds,
        );
        assert_eq!(provider, "Anthropic");
        assert_eq!(model, "claude-sonnet-5-5");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::Medium)));

        // OpenAI with High -> Effort(High)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::OpenAi(OpenAiModel::Gpt6Astra, Some(ThinkingLevel::High)),
            &creds,
        );
        assert_eq!(provider, "OpenAI");
        assert_eq!(model, "gpt-6-astra");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::High)));

        // DeepSeek
        let (_, provider, model, reasoning) =
            build_provider_model(&ModelProvider::DeepSeek(DeepSeekModel::V4_1Flash), &creds);
        assert_eq!(provider, "DeepSeek");
        assert_eq!(model, "deepseek-flash");
        assert_eq!(reasoning, None);

        // Moonshot (Kimi K3)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::Moonshot(MoonshotModel::KimiK3, Some(ThinkingLevel::High)),
            &creds,
        );
        assert_eq!(provider, "Moonshot");
        assert_eq!(model, "kimi-k3");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::High)));

        // xAI (Grok 4.7)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::Xai(XaiModel::Grok4_7, Some(ThinkingLevel::High)),
            &creds,
        );
        assert_eq!(provider, "xAI");
        assert_eq!(model, "grok-4.7");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::High)));

        // Z.ai (GLM-5.3)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::Zai(ZaiModel::Glm5_3, Some(ThinkingLevel::Low)),
            &creds,
        );
        assert_eq!(provider, "Z.ai");
        assert_eq!(model, "glm-5.3");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::Low)));

        // Mistral (Mistral Large)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::Mistral(MistralModel::MistralLargeLatest, None),
            &creds,
        );
        assert_eq!(provider, "Mistral");
        assert_eq!(model, "mistral-large-latest");
        assert_eq!(reasoning, None);

        // MiniMax (MiniMax-M3)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::MiniMax(MiniMaxModel::MiniMaxM3, None),
            &creds,
        );
        assert_eq!(provider, "MiniMax");
        assert_eq!(model, "MiniMax-M3");
        assert_eq!(reasoning, None);

        // Groq (GPT OSS 120B)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::Groq(GroqModel::GptOss120B, Some(ThinkingLevel::Medium)),
            &creds,
        );
        assert_eq!(provider, "Groq");
        assert_eq!(model, "openai/gpt-oss-120b");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::Medium)));

        // OpenCode Zen (Claude Sonnet 5.5 via Anthropic Messages wire)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::OpenCodeZen(
                OpenCodeZenModel::ClaudeSonnet5_5,
                Some(ThinkingLevel::Medium),
            ),
            &creds,
        );
        assert_eq!(provider, "OpenCode Zen");
        assert_eq!(model, "claude-sonnet-5-5");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::Medium)));

        // OpenCode Go (GLM-5.3 via OpenAI Chat wire with x-opencode-session)
        let (_, provider, model, reasoning) = build_provider_model(
            &ModelProvider::OpenCodeGo(OpenCodeGoModel::Glm5_3, Some(ThinkingLevel::High)),
            &creds,
        );
        assert_eq!(provider, "OpenCode Go");
        assert_eq!(model, "glm-5.3");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::High)));

        // Ollama with empty URL and padded model name trims cleanly without panicking
        let (_, provider, model, reasoning) =
            build_provider_model(&ModelProvider::Ollama("  qwen3:8b  ".to_string()), &creds);
        assert_eq!(provider, "Ollama");
        assert_eq!(model, "qwen3:8b");
        assert_eq!(reasoning, None);
    }

    #[test]
    fn test_openai_compatible_reasoning_respects_catalog_and_custom_overrides() {
        let mut custom_ep = OpenAiCompatibleEndpoint::preset_local_server();
        custom_ep.name = "Gateway".to_string();
        custom_ep.models.push(CustomModelConfig {
            id: "my-local-reasoner".to_string(),
            display_name: "Local Reasoner".to_string(),
            route: OpenAiApiRoute::ChatCompletions,
            context_window: Some(65_536),
            thinking_levels: vec![ThinkingLevel::Low, ThinkingLevel::High],
            input_cost_per_1m: None,
            output_cost_per_1m: None,
        });
        let creds = AiCredentials::default().with_custom_endpoints(vec![custom_ep]);

        // Custom model with [Low, High] clamps XHigh -> High
        let (_, _, _, clamped) = build_provider_model(
            &ModelProvider::OpenAiCompatible {
                provider_name: "Gateway".to_string(),
                model_id: "my-local-reasoner".to_string(),
                thinking: Some(ThinkingLevel::XHigh),
            },
            &creds,
        );
        assert_eq!(clamped, Some(Reasoning::Effort(Effort::High)));

        // Known catalog model routed via OpenAiCompatible that cannot disable thinking
        // suppresses `Reasoning::Off` instead of sending invalid wire parameters.
        let (_, _, _, gemini_off) = build_provider_model(
            &ModelProvider::OpenAiCompatible {
                provider_name: "Gateway".to_string(),
                model_id: "gemini-3.8-flash".to_string(),
                thinking: Some(ThinkingLevel::None),
            },
            &creds,
        );
        assert_eq!(gemini_off, None);
    }

    #[test]
    fn test_native_search_additional_params() {
        assert_eq!(
            native_search_additional_params(&ModelProvider::Gemini(GeminiModel::Flash3_8, None)),
            Some(serde_json::json!({
                "tools": [{ "googleSearch": {} }],
                "toolConfig": {
                    "includeServerSideToolInvocations": true
                }
            }))
        );
        assert_eq!(
            native_search_provider_tool(&ModelProvider::Gemini(GeminiModel::Flash3_8, None)),
            None
        );

        assert_eq!(
            native_search_provider_tool(&ModelProvider::OpenAi(OpenAiModel::Gpt6Astra, None)),
            Some(ProviderToolDefinition::new("web_search"))
        );
        assert_eq!(
            native_search_additional_params(&ModelProvider::OpenAi(OpenAiModel::Gpt6Astra, None)),
            None
        );

        assert_eq!(
            native_search_provider_tool(&ModelProvider::Anthropic(
                AnthropicModel::ClaudeSonnet5,
                None
            )),
            Some(
                ProviderToolDefinition::new("web_search_20250305")
                    .with_config("name", serde_json::json!("web_search"))
            )
        );
        assert_eq!(
            native_search_additional_params(&ModelProvider::Anthropic(
                AnthropicModel::ClaudeSonnet5,
                None
            )),
            None
        );

        assert_eq!(
            native_search_provider_tool(&ModelProvider::Ollama("llama3.2".to_string())),
            None
        );
        assert_eq!(
            native_search_additional_params(&ModelProvider::Ollama("llama3.2".to_string())),
            None
        );
        assert_eq!(
            native_search_provider_tool(&ModelProvider::OpenRouter(
                "anthropic/claude-sonnet-4".to_string()
            )),
            None
        );
        assert_eq!(
            native_search_provider_tool(&ModelProvider::DeepSeek(DeepSeekModel::V4_1Flash)),
            None
        );
        assert_eq!(
            native_search_provider_tool(&ModelProvider::Moonshot(
                MoonshotModel::KimiK3,
                Some(ThinkingLevel::Low)
            )),
            None
        );
    }

    #[test]
    fn test_citations_markdown_escaping_and_scheme_filtering() {
        let text = Text::new("Rust 2024 is supported.").with_citations([Citation::new([
            Source::new(SourceLocation::Url {
                url: "https://example.com/rust-2024".to_string(),
            })
            .title("Rust 2024 Edition Guide"),
            Source::new(SourceLocation::Url {
                url: "https://example.com/wiki/Rust_(language)".to_string(),
            })
            .title("[RFC] Rust\nReference"),
            // Unsafe non-HTTP schemes must be rejected
            Source::new(SourceLocation::Url {
                url: "javascript:alert(1)".to_string(),
            })
            .title("Malicious Link"),
        ])]);

        let search_opaque = Opaque {
            item: serde_json::json!({
                "type": "web_search_call",
                "id": "ws_1",
                "status": "completed",
                "action": {
                    "type": "search",
                    "query": "rust 2024 edition",
                    "queries": ["rust 2024 edition", "rust edition guide"]
                }
            }),
            replay: true,
        };

        let non_search_gemini_opaque = Opaque {
            item: serde_json::json!({
                "toolCall": {
                    "toolType": "CODE_EXECUTION",
                    "args": { "code": "print(1)" }
                }
            }),
            replay: true,
        };

        let turn = AssistantMessage::new(vec![
            AssistantContent::Opaque(search_opaque),
            AssistantContent::Opaque(non_search_gemini_opaque),
            AssistantContent::Text(text),
        ]);
        let messages = vec![Message::user("What edition?"), Message::Assistant(turn)];

        let formatted = append_citations_to_output(&messages, "Rust 2024 is supported.");
        assert!(formatted.contains("**Sources:**"));
        assert!(formatted.contains("- [Rust 2024 Edition Guide](https://example.com/rust-2024)"));
        assert!(formatted.contains(
            "- [\\[RFC\\] Rust Reference](https://example.com/wiki/Rust_%28language%29)"
        ));
        assert!(!formatted.contains("javascript:"));

        // If URLs are already inline in the output, don't duplicate them
        let inline = "Rust 2024 is supported ([guide](https://example.com/rust-2024), [wiki](https://example.com/wiki/Rust_%28language%29)).";
        assert_eq!(append_citations_to_output(&messages, inline), inline);

        let summary = extract_native_search_summary(&messages).expect("should detect search");
        assert!(summary.contains("Queries: rust 2024 edition, rust edition guide"));
        assert!(summary.contains("https://example.com/rust-2024"));
        assert!(!summary.contains("javascript:"));

        // Non-search Gemini server tools alone must not report native_web_search
        let code_exec_only = vec![Message::Assistant(AssistantMessage::new(vec![
            AssistantContent::Opaque(Opaque {
                item: serde_json::json!({
                    "toolCall": { "toolType": "CODE_EXECUTION" }
                }),
                replay: true,
            }),
            AssistantContent::Text(Text::new("Computed result.")),
        ]))];
        assert_eq!(extract_native_search_summary(&code_exec_only), None);
    }

    #[test]
    fn test_catalog_resolution_and_cost() {
        let gemini = Some(ModelProvider::Gemini(GeminiModel::Flash3_8, None));
        assert_eq!(model_context_window(&gemini), Some(1_048_576));

        let anthropic = Some(ModelProvider::Anthropic(
            AnthropicModel::ClaudeSonnet5,
            None,
        ));
        assert_eq!(model_context_window(&anthropic), Some(1_000_000));

        let openai = Some(ModelProvider::OpenAi(OpenAiModel::Gpt6Astra, None));
        assert_eq!(model_context_window(&openai), Some(1_050_000));

        let terra_61 = Some(ModelProvider::OpenAi(
            OpenAiModel::new("gpt-6.1-terra"),
            Some(ThinkingLevel::Medium),
        ));
        assert_eq!(model_context_window(&terra_61), Some(1_050_000));
        assert_eq!(model_input_limit(&terra_61), Some(922_000));
        let terra_usage = Usage::new().input_tokens(1_000_000).output_tokens(200_000);
        let terra_priced = enrich_usage_with_catalog_cost(terra_usage, &terra_61);
        assert!((terra_priced.cost.expect("priced").total - 2.00).abs() < 1e-6);

        let deepseek = Some(ModelProvider::DeepSeek(DeepSeekModel::V4_1Flash));
        assert_eq!(model_context_window(&deepseek), Some(1_048_576));

        let moonshot = Some(ModelProvider::Moonshot(MoonshotModel::KimiK3, None));
        assert_eq!(model_context_window(&moonshot), Some(1_048_576));

        let xai = Some(ModelProvider::Xai(XaiModel::Grok4_7, None));
        assert_eq!(model_context_window(&xai), Some(500_000));

        let zai = Some(ModelProvider::Zai(ZaiModel::Glm5_3, None));
        assert_eq!(model_context_window(&zai), Some(1_000_000));

        let mistral = Some(ModelProvider::Mistral(
            MistralModel::MistralLargeLatest,
            None,
        ));
        assert_eq!(model_context_window(&mistral), Some(1_048_576));
        assert_eq!(model_output_limit(&mistral), Some(262_144));

        let minimax = Some(ModelProvider::MiniMax(MiniMaxModel::MiniMaxM3, None));
        assert_eq!(model_context_window(&minimax), Some(1_000_000));

        let groq = Some(ModelProvider::Groq(GroqModel::GptOss120B, None));
        assert_eq!(model_context_window(&groq), Some(131_072));

        // OpenRouter with a base vendor/model slug resolves via fallback
        let openrouter = Some(ModelProvider::OpenRouter(
            "anthropic/claude-sonnet-5".to_string(),
        ));
        assert_eq!(model_context_window(&openrouter), Some(1_000_000));

        let usage = Usage::new().input_tokens(100_000).output_tokens(10_000);
        let priced = enrich_usage_with_catalog_cost(usage, &openrouter);
        assert!((priced.cost.expect("priced").total - 0.30).abs() < 1e-6);

        // Local Ollama and subscription OpenCode Go models clear even wire-reported costs
        let mut wire_cost_usage = usage;
        wire_cost_usage.cost = Some(Cost::from_total(0.15));

        let ollama = Some(ModelProvider::Ollama("llama3.2:latest".to_string()));
        assert!(
            enrich_usage_with_catalog_cost(wire_cost_usage, &ollama)
                .cost
                .is_none()
        );

        let opencode_go_native = Some(ModelProvider::OpenCodeGo(
            OpenCodeGoModel::Glm5_3,
            Some(ThinkingLevel::Low),
        ));
        assert_eq!(model_context_window(&opencode_go_native), Some(1_000_000));
        assert!(
            enrich_usage_with_catalog_cost(wire_cost_usage, &opencode_go_native)
                .cost
                .is_none()
        );

        let opencode_zen_native = Some(ModelProvider::OpenCodeZen(
            OpenCodeZenModel::ClaudeSonnet5_5,
            Some(ThinkingLevel::Medium),
        ));
        assert_eq!(model_context_window(&opencode_zen_native), Some(1_000_000));
        let opencode_zen_priced = enrich_usage_with_catalog_cost(usage, &opencode_zen_native);
        assert!((opencode_zen_priced.cost.expect("priced").total - 0.30).abs() < 1e-6);
    }

    #[test]
    fn test_custom_endpoints_context_and_cost_overrides() {
        let usage = Usage::new().input_tokens(100_000).output_tokens(10_000);
        let mut wire_cost_usage = usage;
        wire_cost_usage.cost = Some(Cost::from_total(0.15));

        // OpenAiCompatible: OpenCode Go (zero_cost = true) vs OpenCode Zen (catalog fallback) vs custom override
        let go_ep = OpenAiCompatibleEndpoint::preset_opencode_go();
        let mut zen_ep = OpenAiCompatibleEndpoint::preset_opencode_zen();
        let mut custom_ep = OpenAiCompatibleEndpoint::preset_local_server();
        custom_ep.name = "Custom vLLM".to_string();
        custom_ep.zero_cost = false;
        custom_ep.models.push(CustomModelConfig {
            id: "my-local-model".to_string(),
            display_name: "My Local Model".to_string(),
            route: OpenAiApiRoute::ChatCompletions,
            context_window: Some(65_536),
            thinking_levels: vec![ThinkingLevel::Low, ThinkingLevel::High],
            input_cost_per_1m: Some(1.50),
            output_cost_per_1m: Some(6.00),
        });
        zen_ep
            .models
            .push(CustomModelConfig::new("anthropic/claude-sonnet-5"));

        let endpoints = vec![go_ep, zen_ep, custom_ep];

        let go_prov = Some(ModelProvider::OpenAiCompatible {
            provider_name: "OpenCode Go".to_string(),
            model_id: "deepseek-v4-pro".to_string(),
            thinking: None,
        });
        assert_eq!(
            model_context_window_with_endpoints(&go_prov, &endpoints),
            Some(1_048_576)
        );
        assert!(
            enrich_usage_with_endpoints_cost(wire_cost_usage, &go_prov, &endpoints)
                .cost
                .is_none()
        );

        let zen_prov = Some(ModelProvider::OpenAiCompatible {
            provider_name: "OpenCode Zen".to_string(),
            model_id: "anthropic/claude-sonnet-5".to_string(),
            thinking: None,
        });
        let zen_priced = enrich_usage_with_endpoints_cost(usage, &zen_prov, &endpoints);
        assert!((zen_priced.cost.expect("priced").total - 0.30).abs() < 1e-6);

        let custom_prov = Some(ModelProvider::OpenAiCompatible {
            provider_name: "Custom vLLM".to_string(),
            model_id: "my-local-model".to_string(),
            thinking: Some(ThinkingLevel::High),
        });
        assert_eq!(
            model_context_window_with_endpoints(&custom_prov, &endpoints),
            Some(65_536)
        );
        let custom_priced = enrich_usage_with_endpoints_cost(usage, &custom_prov, &endpoints);
        assert!((custom_priced.cost.expect("priced").total - 0.21).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_unconfigured_agent_returns_clean_error_and_clones_cheaply() {
        let creds = AiCredentials::default();
        let agent = create_agent(&None, &creds, "System instructions");
        assert!(!agent.is_ready());
        assert!(agent.provider().is_none());

        let cloned = agent.clone();
        let err = cloned
            .prompt("Hello")
            .await
            .expect_err("unconfigured agent should return error");
        assert!(err.to_string().contains("No AI model selected"));
    }

    #[test]
    fn test_chatgpt_subscription_provider_specs_search_and_zero_cost() {
        let creds = AiCredentials::default();
        let chatgpt_prov =
            ModelProvider::ChatGpt(ChatGptModel::new("gpt-6.2-sol"), Some(ThinkingLevel::High));

        let (backend, provider_name, model_name, reasoning) =
            build_provider_model(&chatgpt_prov, &creds);
        assert!(matches!(backend, ProviderModelBackend::SignedIn(_)));
        assert_eq!(provider_name, "ChatGPT");
        assert_eq!(model_name, "gpt-6.2-sol");
        assert_eq!(reasoning, Some(Reasoning::Effort(Effort::High)));

        let wrapped = Some(chatgpt_prov.clone());
        assert_eq!(model_context_window(&wrapped), Some(1_050_000));
        assert_eq!(model_input_limit(&wrapped), Some(922_000));
        assert_eq!(model_output_limit(&wrapped), Some(128_000));
        assert_eq!(
            native_search_provider_tool(&chatgpt_prov),
            Some(ProviderToolDefinition::new("web_search"))
        );

        let mut synthetic_wire_cost = Usage::new().input_tokens(50_000).output_tokens(10_000);
        synthetic_wire_cost.cost = Some(Cost::from_total(1.25));
        assert!(
            enrich_usage_with_catalog_cost(synthetic_wire_cost, &wrapped)
                .cost
                .is_none(),
            "ChatGPT subscription turns must report $0.00 marginal token cost"
        );
    }
}
