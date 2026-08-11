use std::collections::BTreeMap;
use std::ffi::OsString;
use std::ops::AddAssign;
use std::path::PathBuf;
use std::process::Command;

use ai_monitor::codex::{
    self, ProfileStore, ResetCredits, Snapshot, detailed_credits, expiring, fetch,
};
use ai_monitor::model::{Usage, UsageReport};
use ai_monitor::opencode::{OpenCodeProvider, discover_db_path};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::{Shell, generate};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Parser)]
#[command(
    name = "ai-monitor",
    version,
    about = "Unified local usage monitor for AI coding tools"
)]
struct Cli {
    #[arg(long, value_enum, default_value_t, global = true)]
    format: OutputFormat,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum OutputFormat {
    #[default]
    Terminal,
    Json,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Show Codex limits and OpenCode usage together.
    Overview {
        #[arg(short, long, default_value_t = 7)]
        days: u32,
        #[arg(long)]
        all_projects: bool,
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Inspect Codex accounts and subscription limits.
    Codex {
        #[command(subcommand)]
        command: CodexCommands,
    },
    /// Analyze local OpenCode usage.
    Opencode {
        #[command(subcommand)]
        command: OpenCodeCommands,
    },
    /// Check local provider dependencies and storage.
    Doctor,
    /// Generate shell completion scripts.
    Completion { shell: Shell },
}

#[derive(Debug, Subcommand)]
enum CodexCommands {
    /// List isolated Codex profiles.
    Profiles,
    /// Set the default profile.
    Default { name: String },
    /// Log in through the official Codex CLI.
    Login {
        name: String,
        #[arg(long)]
        force: bool,
    },
    /// Show one profile's account and rate limits.
    Usage {
        #[arg(long)]
        profile: Option<String>,
    },
    /// Show reset credits. Private fallback requires explicit opt-in.
    Credits {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        allow_private_api: bool,
    },
    /// Find reset credits expiring soon.
    Expiring {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, default_value_t = 7)]
        days: u32,
        #[arg(long)]
        allow_private_api: bool,
    },
    /// Show all profiles and limits.
    All,
    /// Run Codex with an isolated profile.
    Run {
        #[arg(long)]
        profile: Option<String>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Clear credentials through the official Codex CLI.
    Logout {
        #[arg(long)]
        profile: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum OpenCodeCommands {
    /// Summarize usage by day, provider, and model.
    Usage {
        #[arg(short, long, default_value_t = 7)]
        days: u32,
        #[arg(long)]
        all_projects: bool,
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        include_cache: bool,
        #[arg(long, default_value_t = 10)]
        top_models: usize,
    },
    /// Manage the optional all-project time index.
    Optimize {
        #[command(subcommand)]
        action: OptimizeAction,
        #[arg(long)]
        db: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum OptimizeAction {
    /// Check whether the optional index exists.
    Status,
    /// Create an index in the OpenCode database.
    Create {
        /// Confirm modification of the third-party database.
        #[arg(long)]
        yes: bool,
    },
    /// Remove ai-monitor's index from the OpenCode database.
    Remove {
        /// Confirm modification of the third-party database.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Serialize)]
struct ProfileResult {
    profile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse_from(compatibility_args());
    match cli.command {
        Commands::Overview {
            days,
            all_projects,
            project,
            db,
        } => run_overview(cli.format, days, all_projects, project, db),
        Commands::Codex { command } => run_codex(cli.format, command),
        Commands::Opencode { command } => run_opencode(cli.format, command),
        Commands::Doctor => run_doctor(cli.format),
        Commands::Completion { shell } => {
            generate(
                shell,
                &mut Cli::command(),
                "ai-monitor",
                &mut std::io::stdout(),
            );
            Ok(())
        }
    }
}

fn compatibility_args() -> Vec<OsString> {
    let mut args = std::env::args_os().collect::<Vec<_>>();
    let executable = args
        .first()
        .and_then(|path| std::path::Path::new(path).file_stem())
        .and_then(|name| name.to_str());
    match executable {
        Some("chatgpt-status") => args.insert(1, "codex".into()),
        Some("opencode-daily-usage") => {
            args.insert(1, "opencode".into());
            args.insert(2, "usage".into());
        }
        _ => {}
    }
    args
}

fn provider(db: Option<PathBuf>) -> OpenCodeProvider {
    db.map(OpenCodeProvider::with_db_path).unwrap_or_default()
}

fn run_opencode(format: OutputFormat, command: OpenCodeCommands) -> Result<()> {
    match command {
        OpenCodeCommands::Usage {
            days,
            all_projects,
            project,
            db,
            include_cache,
            top_models,
        } => {
            let provider = provider(db);
            let report = provider
                .usage(days, all_projects, project.as_deref())
                .context("failed to read OpenCode usage")?;
            if all_projects && !provider.index_status().unwrap_or(false) {
                eprintln!(
                    "warning: all-project queries may scan the entire database; run `ai-monitor opencode optimize create --yes` to add the optional time index"
                );
            }
            output_usage(format, &report, include_cache, top_models)
        }
        OpenCodeCommands::Optimize { action, db } => {
            let provider = provider(db);
            match action {
                OptimizeAction::Status => {
                    let exists = provider.index_status().context("failed to inspect index")?;
                    output_value(format, &json!({"installed": exists}))?;
                    if matches!(format, OutputFormat::Terminal) {
                        println!(
                            "OpenCode time index: {}",
                            if exists { "installed" } else { "not installed" }
                        );
                    }
                }
                OptimizeAction::Create { yes } => {
                    require_confirmation(yes)?;
                    provider.create_index().context("failed to create index")?;
                    if matches!(format, OutputFormat::Json) {
                        output_value(format, &json!({"installed": true, "changed": true}))?;
                    } else {
                        println!("OpenCode time index created.");
                    }
                }
                OptimizeAction::Remove { yes } => {
                    require_confirmation(yes)?;
                    provider.remove_index().context("failed to remove index")?;
                    if matches!(format, OutputFormat::Json) {
                        output_value(format, &json!({"installed": false, "changed": true}))?;
                    } else {
                        println!("OpenCode time index removed.");
                    }
                }
            }
            Ok(())
        }
    }
}

fn require_confirmation(yes: bool) -> Result<()> {
    if !yes {
        bail!("this modifies the OpenCode database; rerun with --yes after making a backup")
    }
    Ok(())
}

fn run_codex(format: OutputFormat, command: CodexCommands) -> Result<()> {
    let store = ProfileStore::from_env().context("failed to open Codex profile storage")?;
    match command {
        CodexCommands::Profiles => {
            let profiles = store.list()?;
            if matches!(format, OutputFormat::Json) {
                output_value(format, &profiles)?;
            } else {
                let default = store.default_name()?;
                println!("DEFAULT  PROFILE  AUTH");
                for profile in profiles {
                    println!(
                        "{:<7}  {:<7}  {}",
                        if default.as_deref() == Some(&profile.name) {
                            "*"
                        } else {
                            ""
                        },
                        profile.name,
                        if profile.authenticated {
                            "ready"
                        } else {
                            "not logged in"
                        }
                    );
                }
            }
        }
        CodexCommands::Default { name } => {
            store.set_default(&name)?;
            println!("Default Codex profile set to {name}.");
        }
        CodexCommands::Login { name, force } => {
            let status = codex::login(&store, &name, force)?;
            if !status.success() {
                bail!("Codex login exited with {status}");
            }
            if store.default_name()?.is_none() {
                store.set_default(&name)?;
            }
        }
        CodexCommands::Usage { profile } => {
            let profile = store.resolve(profile.as_deref())?;
            let snapshot = fetch(&profile)?;
            output_snapshot(format, &snapshot)?;
        }
        CodexCommands::Credits {
            profile,
            allow_private_api,
        } => {
            let profile = store.resolve(profile.as_deref())?;
            let snapshot = fetch(&profile)?;
            let credits = detailed_credits(&profile, &snapshot, allow_private_api)?;
            output_credits(format, &profile.name, &credits)?;
        }
        CodexCommands::Expiring {
            profile,
            days,
            allow_private_api,
        } => {
            let profiles = match profile {
                Some(name) => vec![store.resolve(Some(&name))?],
                None => store.list()?,
            };
            let mut results = Vec::new();
            let mut errors = Vec::new();
            for profile in profiles {
                match fetch(&profile) {
                    Ok(snapshot) => {
                        let credits = match detailed_credits(&profile, &snapshot, allow_private_api)
                        {
                            Ok(credits) => credits,
                            Err(error) => {
                                errors.push(format!("{}: {error}", profile.name));
                                continue;
                            }
                        };
                        if credits.credits.is_none() {
                            errors.push(format!(
                                "{}: reset-credit details unavailable{}",
                                profile.name,
                                if allow_private_api {
                                    ""
                                } else {
                                    "; retry with --allow-private-api only if you accept the private endpoint risk"
                                }
                            ));
                            continue;
                        }
                        for credit in expiring(&credits, days, Utc::now()) {
                            results.push(json!({"profile": profile.name, "credit": credit}));
                        }
                    }
                    Err(error) => errors.push(format!("{}: {error}", profile.name)),
                }
            }
            if matches!(format, OutputFormat::Json) {
                output_value(
                    format,
                    &json!({"days": days, "expiring": results, "errors": errors}),
                )?;
            } else {
                for item in &results {
                    println!(
                        "{}  {}",
                        item["profile"].as_str().unwrap_or("unknown"),
                        item["credit"]["expires_at"].as_str().unwrap_or("unknown")
                    );
                }
                if results.is_empty() && errors.is_empty() {
                    println!("No reset credits expire within {days} days.");
                }
                for error in errors {
                    eprintln!("warning: {error}");
                }
            }
        }
        CodexCommands::All => {
            let results = fetch_all(&store)?;
            if matches!(format, OutputFormat::Json) {
                output_value(format, &results)?;
            } else {
                for result in results {
                    if let Some(snapshot) = result.snapshot {
                        output_snapshot(format, &snapshot)?;
                    } else {
                        eprintln!("{}: {}", result.profile, result.error.unwrap_or_default());
                    }
                }
            }
        }
        CodexCommands::Run { profile, args } => {
            let profile = store.resolve(profile.as_deref())?;
            let status = codex::run(&profile, &args)?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
        }
        CodexCommands::Logout { profile } => {
            let profile = store.resolve(profile.as_deref())?;
            let status = codex::logout(&profile)?;
            if !status.success() {
                bail!("Codex logout exited with {status}");
            }
        }
    }
    Ok(())
}

fn fetch_all(store: &ProfileStore) -> Result<Vec<ProfileResult>> {
    Ok(store
        .list()?
        .into_iter()
        .map(|profile| match fetch(&profile) {
            Ok(snapshot) => ProfileResult {
                profile: profile.name,
                snapshot: Some(snapshot),
                error: None,
            },
            Err(error) => ProfileResult {
                profile: profile.name,
                snapshot: None,
                error: Some(error.to_string()),
            },
        })
        .collect())
}

fn run_overview(
    format: OutputFormat,
    days: u32,
    all_projects: bool,
    project: Option<PathBuf>,
    db: Option<PathBuf>,
) -> Result<()> {
    let report = provider(db)
        .usage(days, all_projects, project.as_deref())
        .context("failed to read OpenCode usage")?;
    let codex = ProfileStore::from_env()
        .map_err(anyhow::Error::from)
        .and_then(|store| fetch_all(&store))
        .unwrap_or_else(|error| {
            vec![ProfileResult {
                profile: "codex".to_owned(),
                snapshot: None,
                error: Some(error.to_string()),
            }]
        });
    if matches!(format, OutputFormat::Json) {
        output_value(format, &json!({"opencode": report, "codex": codex}))
    } else {
        println!("CODEX");
        for result in codex {
            if let Some(snapshot) = result.snapshot {
                output_snapshot(format, &snapshot)?;
            } else {
                eprintln!("{}: {}", result.profile, result.error.unwrap_or_default());
            }
        }
        println!("\nOPENCODE");
        output_usage(format, &report, false, 10)
    }
}

fn run_doctor(format: OutputFormat) -> Result<()> {
    let codex_version = command_version("codex");
    let opencode_version = command_version("opencode");
    let profile_home = ProfileStore::from_env().map(|store| store.root().to_path_buf());
    let database = discover_db_path(None);
    let value = json!({
        "codex": codex_version,
        "opencode": opencode_version,
        "profile_home": profile_home.ok(),
        "opencode_database": database.ok(),
    });
    if matches!(format, OutputFormat::Json) {
        output_value(format, &value)
    } else {
        println!(
            "Codex:   {}",
            value["codex"].as_str().unwrap_or("not found")
        );
        println!(
            "OpenCode: {}",
            value["opencode"].as_str().unwrap_or("not found")
        );
        println!(
            "Profiles: {}",
            value["profile_home"].as_str().unwrap_or("unavailable")
        );
        println!(
            "Database: {}",
            value["opencode_database"].as_str().unwrap_or("unavailable")
        );
        Ok(())
    }
}

fn command_version(binary: &str) -> Option<String> {
    let output = Command::new(binary).arg("--version").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn output_value(format: OutputFormat, value: &impl Serialize) -> Result<()> {
    if matches!(format, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}

fn output_usage(
    format: OutputFormat,
    report: &UsageReport,
    include_cache: bool,
    top_models: usize,
) -> Result<()> {
    if matches!(format, OutputFormat::Json) {
        return output_value(format, report);
    }
    println!("Range: {} to {}", report.start_day, report.end_day);
    println!("Scope: {}", report.scope);
    let mut days = BTreeMap::<String, Usage>::new();
    let mut models = BTreeMap::<String, Usage>::new();
    for row in &report.rows {
        days.entry(row.day.clone())
            .or_default()
            .add_assign(&row.usage);
        models
            .entry(format!("{}/{}", row.provider, row.model))
            .or_default()
            .add_assign(&row.usage);
    }
    println!("\nDAILY USAGE");
    println!("DATE        TOKENS       MSGS       COST");
    for (day, usage) in days {
        let tokens = if include_cache {
            usage.all_tokens()
        } else {
            usage.active_tokens()
        };
        println!(
            "{day}  {:>10}  {:>9}  ${:>9.4}",
            compact(tokens),
            usage.messages,
            usage.cost_usd
        );
    }
    let mut models = models.into_iter().collect::<Vec<_>>();
    models.sort_by_key(|(_, usage)| {
        std::cmp::Reverse(if include_cache {
            usage.all_tokens()
        } else {
            usage.active_tokens()
        })
    });
    if top_models > 0 {
        models.truncate(top_models);
    }
    println!("\nMODEL RANKING");
    println!("MODEL                                      TOKENS       MSGS       COST");
    for (model, usage) in models {
        let tokens = if include_cache {
            usage.all_tokens()
        } else {
            usage.active_tokens()
        };
        println!(
            "{:<40}  {:>10}  {:>9}  ${:>9.4}",
            truncate(&model, 40),
            compact(tokens),
            usage.messages,
            usage.cost_usd
        );
    }
    Ok(())
}

fn output_snapshot(format: OutputFormat, snapshot: &Snapshot) -> Result<()> {
    if matches!(format, OutputFormat::Json) {
        return output_value(format, snapshot);
    }
    println!(
        "{}  {}  {}",
        snapshot.profile,
        snapshot.account.email.as_deref().unwrap_or("unknown"),
        snapshot.account.plan_type.as_deref().unwrap_or("unknown")
    );
    for limit in &snapshot.limits {
        println!(
            "  {:<9} {:>6.1}%  resets {}",
            limit_label(limit),
            limit.used_percent,
            limit
                .resets_at
                .map(|time| time
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        );
    }
    println!(
        "  reset credits: {}",
        snapshot
            .reset_credits
            .available_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "unknown".to_owned())
    );
    Ok(())
}

fn limit_label(limit: &ai_monitor::codex::LimitWindow) -> String {
    match limit.window_minutes {
        Some(300) => "5 hour".to_owned(),
        Some(10_080) => "Weekly".to_owned(),
        Some(minutes) if minutes % 1_440 == 0 => format!("{} day", minutes / 1_440),
        Some(minutes) if minutes % 60 == 0 => format!("{} hour", minutes / 60),
        Some(minutes) => format!("{minutes} min"),
        None => limit.name.clone(),
    }
}

fn output_credits(format: OutputFormat, profile: &str, credits: &ResetCredits) -> Result<()> {
    if matches!(format, OutputFormat::Json) {
        return output_value(format, credits);
    }
    println!(
        "{profile}: {} reset credits ({})",
        credits
            .available_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "unknown".to_owned()),
        credits.source
    );
    for credit in credits.credits.as_deref().unwrap_or_default() {
        println!(
            "  {}  {}  {}",
            credit.status.as_deref().unwrap_or("unknown"),
            credit.title.as_deref().unwrap_or("Reset credit"),
            credit
                .expires_at
                .map(|time| time
                    .with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        );
    }
    Ok(())
}

fn compact(value: u64) -> String {
    if value >= 1_000_000_000 {
        format!("{:.2}B", value as f64 / 1_000_000_000.0)
    } else if value >= 1_000_000 {
        format!("{:.2}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}
