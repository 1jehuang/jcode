use super::*;

/// Explicit opt-in: uses an installed CLI, isolated empty credential homes,
/// and only offline begin/cancel. Never opens a browser or exchanges tokens.
#[test]
#[ignore = "set JCODE_AUTH_TEST_BINARY to an installed CLI for compatibility validation"]
fn installed_cli_begin_cancel_isolated() {
    let binary =
        PathBuf::from(std::env::var_os("JCODE_AUTH_TEST_BINARY").expect("explicit CLI path"));
    for provider in ["claude", "openai"] {
        let home = tempfile::tempdir().unwrap();
        let client = AuthClient::new(AuthOptions {
            binary: binary.clone(),
            jcode_home: Some(home.path().to_owned()),
            socket: Some(home.path().join("absent-daemon.sock")),
            timeout: Duration::from_secs(20),
        });
        let flow = client.begin(provider, None).unwrap();
        let prompt = flow.start().unwrap();
        assert!(prompt.auth_url.starts_with("https://"));
        drop(prompt);
        flow.cancel().unwrap();
        assert!(!home.path().join("auth.json").exists());
        assert!(!home.path().join("openai-auth.json").exists());
        assert!(
            !home
                .path()
                .join("pending-login/flows")
                .join(&flow.0.flow_id)
                .join(format!("{provider}.json"))
                .exists()
        );
    }
}

#[test]
fn catalog_is_capability_filtered_and_uses_shared_aliases() {
    let client = AuthClient::default();
    assert_eq!(client.resolve_provider("OpenAI").unwrap().id, "openai");
    assert_eq!(client.resolve_provider("anthropic").unwrap().id, "claude");
    assert_eq!(
        client.resolve_provider("anthropic-api").unwrap().method,
        LoginMethod::ApiKey
    );
    assert_eq!(
        client.resolve_provider("gemini-api").unwrap().method,
        LoginMethod::ApiKey
    );
    assert_eq!(
        client.resolve_provider("jcode").unwrap().method,
        LoginMethod::ApiKey
    );
    assert!(client.resolve_provider("grok-build").is_none());
    for provider in client.providers() {
        assert_eq!(client.resolve_provider(provider.id), Some(provider));
        assert!(!provider.display_name.is_empty());
    }
    assert!(client.begin("jcode", None).is_err());
}

#[cfg(unix)]
mod processes {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::thread;

    fn fixture(mode: &str) -> (tempfile::TempDir, AuthClient) {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("fixture.py");
        std::fs::write(
            &binary,
            r#"#!/usr/bin/env python3
import json, os, sys, time
from pathlib import Path
home = Path(os.environ['JCODE_HOME'])
args = sys.argv[1:]
provider = args[args.index('--provider') + 1]
flow = args[args.index('--flow-id') + 1]
mode = (home / 'mode').read_text()
(home / 'argv').write_text(json.dumps(args))
if '--cancel' in args:
    (home / ('cancel-' + flow)).write_text('cancelled')
    print(json.dumps(dict(status='cancelled', provider=provider)))
    sys.exit(0)
if '--print-auth-url' in args:
    if mode == 'oversized':
        print('X' * 70000)
        sys.exit(0)
    if mode == 'bad-json':
        print('private-fixture-secret')
        sys.exit(1)
    print(json.dumps(dict(status='pending', provider=provider,
        auth_url='https://example.com/oauth?state=private-fixture-secret',
        input_kind='complete' if provider == 'copilot' else 'callback_url',
        user_code='ABCD-1234', expires_at_ms=9999999999999)))
    sys.exit(0)
if mode == 'hang':
    (home / 'pid').write_text(str(os.getpid()))
    time.sleep(60)
    sys.exit(1)
payload = sys.stdin.read()
(home / 'stdin-ok').write_text(str(payload == 'private-fixture-secret'))
print('private-fixture-secret', file=sys.stderr)
print(json.dumps(dict(status='authenticated', provider=provider)))
sys.exit(1 if mode == 'warning' else 0)
"#,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(dir.path().join("mode"), mode).unwrap();
        let client = AuthClient::new(AuthOptions {
            binary,
            jcode_home: Some(dir.path().to_owned()),
            socket: Some(dir.path().join("daemon.sock")),
            timeout: Duration::from_secs(3),
        });
        (dir, client)
    }

    #[test]
    fn oauth_round_trip_uses_stdin_and_scoped_flow_not_argv() {
        let (dir, client) = fixture("success");
        let flow = client.begin("OpenAI", Some("work")).unwrap();
        let prompt = flow.start().unwrap();
        assert_eq!(prompt.input_kind, AuthInputKind::CallbackUrl);
        assert!(flow.submit_code("private-fixture-secret").is_err());
        assert!(
            !flow
                .submit_callback("private-fixture-secret")
                .unwrap()
                .validation_warning
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("stdin-ok")).unwrap(),
            "True"
        );
        let args = std::fs::read_to_string(dir.path().join("argv")).unwrap();
        assert!(!args.contains("private-fixture-secret"));
        assert!(args.contains("--callback-url"));
        assert!(args.contains("--flow-id"));
        assert!(args.contains("daemon.sock"));
        assert!(!args.contains("--account"));
        assert!(flow.start().is_err());
    }

    #[test]
    fn saved_credentials_and_validation_failure_are_distinct() {
        let (dir, client) = fixture("warning");
        let listener =
            std::os::unix::net::UnixListener::bind(dir.path().join("daemon.sock")).unwrap();
        let notified = thread::spawn(move || {
            use std::io::BufRead;
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut line = String::new();
            std::io::BufReader::new(stream)
                .read_line(&mut line)
                .unwrap();
            serde_json::from_str::<serde_json::Value>(&line).unwrap()
        });
        let flow = client.begin("openai", None).unwrap();
        flow.start().unwrap();
        assert!(
            flow.submit_callback("private-fixture-secret")
                .unwrap()
                .validation_warning
        );
        assert!(flow.submit_callback("private-fixture-secret").is_err());
        let request = notified.join().unwrap();
        assert_eq!(request["type"], "notify_auth_changed");
        assert_eq!(request["provider"], "openai");
        assert!(!request.to_string().contains("private-fixture-secret"));
    }

    #[test]
    fn errors_never_include_untrusted_output_or_callback_input() {
        for mode in ["bad-json", "oversized"] {
            let (_dir, client) = fixture(mode);
            let flow = client.begin("openai", None).unwrap();
            let error = flow.start().err().unwrap();
            assert!(!error.to_string().contains("private-fixture-secret"));
            flow.cancel().unwrap();
        }
    }

    fn wait_for_pid(dir: &std::path::Path) -> i32 {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(dir.join("pid")) {
                return text.parse().unwrap();
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("fixture process did not start")
    }

    #[test]
    fn concurrent_cancel_interrupts_and_reaps_device_polling() {
        let (dir, client) = fixture("hang");
        let flow = client.begin("copilot", None).unwrap();
        assert_eq!(flow.start().unwrap().input_kind, AuthInputKind::DeviceCode);
        let worker = flow.clone();
        let task = thread::spawn(move || worker.complete_device());
        let pid = wait_for_pid(dir.path());
        let start = Instant::now();
        flow.cancel().unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(task.join().unwrap().is_err());
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert!(
            dir.path()
                .join(format!("cancel-{}", flow.0.flow_id))
                .exists()
        );
        assert!(flow.complete_device().is_err());
    }

    #[test]
    fn timeout_reaps_process_and_unique_ids_isolate_cancellation() {
        let (dir, mut client) = fixture("hang");
        client.options.timeout = Duration::from_millis(150);
        let flow = client.begin("copilot", None).unwrap();
        let other = client.begin("copilot", None).unwrap();
        assert_ne!(flow.0.flow_id, other.0.flow_id);
        flow.start().unwrap();
        let err = flow.complete_device().unwrap_err();
        assert_eq!(err.kind, ErrorKind::Timeout);
        let pid = wait_for_pid(dir.path());
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        flow.cancel().unwrap();
        assert!(
            !dir.path()
                .join(format!("cancel-{}", other.0.flow_id))
                .exists()
        );
        other.cancel().unwrap();
    }

    #[test]
    fn last_drop_cleans_pending_flow_without_blocking_ui() {
        let (dir, client) = fixture("success");
        let flow = client.begin("openai", None).unwrap();
        flow.start().unwrap();
        let marker = dir.path().join(format!("cancel-{}", flow.0.flow_id));
        let clone = flow.clone();
        drop(flow);
        assert!(!marker.exists());
        drop(clone);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists());
    }
}
