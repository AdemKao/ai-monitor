fn bottleneck_limit(snapshot: &Snapshot) -> Option<&LimitWindow> {
    snapshot
        .limits
        .iter()
        .max_by(|left, right| left.used_percent.total_cmp(&right.used_percent))
}

fn remaining_percent(limit: &LimitWindow) -> f64 {
    100.0 - limit.used_percent.clamp(0.0, 100.0)
}

fn quota_state(limit: &LimitWindow) -> (f64, &'static str, &'static str) {
    let remaining = remaining_percent(limit);
    if remaining <= 5.0 {
        (remaining, "CRITICAL", RED)
    } else if remaining <= 10.0 {
        (remaining, "LOW", RED)
    } else if remaining <= 25.0 {
        (remaining, "WARN", YELLOW)
    } else {
        (remaining, "OK", GREEN)
    }
}

fn overview_limit_label(limit: &LimitWindow) -> String {
    let group = limit_group_label(limit);
    let window = limit_window_label_v2(limit);
    if group == "STANDARD" {
        window
    } else if window == limit.name {
        group
    } else {
        format!("{} {}", compact_group_label(&group), compact_window_label(limit))
    }
}

fn render_account_card_v2(
    theme: &Theme,
    result: &ProfileResult,
    selected: Option<&str>,
    use_private_api: bool,
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
        &theme.paint(
            truncate(&format!(" {}{}", result.profile, selected_marker), width),
            CYAN,
        ),
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
        card_line(
            theme,
            width,
            &theme.paint(truncate(&format!(" {message}"), width), YELLOW),
        );
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
    card_line(theme, width, &theme.paint(" USAGE LIMITS", BOLD));
    if snapshot.limits.is_empty() {
        card_line(theme, width, "   No rate-limit window returned by Codex.");
    }

    let mut current_group: Option<String> = None;
    for limit in &snapshot.limits {
        let group = limit_group_label(limit);
        if current_group.as_deref() != Some(group.as_str()) {
            if current_group.is_some() {
                card_line(theme, width, "");
            }
            card_line(
                theme,
                width,
                &theme.paint(truncate(&format!("   {group}"), width), BOLD),
            );
            current_group = Some(group);
        }
        render_limit_row_v2(theme, limit, width);
    }

    card_line(theme, width, "");
    card_line(theme, width, &theme.paint(" RESET CREDITS", BOLD));
    render_credit_summary_v2(
        theme,
        &snapshot.reset_credits,
        &result.profile,
        use_private_api,
        result.credit_error.as_deref(),
        width,
    );

    if let Some(error) = &snapshot.usage_error {
        card_line(
            theme,
            width,
            &theme.paint(
                truncate(&format!("   Usage activity: {error}"), width),
                DIM,
            ),
        );
    }
    println!("  └{}┘", "─".repeat(width));
}

fn render_limit_row_v2(theme: &Theme, limit: &LimitWindow, width: usize) {
    let (remaining, badge, color) = quota_state(limit);
    let label = truncate(&limit_window_label_v2(limit), 11);
    if width >= 72 {
        let bar_width = if width >= 88 { 18 } else { 12 };
        let remaining_text = theme.paint(format!("{remaining:>5.1}% {badge:<8}"), color);
        let bar = theme.paint(progress_bar(remaining, bar_width), color);
        let reset = limit
            .resets_at
            .map(format_reset_summary)
            .unwrap_or_else(|| "unknown".to_owned());
        card_line(
            theme,
            width,
            &format!(
                "   {label:<11} {remaining_text} {bar}  reset {}",
                truncate(&reset, 20)
            ),
        );
    } else {
        let remaining_text = theme.paint(format!("{remaining:>5.1}% {badge}"), color);
        card_line(
            theme,
            width,
            &format!("   {label:<11} {remaining_text}"),
        );
        let bar_width = width.saturating_sub(24).clamp(8, 16);
        let bar = theme.paint(progress_bar(remaining, bar_width), color);
        let reset = limit
            .resets_at
            .map(short_relative_time)
            .unwrap_or_else(|| "unknown".to_owned());
        card_line(
            theme,
            width,
            &format!("             {bar} · {reset}"),
        );
    }
}

fn limit_group_label(limit: &LimitWindow) -> String {
    let Some(limit_id) = limit.limit_id.as_deref() else {
        return "STANDARD".to_owned();
    };
    let id = limit_id.to_ascii_lowercase();
    let name = limit
        .limit_name
        .as_deref()
        .unwrap_or(limit.name.as_str())
        .to_ascii_lowercase();
    if id == "base_model_inference" || id.contains("reserve") || name.contains("reserve") {
        return "LUNA RESERVE".to_owned();
    }
    if id.contains("bengalfox") || id.contains("spark") || name.contains("spark") {
        return "SPARK".to_owned();
    }
    humanize_limit_id(limit.limit_name.as_deref().unwrap_or(limit_id)).to_ascii_uppercase()
}

fn compact_group_label(group: &str) -> &str {
    match group {
        "LUNA RESERVE" => "Reserve",
        "SPARK" => "Spark",
        _ => group,
    }
}

fn humanize_limit_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if matches!(character, '_' | '-') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn limit_window_label_v2(limit: &LimitWindow) -> String {
    if limit.limit_id.is_none() {
        return limit_label(limit);
    }
    limit
        .source_window_minutes
        .map(duration_label)
        .unwrap_or_else(|| limit.name.clone())
}

fn compact_window_label(limit: &LimitWindow) -> String {
    match limit.source_window_minutes {
        Some(300) => "5h".to_owned(),
        Some(10_080) => "W".to_owned(),
        Some(minutes) if minutes % 1_440 == 0 => format!("{}d", minutes / 1_440),
        Some(minutes) if minutes % 60 == 0 => format!("{}h", minutes / 60),
        Some(minutes) => format!("{minutes}m"),
        None => limit.name.clone(),
    }
}

fn duration_label(minutes: u64) -> String {
    match minutes {
        300 => "5 hour".to_owned(),
        10_080 => "Weekly".to_owned(),
        minutes if minutes % 1_440 == 0 => format!("{} day", minutes / 1_440),
        minutes if minutes % 60 == 0 => format!("{} hour", minutes / 60),
        minutes => format!("{minutes} min"),
    }
}
