use super::*;
use base64::Engine;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const KEY: &str = "synthetic-compile-test-key";

#[derive(Clone, Debug)]
struct Request {
    head: String,
    body: Vec<u8>,
}

struct Server {
    base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn new(handler: impl Fn(&Request) -> Vec<u8> + Send + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let end = loop {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end + 4;
                    }
                    assert!(bytes.len() < 64 * 1024);
                };
                let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
                let length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                while bytes.len() < end + length {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).await.unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let request = Request {
                    head,
                    body: bytes[end..end + length].to_vec(),
                };
                captured.lock().unwrap().push(request.clone());
                let response = handler(&request);
                // Size-limit tests intentionally close the connection early.
                let _ = stream.write_all(&response).await;
            }
        });
        Self {
            base,
            requests,
            task,
        }
    }

    fn captured(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn response(status: u16, body: impl AsRef<[u8]>) -> Vec<u8> {
    let body = body.as_ref();
    let mut result = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
    result.extend_from_slice(body);
    result
}

fn me(status: &str, capability: Option<bool>) -> String {
    let mut value = json!({"account_id":"test-account", "email":"test@example.invalid", "tier":"plus", "status":status});
    if let Some(enabled) = capability {
        value["capabilities"] = json!({"remote_compile":enabled});
    }
    value.to_string()
}

fn build_request() -> BuildRequest {
    BuildRequest {
        request_id: "synthetic-request".into(),
        command: "rustc main.rs".into(),
        timeout_seconds: 1,
        files: vec![source::SourceFile {
            path: "main.rs".into(),
            content_base64: "c291cmNl".into(),
            executable: false,
        }],
    }
}

fn context(root: &Path) -> ToolContext {
    ToolContext {
        session_id: "remote-test-session".into(),
        message_id: "message".into(),
        tool_call_id: "remote-test-call".into(),
        working_dir: Some(root.into()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}

// AuthTestSandbox acquires crate::storage::lock_test_env itself. Do not lock twice.
struct Environment {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _sandbox: crate::auth::test_sandbox::AuthTestSandbox,
}

impl Environment {
    fn new() -> Self {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
        let saved = ["JCODE_API_KEY", "JCODE_API_BASE"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        crate::env::remove_var("JCODE_API_KEY");
        crate::env::remove_var("JCODE_API_BASE");
        *ACCESS.lock().unwrap() = None;
        Self {
            saved,
            _sandbox: sandbox,
        }
    }

    fn configure(&self, base: &str) {
        crate::env::set_var("JCODE_API_KEY", KEY);
        crate::env::set_var("JCODE_API_BASE", base);
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        *ACCESS.lock().unwrap() = None;
        for (key, value) in &self.saved {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
    }
}

#[tokio::test]
async fn authenticated_access_transport_and_fail_closed_states() {
    for (status, body, expected) in [
        (200, me("active", Some(true)), Access::Ready),
        (200, me("ACTIVE", Some(true)), Access::Ready),
        (
            200,
            me("inactive", Some(true)),
            Access::SubscriptionRequired,
        ),
        (200, me("active", Some(false)), Access::NotEnabled),
        (200, me("active", None), Access::NotEnabled),
        (401, "credential rejected".into(), Access::SignedOut),
        (402, "payment".into(), Access::SubscriptionRequired),
        (403, "subscription".into(), Access::SubscriptionRequired),
        (503, "offline".into(), Access::Unknown),
        (200, "not-json".into(), Access::Unknown),
        (200, "x".repeat(64 * 1024 + 1), Access::Unknown),
    ] {
        let server = Server::new(move |_| response(status, &body)).await;
        assert_eq!(
            check_access(&client().unwrap(), &server.base, KEY).await,
            expected
        );
        let requests = server.captured();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].head.starts_with("GET /v1/me HTTP/1.1"));
        assert!(
            requests[0]
                .head
                .to_ascii_lowercase()
                .contains(&format!("authorization: bearer {KEY}"))
        );
        assert!(requests[0].body.is_empty());
    }
}

#[tokio::test]
async fn explicit_compute_entitlement_overrides_promotional_budget_and_capability() {
    for (entitled, capability, budget, expected) in [
        (false, true, 100.0, Access::SubscriptionRequired),
        (false, false, 100.0, Access::SubscriptionRequired),
        (true, false, 0.0, Access::NotEnabled),
        (true, true, 0.0, Access::Ready),
    ] {
        let body = json!({"account_id":"promo-test", "email":"promo@example.invalid",
            "tier":"none", "status":"active", "usage":{"budget_usd":budget},
            "capabilities":{"remote_compile":capability},
            "entitlements":{"cloud_compute":entitled}})
        .to_string();
        let server = Server::new(move |_| response(200, &body)).await;
        assert_eq!(
            check_access(&client().unwrap(), &server.base, KEY).await,
            expected
        );
        assert_eq!(server.captured().len(), 1);
    }
}

#[tokio::test]
async fn submit_preserves_nonzero_exit_output_and_metered_usage() {
    let server = Server::new(|_| {
        response(
            200,
            json!({
                "exit_code": 17, "stdout":"compiler stdout", "stderr":"compiler failed",
                "usage":{"compute_seconds":2}, "truncated":true, "cleanup_confirmed":false
            })
            .to_string(),
        )
    })
    .await;
    let result = submit(&client().unwrap(), &server.base, KEY, build_request())
        .await
        .unwrap();
    assert_eq!(result.exit_code, 17);
    assert_eq!(result.stdout, "compiler stdout");
    assert_eq!(result.stderr, "compiler failed");
    assert_eq!(result.usage.as_ref().unwrap()["compute_seconds"], 2);
    assert_eq!(result.truncated, Some(true));
    assert_eq!(result.cleanup_confirmed, Some(false));
    assert_eq!(
        serde_json::to_value(&result).unwrap()["cleanup_confirmed"],
        false
    );
    let requests = server.captured();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].head.starts_with("POST /v1/compile HTTP/1.1"));
    assert!(
        requests[0]
            .head
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {KEY}"))
    );
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["request_id"], "synthetic-request");
    assert_eq!(body["files"][0]["path"], "main.rs");
    assert_eq!(body["timeout_seconds"], 1);
    assert!(!String::from_utf8_lossy(&requests[0].body).contains(KEY));
}

#[tokio::test]
async fn server_admission_errors_distinguish_credits_subscription_and_uncertain_jobs() {
    for (status, expected) in [
        (401, "sign-in expired"),
        (402, "Insufficient cloud-compute credits"),
        (403, "requires a Jcode subscription"),
        (409, "already admitted"),
        (413, "exceeds the service limit"),
        (429, "concurrency limit"),
        (404, "unavailable"),
        (503, "unavailable"),
        (504, "compute is charged"),
        (500, "HTTP 500"),
    ] {
        let server =
            Server::new(move |_| response(status, "untrusted secret server diagnostic")).await;
        let error = submit(&client().unwrap(), &server.base, KEY, build_request())
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(expected), "{status}: {error}");
        assert!(!error.contains("untrusted secret"));
        assert!(!error.contains(KEY));
        if status == 402 {
            assert!(!error.contains("subscribe"));
            assert!(error.contains("No automatic top-up"));
        }
        if status == 403 {
            assert!(error.contains("https://jcode.sh/pricing"));
        }
        assert_eq!(server.captured().len(), 1, "must not retry admission");
    }
}

#[tokio::test]
async fn redirects_never_forward_credentials_or_source() {
    let target = Server::new(|_| response(200, "{}")).await;
    for status in [301, 302, 303, 307, 308] {
        let location = format!("{}/stolen", target.base);
        let server = Server::new(move |_| format!("HTTP/1.1 {status} Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()).await;
        assert_eq!(
            check_access(&client().unwrap(), &server.base, KEY).await,
            Access::Unknown
        );
        let error = submit(&client().unwrap(), &server.base, KEY, build_request())
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(&format!("HTTP {status}")));
        assert_eq!(server.captured().len(), 2);
    }
    assert!(
        target.captured().is_empty(),
        "redirect target received source or credentials"
    );
}

#[tokio::test]
async fn responses_are_bounded_with_content_length_and_chunked_encoding() {
    for chunked in [false, true] {
        let server = Server::new(move |_| {
            let body = vec![b'x'; MAX_RESPONSE_BYTES + 1];
            if chunked {
                let mut bytes =
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                        .to_vec();
                for chunk in body.chunks(32 * 1024) {
                    bytes.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                    bytes.extend_from_slice(chunk);
                    bytes.extend_from_slice(b"\r\n");
                }
                bytes.extend_from_slice(b"0\r\n\r\n");
                bytes
            } else {
                response(200, body)
            }
        })
        .await;
        let error = submit(&client().unwrap(), &server.base, KEY, build_request())
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("size limit"), "{error}");
    }
    let server = Server::new(|_| response(200, "{\"exit_code\":0}")).await;
    assert!(
        submit(&client().unwrap(), &server.base, KEY, build_request())
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("Invalid remote compilation result")
    );
    let server = Server::new(|_| response(200, "12345678")).await;
    let response = client().unwrap().get(&server.base).send().await.unwrap();
    assert_eq!(bounded_response(response, 8).await.unwrap(), b"12345678");
}

#[test]
fn endpoint_rejects_insecure_or_credential_bearing_destinations() {
    for base in [
        "http://example.invalid",
        "ftp://127.0.0.1",
        "https://user:secret@example.invalid",
        "https://example.invalid?token=secret",
        "https://example.invalid#fragment",
        "not a URL",
    ] {
        assert!(endpoint(base, "compile").is_err(), "{base}");
    }
    for base in [
        "http://127.0.0.1:1234/v1",
        "http://localhost:1234/v1",
        "http://[::1]:1234/v1",
        "https://example.invalid/v1",
    ] {
        assert_eq!(
            endpoint(base, "compile").unwrap(),
            format!("{base}/compile")
        );
    }
}

#[test]
fn validates_timeout_action_and_command_without_network() {
    for value in [
        json!({}),
        json!({"command":" "}),
        json!({"command":"a\u{0000}b"}),
        json!({"command":"x".repeat(8193)}),
        json!({"action":"purchase","command":"build"}),
        json!({"command":"build","timeout_seconds":0}),
        json!({"command":"build","timeout_seconds":601}),
    ] {
        assert!(validate_input(&serde_json::from_value::<Input>(value).unwrap()).is_err());
    }
    for seconds in [1, 300, 600] {
        assert!(
            validate_input(
                &serde_json::from_value::<Input>(
                    json!({"command":"build","timeout_seconds":seconds})
                )
                .unwrap()
            )
            .is_ok()
        );
    }
    assert!(
        validate_input(&serde_json::from_value::<Input>(json!({"action":"status"})).unwrap())
            .is_ok()
    );
    for seconds in [json!(-1), json!(1.5), json!("300")] {
        assert!(
            serde_json::from_value::<Input>(json!({"command":"build","timeout_seconds":seconds}))
                .is_err()
        );
    }
}

#[tokio::test]
async fn execute_denied_access_precedes_snapshot_and_upload_even_with_cached_ready() {
    let env = Environment::new();
    let root = env._sandbox.root().join("does-not-exist");
    let tool = CompileRemoteTool::new();
    assert!(credentials().is_none());
    let error = tool
        .execute(json!({"command":"build"}), context(&root))
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("Not signed in"));
    for (status, body, expected) in [
        (
            200,
            me("inactive", Some(true)),
            "requires a Jcode subscription",
        ),
        (200, me("active", None), "not enabled"),
        (401, "invalid auth".into(), "Not signed in"),
        (503, "offline".into(), "could not yet be verified"),
        (200, "invalid-json".into(), "could not yet be verified"),
    ] {
        let server = Server::new(move |_| response(status, &body)).await;
        env.configure(&server.base);
        *ACCESS.lock().unwrap() = Some(CachedAccess {
            identity: identity(&server.base, KEY),
            checked_at: Instant::now(),
            access: Access::Ready,
        });
        let error = tool
            .execute(json!({"command":"build"}), context(&root))
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(expected), "{error}");
        let requests = server.captured();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].head.starts_with("GET "));
        assert!(requests[0].body.is_empty());
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    env.configure(&base);
    let error = tool
        .execute(json!({"command":"build"}), context(&root))
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("could not yet be verified"));
    assert!(!error.contains("canonicalize"));
}

#[tokio::test]
async fn status_never_snapshots_and_invalid_timeout_never_contacts_service() {
    let env = Environment::new();
    let credits = json!({"unit":"microcredits", "granted_microcredits":1000,
        "charged_microcredits":100, "reserved_microcredits":200, "available_microcredits":700,
        "recent_usage":[{"job_key":"test-job", "workload":"compile", "state":"succeeded",
        "reserved_microcredits":200, "charged_microcredits":100, "actual_seconds":2,
        "created_at":123, "settled_at":125}]});
    for available in [true, false] {
        let fixture = credits.clone();
        let server = Server::new(move |request| {
            if request.head.starts_with("GET /v1/me ") {
                response(200, me("active", Some(true)))
            } else if available {
                response(200, json!({"compute":fixture}).to_string())
            } else {
                response(404, "not configured")
            }
        })
        .await;
        env.configure(&server.base);
        let root = env._sandbox.root().join("nonexistent");
        let tool = CompileRemoteTool::new();
        let result = tool
            .execute(json!({"action":"status"}), context(&root))
            .await
            .unwrap();
        let status: Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(status["access"], "available");
        if available {
            assert_eq!(status["compute"], credits);
            assert!(status.get("compute_error").is_none());
        } else {
            assert!(
                status.get("compute").is_none(),
                "missing service must not imply zero credits"
            );
            assert!(
                status["compute_error"]
                    .as_str()
                    .unwrap()
                    .contains("HTTP 404")
            );
        }
        for timeout in [0, 601] {
            let error = tool
                .execute(
                    json!({"command":"build", "timeout_seconds":timeout}),
                    context(&root),
                )
                .await
                .err()
                .unwrap()
                .to_string();
            assert!(error.contains("between 1 and 600"));
        }
        let requests = server.captured();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].head.starts_with("GET /v1/compute/usage "));
        assert!(
            requests[1]
                .head
                .to_ascii_lowercase()
                .contains(&format!("authorization: bearer {KEY}"))
        );
        assert!(requests.iter().all(|request| request.body.is_empty()));
    }
}

#[tokio::test]
async fn compute_credits_rejects_unknown_units_and_malformed_balances() {
    for body in [
        json!({"compute":{"unit":"usd", "granted_microcredits":1, "charged_microcredits":0, "reserved_microcredits":0, "available_microcredits":1, "recent_usage":[]}}),
        json!({"compute":{"unit":"microcredits", "granted_microcredits":-1}}),
        json!({}),
    ] {
        let server = Server::new(move |_| response(200, body.to_string())).await;
        assert!(
            compute_credits(&client().unwrap(), &server.base, KEY)
                .await
                .is_err()
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn execute_uploads_selected_worktree_and_loopback_compiles_real_source() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let env = Environment::new();
    let workspace = tempfile::tempdir().unwrap();
    let repo = workspace.path().join("project");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let mut command = std::process::Command::new("git");
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("GIT_") {
                command.env_remove(name);
            }
        }
        assert!(
            command
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-q"]);
    for (path, contents) in [
        ("main.rs", "invalid original"),
        ("deleted.rs", "deleted"),
        (".env", "synthetic secret"),
        (".gitignore", "ignored\n"),
    ] {
        std::fs::write(repo.join(path), contents).unwrap();
    }
    git(&["add", "-f", "."]);
    std::fs::write(
        repo.join("main.rs"),
        "fn main() { println!(\"loopback compiled\"); }\n",
    )
    .unwrap();
    std::fs::remove_file(repo.join("deleted.rs")).unwrap();
    std::fs::write(repo.join("extra.txt"), "untracked").unwrap();
    std::fs::write(repo.join("ignored"), "ignored secret").unwrap();
    std::fs::write(repo.join("script.sh"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(
        repo.join("script.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::create_dir(repo.join("target")).unwrap();
    std::fs::write(repo.join("target/artifact"), "build output").unwrap();
    symlink(".env", repo.join("secret-link")).unwrap();
    let server = Server::new(|request| {
        if request.head.starts_with("GET ") { return response(200, me("active", Some(true))); }
        let payload: Value = serde_json::from_slice(&request.body).unwrap();
        let files = payload["files"].as_array().unwrap();
        let source = files.iter().find(|f| f["path"] == "main.rs").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(source["content_base64"].as_str().unwrap()).unwrap();
        let build = tempfile::tempdir().unwrap();
        std::fs::write(build.path().join("main.rs"), bytes).unwrap();
        // Only the fixed fixture is compiled. Never execute a client-supplied shell command.
        let output = std::process::Command::new("rustc").args(["main.rs", "-o", "program"]).current_dir(build.path()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let run = std::process::Command::new(build.path().join("program")).output().unwrap();
        response(200, json!({"exit_code":run.status.code().unwrap(), "stdout":String::from_utf8_lossy(&run.stdout), "stderr":String::from_utf8_lossy(&run.stderr), "usage":{"compute_seconds":1}}).to_string())
    }).await;
    env.configure(&server.base);
    let tool = CompileRemoteTool::new();
    let input = json!({"path":"project", "command":"rustc main.rs -o program && ./program"});
    let ctx = context(workspace.path());
    let output = tool.execute(input.clone(), ctx.clone()).await.unwrap();
    let result: Value = serde_json::from_str(&output.output).unwrap();
    assert_eq!(result["stdout"], "loopback compiled\n");
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["usage"]["compute_seconds"], 1);
    let requests = server.captured();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].head.starts_with("GET /v1/me "));
    assert!(requests[1].head.starts_with("POST /v1/compile "));
    let payload: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(payload["command"], input["command"]);
    assert_eq!(payload["timeout_seconds"], 300);
    let files = payload["files"].as_array().unwrap();
    assert_eq!(
        files
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [".gitignore", "extra.txt", "main.rs", "script.sh"]
    );
    assert_eq!(files[3]["executable"], true);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(files[1]["content_base64"].as_str().unwrap())
            .unwrap(),
        b"untracked"
    );
    assert_eq!(
        payload["request_id"],
        format!(
            "{:x}",
            Sha256::digest(format!("{}\0{}", ctx.session_id, ctx.tool_call_id).as_bytes())
        )
    );
    assert_eq!(
        payload.as_object().unwrap().len(),
        4,
        "no environment or credentials in payload"
    );
    tool.execute(input.clone(), ctx.clone()).await.unwrap();
    let mut different = ctx;
    different.tool_call_id = "different-call".into();
    let mut custom_timeout = input;
    custom_timeout["timeout_seconds"] = json!(17);
    tool.execute(custom_timeout, different).await.unwrap();
    let payloads: Vec<Value> = server
        .captured()
        .iter()
        .filter(|r| r.head.starts_with("POST "))
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(payloads[0]["request_id"], payloads[1]["request_id"]);
    assert_ne!(payloads[0]["request_id"], payloads[2]["request_id"]);
    assert_eq!(payloads[2]["timeout_seconds"], 17);
    assert!(!repo.join("program").exists());
    assert!(!workspace.path().join("program").exists());
}
