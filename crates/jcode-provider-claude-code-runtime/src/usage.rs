//! Claude Code subscription usage, read through the CLI's `get_usage`
//! control request on an init-only probe child (no prompt, no tokens).

use serde_json::Value;

/// One usage window in display form.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    /// `5-hour window`, `7-day window`, `7-day window (Fable)`, ...
    pub name: String,
    /// 0..=100.
    pub percent: f32,
    /// RFC 3339 reset time as the CLI reports it.
    pub resets_at: Option<String>,
}

/// Parsed `get_usage` response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaudeCodeUsage {
    pub subscription: Option<String>,
    pub windows: Vec<UsageWindow>,
    /// The CLI could not report limits (API key or proxy logins).
    pub unavailable: bool,
}

fn str_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn percent(value: &Value) -> Option<f32> {
    value.as_f64().map(|p| p.clamp(0.0, 100.0) as f32)
}

/// Parse the `response` body of a `get_usage` control response.
///
/// Prefers the CLI's own `rate_limits.limits[]` list (session, weekly and
/// model-scoped weekly windows, already in percent). Falls back to the
/// `five_hour` / `seven_day` objects for older CLIs.
pub fn parse_get_usage(response: &Value) -> ClaudeCodeUsage {
    let subscription = str_field(response, "subscription_type");
    let available = response
        .get("rate_limits_available")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let Some(rate_limits) = response.get("rate_limits").filter(|v| v.is_object()) else {
        return ClaudeCodeUsage {
            subscription,
            windows: Vec::new(),
            unavailable: true,
        };
    };

    let mut windows = Vec::new();
    if let Some(limits) = rate_limits.get("limits").and_then(Value::as_array) {
        for limit in limits {
            let Some(pct) = limit.get("percent").and_then(percent) else {
                continue;
            };
            let kind = limit.get("kind").and_then(Value::as_str).unwrap_or("");
            let scope_name = limit
                .get("scope")
                .and_then(|s| s.get("model"))
                .and_then(|m| str_field(m, "display_name"));
            let name = match (kind, scope_name) {
                ("session", _) => "5-hour window".to_string(),
                ("weekly_all", _) => "7-day window".to_string(),
                (_, Some(model)) => format!("7-day window ({model})"),
                (other, None) if !other.is_empty() => other.replace('_', " "),
                _ => continue,
            };
            windows.push(UsageWindow {
                name,
                percent: pct,
                resets_at: str_field(limit, "resets_at"),
            });
        }
    }
    if windows.is_empty() {
        for (key, name) in [
            ("five_hour", "5-hour window"),
            ("seven_day", "7-day window"),
        ] {
            let Some(window) = rate_limits.get(key).filter(|w| w.is_object()) else {
                continue;
            };
            let Some(pct) = window.get("utilization").and_then(percent) else {
                continue;
            };
            windows.push(UsageWindow {
                name: name.to_string(),
                percent: pct,
                resets_at: str_field(window, "resets_at"),
            });
        }
    }
    ClaudeCodeUsage {
        subscription,
        unavailable: !available && windows.is_empty(),
        windows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FIXTURE: &str = include_str!("../tests/fixtures/get_usage.json");

    #[test]
    fn real_get_usage_response_maps_to_named_windows() {
        let line: Value = serde_json::from_str(FIXTURE).unwrap();
        let usage = parse_get_usage(&line["response"]["response"]);
        assert_eq!(usage.subscription.as_deref(), Some("max"));
        assert!(!usage.unavailable);
        let names: Vec<&str> = usage.windows.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["5-hour window", "7-day window", "7-day window (Fable)"]
        );
        assert!(usage.windows.iter().all(|w| w.resets_at.is_some()));
        assert!(
            usage
                .windows
                .iter()
                .all(|w| (0.0..=100.0).contains(&w.percent))
        );
    }

    #[test]
    fn falls_back_to_window_objects_without_limits_list() {
        let usage = parse_get_usage(&json!({
            "rate_limits": {
                "five_hour": {"utilization": 12, "resets_at": "2026-10-05T02:00:00Z"},
                "seven_day": {"utilization": 250, "resets_at": null}
            }
        }));
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.windows[0].percent, 12.0);
        assert_eq!(usage.windows[1].percent, 100.0, "clamped");
        assert_eq!(usage.windows[1].resets_at, None);
    }

    #[test]
    fn missing_rate_limits_is_unavailable() {
        let usage = parse_get_usage(&json!({"rate_limits_available": false, "rate_limits": null}));
        assert!(usage.unavailable);
        assert!(usage.windows.is_empty());
    }
}
