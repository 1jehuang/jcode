use super::*;

fn account(base: String) -> Account {
    Account {
        api_key: "secret-account-key".into(),
        email: Some("private@example.com".into()),
        api_base: base,
    }
}

fn assert_event(events: &[String], stage: &str, operation: &str, outcome: &str) {
    let prefix = format!(
        "[CLOUD_TIMING] stage={stage} operation={operation} outcome={outcome} duration_ms="
    );
    assert!(
        events.iter().any(|line| line
            .strip_prefix(&prefix)
            .is_some_and(|duration| duration.parse::<u128>().is_ok())),
        "missing {prefix}: {events:?}"
    );
    for line in events {
        assert!(!line.contains("secret"));
        assert!(!line.contains("private@example.com"));
        assert!(!line.contains("ssh-ed25519"));
        assert!(!line.contains("http://"));
    }
}

#[test]
fn successful_wait_records_http_poll_wait_and_total() {
    let (base, _seen) = tests::serve(vec![
        (200, r#"{"ready":false,"message":"secret-progress"}"#),
        (
            200,
            r#"{"ready":true,"address":"203.0.113.7","user":"u","host_keys":["ssh-ed25519 AAAA"]}"#,
        ),
    ]);
    let mut cloud = CloudClient::new("secret-client");
    cloud.poll_interval = Duration::from_millis(1);
    let target = cloud
        .wait_for_host(&account(base), "ssh-ed25519 secret", &|_| {}, &|| false)
        .unwrap();
    assert_eq!(target.user, "u");
    let events = timing::take_events();
    for stage in ["http_send", "http_body", "http_decode", "reply_decode"] {
        assert_event(&events, stage, "host_connect", "success");
    }
    for stage in ["poll", "poll_wait", "total"] {
        assert_event(&events, stage, "wait_for_host", "success");
    }
    assert_eq!(
        events.iter().filter(|e| e.contains("stage=poll ")).count(),
        2
    );
}

#[test]
fn cancelled_wait_records_cancel_without_changing_error() {
    let (base, _seen) = tests::serve(vec![(200, r#"{"ready":false}"#)]);
    let error = CloudClient::new("t")
        .wait_for_host(&account(base), "secret", &|_| {}, &|| true)
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Cancelled);
    let events = timing::take_events();
    assert_event(&events, "poll_wait", "wait_for_host", "cancelled");
    assert_event(&events, "total", "wait_for_host", "cancelled");
}

#[test]
fn expired_deadline_records_timeout_without_sleeping() {
    let (base, _seen) = tests::serve(vec![(200, r#"{"ready":false}"#)]);
    let error = CloudClient::new("t")
        .wait_for_host_until(&account(base), "secret", &|_| {}, &|| false, Instant::now())
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::StartupTimeout);
    let events = timing::take_events();
    assert_event(&events, "total", "wait_for_host", "timeout");
    assert!(!events.iter().any(|e| e.contains("stage=poll_wait ")));
}

#[test]
fn http_error_and_decode_failure_are_safe() {
    for (status, body, stage) in [
        (401, r#"{"error":{"message":"secret-error"}}"#, "http_send"),
        (200, "secret-not-json", "http_decode"),
        (200, r#"{"ready":"secret-wrong-type"}"#, "reply_decode"),
    ] {
        let (base, _seen) = tests::serve(vec![(status, body)]);
        assert!(
            CloudClient::new("t")
                .wait_for_host(&account(base), "secret", &|_| {}, &|| false)
                .is_err()
        );
        let events = timing::take_events();
        assert_event(&events, stage, "host_connect", "error");
        assert_event(&events, "poll", "wait_for_host", "error");
        assert_event(&events, "total", "wait_for_host", "error");
    }
}

#[test]
fn invalid_ready_target_records_total_error() {
    let (base, _seen) = tests::serve(vec![(200, r#"{"ready":true}"#)]);
    assert!(
        CloudClient::new("t")
            .wait_for_host(&account(base), "secret", &|_| {}, &|| false)
            .is_err()
    );
    assert_event(&timing::take_events(), "total", "wait_for_host", "error");
}

#[test]
fn unknown_paths_and_invalid_requests_never_leak_into_labels() {
    assert_eq!(
        timing::operation("POST", "/secret-account?api_key=secret"),
        "unknown"
    );
    let cloud = CloudClient::new("t").with_api_base("secret-invalid-uri");
    assert!(
        cloud
            .post_raw(
                "/secret-account",
                Some("secret"),
                serde_json::json!({"secret":"secret"})
            )
            .is_err()
    );
    assert_event(&timing::take_events(), "http_send", "unknown", "error");
}

#[test]
fn transport_failures_record_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    assert!(
        CloudClient::new("t")
            .wait_for_host(&account(base), "secret", &|_| {}, &|| false)
            .is_err()
    );
    let events = timing::take_events();
    assert_event(&events, "http_send", "host_connect", "error");
    assert_event(&events, "total", "wait_for_host", "error");
}

#[test]
fn http_timeout_classification_is_static() {
    let result: std::result::Result<(), _> = Err(ureq::Error::Timeout(ureq::Timeout::Global));
    assert_eq!(timing::http_outcome(&result), "timeout");
}

#[test]
fn stalled_http_send_and_body_record_timeouts() {
    use std::io::{Read, Write};
    for stage in ["http_send", "http_body"] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            stream.read(&mut request).unwrap();
            if stage == "http_body" {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
            }
            std::thread::sleep(Duration::from_millis(150));
        });
        let mut cloud = CloudClient::new("t");
        cloud.timeout = Duration::from_millis(50);
        let error = cloud
            .request_host_connect(&account(base), "secret")
            .unwrap_err();
        // Keep existing public error kinds, while the HTTP stage can identify
        // the underlying ureq timeout without logging its error text.
        assert_eq!(
            error.kind,
            if stage == "http_send" {
                ErrorKind::ConnectFailed
            } else {
                ErrorKind::Transport
            }
        );
        assert_event(&timing::take_events(), stage, "host_connect", "timeout");
        server.join().unwrap();
    }
}

#[test]
fn truncated_body_records_error_without_decoding() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nsecret")
            .unwrap();
    });
    assert!(
        CloudClient::new("t")
            .request_host_connect(&account(base), "secret")
            .is_err()
    );
    let events = timing::take_events();
    assert_event(&events, "http_body", "host_connect", "error");
    assert!(!events.iter().any(|e| e.contains("stage=http_decode ")));
    server.join().unwrap();
}
