//! Jcode Cloud: account sign-in and the managed host's connection target.
//!
//! Shared by every client of the Jcode Cloud control plane (desktop, iOS, CLI
//! tools) so sign-in, wake/provision polling and host-key pinning behave the
//! same everywhere. Blocking HTTP via `ureq`, no async runtime, no `ssh`
//! binary: it builds unchanged for iOS.
//!
//! ```no_run
//! use jcode_sdk::cloud::CloudClient;
//! let cloud = CloudClient::new("my-app");
//! let token = cloud.start_email_login("me@example.com")?;
//! let account = cloud.verify_email_login(&token, "123456")?;
//! # let key_public_openssh = String::new();
//! let target = cloud.wait_for_host(&account, &key_public_openssh, &|m| println!("{m}"), &|| false)?;
//! println!("{}@{}:{}", target.user, target.address, target.port);
//! # Ok::<(), jcode_sdk::Error>(())
//! ```
//!
//! With the `ssh-native` feature, [`connect`] does the whole thing (fresh key,
//! wake, pinned in-process SSH, handshake) and returns a [`crate::JcodeClient`].

use crate::{Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Production control plane.
pub const DEFAULT_API_BASE: &str = "https://api.jcode.sh/v1";
/// How long [`CloudClient::wait_for_host`] waits for a host to provision.
pub const PROVISION_DEADLINE: Duration = Duration::from_secs(8 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(4);

/// A signed-in Jcode account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub api_key: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default = "default_base")]
    pub api_base: String,
}

fn default_base() -> String {
    DEFAULT_API_BASE.into()
}

/// APNs environment a device token belongs to. Development-signed builds get
/// sandbox tokens; TestFlight and App Store builds get production tokens.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PushEnvironment {
    Production,
    Sandbox,
}

/// A device registration for `POST /push/devices`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PushDevice {
    /// Lowercase hex APNs device token.
    pub token: String,
    pub environment: PushEnvironment,
    pub bundle_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
}

/// What a push is about. Drives the title, category, and interruption level.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PushKind {
    TurnDone,
    TurnError,
    NeedsInput,
    Wake,
}

/// A notification for `POST /push/send`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PushNotification {
    pub kind: PushKind,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_title: Option<String>,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<u64>,
}

/// `POST /push/send` reply. `devices == 0` means nobody is listening.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct PushReceipt {
    #[serde(default)]
    pub sent: u32,
    #[serde(default)]
    pub devices: u32,
}

/// Hex-encode a raw APNs device token.
pub fn push_token_hex(raw: &[u8]) -> String {
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

/// A pending browser/device sign-in. Show `verification_uri_complete`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceLogin {
    pub device_code: String,
    pub verification_uri_complete: String,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default)]
    pub expires_in: u64,
}

fn default_interval() -> u64 {
    5
}

/// One `POST /cloud/host/connect` reply.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ConnectReply {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub host_keys: Vec<String>,
}

/// A validated SSH target. Only IP literals, safe user names and
/// ed25519/ecdsa-p256 host keys are accepted, so nothing the control plane
/// returns can smuggle SSH options or shell syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudTarget {
    pub address: std::net::IpAddr,
    pub port: u16,
    pub user: String,
    /// OpenSSH `type base64` lines. The connection must match one of them.
    pub host_keys: Vec<String>,
}

impl CloudTarget {
    /// Validate a ready reply.
    pub fn from_reply(reply: &ConnectReply) -> Result<Self> {
        let invalid = |m: &str| Error::new(ErrorKind::UnexpectedReply, m);
        let address = reply
            .address
            .as_deref()
            .ok_or_else(|| invalid("Jcode Cloud returned no host address"))?
            .parse()
            .map_err(|_| invalid("Jcode Cloud returned an invalid host address"))?;
        let user = reply
            .user
            .as_deref()
            .filter(|u| {
                !u.is_empty()
                    && u.len() <= 32
                    && !u.starts_with('-')
                    && u.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            })
            .ok_or_else(|| invalid("Jcode Cloud returned an invalid user"))?;
        let host_keys: Vec<String> = reply
            .host_keys
            .iter()
            .filter(|k| {
                let mut p = k.split(' ');
                matches!((p.next(), p.next(), p.next()),
                    (Some("ssh-ed25519" | "ecdsa-sha2-nistp256"), Some(blob), None)
                        if !blob.is_empty() && blob.len() <= 1024
                            && blob.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)))
            })
            .cloned()
            .collect();
        if host_keys.is_empty() {
            return Err(invalid("Jcode Cloud did not provide verified host keys"));
        }
        if reply.port == Some(0) {
            return Err(invalid("Jcode Cloud returned an invalid port"));
        }
        Ok(Self {
            address,
            port: reply.port.unwrap_or(22),
            user: user.into(),
            host_keys,
        })
    }

    /// A known_hosts file pinning exactly this address, for system OpenSSH
    /// ([`crate::SshConnectOptions::known_hosts_file`]).
    pub fn known_hosts(&self) -> String {
        let host = match (self.address, self.port) {
            (std::net::IpAddr::V4(a), 22) => a.to_string(),
            (std::net::IpAddr::V6(a), 22) => a.to_string(),
            (a, p) => format!("[{a}]:{p}"),
        };
        self.host_keys
            .iter()
            .map(|k| format!("{host} {k}\n"))
            .collect()
    }
}

/// Talks to the Jcode Cloud control plane.
#[derive(Debug, Clone)]
pub struct CloudClient {
    /// e.g. `https://api.jcode.sh/v1`, no trailing slash.
    pub api_base: String,
    /// Identifies the app to the account service (`jcode-ios`, `jcode-desktop`).
    pub client_name: String,
    pub timeout: Duration,
    /// Delay between host wake/provision polls.
    pub poll_interval: Duration,
}

impl CloudClient {
    pub fn new(client_name: impl Into<String>) -> Self {
        Self {
            api_base: DEFAULT_API_BASE.into(),
            client_name: client_name.into(),
            timeout: Duration::from_secs(30),
            poll_interval: POLL_INTERVAL,
        }
    }

    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        self.api_base = base.into().trim_end_matches('/').to_string();
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.api_base)
    }

    fn account(&self, reply: LoginReply) -> Account {
        Account {
            api_key: reply.api_key,
            email: reply.email,
            api_base: self.api_base.clone(),
        }
    }

    /// Email a 6-digit code. Returns the login token for [`Self::verify_email_login`].
    pub fn start_email_login(&self, email: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Start {
            login_token: String,
        }
        let r: Start = self.post(
            "/auth/email/start",
            None,
            serde_json::json!({"email": email, "client_name": self.client_name}),
        )?;
        Ok(r.login_token)
    }

    pub fn verify_email_login(&self, login_token: &str, code: &str) -> Result<Account> {
        let r: LoginReply = self.post(
            "/auth/email/verify",
            None,
            serde_json::json!({"login_token": login_token, "code": code}),
        )?;
        Ok(self.account(r))
    }

    /// Start a browser sign-in (device flow).
    pub fn start_device_login(&self) -> Result<DeviceLogin> {
        self.post(
            "/auth/device",
            None,
            serde_json::json!({"client_name": self.client_name}),
        )
    }

    /// Poll once. `Ok(None)` while the user has not approved yet.
    /// `slow_down` raises `login.interval`.
    pub fn poll_device_login(&self, login: &mut DeviceLogin) -> Result<Option<Account>> {
        let (status, body) = self.post_raw(
            "/auth/token",
            None,
            serde_json::json!({"device_code": login.device_code}),
        )?;
        if (200..300).contains(&status) {
            let r: LoginReply = serde_json::from_value(body).map_err(|e| {
                Error::new(
                    ErrorKind::UnexpectedReply,
                    format!("Unexpected reply from jcode.sh: {e}"),
                )
            })?;
            return Ok(Some(self.account(r)));
        }
        let err = &body["error"];
        match err["code"].as_str().unwrap_or("") {
            "authorization_pending" => Ok(None),
            "slow_down" => {
                login.interval = err["interval"]
                    .as_u64()
                    .or(body["interval"].as_u64())
                    .unwrap_or(login.interval + 2);
                Ok(None)
            }
            code => Err(describe(
                status,
                code,
                err["message"].as_str().unwrap_or(""),
            )),
        }
    }

    /// Poll until approved, expired, or `cancelled()` returns true.
    pub fn wait_for_device_login(
        &self,
        login: &DeviceLogin,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Account> {
        let mut login = login.clone();
        let deadline = Instant::now()
            + Duration::from_secs(if login.expires_in > 0 {
                login.expires_in
            } else {
                900
            });
        loop {
            sleep_unless(Duration::from_secs(login.interval.max(2)), cancelled)?;
            if Instant::now() > deadline {
                return Err(Error::new(
                    ErrorKind::CloudAuth,
                    "Sign-in expired. Try again.",
                ));
            }
            if let Some(account) = self.poll_device_login(&mut login)? {
                return Ok(account);
            }
        }
    }

    /// Ask the control plane to authorize `public_key_openssh` on the
    /// account's host for a short window, waking or provisioning it.
    pub fn request_host_connect(
        &self,
        account: &Account,
        public_key_openssh: &str,
    ) -> Result<ConnectReply> {
        let client = Self {
            api_base: account.api_base.trim_end_matches('/').to_string(),
            ..self.clone()
        };
        client.post(
            "/cloud/host/connect",
            Some(&account.api_key),
            serde_json::json!({"public_key": public_key_openssh}),
        )
    }

    /// Poll [`Self::request_host_connect`] until the host is ready, reporting
    /// human-readable progress, for at most [`PROVISION_DEADLINE`].
    pub fn wait_for_host(
        &self,
        account: &Account,
        public_key_openssh: &str,
        progress: &dyn Fn(&str),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<CloudTarget> {
        let deadline = Instant::now() + PROVISION_DEADLINE;
        let mut last = String::new();
        loop {
            let reply = self.request_host_connect(account, public_key_openssh)?;
            if reply.ready {
                return CloudTarget::from_reply(&reply);
            }
            let message = reply
                .message
                .clone()
                .unwrap_or_else(|| format!("Jcode Cloud machine is {}…", reply.state));
            if message != last {
                progress(&message);
                last = message;
            }
            if Instant::now() >= deadline {
                return Err(Error::new(
                    ErrorKind::StartupTimeout,
                    "Jcode Cloud did not become ready in time. Retry to keep waiting.",
                ));
            }
            sleep_unless(self.poll_interval, cancelled)?;
        }
    }

    /// Register this device's APNs token so the account's jcode servers can
    /// push to it. `token_hex` is the lowercase hex APNs device token.
    pub fn register_push_device(&self, account: &Account, device: &PushDevice) -> Result<()> {
        let body = serde_json::to_value(device)
            .map_err(|e| Error::new(ErrorKind::Transport, e.to_string()))?;
        self.account_call(account, "POST", "/push/devices", body)
    }

    /// Forget an APNs token (sign-out, notifications turned off).
    pub fn unregister_push_device(&self, account: &Account, token_hex: &str) -> Result<()> {
        self.account_call(
            account,
            "DELETE",
            "/push/devices",
            serde_json::json!({"token": token_hex}),
        )
    }

    /// Reset the account's unread badge (the app came to the foreground).
    pub fn reset_push_badge(&self, account: &Account) -> Result<()> {
        self.account_call(account, "POST", "/push/badge", serde_json::json!({"count": 0}))
    }

    /// Ask the control plane to push `notification` to every device on the
    /// account. Servers call this with their own account key.
    pub fn send_push(&self, account: &Account, notification: &PushNotification) -> Result<PushReceipt> {
        let client = Self {
            api_base: account.api_base.trim_end_matches('/').to_string(),
            ..self.clone()
        };
        let body = serde_json::to_value(notification)
            .map_err(|e| Error::new(ErrorKind::Transport, e.to_string()))?;
        client.post("/push/send", Some(&account.api_key), body)
    }

    fn account_call(
        &self,
        account: &Account,
        method: &str,
        path: &str,
        body: serde_json::Value,
    ) -> Result<()> {
        let client = Self {
            api_base: account.api_base.trim_end_matches('/').to_string(),
            ..self.clone()
        };
        let (status, body) = client.send_raw(method, path, Some(&account.api_key), body)?;
        if (200..300).contains(&status) {
            return Ok(());
        }
        let err = &body["error"];
        Err(describe(
            status,
            err["code"].as_str().unwrap_or(""),
            err["message"].as_str().unwrap_or(""),
        ))
    }

    fn agent(&self) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(self.timeout))
            .http_status_as_error(false)
            .user_agent(format!(
                "{} jcode-sdk-rs/{}",
                self.client_name,
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .into()
    }

    fn post_raw(
        &self,
        path: &str,
        api_key: Option<&str>,
        body: serde_json::Value,
    ) -> Result<(u16, serde_json::Value)> {
        self.send_raw("POST", path, api_key, body)
    }

    fn send_raw(
        &self,
        method: &str,
        path: &str,
        api_key: Option<&str>,
        body: serde_json::Value,
    ) -> Result<(u16, serde_json::Value)> {
        let mut req = ureq::http::Request::builder()
            .method(method)
            .uri(self.url(path))
            .header("Content-Type", "application/json");
        if let Some(k) = api_key {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        let req = req
            .body(body.to_string())
            .map_err(|e| Error::new(ErrorKind::Transport, e.to_string()))?;
        let mut resp = self.agent().run(req).map_err(|e| {
            Error::new(
                ErrorKind::ConnectFailed,
                format!("Could not reach jcode.sh: {e}"),
            )
        })?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| Error::new(ErrorKind::Transport, e.to_string()))?;
        Ok((
            status,
            serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
        ))
    }

    fn post<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        api_key: Option<&str>,
        body: serde_json::Value,
    ) -> Result<T> {
        let (status, body) = self.post_raw(path, api_key, body)?;
        if (200..300).contains(&status) {
            return serde_json::from_value(body).map_err(|e| {
                Error::new(
                    ErrorKind::UnexpectedReply,
                    format!("Unexpected reply from jcode.sh: {e}"),
                )
            });
        }
        let err = &body["error"];
        Err(describe(
            status,
            err["code"].as_str().unwrap_or(""),
            err["message"].as_str().unwrap_or(""),
        ))
    }
}

#[derive(Deserialize)]
struct LoginReply {
    api_key: String,
    #[serde(default)]
    email: Option<String>,
}

fn sleep_unless(total: Duration, cancelled: &dyn Fn() -> bool) -> Result<()> {
    let until = Instant::now() + total;
    while Instant::now() < until {
        if cancelled() {
            return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// Map a control-plane error to an SDK error with a sentence a user can act on.
pub fn describe(status: u16, code: &str, message: &str) -> Error {
    match (status, code) {
        (401, _) => Error::new(
            ErrorKind::CloudAuth,
            "Your Jcode sign-in has expired. Sign in again.",
        ),
        (402, "cloud_not_entitled") => Error::new(
            ErrorKind::CloudNotEntitled,
            "Jcode Cloud requires an active paid Jcode subscription.",
        ),
        (402, _) if !message.is_empty() => Error::new(ErrorKind::CloudNotEntitled, message),
        (400..500, _) if !message.is_empty() => Error::new(ErrorKind::InvalidOption, message),
        _ if !message.is_empty() => Error::new(ErrorKind::Transport, message),
        _ => Error::new(
            ErrorKind::Transport,
            format!("Request failed (HTTP {status})"),
        ),
    }
}

/// Wake the account's host and connect a client over in-process SSH: fresh
/// ed25519 key, pinned host keys, `jcode api --stdio`. No `ssh` binary.
#[cfg(feature = "ssh-native")]
pub fn connect(
    cloud: &CloudClient,
    account: &Account,
    progress: &dyn Fn(&str),
    cancelled: &dyn Fn() -> bool,
) -> Result<crate::JcodeClient> {
    let key = crate::native_ssh::NativeSshKey::generate(&cloud.client_name)?;
    let target = cloud.wait_for_host(account, key.public_openssh(), progress, cancelled)?;
    progress("Connecting to your Jcode Cloud machine…");
    let mut options = crate::NativeSshOptions::new(target, key);
    options.client_name = cloud.client_name.clone();
    crate::JcodeClient::connect_native_ssh(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(v: serde_json::Value) -> ConnectReply {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn target_validation_rejects_injection() {
        let ok = reply(
            serde_json::json!({"ready":true,"address":"203.0.113.7","user":"ec2-user","host_keys":["ssh-ed25519 AAAAC3", "ssh-rsa AAAA"]}),
        );
        let t = CloudTarget::from_reply(&ok).unwrap();
        assert_eq!(t.port, 22);
        assert_eq!(t.host_keys, vec!["ssh-ed25519 AAAAC3".to_string()]);
        assert_eq!(t.known_hosts(), "203.0.113.7 ssh-ed25519 AAAAC3\n");
        for bad in [
            serde_json::json!({"address":"evil.example","user":"u","host_keys":["ssh-ed25519 AAAA"]}),
            serde_json::json!({"address":"203.0.113.7","user":"-l root","host_keys":["ssh-ed25519 AAAA"]}),
            serde_json::json!({"address":"203.0.113.7","user":"u","host_keys":["ssh-rsa AAAA"]}),
            serde_json::json!({"address":"203.0.113.7","user":"u","host_keys":["ssh-ed25519 A\n* x"]}),
            serde_json::json!({"address":"203.0.113.7","user":"u","port":0,"host_keys":["ssh-ed25519 AAAA"]}),
        ] {
            assert!(
                CloudTarget::from_reply(&reply(bad.clone())).is_err(),
                "accepted {bad}"
            );
        }
    }

    #[test]
    fn known_hosts_brackets_non_default_ports() {
        let t = CloudTarget::from_reply(&reply(serde_json::json!({"address":"::1","port":2222,"user":"u","host_keys":["ssh-ed25519 AAAA"]}))).unwrap();
        assert_eq!(t.known_hosts(), "[::1]:2222 ssh-ed25519 AAAA\n");
    }

    #[test]
    fn errors_explain_sign_in_and_subscription() {
        assert_eq!(describe(401, "", "").kind, ErrorKind::CloudAuth);
        assert_eq!(
            describe(402, "cloud_not_entitled", "").kind,
            ErrorKind::CloudNotEntitled
        );
        assert_eq!(describe(500, "", "").message, "Request failed (HTTP 500)");
    }

    /// Minimal one-shot HTTP server for flow tests.
    fn serve(replies: Vec<(u16, &'static str)>) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for (status, body) in replies {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = s.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        let body = &text[end + 4..];
                        let chunked = text[..end]
                            .to_ascii_lowercase()
                            .contains("transfer-encoding: chunked");
                        if (chunked && body.ends_with("0\r\n\r\n"))
                            || (!chunked && buf.len() >= end + 4 + len)
                        {
                            tx.send(text).unwrap();
                            break;
                        }
                    }
                }
                write!(s, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (base, rx)
    }

    #[test]
    fn email_login_sends_client_name_and_returns_account() {
        let (base, seen) = serve(vec![
            (200, r#"{"login_token":"lt"}"#),
            (200, r#"{"api_key":"k","email":"a@b"}"#),
        ]);
        let cloud = CloudClient::new("jcode-test").with_api_base(base.clone());
        assert_eq!(cloud.start_email_login("a@b").unwrap(), "lt");
        let first = seen.recv().unwrap();
        assert!(first.starts_with("POST /auth/email/start"));
        let body: serde_json::Value =
            serde_json::from_str(first.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["client_name"], "jcode-test", "{first}");
        let account = cloud.verify_email_login("lt", "123456").unwrap();
        assert_eq!(
            account,
            Account {
                api_key: "k".into(),
                email: Some("a@b".into()),
                api_base: base
            }
        );
    }

    #[test]
    fn device_poll_handles_pending_slow_down_and_success() {
        let (base, _seen) = serve(vec![
            (400, r#"{"error":{"code":"authorization_pending"}}"#),
            (400, r#"{"error":{"code":"slow_down","interval":9}}"#),
            (200, r#"{"api_key":"k"}"#),
        ]);
        let cloud = CloudClient::new("t").with_api_base(base);
        let mut login = DeviceLogin {
            device_code: "d".into(),
            verification_uri_complete: "u".into(),
            interval: 5,
            expires_in: 0,
        };
        assert!(cloud.poll_device_login(&mut login).unwrap().is_none());
        assert!(cloud.poll_device_login(&mut login).unwrap().is_none());
        assert_eq!(login.interval, 9);
        assert_eq!(
            cloud
                .poll_device_login(&mut login)
                .unwrap()
                .unwrap()
                .api_key,
            "k"
        );
    }

    #[test]
    fn host_connect_polls_until_ready_with_bearer_auth() {
        let (base, seen) = serve(vec![
            (200, r#"{"state":"provisioning","ready":false}"#),
            (
                200,
                r#"{"state":"running","ready":true,"address":"203.0.113.7","user":"u","host_keys":["ssh-ed25519 AAAA"]}"#,
            ),
        ]);
        let mut cloud = CloudClient::new("t");
        cloud.poll_interval = Duration::from_millis(10);
        let account = Account {
            api_key: "secret".into(),
            email: None,
            api_base: base,
        };
        let progress = std::sync::Mutex::new(Vec::new());
        let target = cloud
            .wait_for_host(
                &account,
                "ssh-ed25519 KEY",
                &|m| progress.lock().unwrap().push(m.to_string()),
                &|| false,
            )
            .unwrap();
        assert_eq!(target.user, "u");
        assert_eq!(
            progress.into_inner().unwrap(),
            vec!["Jcode Cloud machine is provisioning…"]
        );
        let req = seen.recv().unwrap().to_ascii_lowercase();
        assert!(req.contains("authorization: bearer secret"));
    }

    #[test]
    fn push_device_lifecycle_and_send() {
        let (base, seen) = serve(vec![
            (200, r#"{"ok":true}"#),
            (200, r#"{"sent":1,"devices":1}"#),
            (200, r#"{"ok":true}"#),
            (401, r#"{"error":{"code":"unauthorized","message":"revoked"}}"#),
        ]);
        let cloud = CloudClient::new("t");
        let account = Account {
            api_key: "k".into(),
            email: None,
            api_base: base,
        };
        let device = PushDevice {
            token: push_token_hex(&[0xab, 0x01]),
            environment: PushEnvironment::Production,
            bundle_id: "sh.jcode.mobile.rust".into(),
            device_name: Some("iPhone".into()),
        };
        cloud.register_push_device(&account, &device).unwrap();
        let req = seen.recv().unwrap();
        assert!(req.starts_with("POST /push/devices"));
        assert!(req.contains(r#""token":"ab01""#));
        assert!(req.contains(r#""environment":"production""#));

        let receipt = cloud
            .send_push(
                &account,
                &PushNotification {
                    kind: PushKind::TurnDone,
                    session_id: "s1".into(),
                    session_title: None,
                    body: "done".into(),
                    turn_id: Some(3),
                },
            )
            .unwrap();
        assert_eq!(receipt, PushReceipt { sent: 1, devices: 1 });
        assert!(seen.recv().unwrap().contains(r#""kind":"turn_done""#));

        cloud.unregister_push_device(&account, "ab01").unwrap();
        assert!(seen.recv().unwrap().starts_with("DELETE /push/devices"));

        assert!(cloud.reset_push_badge(&account).is_err());
    }

    #[test]
    #[test]
    fn unauthorized_host_connect_is_cloud_auth() {
        let (base, _seen) = serve(vec![(
            401,
            r#"{"error":{"code":"unauthorized","message":"x"}}"#,
        )]);
        let account = Account {
            api_key: "k".into(),
            email: None,
            api_base: base,
        };
        let err = CloudClient::new("t")
            .request_host_connect(&account, "k")
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::CloudAuth);
    }
}
