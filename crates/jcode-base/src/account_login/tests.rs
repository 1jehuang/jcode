use super::*;
use std::io::{Read, Write};

const DEVICE: &str = r#"{"device_code":"fixture-device-secret","flow_id":"public-flow","verification_uri":"https://jcode.sh/account","verification_uri_complete":"https://jcode.sh/account?flow=public-flow","expires_in":600,"interval":3}"#;
const APPROVED: &str = r#"{"api_key":"fixture-account-secret","account_id":"acct_fixture","email":"fixture@example.invalid","tier":"none","status":"inactive"}"#;

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn server(
    responses: Vec<(u16, &'static str, String)>,
) -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for (status, headers, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..end]);
                    let len = head
                        .lines()
                        .find_map(|line| {
                            let (k, v) = line.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            tx.send(String::from_utf8(request).unwrap()).unwrap();
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    (base, rx)
}

#[tokio::test]
async fn account_only_start_and_approval_use_expected_wire_contract() {
    let (base, requests) = server(vec![(200, "", DEVICE.into()), (200, "", APPROVED.into())]);
    let client = client();
    let flow = start_with(&client, &base).await.unwrap();
    assert_eq!(flow.auth_url(), "https://jcode.sh/account?flow=public-flow");
    assert_eq!(flow.interval(), Duration::from_secs(3));
    assert_eq!(flow.expires_in(), Duration::from_secs(600));
    assert!(!flow.is_expired());
    let request = requests.recv().unwrap();
    assert!(request.starts_with("POST /v1/auth/device "));
    let body: serde_json::Value =
        serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body, serde_json::json!({"client_name":"jcode-desktop"}));
    assert!(!format!("{flow:?}").contains("fixture-device-secret"));
    let result = poll(&client, &flow).await.unwrap();
    assert!(!format!("{result:?}").contains("fixture-account-secret"));
    let LoginPoll::Approved(approved) = result else {
        panic!("not approved")
    };
    assert_eq!(approved.email, "fixture@example.invalid");
    assert_eq!(approved.status, "inactive");
    let request = requests.recv().unwrap();
    assert!(request.starts_with("POST /v1/auth/token "));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(request.split_once("\r\n\r\n").unwrap().1)
            .unwrap(),
        serde_json::json!({"device_code":"fixture-device-secret"})
    );
    // Exactly two requests. No /me, provider selection, or checkout follows approval.
    assert!(requests.recv().is_err());
}

#[tokio::test]
async fn pending_slowdown_denial_and_expiry_are_separate_results() {
    let (base, requests) = server(vec![
        (200, "", DEVICE.into()),
        (428, "", "{}".into()),
        (429, "Retry-After: 12\r\n", "{}".into()),
        (429, "", "{}".into()),
        (403, "", r#"{"error":"access_denied"}"#.into()),
        (400, "", r#"{"error":"expired_token"}"#.into()),
    ]);
    let client = client();
    let mut flow = start_with(&client, &base).await.unwrap();
    assert!(matches!(
        poll(&client, &flow).await.unwrap(),
        LoginPoll::Pending
    ));
    assert!(
        matches!(poll(&client, &flow).await.unwrap(), LoginPoll::SlowDown { retry_after } if retry_after == Duration::from_secs(12))
    );
    assert!(
        matches!(poll(&client, &flow).await.unwrap(), LoginPoll::SlowDown { retry_after } if retry_after == Duration::from_secs(8))
    );
    assert!(matches!(
        poll(&client, &flow).await.unwrap(),
        LoginPoll::Denied
    ));
    assert!(matches!(
        poll(&client, &flow).await.unwrap(),
        LoginPoll::Expired
    ));
    flow.expires_in = Duration::ZERO;
    assert!(matches!(
        poll(&client, &flow).await.unwrap(),
        LoginPoll::Expired
    ));
    assert_eq!(requests.iter().count(), 6); // Local expiry makes no HTTP request.
}

#[test]
fn rejects_unsafe_browser_urls() {
    for url in [
        "http://jcode.sh/account?flow=public-flow",
        "https://evil.invalid/account?flow=public-flow",
        "https://jcode.sh@evil.invalid/account?flow=public-flow",
        "https://user:secret@jcode.sh/account?flow=public-flow",
        "https://jcode.sh:444/account?flow=public-flow",
        "https://jcode.sh/checkout?flow=public-flow",
        "https://jcode.sh/account?flow=public-flow&api_key=secret",
        "https://jcode.sh/account?flow=public-flow#secret",
        "https://jcode.sh/account?flow=bad%0Avalue",
        "https://jcode.sh/account?flow=a",
        "https://jcode.sh/account",
    ] {
        assert_eq!(
            public_auth_url(url).unwrap_err(),
            AccountLoginError::InvalidResponse
        );
    }
}

#[tokio::test]
async fn malicious_error_bodies_and_malformed_responses_are_redacted() {
    for (status, body) in [
        (500, r#"{"error":"fixture-account-secret"}"#),
        (200, "fixture-account-secret"),
        (
            200,
            r#"{"device_code":"fixture-account-secret","verification_uri_complete":"https://evil.invalid/secret"}"#,
        ),
    ] {
        let (base, _requests) = server(vec![(status, "", body.into())]);
        let error = start_with(&client(), &base).await.unwrap_err();
        assert!(!format!("{error:?} {error}").contains("fixture-account-secret"));
    }
    let error = AccountLoginError::from(AccountApiError::Offline(
        "https://secret@host/?api_key=secret".into(),
    ));
    assert!(!format!("{error:?} {error}").contains("secret"));
    assert!(error.is_temporary());
}

// Restore even on panic. Never inspect the user's credential file or contact a
// deployed endpoint: both local config and network are isolated before helpers.
struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl EnvGuard {
    fn new() -> Self {
        let keys = [
            "JCODE_HOME",
            "JCODE_API_KEY",
            "JCODE_API_BASE",
            "JCODE_ACCOUNT_ID",
            "JCODE_ACCOUNT_EMAIL",
            "JCODE_TIER",
            "JCODE_SUBSCRIPTION_ACTIVE",
        ];
        Self(
            keys.into_iter()
                .map(|key| {
                    let old = std::env::var_os(key);
                    crate::env::remove_var(key);
                    (key, old)
                })
                .collect(),
        )
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}

#[tokio::test]
async fn explicit_save_and_current_account_are_sandboxed_without_paid_plan() {
    let _lock = crate::storage::lock_test_env();
    let _env = EnvGuard::new();
    let home = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", home.path());
    let client = client();
    assert!(!has_credentials());
    assert!(current_account(&client).await.unwrap().is_none());
    let (base, requests) = server(vec![(200, "", DEVICE.into()), (200, "", APPROVED.into()),
        (200, "", r#"{"account_id":"acct_fixture","email":"fixture@example.invalid","tier":"none","status":"inactive"}"#.into()),
        (401, "", "{}".into())]);
    crate::env::set_var("JCODE_API_BASE", &base);
    let flow = start(&client).await.unwrap();
    let LoginPoll::Approved(approved) = poll(&client, &flow).await.unwrap() else {
        panic!("not approved")
    };
    assert!(!has_credentials()); // Approval is cancellable until explicitly saved.
    save(&approved).unwrap();
    assert!(has_credentials());
    assert!(!subscription_catalog::is_runtime_mode_enabled());
    let path = subscription_catalog::account_credential_path().unwrap();
    assert!(path.starts_with(home.path()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let me = current_account(&client).await.unwrap().unwrap();
    assert_eq!(me.email, "fixture@example.invalid");
    assert!(!me.has_active_paid_plan());
    assert_eq!(
        current_account(&client).await.unwrap_err(),
        AccountLoginError::Unauthorized
    );
    assert!(has_credentials()); // Errors do not silently delete credentials.
    let requests: Vec<_> = requests.iter().collect();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].starts_with("GET /v1/me "));
    assert!(
        requests[2]
            .to_lowercase()
            .contains("authorization: bearer fixture-account-secret")
    );
}

#[tokio::test]
async fn in_flight_poll_is_cancellable_without_a_detached_worker() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("POST /v1/auth/token "));
        seen_tx.send(()).unwrap();
        // Never approve. Client cancellation must not wait for the 15s timeout.
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\n").await;
        std::future::pending::<()>().await;
    });
    let flow = LoginFlow {
        api_base: base,
        device_code: "fixture-device-secret".into(),
        auth_url: "https://jcode.sh/account?flow=public-flow".into(),
        interval: Duration::from_secs(3),
        expires_in: Duration::from_secs(600),
        started_at: Instant::now(),
    };
    let task = tokio::spawn(async move { poll(&client(), &flow).await });
    tokio::time::timeout(Duration::from_secs(5), seen_rx)
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let error = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.is_cancelled());
    peer.abort();
}
