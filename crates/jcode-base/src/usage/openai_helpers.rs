use super::display::humanize_key;
use super::{OpenAIUsageData, OpenAIUsageWindow, UsageLimit};

#[derive(Debug, Default)]
pub(super) struct ParsedOpenAIUsageReport {
    pub(super) limits: Vec<UsageLimit>,
    pub(super) extra_info: Vec<(String, String)>,
    pub(super) hard_limit_reached: bool,
}

fn normalize_ratio_value(raw: f32) -> f32 {
    if !raw.is_finite() {
        return 0.0;
    }
    if raw > 1.0 {
        (raw / 100.0).clamp(0.0, 1.0)
    } else {
        raw.clamp(0.0, 1.0)
    }
}

fn normalize_percent(raw: f32) -> f32 {
    normalize_ratio_value(raw) * 100.0
}

fn clamp_percent(raw: f32) -> f32 {
    if raw.is_finite() {
        raw.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

pub(super) fn usage_percent_to_ratio(percent: f32) -> f32 {
    clamp_percent(percent) / 100.0
}

fn normalize_limit_key(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn limit_mentions_five_hour(key: &str) -> bool {
    key.contains("5 hour")
        || key.contains("5hr")
        || key.contains("5 h")
        || key.contains("five hour")
}

fn limit_mentions_weekly(key: &str) -> bool {
    key.contains("weekly")
        || key.contains("1 week")
        || key.contains("1w")
        || key.contains("7 day")
        || key.contains("seven day")
}

fn limit_mentions_spark(key: &str) -> bool {
    key.contains("spark")
}

fn to_openai_window(limit: &UsageLimit) -> OpenAIUsageWindow {
    OpenAIUsageWindow {
        name: limit.name.clone(),
        usage_ratio: usage_percent_to_ratio(limit.usage_percent),
        resets_at: limit.resets_at.clone(),
    }
}

pub(super) fn classify_openai_limits(limits: &[UsageLimit]) -> OpenAIUsageData {
    let mut five_hour: Option<OpenAIUsageWindow> = None;
    let mut seven_day: Option<OpenAIUsageWindow> = None;
    let mut spark: Option<OpenAIUsageWindow> = None;
    let mut generic_non_spark: Vec<OpenAIUsageWindow> = Vec::new();

    for limit in limits {
        let key = normalize_limit_key(&limit.name);
        let window = to_openai_window(limit);
        let is_spark = limit_mentions_spark(&key);

        if is_spark && spark.is_none() {
            spark = Some(window.clone());
        }

        if !is_spark {
            if limit_mentions_five_hour(&key) && five_hour.is_none() {
                five_hour = Some(window.clone());
            }
            if limit_mentions_weekly(&key) && seven_day.is_none() {
                seven_day = Some(window.clone());
            }
            generic_non_spark.push(window);
        }
    }

    if five_hour.is_none() {
        five_hour = generic_non_spark.first().cloned();
    }
    if seven_day.is_none() {
        seven_day = generic_non_spark
            .iter()
            .find(|w| {
                five_hour
                    .as_ref()
                    .map(|f| f.name != w.name || f.resets_at != w.resets_at)
                    .unwrap_or(true)
            })
            .cloned();
    }

    OpenAIUsageData {
        five_hour,
        seven_day,
        spark,
        ..Default::default()
    }
}

fn parse_f32_value(value: &serde_json::Value) -> Option<f32> {
    if let Some(n) = value.as_f64() {
        return Some(n as f32);
    }
    value.as_str().and_then(|s| s.trim().parse::<f32>().ok())
}

pub(super) fn parse_usage_percent_from_obj(
    obj: &serde_json::Map<String, serde_json::Value>,
) -> Option<f32> {
    for key in ["usage_percent", "used_percent", "percent_used"] {
        if let Some(value) = obj.get(key).and_then(parse_f32_value) {
            return Some(clamp_percent(value));
        }
    }

    for key in ["usage", "utilization", "usage_ratio", "used_ratio"] {
        if let Some(value) = obj.get(key).and_then(parse_f32_value) {
            return Some(normalize_percent(value));
        }
    }

    let used = obj.get("used").and_then(parse_f32_value);
    let remaining = obj.get("remaining").and_then(parse_f32_value);
    let limit = obj
        .get("limit")
        .or_else(|| obj.get("max"))
        .and_then(parse_f32_value);

    if let (Some(used), Some(limit)) = (used, limit)
        && limit > 0.0
    {
        return Some(((used / limit) * 100.0).clamp(0.0, 100.0));
    }

    if let (Some(remaining), Some(limit)) = (remaining, limit)
        && limit > 0.0
    {
        let used = (limit - remaining).max(0.0);
        return Some(((used / limit) * 100.0).clamp(0.0, 100.0));
    }

    None
}

fn parse_resets_at_from_obj(obj: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    for key in [
        "resets_at",
        "reset_at",
        "resetsAt",
        "resetAt",
        "reset_time",
        "resetTime",
    ] {
        if let Some(value) = obj.get(key).and_then(|v| v.as_str()) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn parse_limit_name(entry: &serde_json::Value, fallback: &str) -> String {
    entry
        .get("name")
        .or_else(|| entry.get("label"))
        .or_else(|| entry.get("display_name"))
        .or_else(|| entry.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or(fallback)
        .to_string()
}

fn parse_bool_value(value: &serde_json::Value) -> Option<bool> {
    if let Some(b) = value.as_bool() {
        return Some(b);
    }

    value
        .as_str()
        .and_then(|s| match s.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
}

pub(super) fn parse_openai_hard_limit_reached(json: &serde_json::Value) -> bool {
    let Some(obj) = json.as_object() else {
        return false;
    };

    if obj.get("limit_reached").and_then(parse_bool_value) == Some(true)
        || obj.get("limitReached").and_then(parse_bool_value) == Some(true)
    {
        return true;
    }

    obj.get("rate_limit")
        .and_then(|rate_limit| rate_limit.as_object())
        .and_then(|rate_limit| rate_limit.get("allowed"))
        .and_then(parse_bool_value)
        == Some(false)
}

fn parse_wham_window(window: &serde_json::Value, name: &str) -> Option<UsageLimit> {
    let obj = window.as_object()?;
    let used_percent = obj
        .get("used_percent")
        .and_then(parse_f32_value)
        .map(clamp_percent)?;
    let resets_at = obj.get("reset_at").and_then(parse_f32_value).map(|ts| {
        chrono::DateTime::from_timestamp(ts as i64, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_else(|| format!("{}", ts as i64))
    });
    Some(UsageLimit {
        name: name.to_string(),
        usage_percent: used_percent,
        resets_at,
    })
}

fn parse_wham_rate_limit(
    rl: &serde_json::Value,
    primary_name: &str,
    secondary_name: &str,
) -> Vec<UsageLimit> {
    let mut out = Vec::new();
    if let Some(pw) = rl.get("primary_window")
        && let Some(limit) = parse_wham_window(pw, primary_name)
    {
        out.push(limit);
    }
    if let Some(sw) = rl.get("secondary_window")
        && !sw.is_null()
        && let Some(limit) = parse_wham_window(sw, secondary_name)
    {
        out.push(limit);
    }
    out
}

pub(super) fn parse_openai_usage_payload(json: &serde_json::Value) -> ParsedOpenAIUsageReport {
    let mut parsed = ParsedOpenAIUsageReport {
        hard_limit_reached: parse_openai_hard_limit_reached(json),
        ..Default::default()
    };

    if let Some(rl) = json.get("rate_limit") {
        parsed
            .limits
            .extend(parse_wham_rate_limit(rl, "5-hour window", "7-day window"));
    }

    if let Some(additional) = json
        .get("additional_rate_limits")
        .and_then(|v| v.as_array())
    {
        for entry in additional {
            let limit_name = entry
                .get("limit_name")
                .and_then(|v| v.as_str())
                .unwrap_or("Additional");
            if let Some(rl) = entry.get("rate_limit") {
                let primary = format!("{} (5h)", limit_name);
                let secondary = format!("{} (7d)", limit_name);
                parsed
                    .limits
                    .extend(parse_wham_rate_limit(rl, &primary, &secondary));
            }
        }
    }

    if parsed.limits.is_empty()
        && let Some(rate_limits) = json.get("rate_limits").and_then(|v| v.as_array())
    {
        for entry in rate_limits {
            if let Some(obj) = entry.as_object()
                && let Some(usage_percent) = parse_usage_percent_from_obj(obj)
            {
                parsed.limits.push(UsageLimit {
                    name: parse_limit_name(entry, "unknown"),
                    usage_percent,
                    resets_at: parse_resets_at_from_obj(obj),
                });
            }
        }
    }

    if parsed.limits.is_empty()
        && let Some(obj) = json.as_object()
    {
        for (key, value) in obj {
            if key == "rate_limits" || key == "rate_limit" || key == "additional_rate_limits" {
                continue;
            }

            if let Some(inner) = value.as_object() {
                if let Some(usage_percent) = parse_usage_percent_from_obj(inner) {
                    parsed.limits.push(UsageLimit {
                        name: humanize_key(key),
                        usage_percent,
                        resets_at: parse_resets_at_from_obj(inner),
                    });
                    continue;
                }

                if let Some(windows) = inner.get("rate_limits").and_then(|v| v.as_array()) {
                    for entry in windows {
                        if let Some(entry_obj) = entry.as_object()
                            && let Some(usage_percent) = parse_usage_percent_from_obj(entry_obj)
                        {
                            parsed.limits.push(UsageLimit {
                                name: parse_limit_name(entry, key),
                                usage_percent,
                                resets_at: parse_resets_at_from_obj(entry_obj),
                            });
                        }
                    }
                }
            }
        }
    }

    if let Some(plan) = json
        .get("plan_type")
        .or_else(|| json.get("plan"))
        .or_else(|| json.get("subscription_type"))
        .and_then(|v| v.as_str())
    {
        parsed
            .extra_info
            .insert(0, ("Plan".to_string(), plan.to_string()));
    }

    parsed
}
