# boxxy-ai-core Agents & Architecture

## Responsibilities
This crate provides the primary interface for AI interactions using the `rig-core` framework. It abstracts away the differences between Gemini and Ollama providers, offering a unified agent interface for the rest of the application.

## Public Components

### `BoxxyAgent`
A wrapper enum around provider-specific `rig::agent::Agent` instances.
- **Variants:** `Gemini`, `Ollama`.
- **Methods:** `chat` (for conversational history) and `prompt` (for single-shot requests).

### `create_agent`
A factory function that instantiates a `BoxxyAgent` based on a `ModelProvider` configuration.
- Handles provider client initialization (e.g. Gemini API keys or Ollama Base URL).
- Configures agent preambles (system prompts).

## Utilities

### `boxxy_ai_core::utils::runtime()`
Provides a singleton, multi-threaded Tokio runtime used for all background I/O and AI generation tasks across the entire application.
