# Telemetry Crate (`boxxy-telemetry`)

## Responsibility
Provides a privacy-first, lightweight observability layer for Boxxy Terminal that records anonymous usage metrics to a coalescing SQLite journal (`telemetry_journal` in `boxxy-db`) and exports spec-compliant **OTLP/HTTP JSON** (`ExportMetricsServiceRequest`) batches to the Supabase backend (`telemetry-ingest`).

## Module Layout

- **`src/lib.rs`**: Public constants and high-level tracking & lifecycle API (`init_with_db`, `init_db`, `init`, `flush_journal`, `disable_and_purge`, `track_launch`, `track_ai_turn`, `track_ai_invocation`, `track_ai_tokens`, `track_ai_latency`, `track_tool_use`, `track_session_resume`).
- **`src/identity.rs`**: Lock-free `session_id` management (`ArcSwap`), `UUID` validation for `install_id` and `session_id`, and `is_telemetry_enabled()` fast-path checks.
- **`src/normalize.rs`**: Canonical privacy normalizers and allowlists:
  - **`BUILTIN_BOXXY_TOOLS` (46 sorted tools):** Buckets any external user MCP tool under `"external_mcp_tool"`.
  - **`BUILTIN_PROVIDERS` (15 built-in providers):** `ChatGPT`, `Gemini`, `Ollama`, `Anthropic`, `OpenAI`, `OpenRouter`, `DeepSeek`, `Moonshot`, `xAI`, `Z.ai`, `Mistral`, `MiniMax`, `Groq`, `OpenCode Zen`, `OpenCode Go`; all user-defined `OpenAiCompatible` endpoints map to `"Custom Endpoints"`.
  - **Model Privacy (`normalize_model_name`):** Redacts `model_name` to `"redacted"` for `"Ollama"` and `"Custom Endpoints"` before writing to SQLite so private local tags or custom fine-tuned model IDs never leave the machine.
  - **Environment & Theme Buckets:** `normalize_os`, `normalize_shell`, `normalize_desktop`, `normalize_pkg_type` (`"flatpak"`, `"distro"`, `"native"`), `resolve_theme_telemetry`, and `normalize_theme` (`"Boxxy Dark"`, `"Boxxy Light"`, `"Boxxy Community"`, `"Adwaita"`).
- **`src/event.rs`**: Deterministic `BTreeMap`-ordered `MetricRecord` serializer ensuring stable JSON key order for SQLite `UNIQUE` coalescing.
- **`src/journal.rs`**: Shared `boxxy-db` pool handle (`parking_lot::RwLock<Option<Db>>`), single-transaction batch recording, 7-day / 5,000-row retention bounds, and instant opt-out purge.
- **`src/exporter.rs`**: `FLUSH_LOCK`-serialized async `reqwest` OTLP/HTTP JSON exporter (`Temporality::Delta` = `1`), preserving exact `recorded_at_ms -> timeUnixNano`, attaching per-row `event_id` UUIDs for end-to-end idempotency, retrying transient `408`/`429`/`5xx` responses with exponential backoff + jitter and `Retry-After` headers, and acknowledging exported values atomically via `Store::ack_telemetry_events`.

## Key Invariants
1. **Zero OTel SDK Bloat:** Emits standard OTLP/HTTP JSON directly via `serde` + `reqwest`, avoiding `opentelemetry_sdk` background OS threads, `PeriodicReader` timer races, and duplicate timestamp erasure.
2. **Serialized & Lossless Concurrent Export:** `FLUSH_LOCK` prevents concurrent callers (30m daemon timer, client disconnect, reload, `SIGTERM`) from exporting the same batch twice, while `Store::ack_telemetry_events` subtracts `exported_value` and rotates `event_id` if new increments coalesced while an HTTP POST was in flight.
3. **100% Opt-Out Compliance:** Disabling `enable_telemetry` in Preferences immediately drops the HTTP exporter and purges `telemetry_journal` without requiring an app or daemon restart. Isolated `--dev` instances disable telemetry automatically unless `BOXXY_TELEMETRY_ENDPOINT` is explicitly set.

## Instrumentation Points

| Metric Name | Tracking Location | Attributes |
| :--- | :--- | :--- |
| `app.launch` | `boxxy-app` | `os`, `arch`, `pkg_type` (`flatpak`/`native`/`distro`), `version`, `shell`, `desktop`, `theme` |
| `ai.invocations` | `boxxy-claw` (`track_ai_turn`) | `model_provider`, `model_name` (`redacted` for Ollama/Custom), `feature` |
| `ai.tokens` | `boxxy-claw` (`track_ai_turn`) | `model_provider`, `model_name` (`redacted` for Ollama/Custom), `role` (`input`/`output`), `feature` |
| `ai.latency` | `boxxy-claw` (`track_ai_turn`) | `model_provider`, `model_name` (`redacted` for Ollama/Custom), `feature` |
| `tool.use` | `boxxy-claw` & `core-toolbox` | `tool_name` (46 built-ins or `external_mcp_tool`) |
| `claw.session_resume` | `boxxy-claw` | `session_type` (`normal`/`pinned`) |
