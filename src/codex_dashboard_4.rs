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
            let hint = truncate(
                &format!("   Details: ai-monitor codex credits --profile {profile}"),
                width,
            );
            card_line(theme, width, &theme.paint(hint, DIM));
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
        }
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
