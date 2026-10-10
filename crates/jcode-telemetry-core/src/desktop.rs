//! Jcode Desktop fleet telemetry.
//!
//! Desktop links this crate via `jcode_base::telemetry` and calls these entry
//! points so we can measure whether Desktop auto-updates reach real users.
//! Payloads never include error messages or filesystem paths.

use super::state_support::{
    get_or_create_id, new_event_id, telemetry_state_path, write_private_file,
};
use super::{DeliveryMode, is_enabled, send_payload, telemetry_envelope};
use serde_json::{Value, json};

const ACTIVE_RECORDED_FILE: &str = "desktop_active_recorded";
const VERSION_RECORDED_FILE: &str = "desktop_version_recorded";
const MAX_TOKEN_LEN: usize = 64;

fn read_state(name: &str) -> Option<String> {
    telemetry_state_path(name)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn write_state(name: &str, value: &str) {
    if let Some(path) = telemetry_state_path(name) {
        write_private_file(&path, value);
    }
}

/// Reduce a free-form value to a short `[a-z0-9_]` token. Anything else
/// (spaces, slashes, punctuation) collapses to `_`, so paths and messages
/// cannot leak through.
pub(super) fn sanitize_token(value: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars().flat_map(char::to_lowercase) {
        if out.len() >= MAX_TOKEN_LEN {
            break;
        }
        let mapped = if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            ch
        } else {
            '_'
        };
        if mapped == '_' && out.ends_with('_') {
            continue;
        }
        out.push(mapped);
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Version strings are short and structured; keep them bounded and printable.
fn sanitize_version(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
        .take(MAX_TOKEN_LEN)
        .collect()
}

fn base_payload(id: String, event: &str, version: &str) -> Value {
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    json!({
        "event_id": new_event_id(),
        "id": id,
        "event": event,
        "version": version,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "schema_version": schema_version,
        "build_channel": build_channel,
        "is_git_checkout": git_checkout,
        "is_ci": ci,
        "ran_from_cargo": from_cargo,
    })
}

/// Report that Desktop `desktop_version` is running. Sends `desktop_active` at
/// most once per UTC day per version, plus `desktop_upgrade` when the version
/// changed since the last observation. Never blocks.
pub fn record_desktop_active(desktop_version: &str) {
    record_desktop_active_on(
        desktop_version,
        &chrono::Utc::now().format("%Y-%m-%d").to_string(),
    );
}

pub(super) fn record_desktop_active_on(desktop_version: &str, utc_date: &str) {
    if !is_enabled() {
        return;
    }
    let version = sanitize_version(desktop_version);
    if version.is_empty() {
        return;
    }
    let Some(id) = get_or_create_id() else {
        return;
    };

    let previous = read_state(VERSION_RECORDED_FILE);
    if let Some(previous) = previous.as_deref()
        && previous != version
    {
        let mut payload = base_payload(id.clone(), "desktop_upgrade", &version);
        payload["from_version"] = json!(previous);
        let _ = send_payload(payload, DeliveryMode::Background);
    }
    if previous.as_deref() != Some(version.as_str()) {
        write_state(VERSION_RECORDED_FILE, &version);
    }

    let marker = format!("{utc_date}|{version}");
    if read_state(ACTIVE_RECORDED_FILE).as_deref() == Some(marker.as_str()) {
        return;
    }
    let payload = base_payload(id, "desktop_active", &version);
    if send_payload(payload, DeliveryMode::Background) {
        write_state(ACTIVE_RECORDED_FILE, &marker);
    }
}

/// Report the outcome of a Desktop update attempt. `outcome` is `success`,
/// `failure`, or `up_to_date`. `install_kind` and `failure_stage` are reduced
/// to short `[a-z0-9_]` tokens. Never blocks.
pub fn record_desktop_update(
    from_version: &str,
    to_version: &str,
    install_kind: &str,
    outcome: &str,
    failure_stage: Option<&str>,
) {
    if !is_enabled() {
        return;
    }
    let Some(id) = get_or_create_id() else {
        return;
    };
    let mut payload = base_payload(id, "desktop_update", &sanitize_version(to_version));
    payload["from_version"] = json!(sanitize_version(from_version));
    payload["install_kind"] = json!(sanitize_token(install_kind));
    payload["update_outcome"] = json!(sanitize_token(outcome));
    payload["update_failure_stage"] = match failure_stage {
        Some(stage) => json!(sanitize_token(stage)),
        None => Value::Null,
    };
    let _ = send_payload(payload, DeliveryMode::Background);
}
