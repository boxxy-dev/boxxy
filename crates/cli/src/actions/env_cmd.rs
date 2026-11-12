use crate::actions::ActionContext;
use crate::output::{OutputFormat, print_json};
use anyhow::Result;
use boxxy_sys_utils::env::{ENV_SPECS, EnvCategory, mask_secret};
use clap::Args;
use serde::Serialize;
use std::process::ExitCode;

#[derive(Args, Debug, Clone, Default)]
#[command(about = "Inspect supported BOXXY_* and provider API-key environment variables")]
pub struct EnvArgs {
    /// Also show internal child-PTY plumbing markers.
    #[arg(long)]
    pub internal: bool,

    /// Emit environment variable inspection report as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Serialize)]
struct EnvEntryReport {
    name: String,
    category: EnvCategory,
    cli_flag: Option<String>,
    value_hint: String,
    summary: String,
    is_set: bool,
    display_value: Option<String>,
}

pub fn run(ctx: &ActionContext, args: &EnvArgs) -> Result<ExitCode> {
    ctx.apply_dev_overrides();

    let mut entries: Vec<EnvEntryReport> = ENV_SPECS
        .iter()
        .filter(|spec| args.internal || spec.category != EnvCategory::Internal)
        .map(|spec| {
            let raw = std::env::var(spec.name).ok().filter(|v| !v.is_empty());
            let display_value = raw.as_ref().map(|v| {
                if spec.secret {
                    mask_secret(v)
                } else {
                    v.clone()
                }
            });
            EnvEntryReport {
                name: spec.name.to_string(),
                category: spec.category,
                cli_flag: spec.cli_flag.map(str::to_string),
                value_hint: spec.value_hint.to_string(),
                summary: spec.summary.to_string(),
                is_set: raw.is_some(),
                display_value,
            }
        })
        .collect();

    // Include custom `api_key_env` names configured on user OpenAI-compatible endpoints.
    let settings_path = boxxy_sys_utils::get_config_dir().join("settings.json");
    if let Ok(raw_json) = std::fs::read_to_string(&settings_path)
        && let Ok(settings) = serde_json::from_str::<boxxy_preferences::Settings>(&raw_json)
    {
        for ep in &settings.openai_compatible_endpoints {
            if let Some(var_name) = ep
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                && !entries.iter().any(|e| e.name == var_name)
            {
                let raw = std::env::var(var_name).ok().filter(|v| !v.is_empty());
                entries.push(EnvEntryReport {
                    name: var_name.to_string(),
                    category: EnvCategory::ProviderKey,
                    cli_flag: None,
                    value_hint: "<KEY>".to_string(),
                    summary: format!("API key override for custom endpoint '{}'.", ep.name),
                    is_set: raw.is_some(),
                    display_value: raw.as_deref().map(mask_secret),
                });
            }
        }
    }

    match ctx.format(args.json) {
        OutputFormat::Json => print_json(&entries)?,
        OutputFormat::Human => {
            let categories = [
                EnvCategory::Isolation,
                EnvCategory::Shell,
                EnvCategory::Diagnostics,
                EnvCategory::ProviderKey,
                EnvCategory::Internal,
            ];

            for (idx, cat) in categories.into_iter().enumerate() {
                if !args.internal && cat == EnvCategory::Internal {
                    continue;
                }
                let group: Vec<&EnvEntryReport> =
                    entries.iter().filter(|e| e.category == cat).collect();
                if group.is_empty() {
                    continue;
                }
                if idx > 0 {
                    println!();
                }
                println!("{}", ctx.style.bold(cat.title()));

                for item in group {
                    let status = match &item.display_value {
                        Some(val) => ctx.style.green(&format!("[set: {val}]")),
                        None => ctx.style.dim("(unset)"),
                    };
                    let flag_note = item
                        .cli_flag
                        .as_deref()
                        .map(|f| format!("  {}", ctx.style.cyan(&format!("({f})"))))
                        .unwrap_or_default();

                    println!(
                        "  {}{}  {}",
                        ctx.style.bold(&format!("{}={}", item.name, item.value_hint)),
                        flag_note,
                        status
                    );
                    println!("      {}", ctx.style.dim(&item.summary));
                }
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}
