//! ZCode Off-Peak (Idle) plan — ticket-queue access to off-peak compute.
//!
//! Protocol facts verified against the official ZCode client and the
//! independent reference implementation
//! https://github.com/TriDefender/zcode-api (protocol knowledge only — no
//! code copied; that project is CC BY-NC-SA 4.0).
//!
//! Control plane (`{origin}/api/v1/off-peak/*`, headers
//! `Authorization: Bearer {jwt}` + `x-coding-plan-api-key: {key}`):
//! - `GET  /ticket/availability` — `{can_take_number, next_take_at}`
//! - `POST /ticket` body `{task_id}` — `{ticket_id, state, position,
//!   next_poll_after}`
//! - `POST /ticket/status` body `{ticket_ids: []}` (max 100) —
//!   `{next_poll_after, tickets: [{ticket_id, state, position,
//!   active_deadline}]}`
//! - `POST /ticket/{id}/settle` — close-out; 4xx resolves as success
//!   (server already cleaned up).
//!
//! Ticket lifecycle: queued → ready (use before `active_deadline`) → active
//! → settled; expired / not_found are terminal.
//!
//! LLM plane: when a ticket is ready, the request goes to
//! `{origin}/api/v1/off-peak/anthropic/v1/messages` with
//! `authorization: Bearer {jwt}`, `x-coding-plan-api-key: {key}`, and
//! `x-off-peak-ticket-id: {ticketId}`.

use crate::auth::zcode::load_credentials;
use anyhow::Context;
use std::time::Duration;

pub const OFF_PEAK_ORIGIN: &str = "https://zcode.z.ai";
pub const OFF_PEAK_LLM_URL: &str =
    "https://zcode.z.ai/api/v1/off-peak/anthropic/v1/messages";
const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BATCH_STATUS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketState {
    Queued,
    Ready,
    Active,
    Settled,
    Expired,
    NotFound,
}

impl TicketState {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Queued),
            "ready" => Some(Self::Ready),
            "active" => Some(Self::Active),
            "settled" => Some(Self::Settled),
            "expired" => Some(Self::Expired),
            "not_found" => Some(Self::NotFound),
            _ => None,
        }
    }

    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready | Self::Active)
    }

    pub fn is_expired(self) -> bool {
        matches!(self, Self::Expired | Self::NotFound)
    }
}

#[derive(Debug, Clone)]
pub struct OffPeakCredentials {
    pub jwt: String,
    pub coding_plan_api_key: String,
}

impl OffPeakCredentials {
    pub fn from_credentials() -> Option<Self> {
        let creds = load_credentials().ok()?;
        let jwt = creds.token.trim().to_string();
        let key = creds.coding_plan_api_key.as_deref()?.trim().to_string();
        if jwt.is_empty() || key.is_empty() {
            return None;
        }
        Some(Self {
            jwt,
            coding_plan_api_key: key,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Availability {
    pub can_take_number: bool,
    pub next_take_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TakeTicketResult {
    pub ticket_id: String,
    pub state: TicketState,
    pub position: Option<u64>,
    pub next_poll_after_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TicketStatus {
    pub ticket_id: String,
    pub state: TicketState,
    pub position: Option<u64>,
    /// Unix seconds — deadline by which a `ready` ticket must be used.
    pub active_deadline: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct BatchStatus {
    pub tickets: Vec<TicketStatus>,
    pub next_poll_after_ms: Option<u64>,
}

fn base_headers(creds: &OffPeakCredentials, has_body: bool) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in [
        ("authorization", format!("Bearer {}", creds.jwt)),
        ("x-coding-plan-api-key", creds.coding_plan_api_key.clone()),
    ] {
        if let (Ok(name), Ok(val)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(&value),
        ) {
            headers.insert(name, val);
        }
    }
    if has_body {
        headers.insert(
            reqwest::header::HeaderName::from_static("content-type"),
            reqwest::header::HeaderValue::from_static("application/json"),
        );
    }
    headers
}

async fn unwrap_data(resp: reqwest::Response) -> anyhow::Result<serde_json::Value> {
    let status = resp.status().as_u16();
    let json: serde_json::Value = resp.json().await.context("invalid JSON from off-peak API")?;
    let code = json.get("code").and_then(|v| v.as_i64());
    if status < 200 || status >= 300 || (code.is_some() && code != Some(0)) {
        anyhow::bail!(
            "off-peak request failed (HTTP {status}, code {:?}): {}",
            code,
            json.get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
        );
    }
    Ok(json.get("data").cloned().unwrap_or(serde_json::Value::Null))
}

/// Probe whether a queue number can currently be taken.
pub async fn availability(
    client: &reqwest::Client,
    creds: &OffPeakCredentials,
) -> anyhow::Result<Availability> {
    let url = format!("{OFF_PEAK_ORIGIN}/api/v1/off-peak/ticket/availability");
    let resp = client
        .get(&url)
        .timeout(CONTROL_TIMEOUT)
        .headers(base_headers(creds, false))
        .send()
        .await
        .context("off-peak availability request failed")?;
    let data = unwrap_data(resp).await?;
    let can_take_number = data.get("can_take_number").and_then(|v| v.as_bool()) == Some(true);
    let next_take_at = data.get("next_take_at").and_then(|v| v.as_u64());
    if !can_take_number && next_take_at.is_none() {
        anyhow::bail!("off-peak availability missing next_take_at while unavailable");
    }
    Ok(Availability {
        can_take_number,
        next_take_at,
    })
}

/// Take a queue number for a task.
pub async fn take_ticket(
    client: &reqwest::Client,
    creds: &OffPeakCredentials,
    task_id: &str,
) -> anyhow::Result<TakeTicketResult> {
    anyhow::ensure!(!task_id.trim().is_empty(), "task_id must be non-empty");
    let url = format!("{OFF_PEAK_ORIGIN}/api/v1/off-peak/ticket");
    let resp = client
        .post(&url)
        .timeout(CONTROL_TIMEOUT)
        .headers(base_headers(creds, true))
        .json(&serde_json::json!({ "task_id": task_id }))
        .send()
        .await
        .context("off-peak takeTicket request failed")?;
    let data = unwrap_data(resp).await?;
    let ticket_id = data
        .get("ticket_id")
        .and_then(|v| v.as_str())
        .context("off-peak takeTicket missing ticket_id")?
        .to_string();
    let state_str = data
        .get("state")
        .and_then(|v| v.as_str())
        .context("off-peak takeTicket missing state")?;
    let state = TicketState::parse(state_str)
        .with_context(|| format!("off-peak takeTicket unknown state {state_str}"))?;
    Ok(TakeTicketResult {
        ticket_id,
        state,
        position: data.get("position").and_then(|v| v.as_u64()),
        next_poll_after_ms: data.get("next_poll_after").and_then(|v| v.as_u64()),
    })
}

/// Batch poll ticket states (server cap 100 ids).
pub async fn batch_status(
    client: &reqwest::Client,
    creds: &OffPeakCredentials,
    ticket_ids: &[String],
) -> anyhow::Result<BatchStatus> {
    if ticket_ids.is_empty() {
        return Ok(BatchStatus {
            tickets: Vec::new(),
            next_poll_after_ms: None,
        });
    }
    let truncated: Vec<&String> = ticket_ids.iter().take(MAX_BATCH_STATUS).collect();
    let url = format!("{OFF_PEAK_ORIGIN}/api/v1/off-peak/ticket/status");
    let resp = client
        .post(&url)
        .timeout(CONTROL_TIMEOUT)
        .headers(base_headers(creds, true))
        .json(&serde_json::json!({ "ticket_ids": truncated }))
        .send()
        .await
        .context("off-peak batchStatus request failed")?;
    let data = unwrap_data(resp).await?;
    let tickets = data
        .get("tickets")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::with_capacity(tickets.len());
    for t in tickets {
        let ticket_id = t
            .get("ticket_id")
            .and_then(|v| v.as_str())
            .context("off-peak batchStatus ticket missing ticket_id")?
            .to_string();
        let state_str = t
            .get("state")
            .and_then(|v| v.as_str())
            .context("off-peak batchStatus ticket missing state")?;
        let state = TicketState::parse(state_str)
            .with_context(|| format!("off-peak batchStatus unknown state {state_str}"))?;
        out.push(TicketStatus {
            ticket_id,
            state,
            position: t.get("position").and_then(|v| v.as_u64()),
            active_deadline: t.get("active_deadline").and_then(|v| v.as_u64()),
        });
    }
    Ok(BatchStatus {
        tickets: out,
        next_poll_after_ms: data.get("next_poll_after").and_then(|v| v.as_u64()),
    })
}

/// Settle a ticket. 4xx resolves as success (server already cleaned up).
pub async fn settle_ticket(
    client: &reqwest::Client,
    creds: &OffPeakCredentials,
    ticket_id: &str,
) -> anyhow::Result<()> {
    let url = format!("{OFF_PEAK_ORIGIN}/api/v1/off-peak/ticket/{ticket_id}/settle");
    let resp = client
        .post(&url)
        .timeout(CONTROL_TIMEOUT)
        .headers(base_headers(creds, false))
        .send()
        .await
        .context("off-peak settle request failed")?;
    let status = resp.status().as_u16();
    if (400..500).contains(&status) || status == 200 {
        return Ok(());
    }
    if (500..600).contains(&status) {
        anyhow::bail!("off-peak settle failed (HTTP {status})");
    }
    let json: serde_json::Value = resp.json().await.context("invalid JSON from off-peak settle")?;
    let code = json.get("code").and_then(|v| v.as_i64());
    if code.is_some() && code != Some(0) {
        anyhow::bail!(
            "off-peak settle biz error ({}): {}",
            code.unwrap_or_default(),
            json.get("msg").and_then(|v| v.as_str()).unwrap_or("unknown"),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_state_parsing() {
        assert!(matches!(TicketState::parse("queued"), Some(TicketState::Queued)));
        assert!(matches!(TicketState::parse("ready"), Some(TicketState::Ready)));
        assert!(TicketState::parse("ready").unwrap().is_ready());
        assert!(TicketState::parse("active").unwrap().is_ready());
        assert!(TicketState::parse("expired").unwrap().is_expired());
        assert!(TicketState::parse("not_found").unwrap().is_expired());
        assert!(TicketState::parse("bogus").is_none());
    }

    #[test]
    fn offpeak_urls_shape() {
        assert_eq!(
            OFF_PEAK_LLM_URL,
            "https://zcode.z.ai/api/v1/off-peak/anthropic/v1/messages"
        );
    }
}
