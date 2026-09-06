fn output_codex_dashboard_v2(
    format: OutputFormat,
    results: &[ProfileResult],
    selected: Option<&str>,
    color: ColorMode,
    use_private_api: bool,
    queried_at: DateTime<Local>,
) -> Result<()> {
    let subscriptions = collect_codex_subscription_info(results, use_private_api);
    if matches!(format, OutputFormat::Json) {
        return output_value(
            format,
            &json!({
                "queried_at": queried_at.to_rfc3339(),
                "account_count": results.len(),
                "selected_profile": selected,
                "accounts": results,
                "subscriptions": subscriptions,
            }),
        );
    }

    let theme = Theme::new(color);
    let width = codex_ui_width();
    let ready = results
        .iter()
        .filter(|result| result.snapshot.is_some())
        .count();
    let constrained = results
        .iter()
        .filter_map(|result| result.snapshot.as_ref())
        .filter_map(bottleneck_limit)
        .filter(|limit| remaining_percent(limit) <= 10.0)
        .count();
    let scope = selected
        .map(|name| format!("selected: {name}"))
        .unwrap_or_else(|| "all accounts".to_owned());
    let account_line = truncate(
        &format!(
            " Accounts: {} total · {} ready · {} low · {}",
            results.len(),
            ready,
            constrained,
            scope
        ),
        width,
    );
    let queried_line = truncate(
        &format!(" Queried: {}", queried_at.format("%Y-%m-%d %H:%M:%S %:z")),
        width,
    );
    let header_subscription = selected
        .and_then(|profile| subscription_for_profile(&subscriptions, profile))
        .or_else(|| (subscriptions.len() == 1).then(|| &subscriptions[0]));
    let subscription_line = header_subscription.map(|subscription| {
        truncate(
            &format!(
                " Plan: {} · renewal: {}",
                subscription.plan_display,
                format_subscription_renewal(subscription)
            ),
            width,
        )
    });

    println!();
    println!(
        "╭{}╮",
        pad_visible(&theme.paint("─ CODEX USAGE DASHBOARD ", BOLD), width)
    );
    println!(
        "│{}│",
        pad_visible(&theme.paint(account_line, CYAN), width)
    );
    println!(
        "│{}│",
        pad_visible(&theme.paint(queried_line, DIM), width)
    );
    if let Some(subscription_line) = subscription_line {
        println!(
            "│{}│",
            pad_visible(&theme.paint(subscription_line, DIM), width)
        );
    }
    println!("╰{}╯", "─".repeat(width));

    println!();
    println!("{}", theme.paint("ACCOUNT OVERVIEW", BOLD));
    if width >= 86 {
        println!(
            "  > PROFILE       STATUS     LOWEST        LIMIT         RESET                CREDITS"
        );
        for result in results {
            render_account_summary_wide(&theme, result, selected);
        }
    } else {
        for result in results {
            render_account_summary_compact(&theme, result, selected, width);
        }
    }

    println!();
    println!("{}", theme.paint("ACCOUNT DETAILS", BOLD));
    if results.is_empty() {
        println!("  No Codex accounts found. Run `ai-monitor codex login NAME`.");
    }
    for result in results {
        render_account_card_v2(
            &theme,
            result,
            selected,
            use_private_api,
            width,
            subscription_for_profile(&subscriptions, &result.profile),
        );
    }

    if !use_private_api && results.iter().any(missing_credit_details) {
        println!();
        println!(
            "{}",
            theme.paint(
                "Note: private reset-credit lookup is disabled by --no-private-api.",
                YELLOW,
            )
        );
    }
    Ok(())
}

fn render_account_summary_wide(
    theme: &Theme,
    result: &ProfileResult,
    selected: Option<&str>,
) {
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
            "  {marker} {:<12} {} {} {} {} {}",
            name,
            styled_cell(theme, status, 10, status_code, false),
            styled_cell(theme, "-", 13, DIM, true),
            styled_cell(theme, "-", 12, DIM, false),
            styled_cell(theme, "-", 20, DIM, false),
            styled_cell(theme, "not ready", 12, DIM, false)
        );
        return;
    };

    let Some(limit) = bottleneck_limit(snapshot) else {
        let (credits, credits_code) = credit_count_parts(&snapshot.reset_credits);
        println!(
            "  {marker} {:<12} {} {} {} {} {}",
            name,
            styled_cell(theme, "[READY]", 10, GREEN, false),
            styled_cell(theme, "unknown", 13, YELLOW, true),
            styled_cell(theme, "-", 12, DIM, false),
            styled_cell(theme, "unknown", 20, DIM, false),
            styled_cell(theme, &credits, 12, credits_code, false)
        );
        return;
    };

    let (remaining, badge, remaining_code) = quota_state(limit);
    let lowest = format!("{remaining:.1}% {badge}");
    let limit_name = truncate(&overview_limit_label(limit), 12);
    let reset = limit
        .resets_at
        .map(format_reset_summary)
        .unwrap_or_else(|| "unknown".to_owned());
    let (credits, credits_code) = credit_count_parts(&snapshot.reset_credits);
    println!(
        "  {marker} {:<12} {} {} {} {} {}",
        name,
        styled_cell(theme, "[READY]", 10, GREEN, false),
        styled_cell(theme, &lowest, 13, remaining_code, true),
        styled_cell(theme, &limit_name, 12, DIM, false),
        styled_cell(theme, &reset, 20, DIM, false),
        styled_cell(theme, &credits, 12, credits_code, false)
    );
}

fn render_account_summary_compact(
    theme: &Theme,
    result: &ProfileResult,
    selected: Option<&str>,
    width: usize,
) {
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
            "  {marker} {:<12} {}",
            name,
            theme.paint(status, status_code)
        );
        return;
    };

    let Some(limit) = bottleneck_limit(snapshot) else {
        println!(
            "  {marker} {:<12} {} · limits unavailable",
            name,
            theme.paint("[READY]", GREEN)
        );
        return;
    };
    let (remaining, badge, remaining_code) = quota_state(limit);
    let quota = theme.paint(format!("{remaining:.1}% {badge}"), remaining_code);
    if width >= 60 {
        println!(
            "  {marker} {:<12} {} · lowest {} · {}",
            name,
            theme.paint("[READY]", GREEN),
            quota,
            overview_limit_label(limit)
        );
    } else {
        println!(
            "  {marker} {:<12} {}",
            name,
            theme.paint("[READY]", GREEN)
        );
        println!(
            "    lowest {} · {}",
            quota,
            truncate(&overview_limit_label(limit), width.saturating_sub(18))
        );
    }

    let reset = limit
        .resets_at
        .map(format_reset_summary)
        .unwrap_or_else(|| "unknown".to_owned());
    let credits = credit_count_parts(&snapshot.reset_credits).0;
    let detail = truncate(&format!("    reset {reset} · credits {credits}"), width);
    println!("  {}", theme.paint(detail, DIM));
}
