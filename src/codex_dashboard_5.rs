const CODEX_ACCOUNT_STATUS_URL: &str =
    "https://chatgpt.com/backend-api/accounts/check/v4-2023-04-27";

#[derive(Clone, Debug, Serialize)]
struct CodexSubscriptionInfo {
    profile: String,
    plan_type: Option<String>,
    plan_display: String,
    renews_at: Option<DateTime<Utc>>,
    source: String,
    error: Option<String>,
}

fn collect_codex_subscription_info(
    results: &[ProfileResult],
    use_private_api: bool,
) -> Vec<CodexSubscriptionInfo> {
    results
        .iter()
        .filter_map(|result| {
            let snapshot = result.snapshot.as_ref()?;
            Some(fetch_codex_subscription_info(
                &result.profile,
                snapshot.account.plan_type.as_deref(),
                use_private_api,
            ))
        })
        .collect()
}

fn fetch_codex_subscription_info(
    profile_name: &str,
    plan_type: Option<&str>,
    use_private_api: bool,
) -> CodexSubscriptionInfo {
    let mut info = CodexSubscriptionInfo {
        profile: profile_name.to_owned(),
        plan_type: plan_type.map(str::to_owned),
        plan_display: format_codex_plan_name(plan_type),
        renews_at: None,
        source: if use_private_api {
            "account-status".to_owned()
        } else {
            "disabled".to_owned()
        },
        error: None,
    };

    if !use_private_api {
        return info;
    }

    if let Err(error) = populate_codex_subscription_info(profile_name, &mut info) {
        info.error = Some(error);
    }
    info
}

fn populate_codex_subscription_info(
    profile_name: &str,
    info: &mut CodexSubscriptionInfo,
) -> std::result::Result<(), String> {
    let store = ProfileStore::from_env().map_err(|error| error.to_string())?;
    let profile = store
        .resolve(Some(profile_name))
        .map_err(|error| error.to_string())?;
    let auth = fs::read(profile.auth_file()).map_err(|error| error.to_string())?;
    let auth: serde_json::Value =
        serde_json::from_slice(&auth).map_err(|error| error.to_string())?;
    let tokens = auth.get("tokens").unwrap_or(&auth);
    let access_token = tokens
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing tokens.access_token".to_owned())?;
    let account_id = tokens
        .get("account_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| auth.get("account_id").and_then(serde_json::Value::as_str));

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client
        .get(CODEX_ACCOUNT_STATUS_URL)
        .bearer_auth(access_token)
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "Codex Desktop")
        .header("Accept", "application/json")
        .header("Origin", "https://chatgpt.com")
        .header("Referer", "https://chatgpt.com/")
        .header(
            "User-Agent",
            concat!("ai-monitor/", env!("CARGO_PKG_VERSION")),
        );
    if let Some(account_id) = account_id {
        request = request.header("ChatGPT-Account-ID", account_id);
    }

    let response = request.send().map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("account status HTTP {}", response.status()));
    }
    let value: serde_json::Value = response.json().map_err(|error| error.to_string())?;
    info.renews_at = subscription_renewal_from_account_status(&value, account_id);
    Ok(())
}

fn format_codex_plan_name(plan_type: Option<&str>) -> String {
    let Some(plan_type) = plan_type.map(str::trim).filter(|value| !value.is_empty()) else {
        return "N/A".to_owned();
    };
    match plan_type.to_ascii_lowercase().as_str() {
        "free" => "Free".to_owned(),
        "plus" => "Plus".to_owned(),
        "pro" => "Pro 20x".to_owned(),
        "prolite" => "Pro 5x".to_owned(),
        "team" => "Team".to_owned(),
        "business" => "Business".to_owned(),
        "enterprise" => "Enterprise".to_owned(),
        "edu" => "Edu".to_owned(),
        other => other
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn subscription_renewal_from_account_status(
    account_status: &serde_json::Value,
    account_id: Option<&str>,
) -> Option<DateTime<Utc>> {
    if let Some(value) = account_status
        .get("account_plan")
        .and_then(|plan| plan.get("subscription_expires_at_timestamp"))
        .and_then(parse_account_status_datetime)
    {
        return Some(value);
    }

    let accounts = account_status.get("accounts")?.as_object()?;
    let matching = account_id.and_then(|account_id| {
        accounts.values().find(|entry| {
            entry
                .pointer("/account/account_id")
                .and_then(serde_json::Value::as_str)
                == Some(account_id)
        })
    });
    let selected = matching
        .or_else(|| accounts.get("default"))
        .or_else(|| accounts.values().next())?;
    selected
        .pointer("/entitlement/expires_at")
        .and_then(parse_account_status_datetime)
}

fn parse_account_status_datetime(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    if let Some(number) = value.as_f64() {
        if !number.is_finite() {
            return None;
        }
        let seconds = if number.abs() > 100_000_000_000.0 {
            number / 1000.0
        } else {
            number
        };
        return DateTime::from_timestamp(seconds.round() as i64, 0);
    }
    let text = value.as_str()?;
    if let Ok(number) = text.parse::<f64>() {
        return parse_account_status_datetime(&serde_json::json!(number));
    }
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}

fn subscription_for_profile<'a>(
    subscriptions: &'a [CodexSubscriptionInfo],
    profile: &str,
) -> Option<&'a CodexSubscriptionInfo> {
    subscriptions
        .iter()
        .find(|subscription| subscription.profile == profile)
}

fn format_subscription_renewal(subscription: &CodexSubscriptionInfo) -> String {
    subscription
        .renews_at
        .map(|time| {
            time.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M %:z")
                .to_string()
        })
        .unwrap_or_else(|| "N/A".to_owned())
}

fn output_credits_v2(
    format: OutputFormat,
    color: ColorMode,
    profile: &str,
    credits: &ResetCredits,
) -> Result<()> {
    if matches!(format, OutputFormat::Json) {
        return output_value(format, credits);
    }

    let theme = Theme::new(color);
    let width = codex_ui_width();
    println!();
    println!("  ┌{}┐", "─".repeat(width));
    card_line(
        &theme,
        width,
        &theme.paint(truncate(&format!(" {profile} · RESET CREDITS"), width), BOLD),
    );
    card_line(&theme, width, "");
    let (count, count_color) = credit_count_parts(credits);
    let summary = format!("   {count} · source {}", credits.source);
    card_line(
        &theme,
        width,
        &theme.paint(truncate(&summary, width), count_color),
    );

    match credits.credits.as_deref() {
        Some(rows) if !rows.is_empty() => {
            for (index, credit) in rows.iter().enumerate() {
                render_credit_detail_v2(&theme, index + 1, credit, width);
            }
        }
        Some(_) if credits.available_count.unwrap_or(0) > 0 => card_line(
            &theme,
            width,
            &theme.paint(
                truncate("   Credit lifecycle details are unavailable.", width),
                YELLOW,
            ),
        ),
        Some(_) => card_line(&theme, width, "   No reset credits currently available."),
        None => card_line(
            &theme,
            width,
            &theme.paint(
                truncate("   Reset-credit detail rows are unavailable.", width),
                YELLOW,
            ),
        ),
    }
    println!("  └{}┘", "─".repeat(width));
    Ok(())
}

#[cfg(test)]
mod codex_subscription_tests {
    use super::*;

    #[test]
    fn formats_known_codex_plan_names() {
        assert_eq!(format_codex_plan_name(Some("prolite")), "Pro 5x");
        assert_eq!(format_codex_plan_name(Some("pro")), "Pro 20x");
        assert_eq!(format_codex_plan_name(Some("plus")), "Plus");
    }

    #[test]
    fn parses_legacy_subscription_expiry() {
        let value = serde_json::json!({
            "account_plan": {"subscription_expires_at_timestamp": 1_788_901_920_u64}
        });
        assert!(subscription_renewal_from_account_status(&value, None).is_some());
    }

    #[test]
    fn selects_matching_account_entitlement() {
        let value = serde_json::json!({
            "accounts": {
                "default": {
                    "account": {"account_id": "fallback"},
                    "entitlement": {"expires_at": "2026-10-01T00:00:00Z"}
                },
                "work": {
                    "account": {"account_id": "target"},
                    "entitlement": {"expires_at": "2026-10-06T00:31:00+08:00"}
                }
            }
        });
        let renewal = subscription_renewal_from_account_status(&value, Some("target")).unwrap();
        assert_eq!(renewal.to_rfc3339(), "2026-10-05T16:31:00+00:00");
    }
}
