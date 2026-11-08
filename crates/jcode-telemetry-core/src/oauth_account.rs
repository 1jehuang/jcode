//! OAuth identity delivery is separate from events and never accepts tokens.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        mpsc::{SyncSender, sync_channel},
    },
    time::Duration,
};

pub const OAUTH_ACCOUNT_NOTICE_TEXT: &str =
    "Telemetry includes your OAuth email. /telemetry to opt out.";

/// Opaque receipt. A poll is not an acknowledgment: callers must acknowledge
/// only after the status line containing the notice has actually been rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthAccountNotice {
    receipt: String,
}
struct Candidate {
    provider: String,
    email: String,
}
struct Pending {
    notice: OAuthAccountNotice,
    id: String,
    candidate: Candidate,
    marker: PathBuf,
    _lease: File,
    acknowledged: bool,
}
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
static SENDER: OnceLock<Option<SyncSender<Command>>> = OnceLock::new();
enum Command {
    Candidate(Candidate),
    Displayed(OAuthAccountNotice),
}

/// Nonblocking, bounded enqueue. No identity is created while opted out.
/// Repeated calls are safe, including after deferred network failures.
pub fn enqueue_oauth_account(provider: &str, email: &str) -> bool {
    if !super::TelemetryCategory::Accounts.enabled() {
        return false;
    }
    let email = canonical_email(email);
    if !valid_email(&email) {
        return false;
    }
    let provider = match provider.to_ascii_lowercase().as_str() {
        "anthropic" | "claude" => "anthropic",
        "openai" | "codex" => "openai",
        "copilot" | "github" => "copilot",
        "gemini" => "gemini",
        "antigravity" => "antigravity",
        _ => return false,
    };
    send(Command::Candidate(Candidate {
        provider: provider.to_ascii_lowercase(),
        email,
    }))
}

/// Does not consume the pending notice or authorize identity upload.
pub fn pending_oauth_account_notice() -> Option<OAuthAccountNotice> {
    if !super::is_enabled() {
        return None;
    }
    let pending = match PENDING.try_lock() {
        Ok(pending) => pending,
        // Render-time reads are deliberately nonblocking. A later frame retries.
        Err(std::sync::TryLockError::WouldBlock) => return None,
        Err(std::sync::TryLockError::Poisoned(_)) => {
            super::logging::warn("OAuth telemetry notice state is poisoned, withholding notice");
            return None;
        }
    };
    pending
        .as_ref()
        .filter(|p| !p.acknowledged)
        .map(|p| p.notice.clone())
}

/// Call after a successful rendered frame, not when a notice is merely queued.
/// Persistence and upload happen on the worker, never on the render thread.
pub fn acknowledge_oauth_account_notice_displayed(notice: &OAuthAccountNotice) -> bool {
    let Ok(mut pending) = PENDING.try_lock() else {
        return false;
    };
    let Some(p) = pending
        .as_mut()
        .filter(|p| p.notice == *notice && !p.acknowledged)
    else {
        return false;
    };
    if !send(Command::Displayed(notice.clone())) {
        return false;
    }
    p.acknowledged = true;
    true
}
fn send(command: Command) -> bool {
    SENDER
        .get_or_init(|| {
            let (tx, rx) = sync_channel(32);
            let worker = std::thread::Builder::new()
                .name("oauth-telemetry".into())
                .spawn(move || {
                    let Ok(client) = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(5))
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                    else {
                        return;
                    };
                    let transport = Http(client);
                    let mut attempts = std::collections::HashMap::new();
                    for command in rx {
                        if let Command::Candidate(c) = &command {
                            let key = hash(&format!("{}:{}", c.provider, c.email));
                            let now = std::time::Instant::now();
                            if attempts.get(&key).is_some_and(|at: &std::time::Instant| {
                                now.duration_since(*at) < Duration::from_secs(60)
                            }) {
                                continue;
                            }
                            if attempts.len() >= 64 {
                                attempts.clear();
                            }
                            attempts.insert(key, now);
                        }
                        handle(command, &transport);
                    }
                });
            match worker {
                Ok(_) => Some(tx),
                Err(_) => {
                    super::logging::warn("OAuth telemetry worker could not start");
                    None
                }
            }
        })
        .as_ref()
        .is_some_and(|tx| tx.try_send(command).is_ok())
}
fn valid_email(email: &str) -> bool {
    if email.len() > 254
        || !email.is_ascii()
        || email
            .bytes()
            .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
    {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty() && !domain.is_empty() && !domain.contains('@') && domain.contains('.')
}
fn canonical_email(email: &str) -> String {
    email.trim().to_lowercase()
}
fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
struct Check {
    installation_known: bool,
    email_tracked: bool,
}
trait Transport {
    fn check(&self, id: &str, candidate: &Candidate) -> Option<Check>;
    fn upload(&self, id: &str, candidate: &Candidate);
}
struct Http(reqwest::blocking::Client);
impl Transport for Http {
    fn check(&self, id: &str, c: &Candidate) -> Option<Check> {
        // Unit tests cannot accidentally contact the real telemetry service.
        if cfg!(test) {
            return None;
        }
        let response = (|| {
            self.0
                .post("https://telemetry.jcode.sh/v1/oauth-account/check")
                .json(&json!({"id":id,"provider":c.provider,"email_hash":hash(&c.email)}))
                .send()?
                .error_for_status()?
                .json::<Value>()
        })();
        let response = match response {
            Ok(response) => response,
            Err(_) => {
                // Never include response bodies, URLs, or identity values.
                super::logging::debug(
                    "OAuth telemetry preflight failed, identity delivery deferred",
                );
                return None;
            }
        };
        Some(Check {
            installation_known: response.get("installation_known")?.as_bool()?,
            email_tracked: response.get("email_tracked")?.as_bool()?,
        })
    }
    fn upload(&self, id: &str, c: &Candidate) {
        if cfg!(test) || !super::is_enabled() {
            return;
        }
        // Never log requests, responses, errors, or raw identity values.
        if self
            .0
            .post("https://telemetry.jcode.sh/v1/oauth-account")
            .json(&json!({"id":id,"provider":c.provider,"email":c.email}))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .is_err()
        {
            super::logging::debug("OAuth telemetry identity upload failed");
        }
    }
}
#[derive(Debug, PartialEq)]
enum Decision {
    Defer,
    Skip,
    Notice,
    Upload,
}
fn decision(enabled: bool, check: Option<Check>, displayed: bool) -> Decision {
    if !enabled {
        return Decision::Defer;
    }
    match check {
        None => Decision::Defer,
        Some(c) if c.email_tracked => Decision::Skip,
        Some(c) if c.installation_known && !displayed => Decision::Notice,
        Some(_) => Decision::Upload,
    }
}
fn private_file(path: &Path) -> Option<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => Some(file),
        Err(_) => {
            super::logging::warn("OAuth telemetry notice file could not be opened");
            None
        }
    }
}
fn notice_displayed(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|v| v == b"displayed\n")
}
fn persist_displayed(path: &Path) -> bool {
    use std::io::Write;
    let Some(mut file) = private_file(path) else {
        return false;
    };
    file.write_all(b"displayed\n").is_ok() && file.sync_all().is_ok()
}
fn handle(command: Command, transport: &impl Transport) {
    handle_in(
        command,
        transport,
        &super::is_enabled,
        &|| {
            Some((
                super::get_or_create_id()?,
                match super::storage::jcode_dir() {
                    Ok(dir) => dir,
                    Err(_) => {
                        super::logging::warn("OAuth telemetry home could not be resolved");
                        return None;
                    }
                },
            ))
        },
        &PENDING,
    );
}
fn handle_in(
    command: Command,
    transport: &impl Transport,
    enabled: &impl Fn() -> bool,
    identity: &impl Fn() -> Option<(String, PathBuf)>,
    state: &Mutex<Option<Pending>>,
) {
    if !enabled() && matches!(command, Command::Candidate(_)) {
        if let Ok(mut pending) = state.lock() {
            *pending = None;
        }
        return;
    }
    match command {
        Command::Candidate(candidate) => {
            if state.lock().map_or(true, |p| p.is_some()) {
                return;
            }
            let Some((id, dir)) = identity() else {
                return;
            };
            if std::fs::create_dir_all(&dir).is_err() {
                super::logging::warn("OAuth telemetry notice directory could not be created");
                return;
            }
            // A kernel lease is released on crash. Never unlink the lock inode.
            let key = hash(&id);
            let Some(lease) = private_file(&dir.join(format!("oauth_notice_{key}.lock"))) else {
                return;
            };
            if lease.try_lock().is_err() {
                return;
            }
            let marker = dir.join(format!("oauth_notice_{key}.displayed"));
            if !enabled() {
                return;
            }
            let check = transport.check(&id, &candidate);
            match decision(enabled(), check, notice_displayed(&marker)) {
                Decision::Upload => {
                    if enabled() {
                        transport.upload(&id, &candidate);
                    }
                }
                Decision::Notice => {
                    if let Ok(mut pending) = state.lock() {
                        *pending = Some(Pending {
                            notice: OAuthAccountNotice {
                                receipt: uuid::Uuid::new_v4().to_string(),
                            },
                            id,
                            candidate,
                            marker,
                            _lease: lease,
                            acknowledged: false,
                        });
                    }
                }
                _ => {}
            }
        }
        Command::Displayed(notice) => {
            let Ok(mut pending) = state.lock() else {
                return;
            };
            if !pending.as_ref().is_some_and(|p| p.notice == notice) {
                return;
            }
            let Some(p) = pending.take() else {
                return;
            };
            drop(pending);
            // Marker contains no identity, keyed only by the installation hash.
            if !persist_displayed(&p.marker) {
                return;
            }
            // Re-preflight after display: opt-out and server tracking may change.
            if !enabled() {
                return;
            }
            let check = transport.check(&p.id, &p.candidate);
            if decision(enabled(), check, true) == Decision::Upload && enabled() {
                transport.upload(&p.id, &p.candidate);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(known: bool, tracked: bool) -> Option<Check> {
        Some(Check {
            installation_known: known,
            email_tracked: tracked,
        })
    }
    struct Mock {
        known: bool,
        tracked: bool,
        fail: bool,
        calls: std::cell::Cell<usize>,
        uploads: std::cell::Cell<usize>,
        opt_out_on_check: Option<std::rc::Rc<std::cell::Cell<bool>>>,
    }
    impl Mock {
        fn new(known: bool, tracked: bool, fail: bool) -> Self {
            Self {
                known,
                tracked,
                fail,
                calls: Default::default(),
                uploads: Default::default(),
                opt_out_on_check: None,
            }
        }
    }
    impl Transport for Mock {
        fn check(&self, _: &str, _: &Candidate) -> Option<Check> {
            self.calls.set(self.calls.get() + 1);
            if let Some(enabled) = &self.opt_out_on_check {
                enabled.set(false);
            }
            if self.fail {
                None
            } else {
                check(self.known, self.tracked)
            }
        }
        fn upload(&self, _: &str, _: &Candidate) {
            self.uploads.set(self.uploads.get() + 1);
        }
    }
    fn candidate() -> Command {
        Command::Candidate(Candidate {
            provider: "openai".into(),
            email: "alice@example.com".into(),
        })
    }
    #[test]
    fn mock_notice_ack_restart_and_stale_ack() {
        let dir = tempfile::tempdir().unwrap();
        let identity = || Some(("installation".into(), dir.path().to_path_buf()));
        let state = Mutex::new(None);
        let transport = Mock::new(true, false, false);
        handle_in(candidate(), &transport, &|| true, &identity, &state);
        let (notice, marker) = {
            let p = state.lock().unwrap();
            let p = p.as_ref().unwrap();
            (p.notice.clone(), p.marker.clone())
        };
        // Polling and duplicate candidates never authorize an upload.
        handle_in(candidate(), &transport, &|| true, &identity, &state);
        assert_eq!(transport.uploads.get(), 0);
        assert!(!notice_displayed(&marker));
        handle_in(
            Command::Displayed(OAuthAccountNotice {
                receipt: "stale".into(),
            }),
            &transport,
            &|| true,
            &identity,
            &state,
        );
        assert!(!notice_displayed(&marker));
        handle_in(
            Command::Displayed(notice.clone()),
            &transport,
            &|| true,
            &identity,
            &state,
        );
        assert_eq!(transport.uploads.get(), 1);
        assert!(notice_displayed(&marker));
        assert!(state.lock().unwrap().is_none());
        handle_in(
            Command::Displayed(notice),
            &transport,
            &|| true,
            &identity,
            &state,
        );
        assert_eq!(transport.uploads.get(), 1);
        // Simulate a restart with a fresh process state and another account.
        let restarted = Mutex::new(None);
        let other = Command::Candidate(Candidate {
            provider: "anthropic".into(),
            email: "other@example.com".into(),
        });
        handle_in(other, &transport, &|| true, &identity, &restarted);
        assert!(restarted.lock().unwrap().is_none());
        assert_eq!(transport.uploads.get(), 2);
    }
    #[test]
    fn mock_opt_out_before_identity_and_during_preflight() {
        let state = Mutex::new(None);
        let transport = Mock::new(false, false, false);
        handle_in(
            candidate(),
            &transport,
            &|| false,
            &|| panic!("opt-out must not read/create identity"),
            &state,
        );
        assert_eq!(transport.calls.get(), 0);
        let dir = tempfile::tempdir().unwrap();
        let enabled = std::rc::Rc::new(std::cell::Cell::new(true));
        let mut transport = Mock::new(false, false, false);
        transport.opt_out_on_check = Some(enabled.clone());
        handle_in(
            candidate(),
            &transport,
            &|| enabled.get(),
            &|| Some(("install".into(), dir.path().to_path_buf())),
            &state,
        );
        assert_eq!(transport.calls.get(), 1);
        assert_eq!(transport.uploads.get(), 0);
        assert!(state.lock().unwrap().is_none());
    }
    #[test]
    fn mock_opt_out_after_render_persists_notice_without_traffic() {
        let dir = tempfile::tempdir().unwrap();
        let identity = || Some(("install".into(), dir.path().to_path_buf()));
        let state = Mutex::new(None);
        let transport = Mock::new(true, false, false);
        handle_in(candidate(), &transport, &|| true, &identity, &state);
        let (notice, marker) = {
            let p = state.lock().unwrap();
            let p = p.as_ref().unwrap();
            (p.notice.clone(), p.marker.clone())
        };
        handle_in(
            Command::Displayed(notice),
            &transport,
            &|| false,
            &identity,
            &state,
        );
        assert!(notice_displayed(&marker));
        assert_eq!(transport.calls.get(), 1);
        assert_eq!(transport.uploads.get(), 0);
    }
    #[test]
    fn mock_new_tracked_and_failed_preflight() {
        for (known, tracked, fail, uploads) in [
            (false, false, false, 1),
            (true, true, false, 0),
            (false, true, false, 0),
            (true, false, true, 0),
            (false, false, true, 0),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let state = Mutex::new(None);
            let transport = Mock::new(known, tracked, fail);
            handle_in(
                candidate(),
                &transport,
                &|| true,
                &|| Some(("install".into(), dir.path().to_path_buf())),
                &state,
            );
            assert_eq!(transport.uploads.get(), uploads);
            assert!(state.lock().unwrap().is_none());
        }
    }
    #[test]
    fn mock_concurrent_pending_and_failed_ack_preflight() {
        let dir = tempfile::tempdir().unwrap();
        let identity = || Some(("install".into(), dir.path().to_path_buf()));
        let first = Mutex::new(None);
        let second = Mutex::new(None);
        let transport = Mock::new(true, false, false);
        handle_in(candidate(), &transport, &|| true, &identity, &first);
        handle_in(candidate(), &transport, &|| true, &identity, &second);
        assert_eq!(transport.calls.get(), 1);
        assert!(second.lock().unwrap().is_none());
        let (notice, marker) = {
            let p = first.lock().unwrap();
            let p = p.as_ref().unwrap();
            (p.notice.clone(), p.marker.clone())
        };
        let failing = Mock::new(true, false, true);
        handle_in(
            Command::Displayed(notice),
            &failing,
            &|| true,
            &identity,
            &first,
        );
        assert_eq!(failing.uploads.get(), 0);
        assert!(notice_displayed(&marker));
        handle_in(candidate(), &transport, &|| true, &identity, &second);
        assert!(second.lock().unwrap().is_none());
        assert_eq!(transport.uploads.get(), 1);
    }
    #[test]
    fn incomplete_marker_is_not_acknowledgment() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        private_file(&marker).unwrap();
        assert!(!notice_displayed(&marker));
        assert!(persist_displayed(&marker));
        assert!(notice_displayed(&marker));
    }
    #[test]
    fn strict_email_validation() {
        assert!(valid_email("alice@example.com"));
        for invalid in [
            "",
            "alice",
            "@example.com",
            "a@",
            "a@b@c.com",
            "a b@example.com",
            "é@example.com",
            "a@example",
        ] {
            assert!(!valid_email(invalid));
        }
        assert!(!valid_email(&format!("{}@example.com", "x".repeat(255))));
    }
    #[test]
    fn canonical_hash() {
        assert_eq!(
            canonical_email("  Alice@EXAMPLE.COM \n"),
            "alice@example.com"
        );
        assert_eq!(
            hash("alice@example.com"),
            "ff8d9819fc0e12bf0d24892e45987e249a28dce836a85cad60e28eaaa8c6d976"
        );
    }
    #[test]
    fn preflight_state_machine() {
        for known in [false, true] {
            for tracked in [false, true] {
                for displayed in [false, true] {
                    assert_eq!(
                        decision(false, check(known, tracked), displayed),
                        Decision::Defer
                    );
                    let expected = if tracked {
                        Decision::Skip
                    } else if known && !displayed {
                        Decision::Notice
                    } else {
                        Decision::Upload
                    };
                    assert_eq!(decision(true, check(known, tracked), displayed), expected);
                }
            }
        }
        assert_eq!(decision(true, None, false), Decision::Defer);
        assert_eq!(decision(true, None, true), Decision::Defer);
    }
    #[test]
    fn persistent_install_marker_and_kernel_lease() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        let first = private_file(&path).unwrap();
        first.try_lock().unwrap();
        let second = private_file(&path).unwrap();
        assert!(second.try_lock().is_err());
        drop(first);
        second.try_lock().unwrap();
        let marker = dir
            .path()
            .join(format!("oauth_notice_{}.displayed", hash("install-a")));
        assert!(!marker.exists());
        private_file(&marker).unwrap().sync_all().unwrap();
        assert!(marker.exists());
        assert!(
            !dir.path()
                .join(format!("oauth_notice_{}.displayed", hash("install-b")))
                .exists()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                marker.metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
