//! Cross-repository acceptance, deliberately opt-in: needs Node with node:sqlite and
//! solosystems-backend beside this checkout (or SOLOSYSTEMS_API_DIR).
//! Build with justrust test -p jcode-base --test mobile_local_backend_acceptance --no-run,
//! then execute the fetched test binary locally with --ignored (sibling JS runtime required).
use anyhow::Result;
use jcode_base::mobile_runner::{LiveSession, LocalSessionHost, run_local_once};
use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::io::{AsyncBufReadExt, BufReader};

struct Host {
    busy: AtomicBool,
    fail: AtomicBool,
    delivered: Mutex<Vec<(String, String)>>,
}
impl LocalSessionHost for Host {
    fn live_sessions(&self) -> Pin<Box<dyn Future<Output = Vec<LiveSession>> + Send + '_>> {
        Box::pin(async {
            vec![LiveSession {
                session_id: "acceptance-local".into(),
                busy: self.busy.load(Ordering::SeqCst),
            }]
        })
    }
    fn deliver<'a>(
        &'a self,
        session: &'a str,
        text: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            if self.fail.load(Ordering::SeqCst) {
                anyhow::bail!("session closed before delivery");
            }
            self.delivered
                .lock()
                .unwrap()
                .push((session.into(), text.into()));
            Ok(())
        })
    }
}

async fn api(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
    status: u16,
) -> Value {
    let mut request = client
        .request(method, format!("{base}{path}"))
        .bearer_auth(key);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let actual = response.status().as_u16();
    let value: Value = response.json().await.unwrap();
    assert_eq!(actual, status, "{path}: {value}");
    value
}

#[tokio::test]
#[ignore = "requires sibling solosystems-backend and Node node:sqlite, run explicitly with --ignored"]
async fn phone_message_crosses_real_backend_sqlite_and_desktop_runner() {
    use reqwest::Method::{GET, POST};
    // Resolve at runtime so a hosted-built test binary can run in this checkout.
    let cwd = std::env::current_dir().unwrap();
    let manifest = if cwd.join("crates/jcode-base").is_dir() {
        cwd.join("crates/jcode-base")
    } else {
        cwd
    };
    let backend = std::env::var_os("SOLOSYSTEMS_API_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../../solosystems-backend/workers/api"));
    assert!(
        backend.join("src/mobile_sync.js").exists(),
        "missing backend at {}",
        backend.display()
    );
    let mut child = tokio::process::Command::new("node")
        .arg(manifest.join("tests/fixtures/mobile_local_backend.mjs"))
        .arg(backend)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let line = tokio::time::timeout(std::time::Duration::from_secs(15), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .expect("backend exited before listening");
    let ready: Value = serde_json::from_str(&line).unwrap();
    let base = format!("http://127.0.0.1:{}/v1", ready["port"]);
    let key = format!("jck_live_{}", "a".repeat(40));
    let other = format!("jck_live_{}", "b".repeat(40));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let host = Host {
        busy: AtomicBool::new(false),
        fail: AtomicBool::new(false),
        delivered: Mutex::new(vec![]),
    };
    api(&client, &base, &key, POST, "/mobile/sessions/acceptance-local", Some(json!({"source":"local", "host":"acceptance-laptop", "messages":[{"seq":0,"role":"user","text":"initial"}]})), 200).await;
    let message =
        json!({"kind":"message", "session_id":"acceptance-local", "text":"phone continuation 🦀"});
    // Synced is not live: reject rather than accidentally waking a cloud VM.
    api(
        &client,
        &base,
        &key,
        POST,
        "/mobile/commands",
        Some(message.clone()),
        409,
    )
    .await;
    assert!(
        !run_local_once(&client, &base, &key, "acceptance-laptop", &host)
            .await
            .unwrap()
    );
    let queued = api(
        &client,
        &base,
        &key,
        POST,
        "/mobile/commands",
        Some(message.clone()),
        202,
    )
    .await;
    assert_eq!(queued["host"]["state"], "local");
    assert_eq!(queued["command"]["target"], "local");
    let id = queued["command"]["command_id"].as_str().unwrap();
    // Cloud polling and another paid account cannot steal this message.
    assert!(
        api(
            &client,
            &base,
            &key,
            POST,
            "/mobile/commands/claim",
            Some(json!({"machine":"cloud"})),
            200
        )
        .await["command"]
            .is_null()
    );
    assert!(
        !run_local_once(&client, &base, &other, "other-laptop", &host)
            .await
            .unwrap()
    );
    api(
        &client,
        &base,
        &other,
        GET,
        &format!("/mobile/commands/{id}"),
        None,
        404,
    )
    .await;
    assert!(
        run_local_once(&client, &base, &key, "acceptance-laptop", &host)
            .await
            .unwrap()
    );
    let completed = api(
        &client,
        &base,
        &key,
        GET,
        &format!("/mobile/commands/{id}"),
        None,
        200,
    )
    .await;
    assert_eq!(completed["command"]["status"], "done");
    assert_eq!(
        completed["command"]["result_session_id"],
        "acceptance-local"
    );
    // Exercise the incremental transcript route the phone uses after desktop sync.
    api(
        &client,
        &base,
        &key,
        POST,
        "/mobile/sessions/acceptance-local",
        Some(json!({"messages":[
            {"seq":1,"role":"user","text":"phone continuation 🦀"},
            {"seq":2,"role":"assistant","text":"continuation response"}
        ]})),
        200,
    )
    .await;
    let tail = api(
        &client,
        &base,
        &key,
        GET,
        "/mobile/sessions/acceptance-local?after_seq=0",
        None,
        200,
    )
    .await;
    assert_eq!(tail["messages"].as_array().unwrap().len(), 2);
    assert_eq!(tail["messages"][0]["text"], "phone continuation 🦀");
    assert_eq!(tail["messages"][1]["text"], "continuation response");
    assert_eq!(
        *host.delivered.lock().unwrap(),
        vec![("acceptance-local".into(), "phone continuation 🦀".into())]
    );
    assert!(
        !run_local_once(&client, &base, &key, "acceptance-laptop", &host)
            .await
            .unwrap()
    );
    // Busy sessions remain live and can receive a queued continuation.
    host.busy.store(true, Ordering::SeqCst);
    assert!(
        !run_local_once(&client, &base, &key, "acceptance-laptop", &host)
            .await
            .unwrap()
    );
    let listing = api(&client, &base, &key, GET, "/mobile/sessions", None, 200).await;
    assert_eq!(listing["sessions"][0]["busy"], true);
    let failed = api(
        &client,
        &base,
        &key,
        POST,
        "/mobile/commands",
        Some(message),
        202,
    )
    .await;
    host.fail.store(true, Ordering::SeqCst);
    assert!(
        run_local_once(&client, &base, &key, "acceptance-laptop", &host)
            .await
            .unwrap()
    );
    let failed_id = failed["command"]["command_id"].as_str().unwrap();
    let failure = api(
        &client,
        &base,
        &key,
        GET,
        &format!("/mobile/commands/{failed_id}"),
        None,
        200,
    )
    .await;
    assert_eq!(failure["command"]["status"], "failed");
    assert!(
        !run_local_once(&client, &base, &key, "acceptance-laptop", &host)
            .await
            .unwrap()
    );
    assert_eq!(host.delivered.lock().unwrap().len(), 1);
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}
