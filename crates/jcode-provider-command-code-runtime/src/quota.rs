//! Lazy Command Code credits diagnostics. Billing is advisory and never part
//! of the generation critical path.
use anyhow::Result;
use jcode_provider_command_code::CREDITS_URL_BASE;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::{Arc, Mutex}, time::{Duration, Instant}};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CreditWindow { pub cap: Option<f64>, pub used: Option<f64>, pub reset_at: Option<String> }
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CommandCodeCredits { pub five_hour: Option<CreditWindow>, pub weekly: Option<CreditWindow>, pub credits: Option<f64> }
fn number(value: &Value, keys: &[&str]) -> Option<f64> { keys.iter().find_map(|key| value.get(*key).and_then(Value::as_f64)) }
fn window(value: Option<&Value>) -> Option<CreditWindow> {
    let value = value?.as_object()?;
    Some(CreditWindow { cap: number(&Value::Object(value.clone()), &["cap", "limit", "maximum"]), used: number(&Value::Object(value.clone()), &["used", "usage"]), reset_at: value.get("resetAt").or_else(|| value.get("reset_at")).and_then(Value::as_str).map(str::to_owned) })
}
/// Accept both wrapped `{windowLimits: ...}` and direct payload shapes.
pub fn parse_credits(payload: &Value) -> CommandCodeCredits {
    let root = payload.get("data").unwrap_or(payload);
    let windows = root.get("windowLimits").or_else(|| root.get("window_limits")).unwrap_or(root);
    CommandCodeCredits { five_hour: window(windows.get("fiveHour").or_else(|| windows.get("five_hour"))), weekly: window(windows.get("weekly").or_else(|| windows.get("sevenDay"))), credits: number(root, &["credits", "balance", "remaining"]) }
}
#[derive(Clone)]
pub struct CommandCodeQuotaCache { entries: Arc<Mutex<HashMap<String, (Instant, CommandCodeCredits)>>>, ttl: Duration }
impl Default for CommandCodeQuotaCache { fn default() -> Self { Self::new() } }
impl CommandCodeQuotaCache {
    pub fn new() -> Self { Self { entries: Arc::new(Mutex::new(HashMap::new())), ttl: Duration::from_secs(60) } }
    pub fn get(&self, account: &str) -> Option<CommandCodeCredits> { self.entries.lock().ok()?.get(account).and_then(|(at, value)| (at.elapsed() < self.ttl).then(|| value.clone())) }
    pub fn insert(&self, account: impl Into<String>, value: CommandCodeCredits) { if let Ok(mut entries) = self.entries.lock() { entries.insert(account.into(), (Instant::now(), value)); } }
    pub fn clear(&self) { if let Ok(mut entries) = self.entries.lock() { entries.clear(); } }
}
/// Fetch credits lazily. Network and parse errors are deliberately soft.
pub async fn command_code_credits(client: &Client, api_key: &str, org_id: Option<&str>, account: &str, cache: &CommandCodeQuotaCache) -> Option<CommandCodeCredits> {
    if let Some(value) = cache.get(account) { return Some(value); }
    let mut request = client.get(CREDITS_URL_BASE).bearer_auth(api_key);
    if let Some(org_id) = org_id { request = request.query(&[("orgId", org_id)]); }
    let value = parse_credits(&request.send().await.ok()?.error_for_status().ok()?.json::<Value>().await.ok()?);
    cache.insert(account, value.clone()); Some(value)
}
pub fn command_code_credits_from_json(payload: &str) -> Result<CommandCodeCredits> { Ok(parse_credits(&serde_json::from_str(payload)?)) }
