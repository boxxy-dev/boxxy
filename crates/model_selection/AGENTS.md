# boxxy-model-selection

## Role
Provides UI components and types for selecting AI models and their specific parameters (like "Thinking Level").

## Responsibility
- **Dynamic Catalog-Driven Models (`models.rs`):** `ChatGptModel` (`chatgpt`, with live account override cache `set_chatgpt_account_models`), `GeminiModel`, `AnthropicModel`, `OpenAiModel`, `DeepSeekModel`, `MoonshotModel`, `XaiModel`, `ZaiModel`, `MistralModel`, `MiniMaxModel`, `GroqModel`, `OpenCodeZenModel` (`opencode`), and `OpenCodeGoModel` (`opencode-go`) are `#[serde(transparent)]` string wrappers that dynamically query `rig::catalog::Catalog::builtin()` (`catalog_models_for_vendor`) for active, tool-capable chat models and dynamically compute each model's supported thinking levels (`catalog_thinking_levels`) and default thinking level (`catalog_default_thinking_level`) from `ModelSpec.reasoning`. `ModelProvider` also exposes capability predicates (`supports_native_search()` and `supports_image_generation()`).
- **Provider Registry Pattern:** Uses the `AiProvider` trait (including `uses_oauth()` for OAuth-backed subscription providers) to abstract provider-specific logic (ChatGPT, Gemini, Ollama, Anthropic, OpenAI, OpenRouter, DeepSeek, Moonshot, xAI, Z.ai, Mistral, MiniMax, Groq, OpenCode Zen, OpenCode Go, plus user-configured `OpenAiCompatibleEndpoint` providers via `get_providers_with_custom`).
- **OpenAI-Compatible Endpoints (`OpenAiCompatibleEndpoint` & `CustomModelConfig`):** Supports arbitrary OpenAI-compatible servers with built-in presets for **OpenCode Go**, **OpenCode Zen**, **LiteLLM**, and **Local vLLM / llama.cpp**, live `/models` discovery (`fetch_openai_compatible_models`), catalog-driven route inference (`OpenAiApiRoute::infer_for_model` checking `ChatTools::Never` / non-disableable `ChatTools::ReasoningOff`), custom context window overrides, comma-separated thinking modes (`ThinkingLevel::parse_comma_separated`, mapping both `Effort::XHigh` and `Effort::Max` to `"Max"`), custom $/1M pricing, and automatic fallback to `rig::catalog::Catalog::builtin()` (`resolve_any_catalog_spec`).
- **Dynamic Model Discovery:** Fetches available local models from the Ollama API (`http://localhost:11434/api/tags`) via a shared single-flight `OllamaTagsCache` so opening the dialog performs at most one localhost fetch across all selectors.
- **Model Assignment Dialog (`GlobalModelSelectorDialog` & `resources/ui/model_selection_dialog.ui`):** Provides `Characters` (mandatory 3-column horizontal `Default for all characters` + master-detail `Character Overrides` with `Default` / `Custom` / `Broken` badges and a single shared rebinding detail selector) and `Dreams` (system-wide Dreams and Memories model with a `Use default character model` switch). Enforces the mandatory Default model rule by locking `Character Overrides` and `Done` until a valid Default model is selected.
- **Flexible Selector Modes (`SingleModelSelector`):** Supports `SelectorLayout::{Vertical, Horizontal}` with steady 3-column geometry, case-insensitive `StringFilterMatchMode::Substring` search filtering on the Model `GtkDropDown` across both Claw (Default & Character Overrides) and Dreams selectors, and `allow_none: bool` (using `selected_provider_index()` so `allow_none = false` override cards never expose `"None"`).
- **OpenRouter Support:** Uses a free-form `gtk::Entry` to support OpenRouter's massive model catalog rather than hardcoded dropdowns.
- **Key-based Filtering:** Automatically hides providers if their required API key is missing from the global settings or environment variables.
- **Persistence:** Remembers and restores the last selected Ollama model across provider switches.
- **Safety:** Uses `Cell<bool>` re-entrancy guards and non-blocking `try_borrow` / async `gio::File` avatar loading to prevent UI thread panics or main-thread disk I/O.
- Decouples the UI model selection logic from global preferences via `CharacterModelTarget` and `ModelAssignmentConfig`.

## Architecture
- `models.rs`: Pure data definitions and enums.
- `registry.rs`: The abstraction layer and registry of implementations.
- `ui/selector.rs`: The reusable model selection dropdown widget.
- `ui/dialog.rs`: The tabbed dialog for global model configuration.
