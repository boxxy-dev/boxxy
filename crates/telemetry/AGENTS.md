# Telemetry Crate (`boxxy-telemetry`)

## Responsibility
Provides a privacy-first, industry-standard observability layer for Boxxy Terminal using **OpenTelemetry (OTel) v0.32.0**. It handles the collection, batching, and secure export of anonymous usage statistics to a Supabase backend.

## Architectural Design

### 1. Isolation & Efficiency
- **Heavy Dependency Guard**: All OpenTelemetry SDK and OTLP exporter dependencies are contained within this crate to minimize build times and binary bloat for other crates.
- **Asynchronous & Non-Blocking**: Telemetry operations never block the GTK UI thread. All networking and batching occur in background threads managed by the OTel SDK.
- **Delta Temporality**: Uses `Temporality::Delta` to report only new values since the last export, preventing duplicate data and keeping the database lean.
- **10-Minute Batching**: Metrics are aggregated locally and exported every 600 seconds to minimize battery and bandwidth impact.

### 2. Anonymous Identity Management
- **`install_id`**: A random UUIDv4 generated on first launch and persisted in `settings.json`. This tracks unique installs without any connection to PII.
- **`session_id`**: A random UUIDv4 generated per application launch and kept only in memory. This allows for calculating session length and distinguishing between "many launches" and "many users."
- **Zero PII Policy**: The system is designed to never capture file paths, usernames, prompt contents, or terminal outputs.

### 3. Ingestion Pipeline
- **Protocol**: Exports data via **OTLP/HTTP JSON** for maximum compatibility with serverless environments (Supabase Edge Functions).
- **Secure Gateway**: Data is sent to a Supabase Edge Function which validates the schema (`v: 1`) and enforces rate limiting before inserting flattened rows into the `telemetry_events` table.

## Instrumentation Points

| Metric Name | Tracking Location | Description |
| :--- | :--- | :--- |
| `app.launch` | `boxxy-app` | Tracks OS, Architecture, Shell, and Version. |
| `ai.tokens` | `boxxy-claw` | Tracks input/output token counts per model. |
| `ai.latency` | `boxxy-claw` | Tracks time-to-first-token for performance monitoring. |
| `tool.use` | `core-toolbox` | Tracks which AI tools (e.g., `file_read`, `sys_shell`) are most popular. |
| `claw.session_resume` | `boxxy-claw` | Tracks retention via session restoration frequency. |

## Usage Guidelines

### Initialization
Telemetry is initialized in `boxxy-app`'s main entry point. It respects the `enable_telemetry` user preference but is **enabled by default** during the Preview Phase.

```rust
// In boxxy-app main.rs
boxxy_telemetry::init().await;
```

### Tracking Events
Other crates should use the high-level, Boxxy-specific API:

```rust
// Track AI usage
boxxy_telemetry::track_ai_tokens("gemini-1.5-pro", "output", 150).await;

// Track tool execution
boxxy_telemetry::track_tool_use("sys_shell_exec").await;
```

### Debugging
In local development (debug builds), telemetry events are also mirrored to **Stdout** for easy verification. To see detailed activity, run with:
`RUST_LOG=boxxy_telemetry=debug cargo run -p boxxy-app`
