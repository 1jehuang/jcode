//! Fable earned resets using Claude Code 2.1.322's `cedar_ember` contract.
//! See docs/anthropic-fable-resets.md for primary-source evidence.

use super::anthropic_reset::{authorize, fresh_access_token, get_json, reset_client};
use anyhow::{Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Write;

const BASE: &str = "https://api.anthropic.com";
const STATUS: &str = "/api/oauth/usage?cedar_ember=1&skip_spend=1";

struct InvalidateOnDrop<'a>(&'a str);

impl Drop for InvalidateOnDrop<'_> {
    fn drop(&mut self) {
        super::anthropic_reset::invalidate_anthropic_usage_reset_state(Some(self.0));
    }
}

#[derive(Debug, Deserialize)]
struct Status {
    eligible: bool,
    at_limit: bool,
    exhausted: Vec<String>,
    grants: Vec<Grant>,
}

#[derive(Debug, Deserialize)]
struct Grant {
    id: String,
    resets_left: u64,
    starts_at: Option<String>,
    ends_at: Option<String>,
    clears: Vec<String>,
    paused: bool,
    usable_now: bool,
    gives_back_weekly: bool,
    blocking: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

fn eligible_grant(status: &Status, now: chrono::DateTime<chrono::Utc>) -> Option<&Grant> {
    if !status.eligible
        || !status.at_limit
        || !status
            .exhausted
            .iter()
            .any(|limit| matches!(limit.as_str(), "seven_day" | "seven_day_overage_included"))
    {
        return None;
    }
    status.grants.iter().find(|grant| {
        grant.gives_back_weekly && valid_id(&grant.id) && grant.resets_left > 0
            && !grant.paused && grant.usable_now && grant.blocking.is_empty()
            && grant.starts_at.as_deref().is_none_or(|at| {
                chrono::DateTime::parse_from_rfc3339(at).is_ok_and(|at| at <= now)
            })
            && grant.ends_at.as_deref().is_some_and(|at| {
                chrono::DateTime::parse_from_rfc3339(at).is_ok_and(|at| at > now)
            })
            // Do not spend a Fable grant against an unrelated model's limit.
            && status.exhausted.iter().all(|limit| {
                matches!(limit.as_str(), "five_hour" | "seven_day" | "seven_day_overage_included")
                    && (grant.clears.contains(limit) || limit == "seven_day")
            })
    })
}

fn profile_identity(profile: &serde_json::Value) -> Result<(String, String)> {
    let extract = |path| {
        profile
            .pointer(path)
            .and_then(|v| v.as_str())
            .filter(|id| {
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
            .map(str::to_owned)
            .context("Claude reset profile has no verified account/organization identity")
    };
    Ok((extract("/account/uuid")?, extract("/organization/uuid")?))
}

async fn status(client: &reqwest::Client, base: &str, token: &str) -> Result<Option<Status>> {
    let value = get_json(client, &format!("{base}{STATUS}"), token).await?;
    let Some(block) = value.get("cedar_ember") else {
        return Ok(None);
    };
    // Missing or malformed fields are unknown, not permission to spend.
    Ok(serde_json::from_value(block.clone()).ok())
}

/// Redeem an earned Fable grant only after fresh, account-bound exhaustion checks.
/// A cross-process account lock serializes claims. A durable pre-POST marker
/// suppresses repeat spending after timeout, cancellation, process death, malformed
/// results, or an unconfirmed reset. Never falls back from an explicit account.
/// `None` is deliberately unsupported: background retry must pin a named login.
/// This never purchases usage credits or redeems the `juniper_tide` program.
pub(crate) async fn auto_reset_fable_usage(account_label: Option<String>) -> Result<bool> {
    if !crate::config::Config::load()
        .provider
        .anthropic_auto_fable_reset
    {
        return Ok(false);
    }
    let Some(label) = account_label else {
        return Ok(false);
    };
    let token = fresh_access_token(Some(&label)).await?;
    let client = reset_client()?;
    redeem(
        &client,
        BASE,
        &token,
        &label,
        &crate::storage::jcode_dir()?.join("fable-resets"),
    )
    .await
}

async fn redeem(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    label: &str,
    root: &std::path::Path,
) -> Result<bool> {
    // The profile and usage requests use the exact same pinned token.
    let profile = get_json(client, &format!("{base}/api/oauth/profile"), token).await?;
    let (account, organization) = profile_identity(&profile)?;
    std::fs::create_dir_all(root)?;
    let key = hex::encode(Sha256::digest(
        format!("{account}:{organization}").as_bytes(),
    ));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(format!("{key}.lock")))?;
    // Wait asynchronously for another agent rather than triggering failover
    // while its claim is in flight. Errors are not mistaken for contention.
    let started = std::time::Instant::now();
    let mut waited = false;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                waited = true;
                if started.elapsed() >= std::time::Duration::from_secs(90) {
                    return Ok(false);
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    let Some(before) = status(client, base, token).await? else {
        return Ok(false);
    };
    // A concurrent claimant cleared the quota. Retry is safe without spending
    // anything or erasing its possibly uncertain durable journal.
    if waited && !before.at_limit && before.exhausted.is_empty() {
        return Ok(true);
    }
    let marker = root.join(format!("{key}.pending"));
    // Even a fresh "not limited" response cannot prove whether an earlier
    // uncertain grant was spent. Keep its marker for explicit reconciliation.
    if marker.exists() {
        return Ok(false);
    }
    let Some(grant) = eligible_grant(&before, chrono::Utc::now()) else {
        return Ok(false);
    };
    let grant_id = grant.id.clone();
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut journal = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&marker)?;
    journal.write_all(
        serde_json::to_string(&serde_json::json!({
            "program": "cedar_ember", "grant_id": grant_id, "request_id": request_id,
            "created_at": chrono::Utc::now().to_rfc3339()
        }))?
        .as_bytes(),
    )?;
    journal.sync_all()?;
    // Persist the directory entry before a non-refundable mutation.
    #[cfg(unix)]
    std::fs::File::open(root)?.sync_all()?;
    let _invalidate = InvalidateOnDrop(label);
    let response = authorize(client.post(format!("{base}/api/organizations/{organization}/reset_rate_limits")), token)
        .json(&serde_json::json!({"program":"cedar_ember", "grant_id":grant_id, "request_id":request_id}))
        .send().await.context("Fable reset outcome uncertain. Pending marker retained; check Claude Usage before reconciliation.")?;
    anyhow::ensure!(
        response.status().is_success(),
        "Fable reset outcome uncertain (HTTP {}), pending marker retained",
        response.status().as_u16()
    );
    #[derive(Deserialize)]
    struct Claim {
        result: String,
        cleared: Vec<String>,
    }
    let claim: Claim = response
        .json()
        .await
        .context("Unrecognized Fable reset result; pending marker retained")?;
    if claim.result != "reset"
        || !before
            .exhausted
            .iter()
            .all(|limit| limit == "seven_day" || claim.cleared.contains(limit))
    {
        return Ok(false);
    }
    let Some(after) = status(client, base, token).await? else {
        return Ok(false);
    };
    if after.at_limit || !after.exhausted.is_empty() {
        return Ok(false);
    }
    // Only a confirmed successful claim plus fresh clear quota permits another
    // future automatic spend. Unknown outcomes remain suppressed across restarts.
    std::fs::remove_file(&marker)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Status {
        serde_json::from_value(serde_json::json!({
            "eligible":true,"at_limit":true,"exhausted":["seven_day_overage_included"],
            "grants":[{"id":"fable_launch","resets_left":2,"starts_at":null,
                "ends_at":"2027-03-09T07:59:00Z","clears":["five_hour","seven_day_overage_included"],
                "paused":false,"usable_now":true,"gives_back_weekly":true,"blocking":[]}]
        })).unwrap()
    }
    #[test]
    fn actual_cli_schema_selects_fable_not_session_grants() {
        let mut value = fixture();
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(eligible_grant(&value, now).is_some());
        value.grants[0].gives_back_weekly = false;
        assert!(eligible_grant(&value, now).is_none());
    }
    #[test]
    fn refuse_non_exhaustion_blocking_expiry_and_unrelated_limits() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-10T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut value = fixture();
        value.at_limit = false;
        assert!(eligible_grant(&value, now).is_none());
        value = fixture();
        value.exhausted = vec!["five_hour".into()];
        assert!(eligible_grant(&value, now).is_none());
        value = fixture();
        value.exhausted = vec!["seven_day_sonnet".into()];
        assert!(eligible_grant(&value, now).is_none());
        value = fixture();
        value.grants[0].blocking.push("seven_day".into());
        assert!(eligible_grant(&value, now).is_none());
        value = fixture();
        value.grants[0].ends_at = Some("bad".into());
        assert!(eligible_grant(&value, now).is_none());
        value = fixture();
        value.grants[0].ends_at = Some("2025-01-01T00:00:00Z".into());
        assert!(eligible_grant(&value, now).is_none());
    }
    #[test]
    fn profile_requires_both_verified_identities() {
        assert!(profile_identity(&serde_json::json!({"account":{"uuid":"acct"}})).is_err());
        assert_eq!(
            profile_identity(
                &serde_json::json!({"account":{"uuid":"acct"},"organization":{"uuid":"org"}})
            )
            .unwrap(),
            ("acct".into(), "org".into())
        );
    }
    #[test]
    fn fable_automatic_reset_is_independently_opt_in() {
        assert!(!crate::config::ProviderConfig::default().anthropic_auto_fable_reset);
    }

    async fn mock(
        fail_claim: bool,
        weekly: bool,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = requests.clone();
        let task = tokio::spawn(async move {
            let mut spent = false;
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&bytes);
                    if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|s| s.parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if body.len() >= len {
                            break;
                        }
                    }
                }
                let text = String::from_utf8(bytes).unwrap();
                let (code, body) = if text.starts_with("GET /api/oauth/profile ") {
                    (
                        200,
                        serde_json::json!({"account":{"uuid":"account-1"},"organization":{"uuid":"org-1"}}),
                    )
                } else if text.starts_with("GET /api/oauth/usage?cedar_ember=1&skip_spend=1 ") {
                    let mut status = serde_json::to_value(fixture_json()).unwrap();
                    if weekly {
                        status["exhausted"] = serde_json::json!(["seven_day"]);
                    }
                    if spent && !fail_claim {
                        status["at_limit"] = false.into();
                        status["exhausted"] = serde_json::json!([]);
                    }
                    (200, serde_json::json!({"cedar_ember":status}))
                } else {
                    assert!(text.starts_with("POST /api/organizations/org-1/reset_rate_limits "));
                    let body: serde_json::Value =
                        serde_json::from_str(text.split_once("\r\n\r\n").unwrap().1).unwrap();
                    assert_eq!(body["program"], "cedar_ember");
                    assert_eq!(body["grant_id"], "fable_launch");
                    uuid::Uuid::parse_str(body["request_id"].as_str().unwrap()).unwrap();
                    spent = true;
                    (
                        if fail_claim { 500 } else { 200 },
                        serde_json::json!({"result":"reset","cleared":["seven_day_overage_included"]}),
                    )
                };
                assert!(
                    text.to_ascii_lowercase()
                        .contains("authorization: bearer pinned-token")
                );
                seen.lock().unwrap().push(text);
                let body = body.to_string();
                stream.write_all(format!("HTTP/1.1 {code} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        (base, requests, task)
    }

    fn fixture_json() -> serde_json::Value {
        serde_json::json!({"eligible":true,"at_limit":true,"exhausted":["seven_day_overage_included"],
            "grants":[{"id":"fable_launch","resets_left":2,"starts_at":null,
                "ends_at":"2099-03-09T07:59:00Z","clears":["five_hour","seven_day_overage_included"],
                "paused":false,"usable_now":true,"gives_back_weekly":true,"blocking":[]}]})
    }

    #[tokio::test]
    async fn concurrent_agents_spend_one_grant_only_after_verified_clear() {
        let (base, seen, server) = mock(false, false).await;
        let dir = tempfile::tempdir().unwrap();
        let client = reset_client().unwrap();
        let (a, b) = tokio::join!(
            redeem(&client, &base, "pinned-token", "one", dir.path()),
            redeem(
                &client,
                &base,
                "pinned-token",
                "alias-same-account",
                dir.path()
            )
        );
        assert!(a.unwrap());
        assert!(b.unwrap());
        assert!(
            !redeem(&client, &base, "pinned-token", "one", dir.path())
                .await
                .unwrap()
        );
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            1
        );
        assert!(!std::fs::read_dir(dir.path()).unwrap().any(|e| {
            e.unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "pending")
        }));
        server.abort();
    }

    #[tokio::test]
    async fn uncertain_claim_is_suppressed_on_retry_and_restart() {
        let (base, seen, server) = mock(true, false).await;
        let dir = tempfile::tempdir().unwrap();
        let client = reset_client().unwrap();
        assert!(
            redeem(&client, &base, "pinned-token", "one", dir.path())
                .await
                .is_err()
        );
        // A new client has no process-local attempt state. The durable marker wins.
        assert!(
            !redeem(
                &reset_client().unwrap(),
                &base,
                "pinned-token",
                "other-alias",
                dir.path()
            )
            .await
            .unwrap()
        );
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            1
        );
        assert!(std::fs::read_dir(dir.path()).unwrap().any(|e| {
            e.unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "pending")
        }));
        server.abort();
    }

    #[tokio::test]
    async fn fable_weekly_refund_recovers_without_zeroing_whole_weekly_window() {
        let (base, seen, server) = mock(false, true).await;
        let dir = tempfile::tempdir().unwrap();
        assert!(
            redeem(
                &reset_client().unwrap(),
                &base,
                "pinned-token",
                "one",
                dir.path()
            )
            .await
            .unwrap()
        );
        assert_eq!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            1
        );
        server.abort();
    }
}
