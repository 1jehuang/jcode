//! Email addresses other AI tools on this machine are signed in with.
//!
//! Onboarding offers these as one-click choices for the Jcode account email,
//! so most people never have to type one. Reading is strictly local and
//! read-only: no credentials are imported, trusted, or sent anywhere, and
//! only the email address leaves each file.

use serde_json::Value;
use std::path::Path;

/// One address, with every tool it was found in (for a "from Codex, Claude
/// Code" caption). Addresses are compared case-insensitively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedEmail {
    pub email: String,
    pub sources: Vec<&'static str>,
}

/// Emails from the coding-agent subscriptions on this machine, the same tools
/// onboarding already offers to import logins from: Codex, Claude Code,
/// Cursor (CLI and IDE), the Gemini CLI, and the Claude, ChatGPT, Gemini,
/// Antigravity and Google logins Jcode itself stores. Deduplicated in that
/// order. Missing or unreadable files are skipped silently.
pub fn detected_emails() -> Vec<DetectedEmail> {
    let home = |relative: &str| {
        crate::storage::user_home_path(relative)
            .ok()
            .and_then(|path| read_json(&path))
    };
    let jcode = |name: &str| {
        crate::storage::jcode_dir()
            .ok()
            .and_then(|dir| read_json(&dir.join(name)))
    };
    let mut found = vec![
        (
            "Codex",
            home(".codex/auth.json").and_then(|v| codex_email(&v)),
        ),
        (
            "Claude Code",
            home(".claude.json").and_then(|v| claude_code_email(&v)),
        ),
        (
            "Cursor",
            home(".cursor/cli-config.json").and_then(|v| cursor_cli_email(&v)),
        ),
        ("Cursor", crate::auth::cursor::vscdb_cached_email()),
        (
            "Gemini CLI",
            home(".gemini/google_accounts.json").and_then(|v| gemini_email(&v)),
        ),
    ];
    let accounts = |value: Option<Value>, list: &str| {
        value
            .map(|v| account_list_emails(&v, list))
            .unwrap_or_default()
    };
    for email in accounts(jcode("auth.json"), "anthropic_accounts") {
        found.push(("Claude", Some(email)));
    }
    for email in accounts(jcode("openai-auth.json"), "openai_accounts") {
        found.push(("ChatGPT", Some(email)));
    }
    for (source, file) in [
        ("Gemini", "gemini_oauth.json"),
        ("Antigravity", "antigravity_oauth.json"),
        ("Google", "google_oauth.json"),
    ] {
        found.push((source, jcode(file).and_then(|v| top_level_email(&v))));
    }
    collect(found)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn codex_email(auth: &Value) -> Option<String> {
    let token = auth.pointer("/tokens/id_token")?.as_str()?;
    crate::auth::codex::extract_email(token)
}

fn claude_code_email(config: &Value) -> Option<String> {
    Some(
        config
            .pointer("/oauthAccount/emailAddress")?
            .as_str()?
            .to_string(),
    )
}

fn gemini_email(accounts: &Value) -> Option<String> {
    Some(accounts.get("active")?.as_str()?.to_string())
}

fn cursor_cli_email(config: &Value) -> Option<String> {
    Some(config.pointer("/authInfo/email")?.as_str()?.to_string())
}

fn top_level_email(tokens: &Value) -> Option<String> {
    Some(tokens.get("email")?.as_str()?.to_string())
}

/// Every `email` in one of Jcode's multi-account stores, in saved order.
fn account_list_emails(store: &Value, list: &str) -> Vec<String> {
    store
        .get(list)
        .and_then(Value::as_array)
        .map(|accounts| accounts.iter().filter_map(top_level_email).collect())
        .unwrap_or_default()
}

fn collect(found: impl IntoIterator<Item = (&'static str, Option<String>)>) -> Vec<DetectedEmail> {
    let mut emails: Vec<DetectedEmail> = Vec::new();
    for (source, email) in found {
        let Some(email) = email.map(|email| email.trim().to_string()) else {
            continue;
        };
        if !plausible(&email) {
            continue;
        }
        match emails
            .iter_mut()
            .find(|known| known.email.eq_ignore_ascii_case(&email))
        {
            Some(known) if known.sources.contains(&source) => {}
            Some(known) => known.sources.push(source),
            None => emails.push(DetectedEmail {
                email,
                sources: vec![source],
            }),
        }
    }
    emails
}

fn plausible(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !email.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::json;

    fn jwt(payload: Value) -> String {
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        format!("e30.{body}.sig")
    }

    #[test]
    fn reads_each_tools_layout() {
        let codex = json!({"tokens": {"id_token": jwt(json!({"email": "a@x.com"}))}});
        assert_eq!(codex_email(&codex).as_deref(), Some("a@x.com"));
        let claude = json!({"oauthAccount": {"emailAddress": "b@y.org"}});
        assert_eq!(claude_code_email(&claude).as_deref(), Some("b@y.org"));
        let gemini = json!({"active": "c@z.dev", "old": []});
        assert_eq!(gemini_email(&gemini).as_deref(), Some("c@z.dev"));
        assert_eq!(codex_email(&json!({"tokens": {}})), None);
        assert_eq!(claude_code_email(&json!({})), None);
        assert_eq!(gemini_email(&json!({"active": null})), None);
        let cursor = json!({"authInfo": {"email": "d@cur.sor", "userId": 1}});
        assert_eq!(cursor_cli_email(&cursor).as_deref(), Some("d@cur.sor"));
        let google = json!({"access_token": "secret", "email": "e@g.com"});
        assert_eq!(top_level_email(&google).as_deref(), Some("e@g.com"));
        let store = json!({"openai_accounts": [
            {"label": "a", "email": "f@o.ai"},
            {"label": "b"},
            {"label": "c", "email": "g@o.ai"},
        ]});
        assert_eq!(
            account_list_emails(&store, "openai_accounts"),
            vec!["f@o.ai".to_string(), "g@o.ai".to_string()]
        );
        assert!(account_list_emails(&store, "anthropic_accounts").is_empty());
    }

    #[test]
    fn merges_case_insensitive_duplicates_and_drops_junk() {
        let emails = collect([
            ("Codex", Some("Me@Example.com".into())),
            ("Claude Code", Some(" me@example.com ".into())),
            ("Gemini CLI", Some("not-an-email".into())),
            ("Other", None),
            ("Work", Some("w@corp.io".into())),
        ]);
        assert_eq!(
            emails,
            vec![
                DetectedEmail {
                    email: "Me@Example.com".into(),
                    sources: vec!["Codex", "Claude Code"],
                },
                DetectedEmail {
                    email: "w@corp.io".into(),
                    sources: vec!["Work"],
                },
            ]
        );
    }
}
