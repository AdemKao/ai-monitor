fn run_with_codex_dashboard() -> Result<()> {
    let cli = Cli::parse_from(compatibility_args());
    match cli.command {
        Commands::Overview {
            days,
            range,
            all_projects,
            project,
            db,
            private_api,
        } => run_overview_with_codex_dashboard(
            cli.format,
            cli.color,
            UsageRangeOptions { days, range },
            all_projects,
            project,
            db,
            private_api.enabled(),
        ),
        Commands::Codex { command } => {
            run_codex_with_codex_dashboard(cli.format, cli.color, command)
        }
        Commands::Opencode { command } => run_opencode(cli.format, command),
        Commands::Doctor => run_doctor(cli.format),
        Commands::Update { check, yes, force } => run_update(cli.format, check, yes, force),
        Commands::Completion { shell, install } => {
            if install {
                return install_completion(shell);
            }
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

fn run_overview_with_codex_dashboard(
    format: OutputFormat,
    color: ColorMode,
    range_options: UsageRangeOptions,
    all_projects: bool,
    project: Option<PathBuf>,
    db: Option<PathBuf>,
    use_private_api: bool,
) -> Result<()> {
    let provider = provider(db);
    let report = provider
        .usage_period(range_options.period(), all_projects, project.as_deref())
        .context("failed to read OpenCode usage")?;
    let codex_queried_at = Local::now();
    let codex = ProfileStore::from_env()
        .map_err(anyhow::Error::from)
        .and_then(|store| fetch_all(&store, use_private_api, None))
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
        output_codex_dashboard_v2(
            OutputFormat::Terminal,
            &codex,
            None,
            color,
            use_private_api,
            codex_queried_at,
        )?;
        println!("\nOPENCODE");
        let summary = provider
            .usage_period(UsagePeriod::LastDays(30), all_projects, project.as_deref())
            .context("failed to read OpenCode usage summary")?;
        output_usage(
            format,
            &report,
            Some(&summary),
            false,
            10,
            range_options.label(),
        )
    }
}

fn run_codex_with_codex_dashboard(
    format: OutputFormat,
    color: ColorMode,
    command: CodexCommands,
) -> Result<()> {
    match command {
        CodexCommands::Usage {
            profile,
            private_api,
        } => {
            let store =
                ProfileStore::from_env().context("failed to open Codex profile storage")?;
            if let Some(name) = profile.as_deref() {
                store.resolve(Some(name))?;
            }
            let use_private_api = private_api.enabled();
            let queried_at = Local::now();
            let results = fetch_all(&store, use_private_api, profile.as_deref())?;
            output_codex_dashboard_v2(
                format,
                &results,
                profile.as_deref(),
                color,
                use_private_api,
                queried_at,
            )
        }
        CodexCommands::Credits {
            profile,
            private_api,
        } => {
            let store =
                ProfileStore::from_env().context("failed to open Codex profile storage")?;
            let profile = store.resolve(profile.as_deref())?;
            let snapshot = fetch(&profile)?;
            let credits = detailed_credits(&profile, &snapshot, private_api.enabled())?;
            output_credits_v2(format, color, &profile.name, &credits)
        }
        CodexCommands::All { private_api } => {
            let store =
                ProfileStore::from_env().context("failed to open Codex profile storage")?;
            let use_private_api = private_api.enabled();
            let queried_at = Local::now();
            let results = fetch_all(&store, use_private_api, None)?;
            output_codex_dashboard_v2(
                format,
                &results,
                None,
                color,
                use_private_api,
                queried_at,
            )
        }
        other => run_codex(format, color, other),
    }
}

fn codex_ui_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100)
        .saturating_sub(4)
        .clamp(36, 100)
}
