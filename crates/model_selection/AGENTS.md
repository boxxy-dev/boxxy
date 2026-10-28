# boxxy-model-selection

## Role
Provides UI components and types for selecting AI models and their specific parameters (like "Thinking Level").

## Responsibility
- **Dynamic Catalog-Driven Models (`models.rs`):** `GeminiModel`, `AnthropicModel`, `OpenAiModel`, `DeepSeekModel`, `MoonshotModel`, `XaiModel`, `ZaiModel`, `MistralModel`, `MiniMaxModel`, and `GroqModel` are `#[serde(transparent)]` string wrappers that dynamically query `rig::catalog::Catalog::builtin()` (`catalog_models_for_vendor`) for active, tool-capable chat models and dynamically compute each model's supported thinking levels (`catalog_thinking_levels`) and default thinking level (`catalog_default_thinking_level`) from `ModelSpec.reasoning`.
- **Provider Registry Pattern:** Uses the `AiProvider` trait to abstract provider-specific logic (Gemini, Ollama, Anthropic, OpenAI, OpenRouter, DeepSeek, Moonshot, xAI, Z.ai, Mistral, MiniMax, Groq, plus user-configured `OpenAiCompatibleEndpoint` providers via `get_providers_with_custom`).
- **OpenAI-Compatible Endpoints (`OpenAiCompatibleEndpoint` & `CustomModelConfig`):** Supports arbitrary OpenAI-compatible servers with built-in presets for **OpenCode Go**, **OpenCode Zen**, **LiteLLM**, and **Local vLLM / llama.cpp**, live `/models` discovery (`fetch_openai_compatible_models`), route selection (`ChatCompletions` vs `Responses`), custom context window overrides, comma-separated thinking modes (`ThinkingLevel::parse_comma_separated`), custom $/1M pricing, and automatic fallback to `rig::catalog::Catalog::builtin()` (`resolve_any_catalog_spec`).
- **Dynamic Model Discovery:** Automatically fetches available local models from the Ollama API (via `http://localhost:11434/api/tags`) when the Ollama provider is selected.
- **Dynamic UI:** `SingleModelSelector` and `GlobalModelSelectorDialog` are now data-driven, building their dropdowns and options dynamically from the registry instead of hardcoded logic.
- **OpenRouter Support:** Uses a free-form `gtk::Entry` to support OpenRouter's massive model catalog rather than hardcoded dropdowns.
- **Key-based Filtering:** Automatically hides providers if their required API key is missing from the global settings or environment variables.
- **Persistence:** Remembers and restores the last selected Ollama model across provider switches.
- **Safety:** Uses non-blocking `try_borrow` patterns to prevent UI thread panics during GTK signal recursion.
- Decouples the UI model selection logic from global preferences.

## Architecture
- `models.rs`: Pure data definitions and enums.
- `registry.rs`: The abstraction layer and registry of implementations.
- `ui/selector.rs`: The reusable model selection dropdown widget.
- `ui/dialog.rs`: The tabbed dialog for global model configuration.
