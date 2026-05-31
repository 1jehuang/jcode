use super::{
    OpenAIUsageData, PROVIDER_USAGE_CACHE_TTL, ProviderUsage, RATE_LIMIT_BACKOFF, UsageData,
};
use std::time::Instant;

pub(super) fn reset_timestamp_passed(timestamp: Option<&str>) -> bool {
    usage_reset_passed([timestamp])
}

impl UsageData {
    /// Returns a display-safe snapshot that avoids showing pre-reset usage after a window rolled over.
    pub fn display_snapshot(&self) -> Self {
        let mut snapshot = self.clone();

        if reset_timestamp_passed(self.five_hour_resets_at.as_deref()) {
            snapshot.five_hour = 0.0;
            snapshot.five_hour_resets_at = None;
        }

        if reset_timestamp_passed(self.seven_day_resets_at.as_deref()) {
            snapshot.seven_day = 0.0;
            snapshot.seven_day_opus = None;
            snapshot.seven_day_resets_at = None;
        }

        snapshot
    }
}

impl OpenAIUsageData {
    /// Returns a display-safe snapshot that avoids showing pre-reset exhaustion after a window rolled over.
    pub fn display_snapshot(&self) -> Self {
        let mut snapshot = self.clone();
        let mut cleared_any_window = false;

        if let Some(window) = snapshot.five_hour.as_mut()
            && reset_timestamp_passed(window.resets_at.as_deref())
        {
            window.usage_ratio = 0.0;
            window.resets_at = None;
            cleared_any_window = true;
        }

        if let Some(window) = snapshot.seven_day.as_mut()
            && reset_timestamp_passed(window.resets_at.as_deref())
        {
            window.usage_ratio = 0.0;
            window.resets_at = None;
            cleared_any_window = true;
        }

        if let Some(window) = snapshot.spark.as_mut()
            && reset_timestamp_passed(window.resets_at.as_deref())
        {
            window.usage_ratio = 0.0;
            window.resets_at = None;
            cleared_any_window = true;
        }

        if cleared_any_window {
            snapshot.hard_limit_reached = false;
        }

        snapshot
    }
}

pub(super) fn provider_usage_cache_is_fresh(
    now: Instant,
    fetched_at: Instant,
    report: &ProviderUsage,
) -> bool {
    let ttl = if report
        .error
        .as_ref()
        .map(|e| e.contains("429") || e.contains("rate limit") || e.contains("Rate limited"))
        .unwrap_or(false)
    {
        RATE_LIMIT_BACKOFF
    } else {
        PROVIDER_USAGE_CACHE_TTL
    };

    now.duration_since(fetched_at) < ttl
        && !usage_reset_passed(report.limits.iter().map(|limit| limit.resets_at.as_deref()))
}

pub(super) fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

pub(super) fn humanize_key(key: &str) -> String {
    key.replace('_', " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => {
                    let mut s = c.to_uppercase().to_string();
                    s.push_str(&chars.as_str().to_lowercase());
                    s
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_reset_timestamp(timestamp: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(reset) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        Some(reset.with_timezone(&chrono::Utc))
    } else if let Ok(reset) =
        chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.fZ")
    {
        Some(reset.and_utc())
    } else {
        None
    }
}

pub(super) fn usage_reset_passed<'a>(
    timestamps: impl IntoIterator<Item = Option<&'a str>>,
) -> bool {
    let now = chrono::Utc::now();
    timestamps
        .into_iter()
        .flatten()
        .filter_map(parse_reset_timestamp)
        .any(|reset| reset <= now)
}

pub fn format_reset_time(timestamp: &str) -> String {
    if let Some(reset) = parse_reset_timestamp(timestamp) {
        let duration = reset.signed_duration_since(chrono::Utc::now());
        if duration.num_seconds() <= 0 {
            return "now".to_string();
        }
        if duration.num_seconds() < 60 {
            return "1m".to_string();
        }
        let days = duration.num_days();
        let hours = duration.num_hours() % 24;
        let minutes = duration.num_minutes() % 60;
        if days > 0 {
            if hours > 0 {
                format!("{}d {}h", days, hours)
            } else if minutes > 0 {
                format!("{}d {}m", days, minutes)
            } else {
                format!("{}d", days)
            }
        } else if hours > 0 {
            format!("{}h {}m", hours, minutes)
        } else {
            format!("{}m", minutes)
        }
    } else {
        timestamp.to_string()
    }
}

pub fn format_usage_bar(percent: f32, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f32).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);
    let bar: String = "█".repeat(filled) + &"░".repeat(empty);
    format!("{} {:.0}%", bar, percent)
}
