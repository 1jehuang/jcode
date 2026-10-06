//! Google Calendar API client.
//!
//! Uses the shared Google OAuth login (`jcode login google --google-services
//! calendar`). Requests go to the Calendar v3 REST API with the user's access
//! token; token refresh is handled by [`crate::auth::google::get_valid_token`].

use crate::auth::google::{self, GoogleService};
use anyhow::Result;
use serde_json::{Value, json};

const API_BASE: &str = "https://www.googleapis.com/calendar/v3";

pub struct CalendarClient {
    http: reqwest::Client,
}

impl Default for CalendarClient {
    fn default() -> Self {
        Self::new()
    }
}

/// When an event starts or ends. Google represents timed and all-day events
/// differently, so keep that distinction explicit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventTime {
    /// RFC 3339 with offset, e.g. `2026-10-05T19:00:00-07:00`.
    Absolute(String),
    /// Wall-clock time interpreted in an IANA zone, e.g. `2026-10-05T19:00:00`.
    Local {
        date_time: String,
        time_zone: String,
    },
    /// All-day event date, `YYYY-MM-DD`.
    Date(String),
}

impl EventTime {
    pub fn to_json(&self) -> Value {
        match self {
            EventTime::Absolute(dt) => json!({ "dateTime": dt }),
            EventTime::Local {
                date_time,
                time_zone,
            } => json!({ "dateTime": date_time, "timeZone": time_zone }),
            EventTime::Date(date) => json!({ "date": date }),
        }
    }
}

/// Classification of a user-supplied time string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedTime {
    Absolute(String),
    /// Naive local date-time normalized to `YYYY-MM-DDTHH:MM:SS`.
    Naive(String),
    Date(String),
}

/// Parse a time the model passed: RFC 3339 with offset, naive local
/// `YYYY-MM-DD[T ]HH:MM[:SS]`, or date-only `YYYY-MM-DD`.
pub fn parse_time(input: &str) -> Result<ParsedTime> {
    let s = input.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(ParsedTime::Absolute(dt.to_rfc3339()));
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(ParsedTime::Naive(
                dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
            ));
        }
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(ParsedTime::Date(date.format("%Y-%m-%d").to_string()));
    }
    anyhow::bail!(
        "Could not parse time '{}'. Use RFC 3339 (2026-10-05T19:00:00-07:00), local \
         YYYY-MM-DDTHH:MM with time_zone, or YYYY-MM-DD for all-day events.",
        input
    )
}

impl CalendarClient {
    pub fn new() -> Self {
        Self {
            http: crate::provider::shared_http_client(),
        }
    }

    pub fn is_configured(&self) -> bool {
        google::has_service(GoogleService::Calendar)
    }

    pub fn not_configured_message(&self) -> String {
        if google::has_tokens() {
            format!(
                "Google Calendar access has not been granted yet. Your Google login only covers \
                 other services. Ask the user to run `{}` (also enable the Google Calendar API \
                 at {} in their Google Cloud project), then retry.",
                google::login_command_adding(GoogleService::Calendar),
                GoogleService::Calendar.api_library_url()
            )
        } else {
            format!(
                "Google Calendar is not configured. Ask the user to run `jcode login google \
                 --google-services calendar` (enable the Google Calendar API at {} first), then retry.",
                GoogleService::Calendar.api_library_url()
            )
        }
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        let token = google::get_valid_token().await?;
        let url = format!("{}{}", API_BASE, path);
        let mut req = self
            .http
            .request(method, &url)
            .bearer_auth(&token)
            .query(query);
        if let Some(ref b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            let hint = if text.contains("accessNotConfigured") || text.contains("SERVICE_DISABLED")
            {
                format!(
                    "\nThe Google Calendar API is disabled for this OAuth project. Enable it at {} and retry.",
                    GoogleService::Calendar.api_library_url()
                )
            } else if status == reqwest::StatusCode::FORBIDDEN && text.contains("insufficient") {
                format!(
                    "\nThe saved Google login lacks Calendar permission. Run `{}`.",
                    google::login_command_adding(GoogleService::Calendar)
                )
            } else {
                String::new()
            };
            anyhow::bail!(
                "Google Calendar API error {}: {}{}",
                status,
                truncate(&text, 400),
                hint
            );
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn list_calendars(&self) -> Result<Vec<Value>> {
        let resp = self
            .request(
                reqwest::Method::GET,
                "/users/me/calendarList",
                &[("maxResults", "250".to_string())],
                None,
            )
            .await?;
        Ok(items(resp))
    }

    /// The calendar's default IANA time zone.
    pub async fn calendar_time_zone(&self, calendar_id: &str) -> Result<String> {
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/users/me/calendarList/{}", enc(calendar_id)),
                &[],
                None,
            )
            .await?;
        resp.get("timeZone")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("Calendar '{}' has no time zone", calendar_id))
    }

    pub async fn list_events(
        &self,
        calendar_id: &str,
        time_min: Option<&str>,
        time_max: Option<&str>,
        query: Option<&str>,
        max_results: u32,
    ) -> Result<Vec<Value>> {
        let mut params = vec![
            ("singleEvents", "true".to_string()),
            ("orderBy", "startTime".to_string()),
            ("maxResults", max_results.to_string()),
        ];
        if let Some(t) = time_min {
            params.push(("timeMin", t.to_string()));
        }
        if let Some(t) = time_max {
            params.push(("timeMax", t.to_string()));
        }
        if let Some(q) = query {
            params.push(("q", q.to_string()));
        }
        let resp = self
            .request(
                reqwest::Method::GET,
                &format!("/calendars/{}/events", enc(calendar_id)),
                &params,
                None,
            )
            .await?;
        Ok(items(resp))
    }

    pub async fn get_event(&self, calendar_id: &str, event_id: &str) -> Result<Value> {
        self.request(
            reqwest::Method::GET,
            &format!("/calendars/{}/events/{}", enc(calendar_id), enc(event_id)),
            &[],
            None,
        )
        .await
    }

    pub async fn create_event(
        &self,
        calendar_id: &str,
        event: Value,
        send_updates: &str,
    ) -> Result<Value> {
        self.request(
            reqwest::Method::POST,
            &format!("/calendars/{}/events", enc(calendar_id)),
            &[("sendUpdates", send_updates.to_string())],
            Some(event),
        )
        .await
    }

    pub async fn quick_add(&self, calendar_id: &str, text: &str) -> Result<Value> {
        self.request(
            reqwest::Method::POST,
            &format!("/calendars/{}/events/quickAdd", enc(calendar_id)),
            &[
                ("text", text.to_string()),
                ("sendUpdates", "none".to_string()),
            ],
            None,
        )
        .await
    }

    pub async fn patch_event(
        &self,
        calendar_id: &str,
        event_id: &str,
        patch: Value,
        send_updates: &str,
    ) -> Result<Value> {
        self.request(
            reqwest::Method::PATCH,
            &format!("/calendars/{}/events/{}", enc(calendar_id), enc(event_id)),
            &[("sendUpdates", send_updates.to_string())],
            Some(patch),
        )
        .await
    }

    pub async fn delete_event(
        &self,
        calendar_id: &str,
        event_id: &str,
        send_updates: &str,
    ) -> Result<()> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/calendars/{}/events/{}", enc(calendar_id), enc(event_id)),
            &[("sendUpdates", send_updates.to_string())],
            None,
        )
        .await?;
        Ok(())
    }
}

fn enc(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

fn items(resp: Value) -> Vec<Value> {
    resp.get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn truncate(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        trimmed.to_string()
    } else {
        let cut: String = trimmed.chars().take(max).collect();
        format!("{cut}…")
    }
}

fn time_field(event: &Value, key: &str) -> String {
    let Some(t) = event.get(key) else {
        return String::new();
    };
    if let Some(dt) = t.get("dateTime").and_then(Value::as_str) {
        return dt.to_string();
    }
    if let Some(d) = t.get("date").and_then(Value::as_str) {
        return format!("{d} (all day)");
    }
    String::new()
}

/// One-block summary of an event for tool output.
pub fn format_event(event: &Value) -> String {
    let s = |key: &str| event.get(key).and_then(Value::as_str).unwrap_or("");
    let mut lines = vec![
        if s("summary").is_empty() {
            "(no title)".to_string()
        } else {
            s("summary").to_string()
        },
        format!(
            "   When: {} -> {}",
            time_field(event, "start"),
            time_field(event, "end")
        ),
    ];
    if !s("location").is_empty() {
        lines.push(format!("   Where: {}", s("location")));
    }
    if let Some(attendees) = event.get("attendees").and_then(Value::as_array) {
        let list: Vec<String> = attendees
            .iter()
            .filter_map(|a| {
                let email = a.get("email").and_then(Value::as_str)?;
                let status = a
                    .get("responseStatus")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Some(if status.is_empty() {
                    email.to_string()
                } else {
                    format!("{email} ({status})")
                })
            })
            .collect();
        if !list.is_empty() {
            lines.push(format!("   Attendees: {}", list.join(", ")));
        }
    }
    if !s("description").is_empty() {
        lines.push(format!("   Notes: {}", truncate(s("description"), 300)));
    }
    if let Some(overrides) = event
        .get("reminders")
        .and_then(|r| r.get("overrides"))
        .and_then(Value::as_array)
        .filter(|o| !o.is_empty())
    {
        let list: Vec<String> = overrides
            .iter()
            .filter_map(|o| {
                Some(format!(
                    "{} {}m before",
                    o.get("method")?.as_str()?,
                    o.get("minutes")?.as_i64()?
                ))
            })
            .collect();
        lines.push(format!("   Reminders: {}", list.join(", ")));
    }
    if !s("status").is_empty() && s("status") != "confirmed" {
        lines.push(format!("   Status: {}", s("status")));
    }
    lines.push(format!("   ID: {}", s("id")));
    if !s("htmlLink").is_empty() {
        lines.push(format!("   Link: {}", s("htmlLink")));
    }
    lines.join("\n")
}

pub fn format_calendar(entry: &Value) -> String {
    let s = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or("");
    let primary = entry
        .get("primary")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    format!(
        "{}{}\n   ID: {}\n   Access: {}  Time zone: {}",
        s("summary"),
        if primary { " (primary)" } else { "" },
        s("id"),
        s("accessRole"),
        s("timeZone"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_classifies_inputs() {
        assert_eq!(
            parse_time("2026-10-05T19:00:00-07:00").unwrap(),
            ParsedTime::Absolute("2026-10-05T19:00:00-07:00".into())
        );
        assert_eq!(
            parse_time("2026-10-06T02:00:00Z").unwrap(),
            ParsedTime::Absolute("2026-10-06T02:00:00+00:00".into())
        );
        assert_eq!(
            parse_time("2026-10-05 19:00").unwrap(),
            ParsedTime::Naive("2026-10-05T19:00:00".into())
        );
        assert_eq!(
            parse_time("2026-10-05T07:30:15").unwrap(),
            ParsedTime::Naive("2026-10-05T07:30:15".into())
        );
        assert_eq!(
            parse_time("2026-10-05").unwrap(),
            ParsedTime::Date("2026-10-05".into())
        );
        assert!(parse_time("tomorrow at 7").is_err());
    }

    #[test]
    fn event_time_json_shapes() {
        assert_eq!(
            EventTime::Local {
                date_time: "2026-10-05T19:00:00".into(),
                time_zone: "America/Los_Angeles".into()
            }
            .to_json(),
            json!({"dateTime": "2026-10-05T19:00:00", "timeZone": "America/Los_Angeles"})
        );
        assert_eq!(
            EventTime::Date("2026-10-05".into()).to_json(),
            json!({"date": "2026-10-05"})
        );
    }

    #[test]
    fn format_event_includes_key_fields() {
        let event = json!({
            "id": "abc",
            "summary": "Respond to Getty",
            "start": {"dateTime": "2026-10-05T19:00:00-07:00"},
            "end": {"dateTime": "2026-10-05T19:30:00-07:00"},
            "attendees": [{"email": "a@example.com", "responseStatus": "accepted"}],
            "reminders": {"useDefault": false, "overrides": [{"method": "popup", "minutes": 0}]},
            "htmlLink": "https://calendar.google.com/x"
        });
        let out = format_event(&event);
        assert!(out.starts_with("Respond to Getty"));
        assert!(out.contains("2026-10-05T19:00:00-07:00 -> 2026-10-05T19:30:00-07:00"));
        assert!(out.contains("a@example.com (accepted)"));
        assert!(out.contains("popup 0m before"));
        assert!(out.contains("ID: abc"));

        let all_day =
            json!({"id": "d", "start": {"date": "2026-10-06"}, "end": {"date": "2026-10-07"}});
        let out = format_event(&all_day);
        assert!(out.starts_with("(no title)"));
        assert!(out.contains("2026-10-06 (all day)"));
    }
}
