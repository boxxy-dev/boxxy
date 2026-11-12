# CLI Crate (`boxxy-cli`)

## Responsibility
Provides the Ghostty-inspired `boxxy-terminal [+action] [options] [dir] [-e <command> [args...]]` command-line parser, headless `+action` dispatcher, and structured diagnostic tooling.

Designed to stay **100% free of GTK / Libadwaita / GResource initialization** so CLI actions execute in milliseconds over headless SSH, scripts, and TTYs without opening windows or touching Wayland/X11 displays.

## Architecture

- **`parser.rs`**:
  - Splits greedy command tails (`-e <cmd> [args...]`, `--command=<cmd>`, and `-- <cmd> [args...]`) before flag parsing so flags belonging to child programs (such as `nvim -u NONE +42` or `tool --help`) never collide with `boxxy-terminal` options.
  - Detects Ghostty-style `+action` tokens at the first positional slot. `+new-window` and `+new-tab` map cleanly into `LaunchRequest`, while headless `+actions` (`+version`, `+doctor`, `+env`, `+agent`, `+completions`, `+help`) run immediately and return `ParseOutcome::Exit(ExitCode)`.
  - Encodes validated `LaunchRequest` payloads via `LaunchRequest::to_gapplication_args` (`-- __boxxy_launch_v1__:<json>`) for `gio::Application::run_with_args`, preventing GLib's internal `GOptionContext` from hijacking child `--help`/`--version` flags while preserving native single-instance D-Bus forwarding, `--gapplication-replace`, and Wayland `XDG_ACTIVATION_TOKEN` focus handoff.
  - Renders tiered help: concise everyday summary on `-h` and full reference (including isolated `--dev` flags and environment pointers) on `--help`.
- **`output.rs`**: TTY- and `NO_COLOR`-aware `TermStyle` helper plus `OutputFormat` (`Human` vs `--json`).
- **`actions/`**:
  - **`actions/mod.rs` (`ActionContext`)**: Shared context carrying `CliDevOverrides`, `TermStyle`, and a lightweight single-threaded Tokio runner (`run_async`) for on-demand session D-Bus RPCs to `boxxy-agent`.
  - **`actions/version.rs` (`+version`)**: Version, git commit hash, build channel (`native`, `flatpak`, `system-package`), OS/arch, and self-update availability.
  - **`actions/doctor.rs` (`+doctor`)**: Non-destructive health check covering resolved paths/SQLite DB, strict `settings.json` JSON validation, community `.toml` theme validation, `CHARACTER.toml` validation, MCP server `stdio` binary / `http(s)` URL sanity checks, AI model key readiness, shell integration status, and live `boxxy-agent` D-Bus status & version/config-dir match.
  - **`actions/env_cmd.rs` (`+env`)**: Renders the canonical `boxxy_sys_utils::env::ENV_SPECS` registry (plus custom endpoint `api_key_env` vars), showing live values with secrets masked (`--internal` includes child-PTY plumbing vars).
  - **`actions/agent.rs` (`+agent`)**: User-facing D-Bus control plane (`status`, `sessions`, `stop`, `restart`) backed by `boxxy_agent::cli_ops`, working identically across Native, Nix, and Flatpak.
  - **`actions/completions.rs` (`+completions`)**: Generates shell completions (including `+action` completion hooks) for `bash`, `zsh`, `fish`, and `nushell`.

## Future Extension (`+claw`)
Because `ClawSession`, `CharacterRegistry`, `WorkspaceRegistry`, and `boxxy-core-toolbox` run inside the headless `boxxy-agent` daemon over session D-Bus, future CLI Claw workflows (`actions/claw.rs` / `ActionSubcommand::Claw`) plug directly into `ActionContext::run_async` without any parser or GTK changes.
