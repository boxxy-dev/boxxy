//! `boxxy-cli`: Ghostty-style `[+action]` command-line parser, headless diagnostics,
//! and future-extensible CLI action framework for Boxxy Terminal.

pub mod actions;
pub mod output;
pub mod parser;

pub use parser::{
    DevFlags, LaunchRequest, LaunchTarget, ParseOutcome, detect_bin_name, parse_and_dispatch,
    parse_remote_launch,
};

use std::path::PathBuf;

/// Parses `std::env::args_os()` at process startup, executing any headless `+action`,
/// `--help`, or `--version` immediately before GTK or GResources initialize.
#[must_use]
pub fn dispatch_from_env() -> ParseOutcome {
    let cwd: Option<PathBuf> = std::env::current_dir().ok();
    parse_and_dispatch(std::env::args_os(), cwd.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::doctor::{CheckLevel, check_mcp_servers};
    use boxxy_mcp::config::{BackoffConfig, McpServerConfig, McpTransport};
    use std::path::Path;
    use std::process::ExitCode;

    fn parse_launch(args: &[&str], cwd: &Path) -> LaunchRequest {
        match parse_and_dispatch(args, Some(cwd)) {
            ParseOutcome::Launch(req) => req,
            ParseOutcome::Exit(code) => panic!("expected Launch, got Exit({code:?}) for {args:?}"),
        }
    }

    #[test]
    fn launch_defaults_and_positional_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sub = tmp.path().join("workspace");
        std::fs::create_dir_all(&sub).unwrap();

        let req = parse_launch(&["boxxy-terminal", "workspace"], tmp.path());
        assert_eq!(req.target, LaunchTarget::Default);
        assert_eq!(req.working_dir.as_deref(), Some(sub.as_path()));
        assert_eq!(req.command, None);
        assert!(!req.hold);
    }

    #[test]
    fn launch_resolves_file_argument_to_parent_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("module.rs");
        std::fs::write(&file, "fn demo() {}\n").unwrap();

        let req = parse_launch(&["boxxy-terminal", "module.rs"], tmp.path());
        assert_eq!(req.working_dir.as_deref(), Some(tmp.path()));
    }

    #[test]
    fn launch_captures_greedy_e_command_with_flags_and_plus_args() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let req = parse_launch(
            &[
                "boxxy-terminal",
                "--tab",
                "--hold",
                "--title",
                "Build Check",
                "-d",
                ".",
                "-e",
                "nvim",
                "-u",
                "NONE",
                "+42",
                "src/parser.rs",
            ],
            tmp.path(),
        );
        assert_eq!(req.target, LaunchTarget::NewTab);
        assert!(req.hold);
        assert_eq!(req.title.as_deref(), Some("Build Check"));
        assert_eq!(req.working_dir.as_deref(), Some(tmp.path()));
        assert_eq!(
            req.command,
            Some(vec![
                "nvim".to_string(),
                "-u".to_string(),
                "NONE".to_string(),
                "+42".to_string(),
                "src/parser.rs".to_string(),
            ])
        );
    }

    #[test]
    fn launch_supports_double_dash_command_and_plus_window_aliases() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let req = parse_launch(
            &["boxxy-terminal", "+new-tab", "--", "cargo", "test", "--lib"],
            tmp.path(),
        );
        assert_eq!(req.target, LaunchTarget::NewTab);
        assert_eq!(
            req.command,
            Some(vec![
                "cargo".to_string(),
                "test".to_string(),
                "--lib".to_string(),
            ])
        );

        let win_req = parse_launch(&["boxxy-terminal", "+new-window"], tmp.path());
        assert_eq!(win_req.target, LaunchTarget::NewWindow);
    }

    #[test]
    fn launch_parses_dev_isolation_flags_and_gapplication_replace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg = tmp.path().join("isolated-cfg");
        let req = parse_launch(
            &[
                "boxxy-terminal",
                "--dev",
                "--sync-settings",
                "--instance=alpha-1",
                "--ephemeral",
                "--debug-context",
                "--config-dir",
                &cfg.to_string_lossy(),
                "--gapplication-replace",
                "--new-window",
            ],
            tmp.path(),
        );
        assert!(req.dev.dev);
        assert!(req.dev.sync_settings);
        assert!(req.dev.ephemeral);
        assert!(req.dev.debug_context);
        assert_eq!(req.dev.instance.as_deref(), Some("alpha-1"));
        assert_eq!(req.dev.config_dir.as_deref(), Some(cfg.as_path()));
        assert!(req.replace_existing);
        assert_eq!(req.target, LaunchTarget::NewWindow);
    }

    #[test]
    fn empty_e_command_and_unknown_plus_action_exit_with_error_code() {
        let tmp = tempfile::tempdir().expect("tempdir");
        match parse_and_dispatch(["boxxy-terminal", "-e"], Some(tmp.path())) {
            ParseOutcome::Exit(code) => assert_eq!(code, ExitCode::from(2)),
            ParseOutcome::Launch(_) => panic!("empty -e must fail"),
        }

        match parse_and_dispatch(["boxxy-terminal", "+nonexistent"], Some(tmp.path())) {
            ParseOutcome::Exit(code) => assert_eq!(code, ExitCode::from(2)),
            ParseOutcome::Launch(_) => panic!("unknown +action must fail"),
        }
    }

    #[test]
    fn version_and_help_actions_exit_cleanly() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for argv in [
            &["boxxy-terminal", "+version", "--json"][..],
            &["boxxy-terminal", "--version"][..],
            &["boxxy-terminal", "-h"][..],
            &["boxxy-terminal", "--help"][..],
            &["boxxy-terminal", "+help", "doctor"][..],
        ] {
            match parse_and_dispatch(argv, Some(tmp.path())) {
                ParseOutcome::Exit(code) => assert_eq!(code, ExitCode::SUCCESS),
                ParseOutcome::Launch(_) => panic!("expected Exit(0) for {argv:?}"),
            }
        }
    }

    #[test]
    fn doctor_mcp_validator_catches_bad_urls_and_missing_binaries() {
        let valid_http = McpServerConfig {
            name: "remote-docs".to_string(),
            transport: McpTransport::Http {
                url: "https://example.com/mcp".to_string(),
                headers: Default::default(),
                streamable: true,
            },
            enabled: true,
            timeout_ms: 30_000,
            max_retries: 2,
            backoff: BackoffConfig::default(),
        };
        let check_ok = check_mcp_servers(std::slice::from_ref(&valid_http));
        assert_eq!(check_ok.level, CheckLevel::Ok);

        let broken_stdio = McpServerConfig {
            name: "missing-tool".to_string(),
            transport: McpTransport::Stdio {
                command: "definitely-not-a-real-binary-xyz987".to_string(),
                args: vec![],
                env: Default::default(),
            },
            enabled: true,
            timeout_ms: 30_000,
            max_retries: 2,
            backoff: BackoffConfig::default(),
        };
        let check_fail = check_mcp_servers(&[valid_http, broken_stdio]);
        assert_eq!(check_fail.level, CheckLevel::Fail);

        let disabled_bad_url = McpServerConfig {
            name: "draft-http".to_string(),
            transport: McpTransport::Http {
                url: "ftp://example.com/invalid".to_string(),
                headers: Default::default(),
                streamable: false,
            },
            enabled: false,
            timeout_ms: 30_000,
            max_retries: 2,
            backoff: BackoffConfig::default(),
        };
        let check_warn = check_mcp_servers(&[disabled_bad_url]);
        assert_eq!(check_warn.level, CheckLevel::Warn);
    }

    #[test]
    fn gapplication_args_roundtrip_preserves_embedded_help_and_flags_after_e() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let original = parse_launch(
            &[
                "boxxy-terminal",
                "--tab",
                "--hold",
                "--title",
                "CLI Check",
                "-d",
                ".",
                "-e",
                "tool-runner",
                "--help",
                "--version",
                "-x",
            ],
            tmp.path(),
        );

        let gapp_argv = original.to_gapplication_args("boxxy-terminal");
        // Neither `--help` nor `--version` appears as a bare flag in `gapp_argv`
        assert!(!gapp_argv.iter().any(|a| a == "--help" || a == "--version"));

        // Simulate GLib stripping the `--` separator before forwarding to `connect_command_line`
        let stripped: Vec<String> = gapp_argv.into_iter().filter(|a| a != "--").collect();
        let decoded = parse_remote_launch(&stripped, None).expect("remote decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn gapplication_args_preserves_service_mode_and_backwards_compatible_plain_flags() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // 1. `--gapplication-service` must emit ONLY `[bin, "--gapplication-service"]` for GLib
        let svc = parse_launch(&["boxxy-terminal", "--gapplication-service"], tmp.path());
        assert_eq!(
            svc.to_gapplication_args("boxxy-terminal"),
            vec!["boxxy-terminal", "--gapplication-service"]
        );

        // 2. Plain `--tab <dir>` emits standard flags compatible with pre-upgrade windows
        let tab_req = parse_launch(&["boxxy-terminal", "--tab", "."], tmp.path());
        let plain_argv = tab_req.to_gapplication_args("boxxy-terminal");
        assert_eq!(
            plain_argv,
            vec![
                "boxxy-terminal".to_string(),
                "--tab".to_string(),
                "--working-directory".to_string(),
                tmp.path().to_string_lossy().into_owned(),
            ]
        );
        let decoded_plain = parse_remote_launch(&plain_argv, None).expect("plain decode");
        assert_eq!(decoded_plain.target, LaunchTarget::NewTab);
        assert_eq!(decoded_plain.working_dir.as_deref(), Some(tmp.path()));

        // 3. `-e <cmd>` without `-d` defaults `working_dir` to caller's CWD
        let cmd_req = parse_launch(&["boxxy-terminal", "-e", "htop"], tmp.path());
        assert_eq!(cmd_req.working_dir.as_deref(), Some(tmp.path()));

        // 4. `cargo dev-app +version` (`--dev` before `+action`) dispatches headlessly
        match parse_and_dispatch(["boxxy-terminal", "--dev", "+version"], Some(tmp.path())) {
            ParseOutcome::Exit(code) => assert_eq!(code, ExitCode::SUCCESS),
            ParseOutcome::Launch(_) => panic!("expected +version to exit headlessly"),
        }
    }
}
