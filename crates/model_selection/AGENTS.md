# boxxy-model-selection

## Role
Provides UI components and types for selecting AI models and their specific parameters (like "Thinking Level").

## Responsibility
- Defines `ModelProvider` which encapsulates the chosen model (e.g., Gemini Flash vs Pro, or dynamic Ollama models).
- Exposes `GlobalModelSelectorPopover`, a GTK Popover that allows users to configure AI models for both AI Chat and Boxxy Apps independently.
- **Dynamic Model Discovery:** Automatically fetches available local models from the Ollama API (via `http://localhost:11434/api/tags`) when the Ollama provider is selected.
- **Persistence:** Remembers and restores the last selected Ollama model across provider switches.
- **Safety:** Uses non-blocking `try_borrow` patterns to prevent UI thread panics during GTK signal recursion.
- Decouples the UI model selection logic from global preferences.
