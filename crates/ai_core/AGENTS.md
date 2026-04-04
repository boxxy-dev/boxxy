# boxxy-ai-core Agents & Architecture

## Responsibilities
This crate provides the primary interface for AI interactions using the `rig-core` framework. It abstracts away the differences between Gemini, Anthropic, and Ollama providers, offering a unified agent interface for the rest of the application.

## Debugging & Observability
This crate provides a specialized `ModelContextHook` using the `rig::agent::PromptHook` trait. It intercepts and logs all model interactions:
- **`BOXXY_DEBUG_CONTEXT=1`**: A dedicated environment variable that activates detailed logging of the full context (Preamble, History, User Prompt) and final model responses.
- **Targeted Logs**: All interactions are logged using the `model-context` target at the `info!` level, ensuring they remain available in release builds for production troubleshooting without interfering with standard logs.

### `AiCredentials`
A unified payload struct that carries API keys and base URLs.
- Uses a `HashMap<String, String>` for API keys to allow dynamic scaling without changing function signatures.

### `create_agent`
A factory function that instantiates a `BoxxyAgent` based on a `ModelProvider` configuration and `AiCredentials`.
- Handles provider client initialization.
- Configures agent preambles (system prompts).

## Utilities

### `boxxy_ai_core::utils::runtime()`
Provides a singleton, multi-threaded Tokio runtime used for all background I/O and AI generation tasks across the entire application.
