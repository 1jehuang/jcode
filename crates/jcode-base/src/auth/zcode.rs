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
//!
//! Signing semantics (protocol facts verified against the independent
//! reference implementation https://github.com/TriDefender/zcode-api):
//! - Only coding-plan credentials sign. Start-plan, off-peak and highspeed
//!   routes are never signed (static client-side rule since 3.15.1).
//! - The handshake is fail-open: a failure sends the request unsigned rather
//!   than erroring, and a 401 VERIFY_SIGNATURE_* / VERIFY_APIKEY_EXPIRED
//!   rejection re-handshakes once, then permanently bypasses signing for
//!   that credential.
//! - Coding-plan credentials are `{apiKeyId}.{secret}` pairs issued by the
//!   `api_keys` + `api_keys/copy/{id}` biz endpoints; the project-access JWT
//!   form (`accessToken`) is only used by the bigmodel OAuth login chain.

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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
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

pub const DEFAULT_ZCODE_APP_VERSION: &str = "3.18.2";
pub const ZCODE_APP_ID: &str = "zcode";
const KDF_SALT: &[u8] = b"WD_CLIENT_SIGN_KDF_SALT";
const KDF_INFO_HMAC: &[u8] = b"getSignKey_hmac";
const KDF_INFO_ED25519: &[u8] = b"ed25519_priv";
const HANDSHAKE_ACTION: &str = "get_sign_key";
const POW_BITS: usize = 8;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

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
struct JwtHeader {
    #[serde(default)]
    alg: Option<String>,
    #[serde(default, rename = "sign_type")]
    sign_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JwtPayload {
    #[serde(default, rename = "token_use")]
    token_use: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
}

pub fn parse_signing_credential(raw: &str) -> Option<ParsedSigningCredential> {
    let parts: Vec<&str> = raw.split('.').collect();
    if parts.len() == 3 && !parts[2].is_empty() {
        // Strict project-access JWT parse (mirrors the 3.15.1 client): the
        // header must be HS256/SIGN and the payload must be a project_access
        // token carrying a non-empty api_key claim. Anything else is not a
        // signable credential and falls through to the apiKey form check.
        let header_ok = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[0])
            .ok()
            .and_then(|b| serde_json::from_slice::<JwtHeader>(&b).ok())
            .is_some_and(|h| h.alg.as_deref() == Some("HS256") && h.sign_type.as_deref() == Some("SIGN"));
        if !header_ok {
            return None;
        }
        if let Ok(payload_bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1])
            && let Ok(payload) = serde_json::from_slice::<JwtPayload>(&payload_bytes)
            && payload.token_use.as_deref() == Some("project_access")
            && let Some(api_key) = payload.api_key
            && !api_key.trim().is_empty()
        {
            return Some(ParsedSigningCredential {
                api_key_id: api_key,
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
    msg: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    success: Option<bool>,
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
    /// Permanent bypass after two VERIFY rejections for this credential.
    bypass: AtomicBool,
}

const VERIFY_SIGNATURE_INVALID: &str = "VERIFY_SIGNATURE_INVALID";
const VERIFY_APIKEY_EXPIRED: &str = "VERIFY_APIKEY_EXPIRED";

impl Default for ZCodeClientSigner {
    fn default() -> Self {
        Self::new()
    }
}

impl ZCodeClientSigner {
    pub fn new() -> Self {
        Self {
            origin: ZAI_API_ORIGIN,
            key_pair: RwLock::new(None),
            bypass: AtomicBool::new(false),
        }
    }

    pub async fn invalidate(&self) {
        *self.key_pair.write().await = None;
    }

    /// Permanent fail-open bypass (mirrors the official client: two
    /// consecutive 401 VERIFY_* rejections stop signing for this credential).
    pub fn set_bypass(&self) {
        self.bypass.store(true, Ordering::SeqCst);
        crate::logging::info(
            "ZCode client signing bypassed after repeated VERIFY rejections; sending unsigned",
        );
    }

    /// True when the response is a signing rejection the client retries on:
    /// HTTP 401 whose envelope mentions VERIFY_SIGNATURE_INVALID or
    /// VERIFY_APIKEY_EXPIRED.
    pub fn is_verify_failure(status: u16, body: &str) -> bool {
        if status != 401 {
            return false;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
            return false;
        };
        let candidates = [
            json.get("msg").and_then(|v| v.as_str()),
            json.get("reason").and_then(|v| v.as_str()),
            json.pointer("/data/reason").and_then(|v| v.as_str()),
            json.pointer("/error/reason").and_then(|v| v.as_str()),
            json.pointer("/error/message").and_then(|v| v.as_str()),
        ];
        candidates
            .iter()
            .any(|c| matches!(*c, Some(VERIFY_SIGNATURE_INVALID) | Some(VERIFY_APIKEY_EXPIRED)))
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
        // accessToken (project-access JWT) form uses Bearer auth; apiKey form
        // keeps the bare `{id}.{secret}` Authorization string.
        let (auth_header, auth_value) = if cred.handshake_field == "accessToken" {
            (format!("Bearer {}", cred.secret), cred.credential.clone())
        } else {
            (cred.credential.clone(), cred.credential.clone())
        };
        let body = serde_json::json!({
            cred.handshake_field: auth_value,
            "nonce": nonce,
            "sig": sig,
            "ts": ts,
        });
        let resp = client
            .post(&handshake_url)
            .header("Authorization", auth_header)
            .header("User-Agent", format!("ZCode/{DEFAULT_ZCODE_APP_VERSION}"))
            .header("X-ZCode-App-Version", DEFAULT_ZCODE_APP_VERSION)
            .json(&body)
            .timeout(HANDSHAKE_TIMEOUT)
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
        if parsed.success == Some(false) || parsed.code.is_some_and(|c| c != 0 && c != 200) {
            let detail = parsed
                .msg
                .or(parsed.message)
                .unwrap_or_else(|| "unknown error".to_string());
            if parsed.code == Some(1003) {
                if let Ok(mut creds) = load_credentials() {
                    creds.coding_plan_api_key = None;
                    let _ = save_credentials(&creds);
                }
                bail!(
                    "ZCode signing handshake token expired (code 1003: {detail}); please re-authenticate with ZCode"
                );
            }
            bail!(
                "ZCode signing handshake failed (code {:?}): {detail}",
                parsed.code
            );
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

    /// Sign the request headers. Fail-open: any signing error returns empty
    /// headers so the caller sends the request unsigned instead of failing.
    pub async fn signed_headers(
        &self,
        client: &reqwest::Client,
        raw_credential: &str,
        session_id: &str,
    ) -> Vec<(&'static str, String)> {
        if self.bypass.load(Ordering::SeqCst) {
            return Vec::new();
        }
        let Ok(cred) = parse_signing_credential(raw_credential)
            .context("invalid ZCode credential for signing")
        else {
            crate::logging::info(
                "ZCode credential is not signable (no {apiKeyId}.{secret} separator) — sending unsigned",
            );
            return Vec::new();
        };
        let kp = match self.ensure_key_pair(client, &cred).await {
            Ok(kp) => kp,
            Err(err) => {
                crate::logging::info(&format!(
                    "ZCode signing handshake failed — sending unsigned ({err:#})"
                ));
                self.invalidate().await;
                return Vec::new();
            }
        };
        let Ok(ts_usign) = u64::try_from(chrono::Utc::now().timestamp_millis()) else {
            return Vec::new();
        };
        let ts = ts_usign.to_string();
        let nonce = match random_hex(16) {
            Ok(n) => n,
            Err(err) => {
                crate::logging::info(&format!(
                    "ZCode signing nonce generation failed — sending unsigned ({err:#})"
                ));
                return Vec::new();
            }
        };
        let pow = match solve_pow(&cred.api_key_id, ZCODE_APP_ID, session_id, &ts) {
            Ok(p) => p,
            Err(err) => {
                crate::logging::info(&format!(
                    "ZCode proof-of-work failed — sending unsigned ({err:#})"
                ));
                return Vec::new();
            }
        };
        let msg = format!(
            "{}\n{ts}\n{DEFAULT_ZCODE_APP_VERSION}\n{session_id}\n{nonce}",
            cred.api_key_id
        );
        let sig =
            base64::engine::general_purpose::STANDARD.encode(kp.sign(msg.as_bytes()).as_ref());
        vec![
            ("X-Client-Ts", ts),
            ("X-Client-Version", DEFAULT_ZCODE_APP_VERSION.to_string()),
            ("X-Client-Sig", sig),
            ("X-Session-Id", session_id.to_string()),
            ("X-Client-Nonce", nonce),
            ("X-App-Id", ZCODE_APP_ID.to_string()),
            ("X-Client-Pow", pow),
        ]
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

    // Fetch the secret via the copy endpoint. The signing credential is the
    // `{apiKeyId}.{secret}` pair — the project-access JWT form is only used
    // by the bigmodel OAuth login chain and can never sign here (its
    // privateCipher is undecryptable with this secret; live-verified against
    // https://github.com/TriDefender/zcode-api).
    let copy_resp: serde_json::Value = client
        .get(format!(
            "{ZAI_API_ORIGIN}/api/biz/v1/organization/{org_id}/projects/{proj_id}/api_keys/copy/{api_key_id}"
        ))
        .bearer_auth(biz_tok)
        .send()
        .await
        .context("Z.AI api_keys copy fetch failed")?
        .json()
        .await
        .context("invalid JSON from api_keys copy")?;
    let secret_key = copy_resp
        .pointer("/data/secretKey")
        .or_else(|| copy_resp.get("secretKey"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .context("Z.AI api_keys copy missing secretKey")?;
    let full_key = format!("{api_key_id}.{secret_key}");
    creds.coding_plan_api_key = Some(full_key.clone());
    let _ = save_credentials(&creds);
    Ok(full_key)
}

/// Coding-plan usage windows from the monitor plane (`/api/monitor/usage/quota/limit`,
/// the same endpoint the official usage panel reads). Note: upstream `number`
/// is the window total and is NOT comparable with `remaining`; only
/// `remaining` and `percentage` are display-safe.
#[derive(Debug, Clone, Deserialize)]
pub struct QuotaCodingLimit {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default, rename = "number")]
    pub total: Option<f64>,
    #[serde(default)]
    pub usage: Option<f64>,
    #[serde(default)]
    pub remaining: Option<f64>,
    #[serde(default)]
    pub percentage: Option<f64>,
    #[serde(default, rename = "next_reset_time")]
    pub next_reset_time: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuotaCodingPlan {
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub limits: Vec<QuotaCodingLimit>,
}

pub async fn fetch_coding_plan_quota(client: &reqwest::Client) -> Result<QuotaCodingPlan> {
    let creds = load_credentials()?;
    let key = creds
        .coding_plan_api_key
        .clone()
        .filter(|s| !s.trim().is_empty())
        .context("no cached coding-plan key; login to ZCode first")?;
    let resp = client
        .get("https://api.z.ai/api/monitor/usage/quota/limit")
        .timeout(Duration::from_secs(15))
        .header("authorization", &key)
        .header("accept", "application/json")
        .send()
        .await
        .context("ZCode quota/limit request failed")?;
    let status = resp.status().as_u16();
    let json: serde_json::Value = resp
        .json()
        .await
        .context("invalid JSON from ZCode quota/limit")?;
    let code = json.get("code").and_then(|v| v.as_i64());
    if status < 200 || status >= 300 || (code.is_some() && code != Some(0) && code != Some(200)) {
        anyhow::bail!(
            "ZCode quota/limit failed (HTTP {status}, code {:?}): {}",
            code,
            json.get("msg").and_then(|v| v.as_str()).unwrap_or("unknown"),
        );
    }
    let data = json.get("data").cloned().unwrap_or(serde_json::Value::Null);
    let plan: QuotaCodingPlan = serde_json::from_value(data)
        .context("unexpected ZCode quota/limit shape")?;
    // Drop rows with no usable number (display noise, per the official panel).
    let limits = plan
        .limits
        .into_iter()
        .filter(|l| l.total.is_some() || l.usage.is_some() || l.remaining.is_some())
        .collect();
    Ok(QuotaCodingPlan {
        level: plan.level,
        limits,
    })
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

        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"alg":"HS256","sign_type":"SIGN"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"token_use":"project_access","api_key":"ak_999"}"#);
        let jwt = format!("{header}.{payload}.sig");
        let parsed = parse_signing_credential(&jwt).unwrap();
        assert_eq!(parsed.api_key_id, "ak_999");
        assert_eq!(parsed.handshake_field, "accessToken");
    }

    #[test]
    fn rejects_non_signable_jwt_forms() {
        // OAuth login JWTs (claude-style) must not be accepted for signing.
        let wrong_alg = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"alg":"HS256"}"#);
        let project_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"token_use":"project_access","api_key":"ak_1"}"#);
        assert!(parse_signing_credential(&format!("{wrong_alg}.{project_payload}.sig")).is_none());

        let good_header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"alg":"HS256","sign_type":"SIGN"}"#);
        // login JWT (token_use absent or login) is not a signing credential
        let login_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"sub":"user","api_key":"ak_1"}"#);
        assert!(parse_signing_credential(&format!("{good_header}.{login_payload}.sig")).is_none());
        // empty api_key claim
        let empty_payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"token_use":"project_access","api_key":"  "}"#);
        assert!(parse_signing_credential(&format!("{good_header}.{empty_payload}.sig")).is_none());
    }

    #[test]
    fn verify_failure_detection() {
        assert!(ZCodeClientSigner::is_verify_failure(
            401,
            r#"{"code":1004,"msg":"VERIFY_SIGNATURE_INVALID"}"#
        ));
        assert!(ZCodeClientSigner::is_verify_failure(
            401,
            r#"{"data":{"reason":"VERIFY_APIKEY_EXPIRED"}}"#
        ));
        assert!(!ZCodeClientSigner::is_verify_failure(
            401,
            r#"{"msg":"invalid api key"}"#
        ));
        assert!(!ZCodeClientSigner::is_verify_failure(
            200,
            r#"{"msg":"VERIFY_SIGNATURE_INVALID"}"#
        ));
        assert!(!ZCodeClientSigner::is_verify_failure(401, "not json"));
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
