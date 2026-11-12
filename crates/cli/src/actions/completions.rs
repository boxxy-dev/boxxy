use crate::actions::ActionContext;
use anyhow::Result;
use clap::{Args, ValueEnum};
use clap_complete::{Shell, generate};
use std::io::Write;
use std::process::ExitCode;

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    #[value(alias = "nu")]
    Nushell,
}

#[derive(Args, Debug, Clone)]
#[command(about = "Generate shell completion scripts (bash, zsh, fish, nushell)")]
pub struct CompletionsArgs {
    /// Target shell to generate completions for.
    #[arg(value_enum)]
    pub shell: CompletionShell,
}

pub const PUBLIC_PLUS_ACTIONS: &[(&str, &str)] = &[
    ("+version", "Print Boxxy Terminal version and build information"),
    (
        "+doctor",
        "Validate config, themes, characters, MCP servers, and daemon health",
    ),
    (
        "+env",
        "Inspect supported BOXXY_* and provider API-key environment variables",
    ),
    (
        "+agent",
        "Inspect or control the host-side boxxy-agent D-Bus daemon",
    ),
    (
        "+completions",
        "Generate shell completion scripts (bash, zsh, fish, nushell)",
    ),
    ("+new-window", "Open a new Boxxy Terminal window"),
    ("+new-tab", "Open a new tab in the active Boxxy window"),
    ("+help", "Show help for Boxxy Terminal or a specific +action"),
];

pub fn run(ctx: &ActionContext, args: &CompletionsArgs) -> Result<ExitCode> {
    let bin = &ctx.bin_name;
    let mut stdout = std::io::stdout();

    match args.shell {
        CompletionShell::Bash => {
            let mut cmd = crate::parser::build_completion_command(bin);
            generate(Shell::Bash, &mut cmd, bin, &mut stdout);
            let plus_words: String = PUBLIC_PLUS_ACTIONS
                .iter()
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(" ");
            writeln!(
                stdout,
                "\n# Complete +action names when typing `{bin} +<TAB>`\n\
                 _boxxy_plus_wrap() {{\n\
                     local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n\
                     if [[ $COMP_CWORD -eq 1 && \"$cur\" == +* ]]; then\n\
                         COMPREPLY=( $(compgen -W \"{plus_words}\" -- \"$cur\") )\n\
                         return 0\n\
                     fi\n\
                     _{fn_name} \"$@\"\n\
                 }}\n\
                 complete -F _boxxy_plus_wrap -o bashdefault -o default {bin}",
                fn_name = bin.replace('-', "_")
            )?;
        }
        CompletionShell::Zsh => {
            let mut cmd = crate::parser::build_completion_command(bin);
            generate(Shell::Zsh, &mut cmd, bin, &mut stdout);
        }
        CompletionShell::Fish => {
            let mut cmd = crate::parser::build_completion_command(bin);
            generate(Shell::Fish, &mut cmd, bin, &mut stdout);
            writeln!(stdout, "\n# Boxxy +action completions")?;
            for (name, desc) in PUBLIC_PLUS_ACTIONS {
                writeln!(
                    stdout,
                    "complete -c {bin} -n '__fish_is_first_arg' -a '{name}' -d '{desc}'"
                )?;
            }
        }
        CompletionShell::Nushell => {
            write_nushell_completions(&mut stdout, bin)?;
        }
    }

    Ok(ExitCode::SUCCESS)
}

fn write_nushell_completions(out: &mut dyn Write, bin: &str) -> Result<()> {
    writeln!(out, "# Nushell completions for {bin}")?;
    writeln!(out, "def \"nu-complete {bin} actions\" [] {{")?;
    writeln!(out, "    [")?;
    for (name, desc) in PUBLIC_PLUS_ACTIONS {
        writeln!(
            out,
            "        {{ value: \"{name}\", description: \"{desc}\" }}"
        )?;
    }
    writeln!(out, "    ]")?;
    writeln!(out, "}}\n")?;
    writeln!(
        out,
        "export extern \"{bin}\" [\n\
             action?: string@\"nu-complete {bin} actions\"  # Optional +action or working directory\n\
             --new-window                                  # Open a new window\n\
             --tab (-t)                                    # Open a new tab in the active window\n\
             --present                                     # Raise active window or launch one\n\
             --working-directory (-d): directory           # Set the terminal's working directory\n\
             --command (-e): string                        # Execute command inside the terminal\n\
             --hold                                        # Keep pane open after -e command exits\n\
             --title: string                               # Set initial custom tab title\n\
             --dev                                         # Run isolated development instance\n\
             --instance: string                            # Run named isolated instance\n\
             --ephemeral                                   # Use clean factory-default runtime dir\n\
             --sync-settings                               # Refresh dev settings from Stable\n\
             --config-dir: directory                       # Override active config directory\n\
             --debug-context                               # Log full LLM context payloads\n\
             --help (-h)                                   # Print help\n\
             --version (-V)                                # Print version\n\
             ...args: any\n\
         ]"
    )?;
    Ok(())
}
