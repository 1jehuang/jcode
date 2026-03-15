use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::RwLock;

#[derive(Debug, Clone)]
pub struct CodexCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: Option<String>,
    pub account_id: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiAccount {
    pub label: String,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JcodeOpenAiAuthFile {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub openai_accounts: Vec<OpenAiAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_openai_account: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyAuthFile {
    tokens: Option<LegacyTokens>,
    #[serde(rename = "OPENAI_API_KEY")]
    api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyTokens {
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    account_id: Option<String>,
    expires_at: Option<i64>,
}

static ACTIVE_ACCOUNT_OVERRIDE: RwLock<Option<String>> = RwLock::new(None);

pub fn set_active_account_override(label: Option<String>) {
    if let Ok(mut guard) = ACTIVE_ACCOUNT_OVERRIDE.write() {
        *guard = label;
    }
}

pub fn get_active_account_override() -> Option<String> {
    ACTIVE_ACCOUNT_OVERRIDE
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}

fn jcode_auth_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("openai-auth.json"))
}

fn legacy_auth_path() -> Result<PathBuf> {
    crate::storage::user_home_path(".codex/auth.json")
}

pub fn load_auth_file() -> Result<JcodeOpenAiAuthFile> {
    let path = jcode_auth_path()?;
    let mut auth = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Could not read OpenAI credentials from {:?}", path))?;
        serde_json::from_str(&content).context("Could not parse jcode OpenAI auth file")?
    } else {
        JcodeOpenAiAuthFile::default()
    };

    if auth.openai_accounts.is_empty()
        && let Ok(legacy) = load_legacy_oauth_credentials()
        && !legacy.access_token.is_empty()
    {
        crate::logging::info(
            "Migrating legacy .codex/auth.json OAuth tokens to jcode multi-account OpenAI auth format",
        );
        auth.openai_accounts.push(account_from_credentials(
            "default",
            &legacy,
            legacy.id_token.as_deref().and_then(extract_email),
        ));
        auth.active_openai_account = Some("default".to_string());
        if save_auth_file(&auth).is_ok() {
            let _ = clear_legacy_oauth_tokens();
        }
    }

    Ok(auth)
}

pub fn save_auth_file(auth: &JcodeOpenAiAuthFile) -> Result<()> {
    let auth_path = jcode_auth_path()?;
    let auth_dir = auth_path
        .parent()
        .context("OpenAI auth path has no parent directory")?;
    std::fs::create_dir_all(auth_dir)?;
    crate::platform::set_directory_permissions_owner_only(auth_dir)?;

    let clean = JcodeOpenAiAuthFile {
        openai_accounts: auth.openai_accounts.clone(),
        active_openai_account: auth.active_openai_account.clone(),
    };

    let json = serde_json::to_string_pretty(&clean)?;
    std::fs::write(&auth_path, json)?;
    crate::platform::set_permissions_owner_only(&auth_path)?;
    Ok(())
}

pub fn list_accounts() -> Result<Vec<OpenAiAccount>> {
    let auth = load_auth_file()?;
    Ok(auth.openai_accounts)
}

pub fn active_account_label() -> Option<String> {
    if let Some(label) = get_active_account_override() {
        return Some(label);
    }
    let auth = load_auth_file().ok()?;
    auth.active_openai_account.or_else(|| {
        auth.openai_accounts
            .first()
            .map(|account| account.label.clone())
    })
}

pub fn set_active_account(label: &str) -> Result<()> {
    let mut auth = load_auth_file()?;
    if !auth
        .openai_accounts
        .iter()
        .any(|account| account.label == label)
    {
        anyhow::bail!("No OpenAI account with label '{}' found", label);
    }
    auth.active_openai_account = Some(label.to_string());
    save_auth_file(&auth)?;
    set_active_account_override(Some(label.to_string()));
    Ok(())
}

pub fn upsert_account(account: OpenAiAccount) -> Result<String> {
    let mut auth = load_auth_file()?;
    let label = account.label.clone();

    if let Some(existing) = auth
        .openai_accounts
        .iter_mut()
        .find(|existing| existing.label == label)
    {
        *existing = account;
    } else {
        auth.openai_accounts.push(account);
    }

    if auth.active_openai_account.is_none() || auth.openai_accounts.len() == 1 {
        auth.active_openai_account = Some(label.clone());
    }

    save_auth_file(&auth)?;
    Ok(label)
}

pub fn remove_account(label: &str) -> Result<()> {
    let mut auth = load_auth_file()?;
    let before = auth.openai_accounts.len();
    auth.openai_accounts
        .retain(|account| account.label != label);
    if auth.openai_accounts.len() == before {
        anyhow::bail!("No OpenAI account with label '{}' found", label);
    }

    if auth.active_openai_account.as_deref() == Some(label) {
        auth.active_openai_account = auth.openai_accounts.first().map(|a| a.label.clone());
    }

    save_auth_file(&auth)?;

    if get_active_account_override().as_deref() == Some(label) {
        set_active_account_override(auth.active_openai_account.clone());
    }

    Ok(())
}

pub fn update_account_tokens(
    label: &str,
    access_token: &str,
    refresh_token: &str,
    id_token: Option<String>,
    account_id: Option<String>,
    expires_at: Option<i64>,
) -> Result<()> {
    let mut auth = load_auth_file()?;
    if let Some(account) = auth
        .openai_accounts
        .iter_mut()
        .find(|account| account.label == label)
    {
        account.access_token = access_token.to_string();
        account.refresh_token = refresh_token.to_string();
        account.id_token = id_token.clone();
        account.account_id =
            account_id.or_else(|| id_token.as_deref().and_then(extract_account_id));
        account.expires_at = expires_at;
        account.email = id_token.as_deref().and_then(extract_email);
        save_auth_file(&auth)?;
        Ok(())
    } else {
        anyhow::bail!(
            "No OpenAI account with label '{}' found for token update",
            label
        );
    }
}

pub fn update_account_profile(label: &str, email: Option<String>) -> Result<()> {
    let mut auth = load_auth_file()?;
    if let Some(account) = auth
        .openai_accounts
        .iter_mut()
        .find(|account| account.label == label)
    {
        account.email = email;
        save_auth_file(&auth)?;
        Ok(())
    } else {
        anyhow::bail!(
            "No OpenAI account with label '{}' found for profile update",
            label
        );
    }
}

pub fn load_credentials() -> Result<CodexCredentials> {
    let env_api_key = load_env_api_key();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut expired_candidates: Vec<(&str, CodexCredentials)> = Vec::new();

    if let Ok(creds) = load_jcode_credentials() {
        if creds
            .expires_at
            .map(|expires_at| expires_at > now_ms)
            .unwrap_or(true)
        {
            return Ok(creds);
        }
        expired_candidates.push(("jcode", creds));
    }

    if let Ok(creds) = load_legacy_oauth_credentials() {
        if creds
            .expires_at
            .map(|expires_at| expires_at > now_ms)
            .unwrap_or(true)
        {
            return Ok(creds);
        }
        expired_candidates.push(("legacy", creds));
    }

    if let Ok(creds) = load_legacy_api_key_credentials() {
        return Ok(creds);
    }

    if let Some(api_key) = env_api_key {
        return Ok(CodexCredentials {
            access_token: api_key,
            refresh_token: String::new(),
            id_token: None,
            account_id: None,
            expires_at: None,
        });
    }

    if let Some((source, creds)) = expired_candidates.into_iter().next() {
        crate::logging::info(&format!(
            "{} OpenAI OAuth token expired; will attempt refresh.",
            source
        ));
        return Ok(creds);
    }

    anyhow::bail!("No OpenAI tokens or API key found")
}

pub fn load_credentials_for_account(label: &str) -> Result<CodexCredentials> {
    let auth = load_auth_file()?;
    let account = auth
        .openai_accounts
        .iter()
        .find(|account| account.label == label)
        .with_context(|| format!("No OpenAI account with label '{}'", label))?;
    Ok(credentials_from_account(account))
}

pub fn upsert_account_from_tokens(
    label: &str,
    access_token: &str,
    refresh_token: &str,
    id_token: Option<String>,
    expires_at: Option<i64>,
) -> Result<String> {
    let creds = CodexCredentials {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        account_id: id_token.as_deref().and_then(extract_account_id),
        id_token,
        expires_at,
    };
    let email = creds.id_token.as_deref().and_then(extract_email);
    upsert_account(account_from_credentials(label, &creds, email))
}

fn load_jcode_credentials() -> Result<CodexCredentials> {
    let auth = load_auth_file()?;
    if auth.openai_accounts.is_empty() {
        anyhow::bail!("No OpenAI accounts configured in jcode auth file")
    }

    let active_label = get_active_account_override()
        .or(auth.active_openai_account)
        .unwrap_or_else(|| "default".to_string());

    let account = auth
        .openai_accounts
        .iter()
        .find(|account| account.label == active_label)
        .or_else(|| auth.openai_accounts.first())
        .context("No OpenAI accounts in jcode auth file")?;

    Ok(credentials_from_account(account))
}

fn load_legacy_oauth_credentials() -> Result<CodexCredentials> {
    let file = load_legacy_auth_file()?;
    let tokens = file
        .tokens
        .context("No OAuth tokens found in legacy Codex auth file")?;
    Ok(credentials_from_legacy_tokens(&tokens))
}

fn load_legacy_api_key_credentials() -> Result<CodexCredentials> {
    let file = load_legacy_auth_file()?;
    let api_key = file
        .api_key
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .context("No API key found in legacy Codex auth file")?;
    Ok(CodexCredentials {
        access_token: api_key,
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    })
}

fn load_legacy_auth_file() -> Result<LegacyAuthFile> {
    let path = legacy_auth_path()?;
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read credentials from {:?}", path))?;
    serde_json::from_str(&content).context("Could not parse Codex credentials")
}

fn clear_legacy_oauth_tokens() -> Result<()> {
    let path = legacy_auth_path()?;
    if !path.exists() {
        return Ok(());
    }

    let mut auth = load_legacy_auth_file()?;
    auth.tokens = None;

    if auth.api_key.is_some() {
        let json = serde_json::to_string_pretty(&auth)?;
        std::fs::write(&path, json)?;
        crate::platform::set_permissions_owner_only(&path)?;
    } else {
        std::fs::remove_file(&path)?;
    }

    Ok(())
}

fn credentials_from_account(account: &OpenAiAccount) -> CodexCredentials {
    CodexCredentials {
        access_token: account.access_token.clone(),
        refresh_token: account.refresh_token.clone(),
        id_token: account.id_token.clone(),
        account_id: account
            .account_id
            .clone()
            .or_else(|| account.id_token.as_deref().and_then(extract_account_id)),
        expires_at: account.expires_at,
    }
}

fn credentials_from_legacy_tokens(tokens: &LegacyTokens) -> CodexCredentials {
    CodexCredentials {
        access_token: tokens.access_token.clone(),
        refresh_token: tokens.refresh_token.clone(),
        id_token: tokens.id_token.clone(),
        account_id: tokens
            .account_id
            .clone()
            .or_else(|| tokens.id_token.as_deref().and_then(extract_account_id)),
        expires_at: tokens.expires_at,
    }
}

fn account_from_credentials(
    label: &str,
    credentials: &CodexCredentials,
    email: Option<String>,
) -> OpenAiAccount {
    OpenAiAccount {
        label: label.to_string(),
        access_token: credentials.access_token.clone(),
        refresh_token: credentials.refresh_token.clone(),
        id_token: credentials.id_token.clone(),
        account_id: credentials
            .account_id
            .clone()
            .or_else(|| credentials.id_token.as_deref().and_then(extract_account_id)),
        expires_at: credentials.expires_at,
        email,
    }
}

fn load_env_api_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn extract_account_id(id_token: &str) -> Option<String> {
    let payload = decode_jwt_payload(id_token)?;
    let auth = payload.get("https://api.openai.com/auth")?;
    auth.get("chatgpt_account_id")?
        .as_str()
        .map(|value| value.to_string())
}

pub fn extract_email(id_token: &str) -> Option<String> {
    let payload = decode_jwt_payload(id_token)?;
    payload
        .get("email")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload_b64 = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload_b64.as_bytes()).ok()?;
    serde_json::from_slice::<Value>(&decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, previous }
        }

        fn set_path(key: &'static str, value: &std::path::Path) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                crate::env::set_var(self.key, previous);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn auth_file_with_oauth_tokens() {
        let json = r#"{
            "tokens": {
                "access_token": "at_openai_123",
                "refresh_token": "rt_openai_456",
                "id_token": "header.payload.signature",
                "account_id": "acct_789",
                "expires_at": 9999999999999
            }
        }"#;
        let file: LegacyAuthFile = serde_json::from_str(json).unwrap();
        let tokens = file.tokens.unwrap();
        assert_eq!(tokens.access_token, "at_openai_123");
        assert_eq!(tokens.refresh_token, "rt_openai_456");
        assert_eq!(
            tokens.id_token,
            Some("header.payload.signature".to_string())
        );
        assert_eq!(tokens.account_id, Some("acct_789".to_string()));
        assert_eq!(tokens.expires_at, Some(9999999999999));
    }

    #[test]
    fn auth_file_with_api_key_only() {
        let json = r#"{
            "OPENAI_API_KEY": "sk-test-key-123"
        }"#;
        let file: LegacyAuthFile = serde_json::from_str(json).unwrap();
        assert!(file.tokens.is_none());
        assert_eq!(file.api_key, Some("sk-test-key-123".to_string()));
    }

    #[test]
    fn auth_file_minimal_tokens() {
        let json = r#"{
            "tokens": {
                "access_token": "at",
                "refresh_token": "rt"
            }
        }"#;
        let file: LegacyAuthFile = serde_json::from_str(json).unwrap();
        let tokens = file.tokens.unwrap();
        assert_eq!(tokens.access_token, "at");
        assert!(tokens.id_token.is_none());
        assert!(tokens.account_id.is_none());
        assert!(tokens.expires_at.is_none());
    }

    #[test]
    fn decode_jwt_payload_valid() {
        let payload = serde_json::json!({
            "sub": "user123",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct_abc"
            }
        });
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{}.signature", payload_b64);

        let decoded = decode_jwt_payload(&token).unwrap();
        assert_eq!(decoded["sub"], "user123");
    }

    #[test]
    fn extract_account_id_from_jwt() {
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct_test_123"
            }
        });
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{}.signature", payload_b64);

        assert_eq!(
            extract_account_id(&token),
            Some("acct_test_123".to_string())
        );
    }

    #[test]
    fn extract_email_from_jwt() {
        let payload = serde_json::json!({
            "email": "user@example.com"
        });
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let token = format!("header.{}.signature", payload_b64);

        assert_eq!(extract_email(&token), Some("user@example.com".to_string()));
    }

    #[test]
    fn load_credentials_falls_back_to_env_api_key() {
        let _lock = ENV_LOCK.lock().unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        let _api_key = EnvVarGuard::set("OPENAI_API_KEY", "sk-env-test");

        let creds = load_credentials().unwrap();
        assert_eq!(creds.access_token, "sk-env-test");
        assert!(creds.refresh_token.is_empty());
        assert!(creds.id_token.is_none());
        assert!(creds.expires_at.is_none());
    }

    #[test]
    fn multi_account_active_switch_works() {
        let _lock = ENV_LOCK.lock().unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

        upsert_account(OpenAiAccount {
            label: "personal".to_string(),
            access_token: "at_personal".to_string(),
            refresh_token: "rt_personal".to_string(),
            id_token: None,
            account_id: Some("acct_personal".to_string()),
            expires_at: Some(10),
            email: Some("personal@example.com".to_string()),
        })
        .unwrap();
        upsert_account(OpenAiAccount {
            label: "work".to_string(),
            access_token: "at_work".to_string(),
            refresh_token: "rt_work".to_string(),
            id_token: None,
            account_id: Some("acct_work".to_string()),
            expires_at: Some(20),
            email: Some("work@example.com".to_string()),
        })
        .unwrap();

        assert_eq!(active_account_label().as_deref(), Some("personal"));
        set_active_account("work").unwrap();
        assert_eq!(active_account_label().as_deref(), Some("work"));

        let creds = load_credentials().unwrap();
        assert_eq!(creds.access_token, "at_work");
        assert_eq!(creds.account_id.as_deref(), Some("acct_work"));
    }

    #[test]
    fn load_auth_file_migrates_legacy_codex_tokens() {
        let _lock = ENV_LOCK.lock().unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

        let legacy_path = temp
            .path()
            .join("external")
            .join(".codex")
            .join("auth.json");
        std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        std::fs::write(
            &legacy_path,
            r#"{
                "tokens": {
                    "access_token": "at_legacy",
                    "refresh_token": "rt_legacy",
                    "account_id": "acct_legacy",
                    "expires_at": 1234
                }
            }"#,
        )
        .unwrap();

        let auth = load_auth_file().unwrap();
        assert_eq!(auth.openai_accounts.len(), 1);
        assert_eq!(auth.openai_accounts[0].label, "default");
        assert_eq!(auth.active_openai_account.as_deref(), Some("default"));
        assert!(
            !legacy_path.exists(),
            "expected legacy OAuth file to be removed after migration"
        );
    }

    #[test]
    fn load_auth_file_clears_legacy_oauth_tokens_but_keeps_api_key() {
        let _lock = ENV_LOCK.lock().unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

        let legacy_path = temp
            .path()
            .join("external")
            .join(".codex")
            .join("auth.json");
        std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        std::fs::write(
            &legacy_path,
            r#"{
                "tokens": {
                    "access_token": "at_legacy",
                    "refresh_token": "rt_legacy",
                    "account_id": "acct_legacy",
                    "expires_at": 1234
                },
                "OPENAI_API_KEY": "sk-legacy"
            }"#,
        )
        .unwrap();

        let auth = load_auth_file().unwrap();
        assert_eq!(auth.openai_accounts.len(), 1);

        let legacy: LegacyAuthFile =
            serde_json::from_str(&std::fs::read_to_string(&legacy_path).unwrap()).unwrap();
        assert!(legacy.tokens.is_none());
        assert_eq!(legacy.api_key.as_deref(), Some("sk-legacy"));
    }
}
