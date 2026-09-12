//! Local, native login control. This is not a chat or tool-call API.
//!
//! Run blocking flow methods on a worker thread and retain a clone for Cancel.
//! Never put prompts, callback input, child output, or API keys in transcripts,
//! telemetry, or debug logs. Remote SDK connections must not use this local client.
use crate::{Error, ErrorKind, Result};
use jcode_provider_metadata::{LoginProviderDescriptor, LoginProviderTarget};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const INPUT_LIMIT: usize = 16 * 1024;
const OUTPUT_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct AuthOptions {
    /// Trusted local executable. No shell is used.
    pub binary: PathBuf,
    /// Credential home. None inherits the current JCODE_HOME/default home.
    pub jcode_home: Option<PathBuf>,
    /// Native daemon socket, NOT the harness API socket.
    pub socket: Option<PathBuf>,
    /// Deadline per subprocess, including device-code polling.
    pub timeout: Duration,
}

impl Default for AuthOptions {
    fn default() -> Self {
        Self {
            binary: std::env::var_os("JCODE_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(|| "jcode".into()),
            jcode_home: None,
            socket: None,
            timeout: Duration::from_secs(900),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginMethod {
    OAuth,
    DeviceCode,
    ApiKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoginProvider {
    pub id: &'static str,
    pub display_name: &'static str,
    pub detail: &'static str,
    pub method: LoginMethod,
}

fn supported(provider: LoginProviderDescriptor) -> Option<LoginProvider> {
    use LoginProviderTarget::*;
    let method = match provider.target {
        Claude | OpenAi | Gemini | Antigravity | Google => LoginMethod::OAuth,
        Copilot => LoginMethod::DeviceCode,
        ClaudeApiKey | OpenAiApiKey | OpenRouter | Cursor | Jcode => LoginMethod::ApiKey,
        // The harness key API currently supports only these catalog profiles.
        OpenAiCompatible(profile) if profile.id == "gemini-api" => LoginMethod::ApiKey,
        _ => return None,
    };
    Some(LoginProvider {
        id: provider.id,
        display_name: provider.display_name,
        detail: provider.menu_detail,
        method,
    })
}

#[derive(Clone, Debug)]
pub struct AuthClient {
    options: AuthOptions,
}

impl AuthClient {
    pub fn new(options: AuthOptions) -> Self {
        Self { options }
    }

    /// Only methods implemented by the scriptable CLI or harness key API.
    /// Jcode subscription is API-key-only here, not its interactive device flow.
    pub fn providers(&self) -> Vec<LoginProvider> {
        jcode_provider_metadata::cli_login_providers()
            .into_iter()
            .filter_map(supported)
            .collect()
    }

    /// Resolve canonical IDs, shared aliases, or display names, then capability-filter.
    pub fn resolve_provider(&self, input: &str) -> Option<LoginProvider> {
        jcode_provider_metadata::resolve_login_provider_loose(input).and_then(supported)
    }

    /// Allocate a unique, cancellable login. Call `start` off the UI thread.
    /// For API-key entries use `JcodeClient::set_api_key`, not this method.
    pub fn begin(&self, provider: &str, account: Option<&str>) -> Result<AuthFlow> {
        let provider = self
            .resolve_provider(provider)
            .ok_or_else(|| invalid("Unsupported native login provider"))?;
        if provider.method == LoginMethod::ApiKey {
            return Err(invalid(
                "This provider uses the SDK API-key provisioning method",
            ));
        }
        if self.options.timeout.is_zero() {
            return Err(invalid("Login timeout must be greater than zero"));
        }
        if account.is_some_and(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control))
        {
            return Err(invalid("Invalid login account label"));
        }
        Ok(AuthFlow(Arc::new(FlowInner {
            options: self.options.clone(),
            provider: provider.id,
            account: account.map(str::to_owned),
            flow_id: uuid::Uuid::new_v4().simple().to_string(),
            cancelled: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            state: Mutex::new(State::Created),
            child: Mutex::new(None),
        })))
    }
}

impl Default for AuthClient {
    fn default() -> Self {
        Self::new(AuthOptions::default())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthInputKind {
    CallbackUrl,
    AuthCode,
    AuthCodeOrCallbackUrl,
    DeviceCode,
}

/// Sensitive UI-only data. Deliberately not Debug or serializable.
pub struct AuthPrompt {
    pub auth_url: String,
    pub input_kind: AuthInputKind,
    pub user_code: Option<String>,
    pub expires_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthResult {
    /// Credentials were stored but CLI post-login validation failed.
    /// Do not retry the spent OAuth code. Refresh auth and offer model recovery.
    pub validation_warning: bool,
}

#[derive(Clone)]
pub struct AuthFlow(Arc<FlowInner>);

#[derive(Clone, Copy)]
enum State {
    Created,
    Pending(AuthInputKind),
    Completed,
}
struct FlowInner {
    options: AuthOptions,
    provider: &'static str,
    account: Option<String>,
    flow_id: String,
    cancelled: AtomicBool,
    finished: AtomicBool,
    // Serializes begin/completion/cancel; cancel flag and child lock remain independent.
    state: Mutex<State>,
    child: Mutex<Option<Child>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    Begin,
    Callback,
    Code,
    Complete,
    Cancel,
}
impl Operation {
    fn flag(self) -> &'static str {
        match self {
            Self::Begin => "--print-auth-url",
            Self::Callback => "--callback-url",
            Self::Code => "--auth-code",
            Self::Complete => "--complete",
            Self::Cancel => "--cancel",
        }
    }
}

fn invalid(message: &'static str) -> Error {
    Error::new(ErrorKind::InvalidOption, message)
}
fn failed(message: &'static str) -> Error {
    Error::new(ErrorKind::Transport, message)
}
fn cancelled() -> Error {
    failed("Login cancelled. Already issued credentials are not revoked.")
}

impl AuthFlow {
    pub fn start(&self) -> Result<AuthPrompt> {
        let mut state = self.0.state.lock().unwrap();
        if !matches!(*state, State::Created) {
            return Err(invalid("Login was already started"));
        }
        let (value, success) = self.0.execute(Operation::Begin, None)?;
        if !success || value["status"] != "pending" {
            return Err(failed(
                "Could not begin login. Check the installed Jcode version and retry.",
            ));
        }
        let auth_url = value["auth_url"]
            .as_str()
            .ok_or_else(|| failed("Missing authorization URL"))?;
        let url = url::Url::parse(auth_url).map_err(|_| failed("Invalid authorization URL"))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || auth_url.chars().any(char::is_control)
        {
            return Err(failed("Authorization URL must use HTTPS"));
        }
        let input_kind = match value["input_kind"].as_str() {
            Some("callback_url") => AuthInputKind::CallbackUrl,
            Some("auth_code") => AuthInputKind::AuthCode,
            Some("auth_code_or_callback_url") => AuthInputKind::AuthCodeOrCallbackUrl,
            Some("complete") => AuthInputKind::DeviceCode,
            _ => return Err(failed("Unsupported login input kind")),
        };
        let expires_at_ms = value["expires_at_ms"]
            .as_i64()
            .ok_or_else(|| failed("Missing login expiration"))?;
        let user_code = value["user_code"]
            .as_str()
            .filter(|s| s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
            .map(str::to_owned);
        *state = State::Pending(input_kind);
        Ok(AuthPrompt {
            auth_url: auth_url.to_owned(),
            input_kind,
            user_code,
            expires_at_ms,
        })
    }

    pub fn submit_callback(&self, input: &str) -> Result<AuthResult> {
        self.complete(Operation::Callback, Some(input))
    }
    pub fn submit_code(&self, input: &str) -> Result<AuthResult> {
        self.complete(Operation::Code, Some(input))
    }
    pub fn complete_device(&self) -> Result<AuthResult> {
        self.complete(Operation::Complete, None)
    }

    fn complete(&self, operation: Operation, input: Option<&str>) -> Result<AuthResult> {
        if input.is_some_and(|s| {
            s.trim().is_empty() || s.len() > INPUT_LIMIT || s.contains(['\n', '\r', '\0'])
        }) {
            return Err(invalid(
                "Login input must be a non-empty single line of at most 16 KiB",
            ));
        }
        let mut state = self.0.state.lock().unwrap();
        let State::Pending(kind) = *state else {
            return Err(invalid("Login is not awaiting completion"));
        };
        let accepted = matches!(
            (kind, operation),
            (AuthInputKind::CallbackUrl, Operation::Callback)
                | (AuthInputKind::AuthCode, Operation::Code)
                | (
                    AuthInputKind::AuthCodeOrCallbackUrl,
                    Operation::Callback | Operation::Code
                )
                | (AuthInputKind::DeviceCode, Operation::Complete)
        );
        if !accepted {
            return Err(invalid("This login requires a different input method"));
        }
        let (value, success) = self.0.execute(operation, input)?;
        if value["status"] != "authenticated" {
            return Err(failed(
                "Login was not completed. Check the callback and retry.",
            ));
        }
        *state = State::Completed;
        self.0.finished.store(true, Ordering::Release);
        if !success {
            // CLI validation errors happen after token persistence but before its
            // normal notification. Use the existing daemon control protocol so
            // recovery also works with older harness bridges.
            self.0.notify_daemon_best_effort();
        }
        Ok(AuthResult {
            validation_warning: !success,
        })
    }

    /// Interrupt polling/exchange, kill and reap the owned subprocess, and remove
    /// only this flow's pending state. Safe to call concurrently through a clone.
    /// Cancellation is not logout and cannot roll back an already completed exchange.
    pub fn cancel(&self) -> Result<()> {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.kill_child();
        let _state = self.0.state.lock().unwrap();
        let (value, success) = self.0.execute(Operation::Cancel, None)?;
        if !success || value["status"] != "cancelled" {
            return Err(failed("Could not clean up pending login"));
        }
        self.0.finished.store(true, Ordering::Release);
        Ok(())
    }
}

impl FlowInner {
    fn notify_daemon_best_effort(&self) {
        let socket = self
            .options
            .socket
            .clone()
            .unwrap_or_else(jcode_harness_api::legacy_socket_path);
        let provider = self.provider;
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            if let Ok(mut stream) = jcode_transport::SyncStream::connect(&socket) {
                #[cfg(unix)]
                let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                let request = serde_json::json!({
                    "type": "notify_auth_changed", "id": 1, "provider": provider
                });
                let _ = writeln!(stream, "{request}");
            }
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(Duration::from_secs(1));
    }

    fn command(&self, operation: Operation) -> Command {
        let mut command = Command::new(&self.options.binary);
        command.args(["--no-update", "--no-selfdev"]);
        if let Some(socket) = &self.options.socket {
            command.arg("--socket").arg(socket);
        }
        if let Some(home) = &self.options.jcode_home {
            command.env("JCODE_HOME", home);
        }
        command.args([
            "login",
            "--provider",
            self.provider,
            "--no-browser",
            "--json",
            "--flow-id",
            &self.flow_id,
            operation.flag(),
        ]);
        if matches!(operation, Operation::Callback | Operation::Code) {
            command.arg("-");
        }
        if operation == Operation::Begin {
            if let Some(account) = &self.account {
                command.arg("--account").arg(account);
            }
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        command
    }

    fn kill_child(&self) {
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn execute(
        &self,
        operation: Operation,
        input: Option<&str>,
    ) -> Result<(serde_json::Value, bool)> {
        let is_cancel = operation == Operation::Cancel;
        if !is_cancel && self.cancelled.load(Ordering::Acquire) {
            return Err(cancelled());
        }
        let mut child = self.command(operation).spawn().map_err(|_| {
            Error::new(
                ErrorKind::JcodeNotFound,
                "Could not start the local Jcode login executable",
            )
        })?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        *self.child.lock().unwrap() = Some(child);
        // Writing even a bounded payload can block on a full pipe. Keep it off the
        // cancellation thread, and never format the payload or I/O error.
        let payload = input.map(|s| s.as_bytes().to_vec());
        std::thread::spawn(move || {
            if let Some(payload) = payload {
                let _ = stdin.write_all(&payload);
            }
        });
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout
                .take((OUTPUT_LIMIT + 1) as u64)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = tx.send(result);
        });
        let deadline = Instant::now()
            + if is_cancel {
                self.options.timeout.min(Duration::from_secs(5))
            } else {
                self.options.timeout
            };
        let result = (|| {
            let mut output = None;
            let mut status = None;
            loop {
                if !is_cancel && self.cancelled.load(Ordering::Acquire) {
                    return Err(cancelled());
                }
                if Instant::now() >= deadline {
                    return Err(Error::new(
                        ErrorKind::Timeout,
                        "Login timed out. Cancel and start a new login.",
                    ));
                }
                if output.is_none() {
                    match rx.recv_timeout(Duration::from_millis(10)) {
                        Ok(Ok(bytes)) if bytes.len() <= OUTPUT_LIMIT => output = Some(bytes),
                        Ok(Ok(_)) => return Err(failed("Login response exceeded size limit")),
                        Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return Err(failed("Could not read login response"));
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if status.is_none() {
                    status = self
                        .child
                        .lock()
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .try_wait()
                        .map_err(|_| failed("Could not wait for login process"))?;
                }
                if let (Some(bytes), Some(status)) = (&output, status) {
                    let value: serde_json::Value = serde_json::from_slice(bytes)
                        .map_err(|_| failed("Invalid login response. Update Jcode and retry."))?;
                    if value["provider"].as_str() != Some(self.provider) {
                        return Err(failed("Login provider mismatch"));
                    }
                    return Ok((value, status.success()));
                }
            }
        })();
        if result.is_err() {
            self.kill_child();
        }
        self.child.lock().unwrap().take();
        result
    }
}

impl Drop for FlowInner {
    fn drop(&mut self) {
        self.kill_child();
        if !self.finished.load(Ordering::Acquire) {
            // Avoid blocking UI destruction. The cleanup process is bounded and reaped.
            let mut command = self.command(Operation::Cancel);
            std::thread::spawn(move || {
                command.stdin(Stdio::null()).stdout(Stdio::null());
                if let Ok(mut child) = command.spawn() {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while Instant::now() < deadline {
                        if matches!(child.try_wait(), Ok(Some(_))) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                }
            });
        }
    }
}

#[cfg(test)]
mod tests;
