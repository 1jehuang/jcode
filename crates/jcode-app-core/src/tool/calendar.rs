use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::google_calendar::{
    self, CalendarClient, EventTime, ParsedTime, format_calendar, format_event,
};

const DEFAULT_EVENT_MINUTES: i64 = 30;

pub struct CalendarTool {
    client: CalendarClient,
}

impl CalendarTool {
    pub fn new() -> Self {
        Self {
            client: CalendarClient::new(),
        }
    }
}

#[derive(Deserialize, Default)]
struct CalendarInput {
    action: String,
    #[serde(default)]
    calendar_id: Option<String>,
    #[serde(default)]
    event_id: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    start: Option<String>,
    #[serde(default)]
    end: Option<String>,
    #[serde(default)]
    time_zone: Option<String>,
    #[serde(default)]
    attendees: Option<Vec<String>>,
    #[serde(default)]
    reminder_minutes: Option<Vec<i64>>,
    #[serde(default)]
    send_updates: Option<String>,
    #[serde(default)]
    time_min: Option<String>,
    #[serde(default)]
    time_max: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    max_results: Option<u32>,
    #[serde(default)]
    confirmed: Option<bool>,
}

/// A naive time plus a resolved time zone, or an absolute/all-day time.
fn to_event_time(parsed: &ParsedTime, time_zone: Option<&str>) -> Result<EventTime> {
    Ok(match parsed {
        ParsedTime::Absolute(dt) => EventTime::Absolute(dt.clone()),
        ParsedTime::Date(d) => EventTime::Date(d.clone()),
        ParsedTime::Naive(dt) => EventTime::Local {
            date_time: dt.clone(),
            time_zone: time_zone
                .ok_or_else(|| anyhow::anyhow!("time_zone is required for local times"))?
                .to_string(),
        },
    })
}

/// Default end: 30 minutes after a timed start, or the next day for all-day.
fn default_end(start: &ParsedTime) -> Result<ParsedTime> {
    Ok(match start {
        ParsedTime::Absolute(dt) => {
            let dt = chrono::DateTime::parse_from_rfc3339(dt)?;
            ParsedTime::Absolute(
                (dt + chrono::Duration::minutes(DEFAULT_EVENT_MINUTES)).to_rfc3339(),
            )
        }
        ParsedTime::Naive(dt) => {
            let dt = chrono::NaiveDateTime::parse_from_str(dt, "%Y-%m-%dT%H:%M:%S")?;
            ParsedTime::Naive(
                (dt + chrono::Duration::minutes(DEFAULT_EVENT_MINUTES))
                    .format("%Y-%m-%dT%H:%M:%S")
                    .to_string(),
            )
        }
        ParsedTime::Date(d) => {
            let d = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")?;
            ParsedTime::Date(
                (d + chrono::Duration::days(1))
                    .format("%Y-%m-%d")
                    .to_string(),
            )
        }
    })
}

/// List-window bounds must be RFC 3339. Naive inputs are read in the
/// machine's local time zone; dates mean local midnight.
fn to_rfc3339_bound(input: &str) -> Result<String> {
    use chrono::TimeZone;
    let local = |naive: chrono::NaiveDateTime| -> Result<String> {
        chrono::Local
            .from_local_datetime(&naive)
            .earliest()
            .map(|dt| dt.to_rfc3339())
            .ok_or_else(|| anyhow::anyhow!("'{}' is not a valid local time", input))
    };
    match google_calendar::parse_time(input)? {
        ParsedTime::Absolute(dt) => Ok(dt),
        ParsedTime::Naive(dt) => local(chrono::NaiveDateTime::parse_from_str(
            &dt,
            "%Y-%m-%dT%H:%M:%S",
        )?),
        ParsedTime::Date(d) => local(
            chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d")?
                .and_hms_opt(0, 0, 0)
                .expect("midnight is valid"),
        ),
    }
}

fn validate_send_updates(value: Option<&str>) -> Result<&'static str> {
    match value.unwrap_or("none") {
        "none" => Ok("none"),
        "all" => Ok("all"),
        "externalOnly" | "external_only" => Ok("externalOnly"),
        other => anyhow::bail!(
            "Invalid send_updates '{}'. Use none, all, or externalOnly.",
            other
        ),
    }
}

fn reminders_json(minutes: &[i64]) -> Value {
    json!({
        "useDefault": false,
        "overrides": minutes
            .iter()
            .map(|m| json!({ "method": "popup", "minutes": (*m).clamp(0, 40320) }))
            .collect::<Vec<_>>(),
    })
}

fn attendees_json(emails: &[String]) -> Value {
    Value::Array(
        emails
            .iter()
            .map(|e| e.trim())
            .filter(|e| !e.is_empty())
            .map(|e| json!({ "email": e }))
            .collect(),
    )
}

impl CalendarTool {
    /// Resolve the time zone for naive times: explicit param, else the
    /// calendar's own default zone.
    async fn resolve_time_zone(
        &self,
        params: &CalendarInput,
        calendar_id: &str,
        times: &[&ParsedTime],
    ) -> Result<Option<String>> {
        if let Some(tz) = params.time_zone.as_deref().filter(|t| !t.trim().is_empty()) {
            return Ok(Some(tz.trim().to_string()));
        }
        if times.iter().any(|t| matches!(t, ParsedTime::Naive(_))) {
            return self
                .client
                .calendar_time_zone(calendar_id)
                .await
                .map(Some)
                .context("Could not look up the calendar's time zone; pass time_zone explicitly");
        }
        Ok(None)
    }

    /// Fields shared by create and update. Only supplied fields are set.
    async fn event_body(
        &self,
        params: &CalendarInput,
        calendar_id: &str,
        require_times: bool,
    ) -> Result<Map<String, Value>> {
        let mut body = Map::new();
        if let Some(v) = &params.summary {
            body.insert("summary".into(), json!(v));
        }
        if let Some(v) = &params.description {
            body.insert("description".into(), json!(v));
        }
        if let Some(v) = &params.location {
            body.insert("location".into(), json!(v));
        }
        if let Some(v) = &params.attendees {
            body.insert("attendees".into(), attendees_json(v));
        }
        if let Some(v) = &params.reminder_minutes {
            body.insert("reminders".into(), reminders_json(v));
        }

        let start = params
            .start
            .as_deref()
            .map(google_calendar::parse_time)
            .transpose()?;
        let end = params
            .end
            .as_deref()
            .map(google_calendar::parse_time)
            .transpose()?;
        if require_times && start.is_none() {
            anyhow::bail!("start is required for create");
        }
        if start.is_none() && end.is_some() {
            anyhow::bail!("Pass start together with end when changing an event's time");
        }
        if let Some(start) = start {
            let end = match end {
                Some(end) => end,
                None => default_end(&start)?,
            };
            if matches!(start, ParsedTime::Date(_)) != matches!(end, ParsedTime::Date(_)) {
                anyhow::bail!("start and end must both be dates (all-day) or both be times");
            }
            let tz = self
                .resolve_time_zone(params, calendar_id, &[&start, &end])
                .await?;
            body.insert(
                "start".into(),
                to_event_time(&start, tz.as_deref())?.to_json(),
            );
            body.insert("end".into(), to_event_time(&end, tz.as_deref())?.to_json());
        }
        Ok(body)
    }
}

#[async_trait]
impl Tool for CalendarTool {
    fn name(&self) -> &str {
        "calendar"
    }

    fn description(&self) -> &str {
        "Use Google Calendar: list calendars, view, create, update, and delete events."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["calendars", "list", "get", "create", "quick_add", "update", "delete"],
                    "description": "Action. list defaults to the next 7 days. quick_add parses text like 'Lunch tomorrow 1pm'."
                },
                "calendar_id": {
                    "type": "string",
                    "description": "Calendar ID from 'calendars'. Default: primary."
                },
                "event_id": { "type": "string" },
                "summary": { "type": "string", "description": "Event title." },
                "description": { "type": "string" },
                "location": { "type": "string" },
                "start": {
                    "type": "string",
                    "description": "RFC 3339, local YYYY-MM-DDTHH:MM (time_zone or calendar zone), or YYYY-MM-DD all-day."
                },
                "end": {
                    "type": "string",
                    "description": "Same formats as start. Default: 30 minutes after start, or the next day for all-day."
                },
                "time_zone": {
                    "type": "string",
                    "description": "IANA zone for local start/end, e.g. America/Los_Angeles."
                },
                "attendees": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Guest emails. Replaces the guest list on update."
                },
                "reminder_minutes": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "Popup reminders, minutes before start. 0 = at start. [] = no reminders. Omit for calendar defaults."
                },
                "send_updates": {
                    "type": "string",
                    "enum": ["none", "all", "externalOnly"],
                    "description": "Email guests about the change. Default none. Anything else needs confirmed: true."
                },
                "time_min": { "type": "string", "description": "List window start. Default now." },
                "time_max": { "type": "string", "description": "List window end. Default 7 days after time_min." },
                "query": { "type": "string", "description": "Free-text search for list." },
                "text": { "type": "string", "description": "Text for quick_add." },
                "max_results": { "type": "integer" },
                "confirmed": {
                    "type": "boolean",
                    "description": "Confirm delete or emailing guests."
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: CalendarInput = serde_json::from_value(input)?;

        if !self.client.is_configured() {
            return Ok(ToolOutput::new(self.client.not_configured_message()));
        }

        let calendar_id = params
            .calendar_id
            .clone()
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| "primary".to_string());
        let event_id = || {
            params
                .event_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("event_id is required for {}", params.action))
        };

        match params.action.as_str() {
            "calendars" => {
                let calendars = self.client.list_calendars().await?;
                if calendars.is_empty() {
                    return Ok(ToolOutput::new("No calendars found."));
                }
                let lines: Vec<String> = calendars
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}. {}", i + 1, format_calendar(c)))
                    .collect();
                Ok(ToolOutput::new(format!(
                    "Calendars ({}):\n\n{}",
                    calendars.len(),
                    lines.join("\n\n")
                )))
            }

            "list" | "search" => {
                let max = params.max_results.unwrap_or(25).clamp(1, 250);
                let time_min = match params.time_min.as_deref() {
                    Some(t) => to_rfc3339_bound(t)?,
                    None => chrono::Local::now().to_rfc3339(),
                };
                let time_max = match params.time_max.as_deref() {
                    Some(t) => to_rfc3339_bound(t)?,
                    None => (chrono::DateTime::parse_from_rfc3339(&time_min)?
                        + chrono::Duration::days(7))
                    .to_rfc3339(),
                };
                let events = self
                    .client
                    .list_events(
                        &calendar_id,
                        Some(&time_min),
                        Some(&time_max),
                        params.query.as_deref(),
                        max,
                    )
                    .await?;
                if events.is_empty() {
                    return Ok(ToolOutput::new(format!(
                        "No events between {} and {}.",
                        time_min, time_max
                    )));
                }
                let lines: Vec<String> = events
                    .iter()
                    .enumerate()
                    .map(|(i, e)| format!("{}. {}", i + 1, format_event(e)))
                    .collect();
                Ok(ToolOutput::new(format!(
                    "Events on {} from {} to {} ({}):\n\n{}",
                    calendar_id,
                    time_min,
                    time_max,
                    events.len(),
                    lines.join("\n\n")
                )))
            }

            "get" => {
                let event = self.client.get_event(&calendar_id, event_id()?).await?;
                Ok(ToolOutput::new(format_event(&event)))
            }

            "create" => {
                let send_updates = validate_send_updates(params.send_updates.as_deref())?;
                let body = self.event_body(&params, &calendar_id, true).await?;
                if send_updates != "none" && params.confirmed != Some(true) {
                    return Ok(ToolOutput::new(format!(
                        "Confirmation required: creating this event will email invitations ({}).\n\
                         Event: {}\n\
                         To confirm, call calendar again with the same parameters and confirmed: true. \
                         Or use send_updates: none to add guests without emailing them.",
                        send_updates,
                        serde_json::to_string_pretty(&Value::Object(body))?
                    )));
                }
                let event = self
                    .client
                    .create_event(&calendar_id, Value::Object(body), send_updates)
                    .await?;
                Ok(ToolOutput::new(format!(
                    "Event created.\n{}",
                    format_event(&event)
                )))
            }

            "quick_add" => {
                let text = params
                    .text
                    .as_deref()
                    .or(params.summary.as_deref())
                    .filter(|t| !t.trim().is_empty())
                    .ok_or_else(|| anyhow::anyhow!("text is required for quick_add"))?;
                let event = self.client.quick_add(&calendar_id, text).await?;
                Ok(ToolOutput::new(format!(
                    "Event created from text. Check the parsed time below.\n{}",
                    format_event(&event)
                )))
            }

            "update" => {
                let id = event_id()?;
                let send_updates = validate_send_updates(params.send_updates.as_deref())?;
                let body = self.event_body(&params, &calendar_id, false).await?;
                if body.is_empty() {
                    anyhow::bail!(
                        "Nothing to update. Pass summary, description, location, start/end, attendees, or reminder_minutes."
                    );
                }
                if send_updates != "none" && params.confirmed != Some(true) {
                    return Ok(ToolOutput::new(format!(
                        "Confirmation required: this update will email guests ({}).\n\
                         Changes: {}\n\
                         To confirm, call calendar again with the same parameters and confirmed: true.",
                        send_updates,
                        serde_json::to_string_pretty(&Value::Object(body))?
                    )));
                }
                let event = self
                    .client
                    .patch_event(&calendar_id, id, Value::Object(body), send_updates)
                    .await?;
                Ok(ToolOutput::new(format!(
                    "Event updated.\n{}",
                    format_event(&event)
                )))
            }

            "delete" => {
                let id = event_id()?;
                let send_updates = validate_send_updates(params.send_updates.as_deref())?;
                if params.confirmed != Some(true) {
                    let preview = match self.client.get_event(&calendar_id, id).await {
                        Ok(event) => format_event(&event),
                        Err(e) => format!("(could not load event {}: {})", id, e),
                    };
                    return Ok(ToolOutput::new(format!(
                        "Confirmation required to delete this event:\n{}\n\n\
                         To confirm, call calendar again with action 'delete', the same event_id, and confirmed: true.",
                        preview
                    )));
                }
                self.client
                    .delete_event(&calendar_id, id, send_updates)
                    .await?;
                Ok(ToolOutput::new(format!("Event {} deleted.", id)))
            }

            other => Ok(ToolOutput::new(format!(
                "Unknown calendar action '{}'. Use calendars, list, get, create, quick_add, update, or delete.",
                other
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_end_adds_thirty_minutes_or_one_day() {
        assert_eq!(
            default_end(&ParsedTime::Naive("2026-10-05T23:45:00".into())).unwrap(),
            ParsedTime::Naive("2026-10-06T00:15:00".into())
        );
        assert_eq!(
            default_end(&ParsedTime::Absolute("2026-10-05T19:00:00-07:00".into())).unwrap(),
            ParsedTime::Absolute("2026-10-05T19:30:00-07:00".into())
        );
        assert_eq!(
            default_end(&ParsedTime::Date("2026-12-31".into())).unwrap(),
            ParsedTime::Date("2027-01-01".into())
        );
    }

    #[test]
    fn naive_times_need_a_zone() {
        let naive = ParsedTime::Naive("2026-10-05T19:00:00".into());
        assert!(to_event_time(&naive, None).is_err());
        assert_eq!(
            to_event_time(&naive, Some("America/Los_Angeles")).unwrap(),
            EventTime::Local {
                date_time: "2026-10-05T19:00:00".into(),
                time_zone: "America/Los_Angeles".into()
            }
        );
    }

    #[test]
    fn send_updates_defaults_to_none_and_rejects_unknown() {
        assert_eq!(validate_send_updates(None).unwrap(), "none");
        assert_eq!(
            validate_send_updates(Some("external_only")).unwrap(),
            "externalOnly"
        );
        assert!(validate_send_updates(Some("everyone")).is_err());
    }

    #[test]
    fn reminders_override_defaults() {
        assert_eq!(
            reminders_json(&[0, 10]),
            json!({"useDefault": false, "overrides": [
                {"method": "popup", "minutes": 0},
                {"method": "popup", "minutes": 10}
            ]})
        );
        assert_eq!(
            reminders_json(&[]),
            json!({"useDefault": false, "overrides": []})
        );
    }

    #[test]
    fn list_bounds_accept_absolute_and_local_inputs() {
        assert_eq!(
            to_rfc3339_bound("2026-10-05T19:00:00-07:00").unwrap(),
            "2026-10-05T19:00:00-07:00"
        );
        let local = to_rfc3339_bound("2026-10-05").unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(&local).is_ok());
        assert!(local.starts_with("2026-10-05T00:00:00"));
    }

    #[tokio::test]
    async fn create_body_rejects_mixed_all_day_and_timed() {
        let tool = CalendarTool::new();
        let params = CalendarInput {
            action: "create".into(),
            start: Some("2026-10-05".into()),
            end: Some("2026-10-05T19:00:00-07:00".into()),
            ..Default::default()
        };
        let err = tool
            .event_body(&params, "primary", true)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("both be dates"), "{err}");
    }

    #[tokio::test]
    async fn create_body_with_offset_needs_no_zone_lookup() {
        let tool = CalendarTool::new();
        let params = CalendarInput {
            action: "create".into(),
            summary: Some("Respond to Getty".into()),
            start: Some("2026-10-05T19:00:00-07:00".into()),
            reminder_minutes: Some(vec![0]),
            ..Default::default()
        };
        let body = tool.event_body(&params, "primary", true).await.unwrap();
        assert_eq!(body["summary"], json!("Respond to Getty"));
        assert_eq!(
            body["start"],
            json!({"dateTime": "2026-10-05T19:00:00-07:00"})
        );
        assert_eq!(
            body["end"],
            json!({"dateTime": "2026-10-05T19:30:00-07:00"})
        );
        assert_eq!(body["reminders"]["overrides"][0]["minutes"], json!(0));
    }
}
