use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::ops::AddAssign;
use std::path::PathBuf;
use std::process::Command;

use ai_monitor::codex::{
    self, LimitWindow, Profile, ProfileStore, ResetCredit, ResetCredits, Snapshot,
    detailed_credits, expiring, fetch,
};
use ai_monitor::model::{Usage, UsageReport};
use ai_monitor::opencode::{OpenCodeProvider, discover_db_path};
use ai_monitor::update;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, Utc};
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
    #[arg(long, value_enum, default_value_t, global = true)]
    color: ColorMode,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum OutputFormat {
    #[default]
    Terminal,
    Json,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
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
    /// Check for and install the latest GitHub Release.
    Update {
        /// Only check the latest release without replacing the binary.
        #[arg(long)]
        check: bool,
        /// Replace the current binary without asking for confirmation.
        #[arg(long)]
        yes: bool,
        /// Reinstall even when the current version is already current.
        #[arg(long)]
        force: bool,
    },
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
        #[arg(long)]
        allow_private_api: bool,
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
    All {
        #[arg(long)]
        allow_private_api: bool,
    },
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
    authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    credit_error: Option<String>,
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
        } => run_overview(cli.format, cli.color, days, all_projects, project, db),
        Commands::Codex { command } => run_codex(cli.format, cli.color, command),
        Commands::Opencode { command } => run_opencode(cli.format, command),
        Commands::Doctor => run_doctor(cli.format),
        Commands::Update { check, yes, force } => run_update(cli.format, check, yes, force),
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

fn run_update(format: OutputFormat, check_only: bool, yes: bool, force: bool) -> Result<()> {
    let info = update::check()?;
    let available = force || info.latest > info.current;
    if matches!(format, OutputFormat::Json) {
        if !check_only && available && !yes {
            bail!("JSON updates require --yes");
        }
        if check_only || !available {
            return output_value(
                format,
                &json!({
                    "current": info.current.to_string(),
                    "latest": info.latest.to_string(),
                    "latest_tag": info.latest_tag,
                    "update_available": available,
                    "updated": false,
                }),
            );
        }
    } else if !available {
        println!("ai-monitor {} is already up to date.", info.current);
        return Ok(());
    } else if check_only {
        println!(
            "Update available: {} -> {} ({})",
            info.current, info.latest, info.latest_tag
        );
        return Ok(());
    } else if !yes {
        if !std::io::stdin().is_terminal() {
            bail!("non-interactive updates require --yes");
        }
        println!(
            "Update ai-monitor {} -> {}? [y/N]",
            info.current, info.latest
        );
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .context("could not read update confirmation")?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Update cancelled.");
            return Ok(());
        }
    }

    let installed = update::install_latest()?;
    if matches!(format, OutputFormat::Json) {
        output_value(
            format,
            &json!({
                "current": installed.current.to_string(),
                "latest": installed.latest.to_string(),
                "latest_tag": installed.latest_tag,
                "update_available": true,
                "updated": true,
            }),
        )
    } else {
        println!(
            "Updated ai-monitor {} -> {}. Restart the command to use the new binary.",
            installed.current, installed.latest
        );
        Ok(())
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

fn run_codex(format: OutputFormat, color: ColorMode, command: CodexCommands) -> Result<()> {
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
        CodexCommands::Usage {
            profile,
            allow_private_api,
        } => {
            if let Some(name) = profile.as_deref() {
                store.resolve(Some(name))?;
            }
            let results = fetch_all(&store, allow_private_api)?;
            output_codex_dashboard(
                format,
                &results,
                profile.as_deref(),
                color,
                allow_private_api,
            )?;
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
        CodexCommands::All { allow_private_api } => {
            let results = fetch_all(&store, allow_private_api)?;
            output_codex_dashboard(format, &results, None, color, allow_private_api)?;
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

fn fetch_all(store: &ProfileStore, allow_private_api: bool) -> Result<Vec<ProfileResult>> {
    Ok(store
        .list()?
        .into_iter()
        .map(|profile| fetch_profile(&profile, allow_private_api))
        .collect())
}

fn fetch_profile(profile: &Profile, allow_private_api: bool) -> ProfileResult {
    if !profile.authenticated {
        return ProfileResult {
            profile: profile.name.clone(),
            authenticated: false,
            snapshot: None,
            error: None,
            credit_error: None,
        };
    }

    match fetch(profile) {
        Ok(mut snapshot) => {
            let mut credit_error = None;
            if allow_private_api && snapshot.reset_credits.credits.is_none() {
                match detailed_credits(profile, &snapshot, true) {
                    Ok(credits) => snapshot.reset_credits = credits,
                    Err(error) => credit_error = Some(error.to_string()),
                }
            }
            ProfileResult {
                profile: profile.name.clone(),
                authenticated: true,
                snapshot: Some(snapshot),
                error: None,
                credit_error,
            }
        }
        Err(error) => ProfileResult {
            profile: profile.name.clone(),
            authenticated: true,
            snapshot: None,
            error: Some(error.to_string()),
            credit_error: None,
        },
    }
}

fn run_overview(
    format: OutputFormat,
    color: ColorMode,
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
        .and_then(|store| fetch_all(&store, false))
        .unwrap_or_else(|error| {
            vec![ProfileResult {
                profile: "codex".to_owned(),
                authenticated: false,
                snapshot: None,
                error: Some(error.to_string()),
                credit_error: None,
            }]
        });
    if matches!(format, OutputFormat::Json) {
        output_value(format, &json!({"opencode": report, "codex": codex}))
    } else {
        println!("CODEX");
        output_codex_dashboard(OutputFormat::Terminal, &codex, None, color, false)?;
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

struct Theme {
    enabled: bool,
}

impl Theme {
    fn new(mode: ColorMode) -> Self {
        let enabled = match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => {
                std::io::stdout().is_terminal()
                    && std::env::var_os("NO_COLOR").is_none()
                    && std::env::var("TERM").as_deref() != Ok("dumb")
            }
        };
        Self { enabled }
    }

    fn paint(&self, text: impl AsRef<str>, code: &str) -> String {
        let text = text.as_ref();
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }
}

fn ui_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(96)
        .saturating_sub(4)
        .clamp(76, 100)
}

fn visible_width(value: &str) -> usize {
    let mut width = 0;
    let mut escape = false;
    for character in value.chars() {
        if escape {
            if character == 'm' {
                escape = false;
            }
        } else if character == '\x1b' {
            escape = true;
        } else {
            width += 1;
        }
    }
    width
}

fn pad_visible(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(visible_width(value));
    format!("{value}{}", " ".repeat(padding))
}

fn styled_cell(theme: &Theme, value: &str, width: usize, code: &str, right: bool) -> String {
    let value = truncate(value, width);
    let padded = if right {
        format!("{value:>width$}")
    } else {
        format!("{value:<width$}")
    };
    theme.paint(padded, code)
}

const BOLD: &str = "1";
const DIM: &str = "2";
const RED: &str = "31";
const GREEN: &str = "32";
const YELLOW: &str = "33";
const CYAN: &str = "36";

fn output_codex_dashboard(
    format: OutputFormat,
    results: &[ProfileResult],
    selected: Option<&str>,
    color: ColorMode,
    allow_private_api: bool,
) -> Result<()> {
    if matches!(format, OutputFormat::Json) {
        return output_value(
            format,
            &json!({
                "account_count": results.len(),
                "selected_profile": selected,
                "accounts": results,
            }),
        );
    }

    let theme = Theme::new(color);
    let width = ui_width();
    let ready = results
        .iter()
        .filter(|result| result.snapshot.is_some())
        .count();
    println!();
    println!(
        "╭{}╮",
        pad_visible(&theme.paint("─ CODEX USAGE DASHBOARD ", BOLD), width)
    );
    println!(
        "│{}│",
        pad_visible(
            &theme.paint(
                format!(
                    " Accounts: {} total · {} ready · {}",
                    results.len(),
                    ready,
                    selected
                        .map(|name| format!("selected: {name}"))
                        .unwrap_or_else(|| "all accounts".to_owned())
                ),
                CYAN,
            ),
            width
        )
    );
    println!("╰{}╯", "─".repeat(width));
    println!();
    println!("{}", theme.paint("ACCOUNT OVERVIEW", BOLD));
    println!("  > PROFILE       STATUS       USAGE       RESET                 CREDITS");
    for result in results {
        render_account_summary(&theme, result, selected);
    }

    println!();
    println!("{}", theme.paint("ACCOUNT DETAILS", BOLD));
    if results.is_empty() {
        println!("  No Codex accounts found. Run `ai-monitor codex login NAME`.");
    }
    for result in results {
        render_account_card(&theme, result, selected, allow_private_api, width);
    }

    if !allow_private_api && results.iter().any(missing_credit_details) {
        println!();
        println!(
            "{}",
            theme.paint(
                "Note: some reset-credit expiry dates are unavailable. Use --allow-private-api only if you accept the private endpoint risk.",
                YELLOW,
            )
        );
    }
    Ok(())
}

fn render_account_summary(theme: &Theme, result: &ProfileResult, selected: Option<&str>) {
    let marker = if selected == Some(result.profile.as_str()) {
        ">"
    } else {
        " "
    };
    let name = truncate(&result.profile, 12);
    let Some(snapshot) = &result.snapshot else {
        let (status, status_code) = if result.authenticated {
            ("[ERROR]", RED)
        } else {
            ("[LOGIN]", YELLOW)
        };
        println!(
            "  {marker} {:<12} {} {} {} {}",
            name,
            styled_cell(theme, status, 12, status_code, false),
            styled_cell(theme, "-", 8, DIM, true),
            styled_cell(theme, "-", 20, DIM, false),
            styled_cell(theme, "not ready", 18, DIM, false)
        );
        return;
    };

    let (usage, usage_code) = primary_usage(snapshot);
    let usage_text = format!("{usage:.1}%");
    let reset = primary_limit(snapshot)
        .and_then(|limit| limit.resets_at)
        .map(format_reset_summary)
        .unwrap_or_else(|| "unknown".to_owned());
    let (credits, credits_code) = credit_count_parts(&snapshot.reset_credits);
    println!(
        "  {marker} {:<12} {} {} {} {}",
        name,
        styled_cell(theme, "[READY]", 12, GREEN, false),
        styled_cell(theme, &usage_text, 8, usage_code, true),
        styled_cell(theme, &reset, 20, DIM, false),
        styled_cell(theme, &credits, 18, credits_code, false)
    );
}

fn render_account_card(
    theme: &Theme,
    result: &ProfileResult,
    selected: Option<&str>,
    allow_private_api: bool,
    width: usize,
) {
    let selected_marker = if selected == Some(result.profile.as_str()) {
        " <selected>"
    } else {
        ""
    };
    println!();
    println!("  ┌{}┐", "─".repeat(width));
    card_line(
        theme,
        width,
        &theme.paint(format!(" {}{}", result.profile, selected_marker), CYAN),
    );

    let Some(snapshot) = &result.snapshot else {
        let message = if result.authenticated {
            result
                .error
                .as_deref()
                .unwrap_or("Codex app-server unavailable")
        } else {
            "Not logged in"
        };
        card_line(theme, width, &theme.paint(format!(" {message}"), YELLOW));
        println!("  └{}┘", "─".repeat(width));
        return;
    };

    let account = format!(
        "{} · {}",
        snapshot.account.email.as_deref().unwrap_or("unknown"),
        snapshot.account.plan_type.as_deref().unwrap_or("unknown")
    );
    card_line(
        theme,
        width,
        &format!(" Account: {}", truncate(&account, width.saturating_sub(10))),
    );
    card_line(theme, width, "");
    card_line(theme, width, &theme.paint(" RATE LIMITS", BOLD));
    if snapshot.limits.is_empty() {
        card_line(theme, width, "   No rate-limit window returned by Codex.");
    }
    for limit in &snapshot.limits {
        let (usage, usage_code) = limit_usage(limit);
        let bar = theme.paint(progress_bar(usage, 24), usage_code);
        let percentage = theme.paint(format!("{usage:>5.1}%"), usage_code);
        let reset = limit
            .resets_at
            .map(format_reset_time)
            .unwrap_or_else(|| "unknown".to_owned());
        card_line(
            theme,
            width,
            &format!(
                "   {:<9} {} {}  reset {}",
                limit_label(limit),
                bar,
                percentage,
                reset
            ),
        );
    }
    card_line(theme, width, "");
    card_line(theme, width, &theme.paint(" RESET CREDITS", BOLD));
    card_line(
        theme,
        width,
        &format!(
            "   Available: {}  Source: {}",
            format_credit_count(theme, &snapshot.reset_credits),
            snapshot.reset_credits.source
        ),
    );
    render_credit_details(
        theme,
        &snapshot.reset_credits,
        allow_private_api,
        result.credit_error.as_deref(),
        width,
    );
    if let Some(error) = &snapshot.usage_error {
        card_line(
            theme,
            width,
            &format!("   Usage activity: {}", theme.paint(error, DIM)),
        );
    }
    println!("  └{}┘", "─".repeat(width));
}

fn card_line(_theme: &Theme, width: usize, content: &str) {
    println!("  │{}│", pad_visible(content, width));
}

fn render_credit_details(
    theme: &Theme,
    credits: &ResetCredits,
    allow_private_api: bool,
    detail_error: Option<&str>,
    width: usize,
) {
    match credits.credits.as_deref() {
        Some(rows) => {
            if rows.is_empty() {
                if credits.available_count.unwrap_or(0) > 0 {
                    card_line(
                        theme,
                        width,
                        &theme.paint("   Expiry details unavailable from app-server", YELLOW),
                    );
                } else {
                    card_line(theme, width, "   No reset credits currently available.");
                }
            } else {
                for (index, credit) in rows.iter().enumerate() {
                    render_credit(theme, index + 1, credit, width);
                }
            }
        }
        None => {
            let hint = if let Some(error) = detail_error {
                error
            } else if allow_private_api {
                "Private credit detail lookup failed or returned no rows"
            } else {
                "Expiry details require --allow-private-api"
            };
            card_line(theme, width, &theme.paint(format!("   {hint}"), YELLOW));
        }
    }
}

fn render_credit(theme: &Theme, index: usize, credit: &ResetCredit, width: usize) {
    let status = credit.status.as_deref().unwrap_or("available");
    let status_code = if matches!(status, "active" | "available") {
        GREEN
    } else {
        DIM
    };
    let title = truncate(credit.title.as_deref().unwrap_or("Reset credit"), 24);
    let expires = credit
        .expires_at
        .map(|time| {
            let label = format!(
                "{} ({})",
                time.with_timezone(&Local).format("%Y-%m-%d %H:%M"),
                relative_time(time)
            );
            theme.paint(label, expiry_color(time))
        })
        .unwrap_or_else(|| "no expiry".to_owned());
    card_line(
        theme,
        width,
        &format!(
            "   #{index:<2} {}  {:<24} expires {}",
            theme.paint(status, status_code),
            title,
            expires
        ),
    );
}

fn missing_credit_details(result: &ProfileResult) -> bool {
    result
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.reset_credits.credits.is_none())
}

fn primary_limit(snapshot: &Snapshot) -> Option<&LimitWindow> {
    snapshot
        .limits
        .iter()
        .find(|limit| limit.name == "primary")
        .or_else(|| snapshot.limits.first())
}

fn primary_usage(snapshot: &Snapshot) -> (f64, &'static str) {
    primary_limit(snapshot)
        .map(limit_usage)
        .unwrap_or((0.0, DIM))
}

fn limit_usage(limit: &LimitWindow) -> (f64, &'static str) {
    let usage = limit.used_percent.clamp(0.0, 100.0);
    let color = if usage >= 90.0 {
        RED
    } else if usage >= 75.0 {
        YELLOW
    } else {
        GREEN
    };
    (usage, color)
}

fn format_credit_count(theme: &Theme, credits: &ResetCredits) -> String {
    let (text, color) = credit_count_parts(credits);
    theme.paint(text, color)
}

fn credit_count_parts(credits: &ResetCredits) -> (String, &'static str) {
    match credits.available_count {
        Some(0) => ("0 available".to_owned(), DIM),
        Some(count) => (format!("{count} available"), GREEN),
        None => ("unknown".to_owned(), YELLOW),
    }
}

fn progress_bar(value: f64, width: usize) -> String {
    let filled = ((value.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn format_reset_time(time: DateTime<Utc>) -> String {
    format!(
        "{} ({})",
        time.with_timezone(&Local).format("%Y-%m-%d %H:%M"),
        relative_time(time)
    )
}

fn format_reset_summary(time: DateTime<Utc>) -> String {
    format!(
        "{} ({})",
        time.with_timezone(&Local).format("%m-%d %H:%M"),
        short_relative_time(time)
    )
}

fn relative_time(time: DateTime<Utc>) -> String {
    let seconds = (time - Utc::now()).num_seconds();
    if seconds <= 0 {
        return "now".to_owned();
    }
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("in {days}d {hours}h")
    } else if hours > 0 {
        format!("in {hours}h {minutes}m")
    } else {
        format!("in {minutes}m")
    }
}

fn short_relative_time(time: DateTime<Utc>) -> String {
    let seconds = (time - Utc::now()).num_seconds();
    if seconds <= 0 {
        return "now".to_owned();
    }
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    if days > 0 {
        format!("{days}d {hours}h")
    } else {
        format!("{}h", hours.max(1))
    }
}

fn expiry_color(time: DateTime<Utc>) -> &'static str {
    let seconds = (time - Utc::now()).num_seconds();
    if seconds <= 172_800 {
        RED
    } else if seconds <= 604_800 {
        YELLOW
    } else {
        GREEN
    }
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
