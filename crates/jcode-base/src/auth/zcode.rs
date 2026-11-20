//! ZCode (Z.AI Start Plan / Off-Peak / Coding Plan) credentials, OAuth, and
//! Client Request Signing v4.
//!
//! Jcode talks directly to ZCode's Anthropic-compatible endpoints:
//! - Start Plan: `https://zcode.z.ai/api/v1/zcode-plan/anthropic`
//! - Off-Peak (Idle): `https://zcode.z.ai/api/v1/off-peak/anthropic`
//! - Billing / Usage: `https://zcode.z.ai/api/v1/zcode-plan/billing/current`
//!   and `/billing/balance`
//!
//! Authentication uses ZCode's CLI OAuth flow (`/oauth/cli/init` +
//! `/oauth/cli/poll/{flow_id}`), storing `zcodejwttoken` and
//! `oauth:zai:access_token` in `~/.jcode/zcode_oauth.json`.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::digest::{SHA256, digest};
use ring::hkdf::{HKDF_SHA256, Salt};
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::Ed25519KeyPair;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

pub const ZCODE_ORIGIN: &str = "https://zcode.z.ai";
pub const ZCODE_API_BASE: &str = "https://zcode.z.ai/api/v1";
pub const ZAI_API_ORIGIN: &str = "https://api.z.ai";
pub const ZAI_ANTHROPIC_BASE: &str = "https://api.z.ai/api/anthropic";
pub const ZCODE_PLAN_ANTHROPIC_BASE: &str = "https://zcode.z.ai/api/v1/zcode-plan/anthropic";
pub const ZCODE_OFF_PEAK_ANTHROPIC_BASE: &str = "https://zcode.z.ai/api/v1/off-peak/anthropic";
pub const ZCODE_BILLING_CURRENT_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/billing/current";
pub const ZCODE_BILLING_BALANCE_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/billing/balance";
pub const ZCODE_HANDSHAKE_PATH: &str = "/api/paas/c1f3a7e2/v2/client";

pub const DEFAULT_ZCODE_APP_VERSION: &str = "0.15.2";
pub const ZCODE_APP_ID: &str = "zcode";
const KDF_SALT: &[u8] = b"WD_CLIENT_SIGN_KDF_SALT";
const KDF_INFO_HMAC: &[u8] = b"getSignKey_hmac";
const KDF_INFO_ED25519: &[u8] = b"ed25519_priv";
const HANDSHAKE_ACTION: &str = "get_sign_key";
const POW_BITS: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZCodeUser {
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZCodeCredentials {
    /// `zcodejwttoken` returned by `/oauth/cli/poll/{flow_id}`
    pub token: String,
    /// `zai.access_token`
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<ZCodeUser>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coding_plan_api_key: Option<String>,
    #[serde(default)]
    pub updated_at: i64,
}

pub fn credentials_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("zcode_oauth.json"))
}

pub fn has_cached_login() -> bool {
    load_credentials()
        .map(|c| !c.token.trim().is_empty())
        .unwrap_or(false)
}

pub fn load_credentials() -> Result<ZCodeCredentials> {
    let path = credentials_path()?;
    crate::storage::read_json(&path)
}

pub fn save_credentials(creds: &ZCodeCredentials) -> Result<()> {
    let path = credentials_path()?;
    crate::storage::write_json_secret(&path, creds)
}

pub fn clear_credentials() -> Result<()> {
    let path = credentials_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CliOAuthInitData {
    pub flow_id: String,
    pub authorize_url: String,
    pub expires_at: i64,
    pub poll_interval_sec: u64,
}

#[derive(Debug, Clone)]
pub enum CliOAuthPollResult {
    Pending,
    Failed,
    Ready(ZCodeCredentials),
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
struct PollData {
    status: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    user: Option<ZCodeUser>,
    #[serde(default)]
    zai: Option<PollProviderTokens>,
}

#[derive(Debug, Deserialize)]
struct PollProviderTokens {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

pub fn generate_poll_token() -> Result<String> {
    let rng = SystemRandom::new();
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to generate random poll token"))?;
    Ok(hex::encode(bytes))
}

pub async fn init_cli_oauth(client: &reqwest::Client, poll_token: &str) -> Result<CliOAuthInitData> {
    let url = format!("{ZCODE_API_BASE}/oauth/cli/init");
    let resp = client
        .post(&url)
        .bearer_auth(poll_token)
        .header("User-Agent", format!("ZCode/{DEFAULT_ZCODE_APP_VERSION}"))
        .header("X-ZCode-App-Version", DEFAULT_ZCODE_APP_VERSION)
        .header("X-Title", "Z Code@cli")
        .json(&serde_json::json!({ "provider": "zai" }))
        .send()
        .await
        .context("failed to call ZCode OAuth init")?;

    let status = resp.status();
    let env: Envelope<CliOAuthInitData> = resp
        .json()
        .await
        .with_context(|| format!("invalid JSON from ZCode OAuth init (HTTP {status})"))?;
    if !status.is_success() {
        bail!(
            "ZCode OAuth init failed (HTTP {status}): {}",
            env.message.unwrap_or_default()
        );
    }
    env.data
        .context("ZCode OAuth init response missing data field")
}

pub async fn poll_cli_oauth(
    client: &reqwest::Client,
    flow_id: &str,
    poll_token: &str,
) -> Result<CliOAuthPollResult> {
    let url = format!("{ZCODE_API_BASE}/oauth/cli/poll/{flow_id}");
    let resp = client
        .get(&url)
        .bearer_auth(poll_token)
        .header("User-Agent", format!("ZCode/{DEFAULT_ZCODE_APP_VERSION}"))
        .header("X-ZCode-App-Version", DEFAULT_ZCODE_APP_VERSION)
        .send()
        .await
        .context("failed to poll ZCode OAuth")?;

    let status = resp.status();
    let env: Envelope<PollData> = resp
        .json()
        .await
        .with_context(|| format!("invalid JSON from ZCode OAuth poll (HTTP {status})"))?;
    if !status.is_success() {
        bail!(
            "ZCode OAuth poll failed (HTTP {status}): {}",
            env.message.unwrap_or_default()
        );
    }
    let data = env.data.context("ZCode OAuth poll missing data")?;
    match data.status.as_str() {
        "pending" => Ok(CliOAuthPollResult::Pending),
        "failed" => Ok(CliOAuthPollResult::Failed),
        "ready" => {
            let token = data
                .token
                .filter(|s| !s.trim().is_empty())
                .context("ZCode OAuth ready response missing token")?;
            let zai = data
                .zai
                .context("ZCode OAuth ready response missing zai tokens")?;
            let access_token = zai
                .access_token
                .filter(|s| !s.trim().is_empty())
                .context("ZCode OAuth ready response missing zai.access_token")?;
            Ok(CliOAuthPollResult::Ready(ZCodeCredentials {
                token,
                access_token,
                refresh_token: zai.refresh_token,
                user: data.user,
                coding_plan_api_key: None,
                updated_at: chrono::Utc::now().timestamp(),
            }))
        }
        other => bail!("unexpected ZCode OAuth poll status: {other}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSigningCredential {
    pub api_key_id: String,
    pub secret: String,
    pub credential: String,
    pub handshake_field: &'static str,
}

#[derive(Debug, Deserialize)]
struct JwtPayload {
    api_key: String,
}

pub fn parse_signing_credential(raw: &str) -> Option<ParsedSigningCredential> {
    let parts: Vec<&str> = raw.split('.').collect();
    if parts.len() == 3 && !parts[2].is_empty() {
        if let Ok(payload_bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1])
            && let Ok(payload) = serde_json::from_slice::<JwtPayload>(&payload_bytes)
            && !payload.api_key.trim().is_empty()
        {
            return Some(ParsedSigningCredential {
                api_key_id: payload.api_key,
                secret: raw.to_string(),
                credential: raw.to_string(),
                handshake_field: "accessToken",
            });
        }
        return None;
    }
    let dot = raw.find('.')?;
    if dot == 0 || raw[dot + 1..].contains('.') {
        return None;
    }
    let id = raw[..dot].trim();
    let secret = raw[dot + 1..].trim();
    if id.is_empty() || secret.is_empty() {
        return None;
    }
    Some(ParsedSigningCredential {
        api_key_id: id.to_string(),
        secret: secret.to_string(),
        credential: raw.to_string(),
        handshake_field: "apiKey",
    })
}

fn derive_bytes(secret: &str, info: &[u8]) -> Result<[u8; 32]> {
    let salt = Salt::new(HKDF_SHA256, KDF_SALT);
    let prk = salt.extract(secret.as_bytes());
    let info_arr = [info];
    let okm = prk
        .expand(&info_arr, HKDF_SHA256)
        .map_err(|_| anyhow::anyhow!("HKDF expand failed"))?;
    let mut out = [0u8; 32];
    okm.fill(&mut out)
        .map_err(|_| anyhow::anyhow!("HKDF fill failed"))?;
    Ok(out)
}

pub fn create_handshake_signature(secret: &str, message: &str) -> Result<String> {
    let key_bytes = derive_bytes(secret, KDF_INFO_HMAC)?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, &key_bytes);
    let tag = hmac::sign(&key, message.as_bytes());
    Ok(base64::engine::general_purpose::STANDARD.encode(tag.as_ref()))
}

pub fn decrypt_signing_private_key(
    aad: &str,
    secret: &str,
    private_cipher_b64: &str,
) -> Result<Ed25519KeyPair> {
    let cipher = base64::engine::general_purpose::STANDARD
        .decode(private_cipher_b64)
        .context("invalid base64 privateCipher")?;
    if cipher.len() <= 12 + 16 {
        bail!("privateCipher is too short");
    }
    let key_bytes = derive_bytes(secret, KDF_INFO_ED25519)?;
    let unbound = UnboundKey::new(&AES_256_GCM, &key_bytes)
        .map_err(|_| anyhow::anyhow!("AES-256-GCM key init failed"))?;
    let key = LessSafeKey::new(unbound);
    let nonce = Nonce::try_assume_unique_for_key(&cipher[..12])
        .map_err(|_| anyhow::anyhow!("invalid AES-GCM nonce"))?;
    let mut in_out = cipher[12..].to_vec();
    let plaintext = key
        .open_in_place(nonce, Aad::from(aad.as_bytes()), &mut in_out)
        .map_err(|_| anyhow::anyhow!("AES-GCM decrypt failed"))?;
    let pkcs8_b64 = std::str::from_utf8(plaintext).context("decrypted key is not UTF-8")?;
    let pkcs8 = base64::engine::general_purpose::STANDARD
        .decode(pkcs8_b64.trim())
        .context("decrypted key is not valid base64")?;
    Ed25519KeyPair::from_pkcs8_maybe_unchecked(&pkcs8)
        .map_err(|e| anyhow::anyhow!("invalid Ed25519 PKCS#8 key: {e}"))
}

pub fn has_leading_zero_bits(bytes: &[u8], bits: usize) -> bool {
    let full_bytes = bits / 8;
    if bytes.len() < full_bytes {
        return false;
    }
    if bytes[..full_bytes].iter().any(|&b| b != 0) {
        return false;
    }
    let rem = bits % 8;
    if rem == 0 {
        return true;
    }
    let mask = (0xffu8 << (8 - rem)) & 0xff;
    (bytes.get(full_bytes).copied().unwrap_or(0xff) & mask) == 0
}

fn random_hex(num_bytes: usize) -> Result<String> {
    let rng = SystemRandom::new();
    let mut buf = vec![0u8; num_bytes];
    rng.fill(&mut buf)
        .map_err(|_| anyhow::anyhow!("random fill failed"))?;
    Ok(hex::encode(buf))
}

pub fn solve_pow(api_key_id: &str, app_id: &str, session_id: &str, ts: &str) -> Result<String> {
    let seed_input = format!("{api_key_id}\n{app_id}\n{session_id}\n{ts}");
    let seed_hash = digest(&SHA256, seed_input.as_bytes());
    let prefix = &hex::encode(seed_hash.as_ref())[..32];
    let rand_prefix = random_hex(12)?;
    for i in 0u32..=u32::MAX {
        let candidate = format!("{rand_prefix}{i:08x}");
        let check_input = format!("{prefix}\n{candidate}");
        let h = digest(&SHA256, check_input.as_bytes());
        if has_leading_zero_bits(h.as_ref(), POW_BITS) {
            return Ok(candidate);
        }
    }
    bail!("unable to solve client request proof of work")
}

#[derive(Debug, Deserialize)]
struct HandshakeResponse {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    data: Option<HandshakeData>,
}

#[derive(Debug, Deserialize)]
struct HandshakeData {
    #[serde(rename = "privateCipher")]
    private_cipher: Option<String>,
}

pub struct ZCodeClientSigner {
    origin: &'static str,
    key_pair: RwLock<Option<Arc<Ed25519KeyPair>>>,
}

impl ZCodeClientSigner {
    pub fn new() -> Self {
        Self {
            origin: ZAI_API_ORIGIN,
            key_pair: RwLock::new(None),
        }
    }

    pub async fn invalidate(&self) {
        *self.key_pair.write().await = None;
    }

    async fn ensure_key_pair(
        &self,
        client: &reqwest::Client,
        cred: &ParsedSigningCredential,
    ) -> Result<Arc<Ed25519KeyPair>> {
        if let Some(kp) = self.key_pair.read().await.as_ref() {
            return Ok(Arc::clone(kp));
        }
        let ts = chrono::Utc::now().timestamp_millis().to_string();
        let nonce = random_hex(16)?;
        let sig_msg = format!("{HANDSHAKE_ACTION}\n{}\n{ts}\n{nonce}", cred.api_key_id);
        let sig = create_handshake_signature(&cred.secret, &sig_msg)?;
        let handshake_url = format!("{}{ZCODE_HANDSHAKE_PATH}", self.origin);
        let body = serde_json::json!({
            cred.handshake_field: cred.credential,
            "nonce": nonce,
            "sig": sig,
            "ts": ts,
        });
        let resp = client
            .post(&handshake_url)
            .header("Authorization", &cred.credential)
            .header("User-Agent", format!("ZCode/{DEFAULT_ZCODE_APP_VERSION}"))
            .header("X-ZCode-App-Version", DEFAULT_ZCODE_APP_VERSION)
            .json(&body)
            .send()
            .await
            .context("ZCode signing handshake failed")?;
        if !resp.status().is_success() {
            bail!("ZCode signing handshake HTTP {}", resp.status());
        }
        let parsed: HandshakeResponse = resp.json().await?;
        if parsed.code == Some(500) {
            bail!("ZCode signing handshake server error 500");
        }
        let cipher = parsed
            .data
            .and_then(|d| d.private_cipher)
            .context("ZCode signing handshake missing privateCipher")?;
        let kp = Arc::new(decrypt_signing_private_key(
            &cred.api_key_id,
            &cred.secret,
            &cipher,
        )?);
        *self.key_pair.write().await = Some(Arc::clone(&kp));
        Ok(kp)
    }

    pub async fn signed_headers(
        &self,
        client: &reqwest::Client,
        raw_credential: &str,
        session_id: &str,
    ) -> Result<Vec<(&'static str, String)>> {
        let cred = parse_signing_credential(raw_credential)
            .context("invalid ZCode credential for signing")?;
        let kp = self.ensure_key_pair(client, &cred).await?;
        let ts = chrono::Utc::now().timestamp_millis().to_string();
        let nonce = random_hex(16)?;
        let pow = solve_pow(&cred.api_key_id, ZCODE_APP_ID, session_id, &ts)?;
        let msg = format!(
            "{}\n{ts}\n{DEFAULT_ZCODE_APP_VERSION}\n{session_id}\n{nonce}",
            cred.api_key_id
        );
        let sig = base64::engine::general_purpose::STANDARD.encode(kp.sign(msg.as_bytes()).as_ref());
        Ok(vec![
            ("X-Client-Ts", ts),
            ("X-Client-Version", DEFAULT_ZCODE_APP_VERSION.to_string()),
            ("X-Client-Sig", sig),
            ("X-Session-Id", session_id.to_string()),
            ("X-Client-Nonce", nonce),
            ("X-App-Id", ZCODE_APP_ID.to_string()),
            ("X-Client-Pow", pow),
        ])
    }
}

pub fn zcode_source_headers() -> Vec<(&'static str, String)> {
    vec![
        ("HTTP-Referer", ZCODE_ORIGIN.to_string()),
        ("User-Agent", format!("ZCode/{DEFAULT_ZCODE_APP_VERSION}")),
        ("X-ZCode-App-Version", DEFAULT_ZCODE_APP_VERSION.to_string()),
        ("X-Title", "Z Code@cli".to_string()),
        (
            "X-Platform",
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        ),
        ("X-Client-Language", "en-US".to_string()),
    ]
}

pub async fn resolve_coding_plan_api_key(client: &reqwest::Client) -> Result<String> {
    let mut creds = load_credentials()?;
    if let Some(key) = creds
        .coding_plan_api_key
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(key.to_string());
    }
    let login_resp: serde_json::Value = client
        .post(format!("{ZAI_API_ORIGIN}/api/auth/z/login"))
        .json(&serde_json::json!({ "token": creds.access_token }))
        .send()
        .await
        .context("Z.AI business login failed")?
        .json()
        .await
        .context("invalid JSON from Z.AI business login")?;
    let biz_tok = login_resp
        .pointer("/data/access_token")
        .and_then(|v| v.as_str())
        .context("Z.AI business login missing access_token")?;

    let cust_resp: serde_json::Value = client
        .get(format!("{ZAI_API_ORIGIN}/api/biz/customer/getCustomerInfo"))
        .bearer_auth(biz_tok)
        .send()
        .await
        .context("Z.AI getCustomerInfo failed")?
        .json()
        .await
        .context("invalid JSON from getCustomerInfo")?;
    let org = cust_resp
        .pointer("/data/organizations/0")
        .context("Z.AI getCustomerInfo missing organizations")?;
    let org_id = org
        .get("organizationId")
        .and_then(|v| v.as_str())
        .context("missing organizationId")?;
    let proj_id = org
        .pointer("/projects/0/projectId")
        .and_then(|v| v.as_str())
        .context("missing projectId")?;

    let keys_resp: serde_json::Value = client
        .get(format!(
            "{ZAI_API_ORIGIN}/api/biz/v1/organization/{org_id}/projects/{proj_id}/api_keys"
        ))
        .bearer_auth(biz_tok)
        .send()
        .await
        .context("Z.AI api_keys fetch failed")?
        .json()
        .await
        .context("invalid JSON from api_keys")?;
    let keys = keys_resp
        .get("data")
        .and_then(|v| v.as_array())
        .context("Z.AI api_keys returned empty list")?;
    let selected = keys
        .iter()
        .find(|k| k.get("name").and_then(|v| v.as_str()) == Some("zcode-api-key"))
        .or_else(|| keys.first())
        .context("Z.AI api_keys returned no keys")?;
    let api_key_id = selected
        .get("apiKey")
        .and_then(|v| v.as_str())
        .context("missing apiKey")?;

    let token_resp: serde_json::Value = client
        .post(format!(
            "{ZAI_API_ORIGIN}/api/biz/v1/organization/{org_id}/projects/{proj_id}/api_keys/{api_key_id}/access_tokens"
        ))
        .bearer_auth(biz_tok)
        .json(&serde_json::json!({
            "clientType": "zcode",
            "clientVersion": DEFAULT_ZCODE_APP_VERSION,
        }))
        .send()
        .await
        .context("Z.AI access_tokens exchange failed")?
        .json()
        .await
        .context("invalid JSON from access_tokens")?;
    let full_key = token_resp
        .pointer("/data/accessToken")
        .or_else(|| token_resp.pointer("/data/access_token"))
        .and_then(|v| v.as_str())
        .context("Z.AI access_tokens missing accessToken")?
        .to_string();
    creds.coding_plan_api_key = Some(full_key.clone());
    let _ = save_credentials(&creds);
    Ok(full_key)
}

pub async fn fetch_billing_summary(client: &reqwest::Client) -> Result<serde_json::Value> {
    let creds = load_credentials()?;
    let mut out = serde_json::Map::new();
    let mut errors = Vec::new();
    for (key, url) in [
        ("current", ZCODE_BILLING_CURRENT_URL),
        ("balance", ZCODE_BILLING_BALANCE_URL),
    ] {
        let resp = client
            .get(url)
            .bearer_auth(&creds.token)
            .header("User-Agent", format!("ZCode/{DEFAULT_ZCODE_APP_VERSION}"))
            .header("X-ZCode-App-Version", DEFAULT_ZCODE_APP_VERSION)
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                Ok(v) => {
                    out.insert(key.to_string(), v);
                }
                Err(err) => errors.push(format!("{key}: invalid JSON ({err})")),
            },
            Ok(r) => errors.push(format!("{key}: HTTP {}", r.status())),
            Err(err) => errors.push(format!("{key}: {err}")),
        }
    }
    if out.is_empty() {
        anyhow::bail!("ZCode billing fetch failed: {}", errors.join("; "));
    }
    if !errors.is_empty() {
        out.insert(
            "errors".to_string(),
            serde_json::Value::Array(errors.into_iter().map(serde_json::Value::String).collect()),
        );
    }
    Ok(serde_json::Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_api_key_and_jwt_credentials() {
        let api = parse_signing_credential("key123.secret456").unwrap();
        assert_eq!(api.api_key_id, "key123");
        assert_eq!(api.secret, "secret456");
        assert_eq!(api.handshake_field, "apiKey");

        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256"}"#);
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"api_key":"ak_999"}"#);
        let jwt = format!("{header}.{payload}.sig");
        let parsed = parse_signing_credential(&jwt).unwrap();
        assert_eq!(parsed.api_key_id, "ak_999");
        assert_eq!(parsed.handshake_field, "accessToken");
    }

    #[test]
    fn leading_zero_bits_check() {
        assert!(has_leading_zero_bits(&[0x00, 0xff], 8));
        assert!(!has_leading_zero_bits(&[0x01, 0x00], 8));
        assert!(has_leading_zero_bits(&[0x0f, 0xff], 4));
    }

    #[test]
    fn pow_solves_quickly() {
        let pow = solve_pow("ak_1", "zcode", "sess_1", "1700000000000").unwrap();
        assert_eq!(pow.len(), 32);
    }
}
