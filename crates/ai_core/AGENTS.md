# boxxy-ai-core Agents & Architecture

## Responsibilities
This crate provides the primary interface for AI interactions using the `rig-core` framework. It abstracts away the differences between Gemini, Anthropic, OpenAI, DeepSeek, OpenRouter, and Ollama providers, offering a unified agent interface for the rest of the application.

## Debugging & Observability
This crate provides a specialized `ModelContextHook` using the `rig::agent::AgentHook` trait. It intercepts and logs all model interactions:
- **`BOXXY_DEBUG_CONTEXT=1`**: A dedicated environment variable that activates detailed logging of the full context (Preamble, History, User Prompt), tool calls, and final model responses.
- **Targeted Logs**: All interactions are logged using the `model-context` target at the `info!` level, ensuring they remain available in release builds for production troubleshooting without interfering with standard logs.

### `AiCredentials`
A unified payload struct that carries API keys and base URLs.
- Uses a `HashMap<String, String>` for API keys to allow dynamic scaling without changing function signatures.

### `build_provider_model` & `create_agent`
`build_provider_model` is the shared factory that resolves a `ModelProvider` and `AiCredentials` into a type-erased `rig::DynModel<rig::operation::Completion>` along with portable `rig::completion::Reasoning` options (`Effort` / `Budget` / `Off`).
`create_agent` and `create_agent_with_options` instantiate a `BoxxyAgent` with portable `GenerationOptions` (`.reasoning(..)`, `.cache(CacheRetention::Short)`, and `.on_unsupported(OnUnsupported::Ignore)`), ensuring consistent prompt caching and thinking configurations across both the AI Chat sidebar and the Claw reasoning engine.
`native_search_additional_params` provides the provider-hosted search payload (`googleSearch` for Gemini, `web_search` for OpenAI Responses, `web_search_20250305` for Anthropic), while `append_citations_to_output` and `extract_native_search_summary` normalize URL citations and server-side search telemetry across providers.

## Utilities

### `boxxy_ai_core::utils::runtime()`
Provides a singleton, multi-threaded Tokio runtime used for all background I/O and AI generation tasks across the entire application.
