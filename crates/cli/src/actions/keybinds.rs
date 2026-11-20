use crate::actions::ActionContext;
use crate::output::{OutputFormat, print_json};
use anyhow::Result;
use boxxy_keybindings::{
    ChordStatus, CompiledKeymap, OutgoingOverride, ShortcutCategory, ShortcutId, spec_for, store,
};
use clap::Args;
use serde::Serialize;
use std::collections::BTreeMap;
use std::process::ExitCode;

#[derive(Args, Debug, Clone, Default)]
#[command(about = "Inspect active, default, or customized keyboard shortcuts")]
pub struct KeybindsArgs {
    /// Print built-in factory default shortcuts, ignoring keybindings.toml.
    #[arg(long, conflicts_with = "custom")]
    pub default: bool,

    /// Print only user-customized shortcuts from keybindings.toml.
    #[arg(long)]
    pub custom: bool,

    /// Emit machine-readable JSON output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Serialize)]
struct KeybindJsonRow {
    id: &'static str,
    category: &'static str,
    title: &'static str,
    action: &'static str,
    shortcuts: Vec<String>,
    customized: bool,
    shadowed: Vec<String>,
    overrides: Vec<String>,
}

#[derive(Serialize)]
struct KeybindsJsonReport {
    config_path: std::path::PathBuf,
    bindings: Vec<KeybindJsonRow>,
    warnings: Vec<String>,
}

pub fn run(ctx: &ActionContext, args: &KeybindsArgs) -> Result<ExitCode> {
    ctx.apply_dev_overrides();
    let config_path = boxxy_keybindings::get_keybindings_path();

    let km = if args.default {
        CompiledKeymap::from_overrides(BTreeMap::new(), Vec::new(), None)
    } else {
        let loaded = store::load_overrides_from_path(&config_path);
        CompiledKeymap::from_overrides(loaded.overrides, loaded.warnings, loaded.stamp)
    };

    let mut rows = Vec::new();
    for cat in ShortcutCategory::ALL {
        for id in ShortcutId::ALL {
            let spec = spec_for(id);
            if spec.category != cat {
                continue;
            }
            let customized = km.is_customized(id);
            if args.custom && !customized {
                continue;
            }
            let diag = km.diagnostics_for(id);
            let shortcuts: Vec<String> = km.chords_for(id).iter().map(ToString::to_string).collect();
            let shadowed: Vec<String> = diag
                .chord_statuses
                .iter()
                .filter_map(|(c, st)| match st {
                    ChordStatus::Active => None,
                    ChordStatus::ShadowedBy(winner) => {
                        Some(format!("{c} (shadowed by {})", spec_for(*winner).title))
                    }
                })
                .collect();
            let overrides: Vec<String> = diag
                .outgoing_overrides
                .iter()
                .map(|ov| match ov {
                    OutgoingOverride::Action {
                        chord,
                        shadowed_action,
                        ..
                    } => format!("{chord} -> {}", spec_for(*shadowed_action).title),
                    OutgoingOverride::TerminalBuiltin {
                        chord,
                        builtin_label,
                        ..
                    } => format!("{chord} -> {builtin_label}"),
                })
                .collect();

            rows.push(KeybindJsonRow {
                id: id.as_str(),
                category: cat.title(),
                title: spec.title,
                action: spec.action_name,
                shortcuts,
                customized,
                shadowed,
                overrides,
            });
        }
    }

    let warnings: Vec<String> = km
        .warnings()
        .iter()
        .map(|w| format!("{}: {}", w.key, w.message))
        .collect();

    match ctx.format(args.json) {
        OutputFormat::Json => {
            print_json(&KeybindsJsonReport {
                config_path,
                bindings: rows,
                warnings,
            })?;
        }
        OutputFormat::Human => {
            let s = ctx.style;
            println!("{} {}", s.dim("File:"), config_path.display());
            if rows.is_empty() {
                println!("{}", s.dim("No customized keybindings configured."));
            } else {
                let mut current_cat = "";
                for row in &rows {
                    if row.category != current_cat {
                        current_cat = row.category;
                        println!("\n{}", s.bold(current_cat));
                    }
                    let chords_str = if row.shortcuts.is_empty() {
                        s.dim("disabled")
                    } else {
                        s.cyan(&row.shortcuts.join(", "))
                    };
                    let custom_tag = if row.customized {
                        format!(" {}", s.yellow("[custom]"))
                    } else {
                        String::new()
                    };
                    println!("  {:<34} {}{custom_tag}", row.title, chords_str);
                    for ov in &row.overrides {
                        println!("    {} overrides {ov}", s.yellow("⚡"));
                    }
                }
            }
            for w in &warnings {
                eprintln!("{} {w}", s.yellow("warning:"));
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}
