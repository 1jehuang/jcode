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
            // The old server `allowed` flag is no longer authoritative after
            // rollover. Wait for the existing usage refresh before suggesting
            // an irreversible banked reset.
            snapshot.openai_reset_credits = None;
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

const HOUR_SECONDS: u64 = 60 * 60;
const DAY_SECONDS: u64 = 24 * HOUR_SECONDS;

/// Length of a quota window, inferred from the label the provider reported.
///
/// Anthropic uses fixed `5-hour`/`Weekly` labels; OpenAI derives labels from
/// the window length it reports (`5-hour`, `7-day`, `Monthly`, ...). Anything
/// unrecognised (notably Codex `Spark`) yields `None`, and the caller keeps the
/// reset countdown instead of an elapsed percentage.
pub fn window_seconds_for_label(label: &str) -> Option<u64> {
    let normalized = label.trim().to_ascii_lowercase();
    let normalized = normalized.trim_end_matches(" window").trim();
    match normalized {
        "weekly" => return Some(7 * DAY_SECONDS),
        "daily" => return Some(DAY_SECONDS),
        "monthly" => return Some(30 * DAY_SECONDS),
        _ => {}
    }
    if let Some(hours) = normalized.strip_suffix("-hour") {
        return hours
            .parse::<u64>()
            .ok()
            .and_then(|h| h.checked_mul(HOUR_SECONDS));
    }
    if let Some(days) = normalized.strip_suffix("-day") {
        return days
            .parse::<u64>()
            .ok()
            .and_then(|d| d.checked_mul(DAY_SECONDS));
    }
    None
}

/// How much of a rolling quota window has already elapsed, as a percentage.
///
/// Derived from the window's reset timestamp and its total length: a window
/// resetting in 30 minutes with a 5-hour length is 90% elapsed. Returns `None`
/// when the timestamp cannot be parsed or the window length is unknown, so
/// callers can fall back to the reset countdown.
pub fn window_elapsed_percent(resets_at: &str, window_seconds: u64) -> Option<u8> {
    if window_seconds == 0 {
        return None;
    }
    let reset = parse_reset_timestamp(resets_at)?;
    let remaining = reset
        .signed_duration_since(chrono::Utc::now())
        .num_seconds();
    if remaining <= 0 {
        return Some(100);
    }
    let remaining = (remaining as u64).min(window_seconds);
    let elapsed = window_seconds - remaining;
    // Widened so provider-supplied window lengths near u64::MAX cannot overflow.
    let percent =
        (u128::from(elapsed) * 100 + u128::from(window_seconds) / 2) / u128::from(window_seconds);
    Some(percent.min(100) as u8)
}

pub fn format_usage_bar(percent: f32, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f32).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);
    let bar: String = "█".repeat(filled) + &"░".repeat(empty);
    // Right-align the number so the `%` (and any "· resets" suffix) lines up
    // across rows with different percentages.
    format!("{} {:>3.0}%", bar, percent)
}

#[cfg(test)]
mod elapsed_tests {
    use super::{DAY_SECONDS, HOUR_SECONDS, window_elapsed_percent, window_seconds_for_label};

    const FIVE_HOURS: u64 = 5 * 60 * 60;
    const SEVEN_DAYS: u64 = 7 * 24 * 60 * 60;

    fn resets_in(seconds: i64) -> String {
        (chrono::Utc::now() + chrono::Duration::seconds(seconds)).to_rfc3339()
    }

    #[test]
    fn elapsed_percent_tracks_how_far_the_window_has_run() {
        let half = window_elapsed_percent(&resets_in(FIVE_HOURS as i64 / 2), FIVE_HOURS)
            .expect("parsable reset");
        assert!((49..=51).contains(&half), "expected ~50%, got {half}");

        let nearly_done =
            window_elapsed_percent(&resets_in(30 * 60), FIVE_HOURS).expect("parsable reset");
        assert!(
            (89..=91).contains(&nearly_done),
            "expected ~90%, got {nearly_done}"
        );

        let weekly =
            window_elapsed_percent(&resets_in(32 * 60 * 60), SEVEN_DAYS).expect("parsable reset");
        assert!((80..=82).contains(&weekly), "expected ~81%, got {weekly}");
    }

    #[test]
    fn elapsed_percent_saturates_and_rejects_unusable_input() {
        assert_eq!(
            window_elapsed_percent(&resets_in(-60), FIVE_HOURS),
            Some(100)
        );
        // A reset further out than the window itself means it just restarted.
        assert_eq!(
            window_elapsed_percent(&resets_in(FIVE_HOURS as i64 * 2), FIVE_HOURS),
            Some(0)
        );
        assert_eq!(window_elapsed_percent("not-a-timestamp", FIVE_HOURS), None);
        assert_eq!(window_elapsed_percent(&resets_in(60), 0), None);
    }

    #[test]
    fn elapsed_percent_survives_huge_provider_windows() {
        assert_eq!(
            window_elapsed_percent(&resets_in(60 * 60), u64::MAX),
            Some(100)
        );
        assert_eq!(
            window_elapsed_percent(&resets_in(60 * 60), u64::MAX / 100 + 1),
            Some(100)
        );
    }

    #[test]
    fn window_lengths_are_inferred_from_provider_labels() {
        assert_eq!(window_seconds_for_label("5-hour"), Some(5 * HOUR_SECONDS));
        assert_eq!(
            window_seconds_for_label("5-hour window"),
            Some(5 * HOUR_SECONDS)
        );
        assert_eq!(window_seconds_for_label("Weekly"), Some(7 * DAY_SECONDS));
        assert_eq!(window_seconds_for_label("7-day"), Some(7 * DAY_SECONDS));
        assert_eq!(window_seconds_for_label("Monthly"), Some(30 * DAY_SECONDS));
        // Codex Spark has no advertised window length, so it keeps a countdown.
        assert_eq!(window_seconds_for_label("Spark"), None);
        assert_eq!(window_seconds_for_label("99999999999999999-hour"), None);
        assert_eq!(window_seconds_for_label("99999999999999999-day"), None);
    }
}
