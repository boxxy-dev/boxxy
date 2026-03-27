# boxxy-ai-core Agents & Architecture

## Responsibilities
This crate provides the primary interface for AI interactions using the `rig-core` framework. It abstracts away the differences between Gemini, Anthropic, and Ollama providers, offering a unified agent interface for the rest of the application.

## Public Components

### `BoxxyAgent`
A wrapper enum around provider-specific `rig::agent::Agent` instances.
- **Variants:** `Gemini`, `Anthropic`, `Ollama`.
- **Methods:** `chat` (for conversational history) and `prompt` (for single-shot requests). Both methods leverage the `.extended_details()` builder pattern to execute automatic tool loops while returning a tuple `(String, Option<rig::completion::Usage>)`. This exposes token consumption metadata (input, output, total) to the UI for observability.

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
