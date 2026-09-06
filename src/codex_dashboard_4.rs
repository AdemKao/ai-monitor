fn render_credit_summary_v2(
    theme: &Theme,
    credits: &ResetCredits,
    profile: &str,
    use_private_api: bool,
    detail_error: Option<&str>,
    width: usize,
) {
    let (count, count_color) = credit_count_parts(credits);
    let next_expiry = next_active_credit_expiry(credits);
    let expiry = next_expiry
        .map(|time| {
            if width >= 64 {
                format!(
                    "{} ({})",
                    time.with_timezone(&Local).format("%Y-%m-%d %H:%M"),
                    relative_time(time)
                )
            } else {
                format!(
                    "{} ({})",
                    time.with_timezone(&Local).format("%m-%d %H:%M"),
                    short_relative_time(time)
                )
            }
        })
        .unwrap_or_else(|| "unknown".to_owned());
    let summary = truncate(&format!("   {count} · next expiry {expiry}"), width);
    card_line(theme, width, &theme.paint(summary, count_color));

    match credits.credits.as_deref() {
        Some(rows) if !rows.is_empty() => {
            for (index, credit) in rows.iter().enumerate() {
                render_credit_detail_v2(theme, index + 1, credit, width);
            }
        }
        Some(_) if credits.available_count.unwrap_or(0) > 0 => {
            card_line(
                theme,
                width,
                &theme.paint(
                    truncate("   Expiry details unavailable from app-server", width),
                    YELLOW,
                ),
            );
        }
        Some(_) => {}
        None => {
            let hint = if let Some(error) = detail_error {
                error
            } else if use_private_api {
                "Private credit detail lookup failed or returned no rows"
            } else {
                "Private credit lookup disabled by --no-private-api"
            };
            card_line(
                theme,
                width,
                &theme.paint(truncate(&format!("   {hint}"), width), YELLOW),
            );
            let command = truncate(
                &format!("   Details: ai-monitor codex credits --profile {profile}"),
                width,
            );
            card_line(theme, width, &theme.paint(command, DIM));
        }
    }
}

fn render_credit_detail_v2(theme: &Theme, index: usize, credit: &ResetCredit, width: usize) {
    let status = credit.status.as_deref().unwrap_or("unknown");
    let normalized = status.to_ascii_lowercase();
    let (badge, color) = if matches!(normalized.as_str(), "available" | "active") {
        ("AVAILABLE", GREEN)
    } else if normalized.contains("expired") {
        ("EXPIRED", RED)
    } else if normalized.contains("used") || normalized.contains("consumed") {
        ("USED", DIM)
    } else if normalized.contains("pending") {
        ("PENDING", YELLOW)
    } else {
        ("UNKNOWN", YELLOW)
    };
    let remaining = credit_remaining_text(credit.expires_at);
    let id = credit
        .id
        .as_deref()
        .map(|value| truncate(value, 12))
        .unwrap_or_else(|| format!("#{index:03}"));
    let heading = format!("   #{index:03} [{badge}] status={status} · id={id}");
    card_line(theme, width, &theme.paint(truncate(&heading, width), color));
    card_line(
        theme,
        width,
        &theme.paint(truncate(&format!("        {remaining}"), width), color),
    );

    let granted = credit.granted_at.map(format_credit_time_v2);
    let expires = credit.expires_at.map(format_credit_time_v2);
    if width >= 86 {
        let lifecycle = format!(
            "        granted {} · expires {}",
            granted.as_deref().unwrap_or("N/A"),
            expires.as_deref().unwrap_or("N/A")
        );
        card_line(theme, width, &theme.paint(truncate(&lifecycle, width), DIM));
    } else {
        card_line(
            theme,
            width,
            &theme.paint(
                truncate(
                    &format!("        granted {}", granted.as_deref().unwrap_or("N/A")),
                    width,
                ),
                DIM,
            ),
        );
        card_line(
            theme,
            width,
            &theme.paint(
                truncate(
                    &format!("        expires {}", expires.as_deref().unwrap_or("N/A")),
                    width,
                ),
                DIM,
            ),
        );
    }

    if let Some(title) = credit.title.as_deref().filter(|title| !title.is_empty()) {
        card_line(
            theme,
            width,
            &theme.paint(truncate(&format!("        {title}"), width), DIM),
        );
    }
}

fn format_credit_time_v2(time: DateTime<Utc>) -> String {
    time.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M %:z")
        .to_string()
}

fn credit_remaining_text(expires_at: Option<DateTime<Utc>>) -> String {
    let Some(expires_at) = expires_at else {
        return "remaining N/A".to_owned();
    };
    let seconds = (expires_at - Utc::now()).num_seconds();
    if seconds <= 0 {
        return "expired".to_owned();
    }
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("remaining {days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("remaining {hours}h {minutes}m")
    } else {
        format!("remaining {minutes}m")
    }
}

fn next_active_credit_expiry(credits: &ResetCredits) -> Option<DateTime<Utc>> {
    let now = Utc::now();
    credits
        .credits
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|credit| {
            credit
                .status
                .as_deref()
                .map(|status| {
                    matches!(status.to_ascii_lowercase().as_str(), "available" | "active")
                })
                .unwrap_or(true)
        })
        .filter_map(|credit| credit.expires_at)
        .filter(|expires| *expires >= now)
        .min()
}

#[cfg(test)]
mod codex_dashboard_tests {
    use super::*;

    fn test_limit(
        name: &str,
        used_percent: f64,
        window_minutes: Option<u64>,
        limit_id: Option<&str>,
        limit_name: Option<&str>,
        source_window_minutes: Option<u64>,
    ) -> LimitWindow {
        LimitWindow {
            name: name.to_owned(),
            used_percent,
            window_minutes,
            resets_at: None,
            limit_id: limit_id.map(str::to_owned),
            limit_name: limit_name.map(str::to_owned),
            source_window_minutes,
        }
    }

    #[test]
    fn bottleneck_uses_the_lowest_remaining_bucket() {
        let snapshot = Snapshot {
            profile: "main".to_owned(),
            account: codex::Account::default(),
            limits: vec![
                test_limit("primary", 0.0, Some(300), None, None, None),
                test_limit("secondary", 97.0, Some(10_080), None, None, None),
                test_limit(
                    "Reserve W",
                    20.0,
                    None,
                    Some("base_model_inference"),
                    Some("gpt-reserve"),
                    Some(10_080),
                ),
            ],
            reset_credits: ResetCredits::default(),
            usage: None,
            usage_error: None,
        };
        let limit = bottleneck_limit(&snapshot).unwrap();
        assert_eq!(limit.window_minutes, Some(10_080));
        assert_eq!(quota_state(limit), (3.0, "CRITICAL", RED));
    }

    #[test]
    fn additional_buckets_are_grouped_by_product() {
        let reserve = test_limit(
            "Reserve W",
            0.0,
            None,
            Some("base_model_inference"),
            Some("gpt-reserve"),
            Some(10_080),
        );
        let spark = test_limit(
            "Spark 5h",
            0.0,
            None,
            Some("bengalfox"),
            Some("gpt-5.3-codex-spark"),
            Some(300),
        );
        assert_eq!(limit_group_label(&reserve), "LUNA RESERVE");
        assert_eq!(limit_group_label(&spark), "SPARK");
        assert_eq!(limit_window_label_v2(&reserve), "Weekly");
        assert_eq!(limit_window_label_v2(&spark), "5 hour");
    }
}
