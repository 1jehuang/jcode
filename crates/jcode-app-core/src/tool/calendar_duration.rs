//! Duration-preserving start-only updates. All-day durations are exclusive-end
//! date spans; timed durations are elapsed instants, including across DST.
use super::*;
use chrono::{DateTime, FixedOffset, NaiveDate};

impl CalendarTool {
    pub(super) async fn preserved_times(
        &self,
        params: &CalendarInput,
        calendar_id: &str,
        start: &ParsedTime,
    ) -> Result<(Value, Value)> {
        let id = params
            .event_id
            .as_deref()
            .context("event_id is required for update")?;
        let existing = self
            .client
            .get_event(calendar_id, id)
            .await
            .context("Could not read existing event duration; pass end explicitly")?;
        anyhow::ensure!(
            existing.get("endTimeUnspecified").and_then(Value::as_bool) != Some(true),
            "Existing event has no specified duration; pass end explicitly"
        );
        let old_start = existing
            .get("start")
            .context("Existing event has no start; pass end explicitly")?;
        let old_end = existing
            .get("end")
            .context("Existing event has no end; pass end explicitly")?;
        if let ParsedTime::Date(new_date) = start {
            let date = |value: &Value| -> Result<NaiveDate> {
                Ok(NaiveDate::parse_from_str(
                    value
                        .get("date")
                        .and_then(Value::as_str)
                        .context("Cannot change timed/all-day kind without an explicit end")?,
                    "%Y-%m-%d",
                )?)
            };
            let duration = date(old_end)? - date(old_start)?;
            anyhow::ensure!(
                duration > chrono::Duration::zero(),
                "Existing event has invalid duration; pass end explicitly"
            );
            let new_start = NaiveDate::parse_from_str(new_date, "%Y-%m-%d")?;
            let new_end = new_start
                .checked_add_signed(duration)
                .context("New event end is out of range")?;
            return Ok((
                json!({"date": new_date}),
                json!({"date": new_end.format("%Y-%m-%d").to_string()}),
            ));
        }
        let zone = params
            .time_zone
            .as_deref()
            .filter(|z| !z.trim().is_empty())
            .or_else(|| old_start.get("timeZone").and_then(Value::as_str))
            .map(|z| z.trim().to_string());
        let old_instant = |value: &Value| -> Result<DateTime<FixedOffset>> {
            let time = value
                .get("dateTime")
                .and_then(Value::as_str)
                .context("Cannot change timed/all-day kind without an explicit end")?;
            if let Ok(instant) = DateTime::parse_from_rfc3339(time) {
                return Ok(instant);
            }
            let zone = value
                .get("timeZone")
                .and_then(Value::as_str)
                .context("Existing event time has no offset or zone; pass end explicitly")?;
            zone::local_in_zone(time, zone)
        };
        let duration = old_instant(old_end)? - old_instant(old_start)?;
        anyhow::ensure!(
            duration > chrono::Duration::zero(),
            "Existing event has invalid duration; pass end explicitly"
        );
        let (instant, zone) = match start {
            ParsedTime::Absolute(dt) => (DateTime::parse_from_rfc3339(dt)?, zone),
            ParsedTime::Naive(dt) => {
                let zone = match zone {
                    Some(zone) => zone,
                    None => self.client.calendar_time_zone(calendar_id).await?,
                };
                (zone::local_in_zone(dt, &zone)?, Some(zone))
            }
            ParsedTime::Date(_) => anyhow::bail!("Expected a timed start"),
        };
        let end = instant
            .checked_add_signed(duration)
            .context("New event end is out of range")?;
        // UTC offsets make the two instants unambiguous. Retain the IANA zone
        // independently (important for recurring events); do not reuse a stale
        // winter/summer offset at the new end.
        let mut start_json = json!({"dateTime": instant.to_utc().to_rfc3339()});
        let mut end_json = json!({"dateTime": end.to_utc().to_rfc3339()});
        if let Some(zone) = zone {
            start_json["timeZone"] = json!(zone);
            end_json["timeZone"] = json!(zone);
        }
        Ok((start_json, end_json))
    }
}

#[path = "calendar_zone.rs"]
mod zone;
