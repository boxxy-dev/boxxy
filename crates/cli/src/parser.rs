//! Ghostty-style `[+action]` + terminal launch CLI parser built on `clap`.

use crate::actions::agent::AgentArgs;
use crate::actions::completions::{CompletionsArgs, PUBLIC_PLUS_ACTIONS};
use crate::actions::doctor::DoctorArgs;
use crate::actions::env_cmd::EnvArgs;
use crate::actions::version::{VersionArgs, full_version_str};
use crate::actions::{ActionContext, finish_action};
use crate::output::TermStyle;
use boxxy_sys_utils::CliDevOverrides;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Target window/tab routing for a GUI terminal launch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LaunchTarget {
    #[default]
    Default,
    NewWindow,
    NewTab,
    Present,
}

/// Validated parameters for launching or activating a Boxxy Terminal GUI window/tab.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaunchRequest {
    pub dev: CliDevOverrides,
    pub target: LaunchTarget,
    pub working_dir: Option<PathBuf>,
    pub command: Option<Vec<String>>,
    pub hold: bool,
    pub title: Option<String>,
    pub replace_existing: bool,
    #[serde(default)]
    pub gapplication_service: bool,
}

const GAPP_LAUNCH_PREFIX: &str = "__boxxy_launch_v1__:";

impl LaunchRequest {
    /// Encodes this validated [`LaunchRequest`] into an `argv` vector safe to pass to
    /// `gio::ApplicationExtManual::run_with_args`.
    ///
    /// Placing the JSON payload after `--` guarantees GLib's internal `GOptionContext`
    /// never intercepts `--help`, `--version`, or unknown flags meant for `-e <cmd>`,
    /// while keeping `--gapplication-replace` and `--gapplication-service` before `--`
    /// so GIO handles single-instance replacement and Wayland activation tokens natively.
    #[must_use]
    pub fn to_gapplication_args(&self, bin_name: &str) -> Vec<String> {
        let mut out = Vec::with_capacity(6);
        out.push(bin_name.to_string());

        // GLib requires `--gapplication-service` to be the sole option when D-Bus activates the app.
        if self.gapplication_service {
            out.push("--gapplication-service".to_string());
            return out;
        }

        if self.replace_existing {
            out.push("--gapplication-replace".to_string());
        }

        // When no `-e` command, `--hold`, or `--title` is present, emit standard flags so
        // forwarding works seamlessly even if an older pre-upgrade Boxxy window is currently open.
        if self.command.is_none() && !self.hold && self.title.is_none() {
            match self.target {
                LaunchTarget::Default => {}
                LaunchTarget::NewWindow => out.push("--new-window".to_string()),
                LaunchTarget::NewTab => out.push("--tab".to_string()),
                LaunchTarget::Present => out.push("--present".to_string()),
            }
            if let Some(ref dir) = self.working_dir {
                out.push("--working-directory".to_string());
                out.push(dir.to_string_lossy().into_owned());
            }
            return out;
        }

        if let Ok(json) = serde_json::to_string(self) {
            out.push("--".to_string());
            out.push(format!("{GAPP_LAUNCH_PREFIX}{json}"));
        }
        out
    }
}

/// Outcome of parsing the command line before GTK starts.
#[derive(Debug)]
pub enum ParseOutcome {
    /// A headless `+action`, `--help`, `--version`, or CLI syntax error was already handled.
    Exit(ExitCode),
    /// Proceed to launch / forward the GUI terminal request.
    Launch(LaunchRequest),
}

/// Shared development & instance-isolation flags accepted across launch mode and all `+actions`.
#[derive(Args, Clone, Debug, Default, PartialEq, Eq)]
pub struct DevFlags {
    /// Run as an isolated development instance alongside Stable Boxxy.
    #[arg(long, global = true)]
    pub dev: bool,

    /// Run as a named isolated instance (`~/.config/boxxy-terminal-<slug>/`).
    #[arg(long, global = true, value_name = "SLUG")]
    pub instance: Option<String>,

    /// Run an isolated instance with clean factory-default settings in `$XDG_RUNTIME_DIR`.
    #[arg(long, global = true)]
    pub ephemeral: bool,

    /// Refresh dev instance settings from Stable Boxxy before launching.
    #[arg(long, global = true)]
    pub sync_settings: bool,

    /// Override the active configuration and SQLite directory (must be an absolute path).
    #[arg(long, global = true, value_name = "ABS_PATH")]
    pub config_dir: Option<PathBuf>,

    /// Log full LLM system prompts, history, and tool payloads on `model-context`.
    #[arg(long, global = true)]
    pub debug_context: bool,
}

impl From<DevFlags> for CliDevOverrides {
    fn from(flags: DevFlags) -> Self {
        Self {
            dev: flags.dev,
            instance: flags.instance,
            ephemeral: flags.ephemeral,
            sync_settings: flags.sync_settings,
            config_dir: flags.config_dir,
            debug_context: flags.debug_context,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "boxxy-terminal",
    version = full_version_str(),
    disable_help_flag = true,
    disable_version_flag = true
)]
struct LaunchCli {
    /// Open a new window.
    #[arg(long, conflicts_with_all = ["tab", "present"])]
    new_window: bool,

    /// Open a new tab in the active window.
    #[arg(short = 't', long, conflicts_with_all = ["new_window", "present"])]
    tab: bool,

    /// Raise the active window if one is open, or launch a new window.
    #[arg(long, conflicts_with_all = ["new_window", "tab"])]
    present: bool,

    /// Set the terminal's initial working directory.
    #[arg(short = 'd', long = "working-directory", value_name = "DIR")]
    working_directory: Option<PathBuf>,

    /// Keep the terminal pane open after the `-e` command exits.
    #[arg(long)]
    hold: bool,

    /// Set an initial custom title for the terminal tab.
    #[arg(long, value_name = "TITLE")]
    title: Option<String>,

    /// Replace an existing primary GApplication instance (used during self-update restarts).
    #[arg(long = "gapplication-replace", hide = true)]
    gapplication_replace: bool,

    /// GTK service activation flag passed by D-Bus desktop activation.
    #[arg(long = "gapplication-service", hide = true)]
    gapplication_service: bool,

    #[command(flatten)]
    dev: DevFlags,

    /// Optional initial directory to open (shorthand for `-d <DIR>`).
    #[arg(value_name = "DIR")]
    positional_dir: Option<PathBuf>,
}

#[derive(Parser, Debug)]
#[command(name = "boxxy-terminal", version = full_version_str())]
struct ActionRootCli {
    #[command(flatten)]
    dev: DevFlags,

    #[command(subcommand)]
    action: ActionSubcommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ActionSubcommand {
    /// Print Boxxy Terminal version and build information.
    #[command(name = "+version")]
    Version(VersionArgs),
    /// Validate config files, themes, characters, MCP servers, and daemon health.
    #[command(name = "+doctor")]
    Doctor(DoctorArgs),
    /// Inspect supported BOXXY_* and provider API-key environment variables.
    #[command(name = "+env")]
    Env(EnvArgs),
    /// Inspect or control the host-side boxxy-agent D-Bus daemon.
    #[command(name = "+agent")]
    Agent(AgentArgs),
    /// Generate shell completion scripts (bash, zsh, fish, nushell).
    #[command(name = "+completions")]
    Completions(CompletionsArgs),
}

const KNOWN_HEADLESS_ACTIONS: &[&str] = &["version", "doctor", "env", "agent", "completions"];

/// Builds a unified `clap::Command` used by `clap_complete` for shell completion generation.
#[must_use]
pub fn build_completion_command(bin_name: &str) -> clap::Command {
    let cmd = LaunchCli::command()
        .bin_name(bin_name.to_string())
        .subcommand_required(false)
        .arg(
            clap::Arg::new("command")
                .short('e')
                .long("command")
                .value_name("CMD")
                .help("Execute command inside the terminal"),
        )
        .arg(
            clap::Arg::new("help")
                .short('h')
                .long("help")
                .action(clap::ArgAction::SetTrue)
                .help("Print help (-h concise, --help extended)"),
        )
        .arg(
            clap::Arg::new("version")
                .short('V')
                .long("version")
                .action(clap::ArgAction::SetTrue)
                .help("Print version"),
        );
    ActionSubcommand::augment_subcommands(cmd)
        .subcommand(clap::Command::new("+new-window").about("Open a new Boxxy Terminal window"))
        .subcommand(
            clap::Command::new("+new-tab").about("Open a new tab in the active Boxxy window"),
        )
        .subcommand(
            clap::Command::new("+help")
                .about("Show help for Boxxy Terminal or a specific +action"),
        )
}

/// Extracts a clean display binary name from `argv[0]` (defaults to `"boxxy-terminal"`).
#[must_use]
pub fn detect_bin_name(arg0: Option<&OsStr>) -> String {
    arg0.and_then(|s| Path::new(s).file_name())
        .and_then(|s| s.to_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("boxxy-terminal")
        .to_string()
}

/// Top-level CLI entry point invoked at the very start of `boxxy-terminal`'s `main()`.
pub fn parse_and_dispatch<I, T>(raw_args: I, base_cwd: Option<&Path>) -> ParseOutcome
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = raw_args.into_iter().map(Into::into).collect();
    let bin_name = detect_bin_name(args.first().map(OsString::as_os_str));
    let style = TermStyle::stdout();

    let (head, command_tail) = match split_command_tail(&args) {
        Ok(res) => res,
        Err(msg) => {
            eprintln!("{} {msg}", style.red("error:"));
            eprintln!("Run '{bin_name} -h' for usage.");
            return ParseOutcome::Exit(ExitCode::from(2));
        }
    };

    // Inspect the first positional token before `-e` / `--` to detect `+action`.
    if let Some((pos_idx, plus_raw)) = find_first_plus_token(&head) {
        let action_name = &plus_raw[1..];

        // `+new-window` and `+new-tab` are Ghostty-compatible launch aliases.
        if action_name == "new-window" || action_name == "new-tab" {
            let mut rewritten = head;
            rewritten[pos_idx] = if action_name == "new-window" {
                OsString::from("--new-window")
            } else {
                OsString::from("--tab")
            };
            return parse_launch_tokens(&bin_name, &rewritten, command_tail, base_cwd, style);
        }

        if command_tail.is_some() {
            eprintln!(
                "{} '-e/--command' cannot be combined with '+{action_name}'.",
                style.red("error:")
            );
            return ParseOutcome::Exit(ExitCode::from(2));
        }

        if action_name == "help" {
            return handle_plus_help(&bin_name, &head, pos_idx, style);
        }

        if KNOWN_HEADLESS_ACTIONS.contains(&action_name) {
            return dispatch_headless_action(&bin_name, &head, pos_idx, action_name, style);
        }

        print_unknown_action_error(&bin_name, action_name, style);
        return ParseOutcome::Exit(ExitCode::from(2));
    }

    parse_launch_tokens(&bin_name, &head, command_tail, base_cwd, style)
}

/// Parses forwarded `gio::ApplicationCommandLine` arguments inside the primary GTK process.
///
/// Accepts both pre-validated [`LaunchRequest::to_gapplication_args`] payloads and raw
/// external D-Bus `CommandLine` argument arrays.
pub fn parse_remote_launch<I, T>(raw_args: I, base_cwd: Option<&Path>) -> Result<LaunchRequest, String>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = raw_args.into_iter().map(Into::into).collect();

    for tok in args.iter().skip(1) {
        if let Some(s) = tok.to_str()
            && let Some(json) = s.strip_prefix(GAPP_LAUNCH_PREFIX)
        {
            return serde_json::from_str::<LaunchRequest>(json).map_err(|e| e.to_string());
        }
    }

    let (mut head, command_tail) = split_command_tail(&args)?;

    if let Some((pos_idx, plus_raw)) = find_first_plus_token(&head) {
        match &plus_raw[1..] {
            "new-window" => head[pos_idx] = OsString::from("--new-window"),
            "new-tab" => head[pos_idx] = OsString::from("--tab"),
            other => return Err(format!("unexpected remote action '+{other}'")),
        }
    }

    let cli = LaunchCli::try_parse_from(&head).map_err(|e| e.to_string())?;
    Ok(build_launch_request(cli, command_tail, base_cwd))
}

fn parse_launch_tokens(
    bin_name: &str,
    head: &[OsString],
    command_tail: Option<Vec<String>>,
    base_cwd: Option<&Path>,
    style: TermStyle,
) -> ParseOutcome {
    // Handle `-h`, `--help`, `-V`, and `--version` before clap when no `-e` consumed them.
    for tok in head.iter().skip(1) {
        let s = tok.to_string_lossy();
        if s == "-h" {
            print_root_help(bin_name, false, style);
            return ParseOutcome::Exit(ExitCode::SUCCESS);
        } else if s == "--help" {
            print_root_help(bin_name, true, style);
            return ParseOutcome::Exit(ExitCode::SUCCESS);
        } else if s == "-V" || s == "--version" {
            let ctx = ActionContext::new(bin_name.to_string(), CliDevOverrides::default());
            let code = finish_action(
                crate::actions::version::run(&ctx, &VersionArgs { json: false }),
                style,
            );
            return ParseOutcome::Exit(code);
        }
    }

    let cmd = LaunchCli::command().bin_name(bin_name.to_string());
    let matches = match cmd.try_get_matches_from(head) {
        Ok(m) => m,
        Err(err) => {
            let _ = err.print();
            return ParseOutcome::Exit(ExitCode::from(2));
        }
    };

    let cli = match LaunchCli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(err) => {
            let _ = err.print();
            return ParseOutcome::Exit(ExitCode::from(2));
        }
    };

    ParseOutcome::Launch(build_launch_request(cli, command_tail, base_cwd))
}

fn build_launch_request(
    cli: LaunchCli,
    command_tail: Option<Vec<String>>,
    base_cwd: Option<&Path>,
) -> LaunchRequest {
    let target = if cli.present {
        LaunchTarget::Present
    } else if cli.tab {
        LaunchTarget::NewTab
    } else if cli.new_window {
        LaunchTarget::NewWindow
    } else {
        LaunchTarget::Default
    };

    let raw_dir = cli.working_directory.or(cli.positional_dir);
    let working_dir = raw_dir
        .map(|d| resolve_working_dir(&d, base_cwd))
        .or_else(|| {
            // If `-e <cmd>` is invoked from a shell without an explicit `-d`, run the
            // command in the caller's current directory rather than an unrelated tab's CWD.
            if command_tail.is_some() {
                base_cwd
                    .map(Path::to_path_buf)
                    .or_else(|| std::env::current_dir().ok())
            } else {
                None
            }
        });

    let mut dev_overrides = CliDevOverrides::from(cli.dev);
    dev_overrides.config_dir = dev_overrides
        .config_dir
        .map(|p| resolve_path_against(&p, base_cwd));

    let title = cli
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    LaunchRequest {
        dev: dev_overrides,
        target,
        working_dir,
        command: command_tail,
        hold: cli.hold,
        title,
        replace_existing: cli.gapplication_replace,
        gapplication_service: cli.gapplication_service,
    }
}

fn resolve_path_against(path: &Path, base_cwd: Option<&Path>) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(base) = base_cwd {
        base.join(path)
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    normalize_lexical_path(&joined)
}

/// Cleans `.` and `..` path components lexically without resolving symlinks (`canonicalize`),
/// so `boxxy-terminal .` and `boxxy-terminal ..` produce clean tab titles and CWD strings.
fn normalize_lexical_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(out.components().next_back(), Some(Component::RootDir) | None) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn resolve_working_dir(raw: &Path, base_cwd: Option<&Path>) -> PathBuf {
    let resolved = resolve_path_against(raw, base_cwd);
    if resolved.is_file() {
        resolved
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(resolved)
    } else {
        resolved
    }
}

fn dispatch_headless_action(
    bin_name: &str,
    head: &[OsString],
    _pos_idx: usize,
    _action_name: &str,
    style: TermStyle,
) -> ParseOutcome {
    let root_cmd = ActionRootCli::command().bin_name(bin_name.to_string());

    let matches = match root_cmd.try_get_matches_from(head) {
        Ok(m) => m,
        Err(err) => {
            let exit = if err.use_stderr() {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            };
            let _ = err.print();
            return ParseOutcome::Exit(exit);
        }
    };

    let parsed = match ActionRootCli::from_arg_matches(&matches) {
        Ok(p) => p,
        Err(err) => {
            let _ = err.print();
            return ParseOutcome::Exit(ExitCode::from(2));
        }
    };

    let ctx = ActionContext::new(bin_name.to_string(), parsed.dev.into());
    let res = match &parsed.action {
        ActionSubcommand::Version(args) => crate::actions::version::run(&ctx, args),
        ActionSubcommand::Doctor(args) => crate::actions::doctor::run(&ctx, args),
        ActionSubcommand::Env(args) => crate::actions::env_cmd::run(&ctx, args),
        ActionSubcommand::Agent(args) => crate::actions::agent::run(&ctx, args),
        ActionSubcommand::Completions(args) => crate::actions::completions::run(&ctx, args),
    };

    ParseOutcome::Exit(finish_action(res, style))
}

fn handle_plus_help(
    bin_name: &str,
    head: &[OsString],
    pos_idx: usize,
    style: TermStyle,
) -> ParseOutcome {
    let target = head
        .get(pos_idx + 1)
        .map(|s| s.to_string_lossy().into_owned());

    if let Some(raw_target) = target {
        let stripped = raw_target.strip_prefix('+').unwrap_or(&raw_target);
        if KNOWN_HEADLESS_ACTIONS.contains(&stripped) {
            let synth = [
                OsString::from(bin_name),
                OsString::from(format!("+{stripped}")),
                OsString::from("--help"),
            ];
            return dispatch_headless_action(bin_name, &synth, 1, stripped, style);
        } else if stripped == "new-window" || stripped == "new-tab" {
            println!(
                "Usage: {bin_name} +{stripped} [options] [dir] [-e <command> [args...]]\n\n\
                 {} a Boxxy Terminal {}.\n\
                 Accepts all standard launch options (`-d`, `-e`, `--hold`, `--title`).",
                if stripped == "new-window" {
                    "Open"
                } else {
                    "Open a new tab in"
                },
                if stripped == "new-window" {
                    "window"
                } else {
                    "active window"
                }
            );
            return ParseOutcome::Exit(ExitCode::SUCCESS);
        }
        print_unknown_action_error(bin_name, stripped, style);
        return ParseOutcome::Exit(ExitCode::from(2));
    }

    print_root_help(bin_name, true, style);
    ParseOutcome::Exit(ExitCode::SUCCESS)
}

/// Splits `argv` into `(head_args, optional_command_vec)` at the first `-e`, `--command`, or `--`
/// while respecting preceding flags that consume a value (`-d`, `--title`, `--instance`, etc.).
fn split_command_tail(args: &[OsString]) -> Result<(Vec<OsString>, Option<Vec<String>>), String> {
    let mut head = Vec::with_capacity(args.len());
    let mut iter = args.iter().enumerate();

    if let Some((_, arg0)) = iter.next() {
        head.push(arg0.clone());
    }

    let mut expects_value = false;
    for (idx, tok) in iter {
        if expects_value {
            head.push(tok.clone());
            expects_value = false;
            continue;
        }

        let lossy = tok.to_string_lossy();
        if lossy == "--" {
            let rest: Vec<String> = args[idx + 1..]
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            let cmd = (!rest.is_empty()).then_some(rest);
            return Ok((head, cmd));
        }

        if lossy == "-e" || lossy == "--command" {
            let rest: Vec<String> = args[idx + 1..]
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            if rest.is_empty() {
                return Err(format!(
                    "a command is required after '{lossy}' (for example: `{lossy} btop`)"
                ));
            }
            return Ok((head, Some(rest)));
        }

        if let Some(inline) = lossy.strip_prefix("--command=") {
            let mut cmd = Vec::with_capacity(args.len() - idx);
            if !inline.is_empty() {
                cmd.push(inline.to_string());
            }
            cmd.extend(
                args[idx + 1..]
                    .iter()
                    .map(|s| s.to_string_lossy().into_owned()),
            );
            if cmd.is_empty() {
                return Err("a command is required for '--command=<CMD>'".to_string());
            }
            return Ok((head, Some(cmd)));
        }

        if lossy.starts_with("-e") && lossy.len() > 2 && !lossy.starts_with("--") {
            let mut cmd = Vec::with_capacity(args.len() - idx);
            cmd.push(lossy[2..].to_string());
            cmd.extend(
                args[idx + 1..]
                    .iter()
                    .map(|s| s.to_string_lossy().into_owned()),
            );
            return Ok((head, Some(cmd)));
        }

        if matches!(
            lossy.as_ref(),
            "-d" | "--working-directory" | "--title" | "--instance" | "--config-dir"
        ) {
            expects_value = true;
        }

        head.push(tok.clone());
    }

    Ok((head, None))
}

/// Finds the index and string of the first positional `+action` token in `head[1..]`.
fn find_first_plus_token(head: &[OsString]) -> Option<(usize, String)> {
    let mut expects_value = false;
    for (idx, tok) in head.iter().enumerate().skip(1) {
        if expects_value {
            expects_value = false;
            continue;
        }
        let s = tok.to_string_lossy();
        if matches!(
            s.as_ref(),
            "-d" | "--working-directory" | "--title" | "--instance" | "--config-dir"
        ) {
            expects_value = true;
            continue;
        }
        if s.starts_with('-') {
            continue;
        }
        if s.starts_with('+') && s.len() > 1 {
            return Some((idx, s.into_owned()));
        }
        // First positional argument is a regular directory path, not a `+action`.
        return None;
    }
    None
}

fn print_unknown_action_error(bin_name: &str, unknown: &str, style: TermStyle) {
    eprintln!(
        "{} unrecognized action '+{unknown}'",
        style.red("error:")
    );

    if let Some(suggestion) = suggest_closest_action(unknown) {
        eprintln!(
            "  {} a similar action exists: '{}'",
            style.cyan("tip:"),
            style.bold(suggestion)
        );
    }
    eprintln!(
        "  {} to open a directory whose name starts with '+', pass './+{unknown}' or '-d +{unknown}'",
        style.dim("note:")
    );
    eprintln!("Run '{bin_name} -h' to see available +actions.");
}

fn suggest_closest_action(input: &str) -> Option<&'static str> {
    let normalized = input.to_ascii_lowercase();
    PUBLIC_PLUS_ACTIONS
        .iter()
        .map(|(name, _)| (*name, edit_distance(&normalized, &name[1..])))
        .filter(|(_, dist)| *dist <= 3)
        .min_by_key(|(_, dist)| *dist)
        .map(|(name, _)| name)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0usize; b_chars.len() + 1];

    for (i, ca) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b_chars.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_chars.len()]
}

fn print_root_help(bin: &str, extended: bool, style: TermStyle) {
    println!(
        "{} {bin} [+action] [options] [dir] [-e <command> [args...]]\n",
        style.bold("Usage:")
    );
    println!("Run the Boxxy terminal emulator or execute a headless `+action` helper.\n");
    println!(
        "If no `+action` is specified, launches Boxxy Terminal (or opens a window/tab in\n\
         the running instance). Pass `-e <command> [args...]` to run a specific program\n\
         inside the terminal instead of your default login shell.\n"
    );

    println!("{}", style.bold("Launch Options:"));
    let launch_opts = [
        ("--new-window", "Open a new Boxxy window"),
        ("-t, --tab", "Open a new tab in the active window"),
        (
            "--present",
            "Raise the active window if open, or launch a new one",
        ),
        (
            "-d, --working-directory <DIR>",
            "Set the initial working directory (or pass [dir] positionally)",
        ),
        (
            "-e, --command <CMD> [ARGS...]",
            "Execute a command inside the terminal (also supports `-- <CMD>`)",
        ),
        (
            "--hold",
            "Keep the pane open after `-e` finishes so output stays visible",
        ),
        ("--title <TITLE>", "Set an initial custom title for the tab"),
        (
            "-h, --help",
            "Show help (`-h` concise summary, `--help` includes dev & env options)",
        ),
        ("-V, --version", "Print version and build metadata"),
    ];
    for (flag, desc) in launch_opts {
        println!("  {:<31} {}", style.cyan(flag), desc);
    }

    println!("\n{}", style.bold("Available Actions:"));
    for (name, desc) in PUBLIC_PLUS_ACTIONS {
        println!("  {:<18} {}", style.green(name), desc);
    }

    if extended {
        println!("\n{}", style.bold("Development & Isolation Options:"));
        let dev_opts = [
            (
                "--dev",
                "Run an isolated `.Devel` instance using `~/.config/boxxy-terminal-dev/`",
            ),
            (
                "--instance <SLUG>",
                "Run a named isolated instance (`~/.config/boxxy-terminal-<slug>/`)",
            ),
            (
                "--ephemeral",
                "Run with clean factory-default settings in `$XDG_RUNTIME_DIR`",
            ),
            (
                "--sync-settings",
                "Copy non-DB settings/themes/characters/skills from Stable into dev",
            ),
            (
                "--config-dir <ABS_PATH>",
                "Override the active configuration and SQLite root directory",
            ),
            (
                "--debug-context",
                "Log full LLM system prompts, history, and tool payloads",
            ),
        ];
        for (flag, desc) in dev_opts {
            println!("  {:<31} {}", style.yellow(flag), desc);
        }

        println!("\n{}", style.bold("Environment Variables:"));
        println!(
            "  Run `{}` to inspect all `BOXXY_*` and `*_API_KEY` variables and see\n  \
             which ones are currently set in your shell.",
            style.bold(&format!("{bin} +env"))
        );
    } else {
        println!(
            "\nRun `{bin} +<action> --help` for action-specific flags, `{bin} +env` for\n\
             environment variables, or `{bin} --help` for isolated `--dev` options."
        );
    }

    if extended {
        println!(
            "\nSpecify `+<action> --help` (for example `{bin} +agent --help` or `{bin} +doctor --help`)\n\
             to see options for a specific action."
        );
    }
}
