#![cfg(unix)]
use super::*;
use tokio_stream::StreamExt;

const SCRIPT: &str = r#"
root="$1"
printf '%s\n' "$$" >> "$root/pids"
pwd >> "$root/cwds"
case "${CURSOR_API_ENDPOINT-unset}" in endpoint-a) printf 'a\n' ;; endpoint-b) printf 'b\n' ;; *) printf 'other\n' ;; esac >> "$root/endpoints"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$root/requests"
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"promptCapabilities":{"image":true}}}}\n' "$id" ;;
    *'"method":"session/new"'*)
      mode=$(cat "$root/mode")
      case "$mode" in
        error) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32603,"message":"discovery failed"}}\n' "$id" ;;
        malformed) printf 'not-json\n' ;;
        empty) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"fake-session","configOptions":[]}}\n' "$id" ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$(cat "$root/new.json")" ;;
      esac
      if [ -e "$root/block" ]; then exec sleep 20; fi ;;
    *'"method":"session/set_config_option"'*)
      if [ -e "$root/malformed-config" ]; then printf 'not-json\n'; continue; fi
      if [ -e "$root/reject-config" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"selection rejected"}}\n' "$id"
        continue
      fi
      case "$line" in *'"configId":"model"'*) file=model ;; *) file=effort ;; esac
      printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$(cat "$root/$file.json")" ;;
    *'"method":"session/prompt"'*)
      if [ -e "$root/permission" ]; then
        printf '%s\n' '{"jsonrpc":"2.0","id":99,"method":"session/request_permission","params":{"toolCall":{"kind":"execute"},"options":[{"optionId":"allow-always"},{"optionId":"reject-once"}]}}'
        IFS= read -r reply || exit 11
        printf '%s\n' "$reply" >> "$root/replies"
      fi
      if [ -s "$root/update.json" ]; then
        printf '{"jsonrpc":"2.0","method":"session/update","params":{"update":%s}}\n' "$(cat "$root/update.json")"
      fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id" ;;
  esac
done
"#;

struct FakeAgent {
    root: tempfile::TempDir,
    provider: CursorAcpProvider,
}

fn config(model: &str, models: &[&str], efforts: &[&str], current: &str) -> Value {
    json!({"sessionId":"fake-session","configOptions":[
        {"id":"model","category":"model","currentValue":model,
         "options":[{"group":"future","options":models.iter().map(|m| json!({"value":m,"name":m})).collect::<Vec<_>>()}]},
        {"id":"thinking","category":"thought_level","currentValue":current,
         "options":efforts.iter().map(|e| json!({"value":e,"name":e})).collect::<Vec<_>>()}
    ]})
}

impl FakeAgent {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("mode"), "ok").unwrap();
        let value = config(
            "future-a-2099",
            &["future-a-2099", "future-b-2099"],
            &["low", "high"],
            "low",
        );
        for name in ["new.json", "model.json", "effort.json"] {
            std::fs::write(root.path().join(name), value.to_string()).unwrap();
        }
        std::fs::write(root.path().join("update.json"), "").unwrap();
        let provider = CursorAcpProvider::with_command(CursorAcpCommand::new(
            "sh",
            ["-c", SCRIPT, "sh", root.path().to_str().unwrap()],
        ));
        Self { root, provider }
    }

    fn write(&self, name: &str, value: &Value) {
        std::fs::write(self.root.path().join(name), value.to_string()).unwrap();
    }

    fn requests(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.path().join("requests"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    async fn pid(&self) -> u32 {
        self.provider
            .session
            .lock()
            .await
            .as_ref()
            .unwrap()
            .child
            .id()
            .unwrap()
    }
}

async fn prompt(provider: &dyn Provider, history: &[Message], system: &str) -> Result<()> {
    let mut stream = provider.complete(history, &[], system, None).await?;
    while let Some(event) = stream.next().await {
        event?;
    }
    Ok(())
}

async fn confirmed_high() -> FakeAgent {
    let fake = FakeAgent::new();
    fake.provider.prefetch_models().await.unwrap();
    fake.write(
        "effort.json",
        &config(
            "future-a-2099",
            &["future-a-2099", "future-b-2099"],
            &["low", "high"],
            "high",
        ),
    );
    fake.provider.set_reasoning_effort("high").unwrap();
    prompt(&fake.provider, &[Message::user("selected-high")], "system")
        .await
        .unwrap();
    assert!(fake.provider.requested_effort.read().unwrap().is_none());
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("high"));
    fake
}

fn effort_requests(fake: &FakeAgent) -> Vec<Value> {
    fake.requests()
        .into_iter()
        .filter(|v| v["params"]["configId"] == "thinking")
        .collect()
}

#[tokio::test]
async fn completed_effort_survives_refresh_system_rebuild_fork_and_dead_process() {
    let fake = confirmed_high().await;
    let first = fake.pid().await;
    fake.provider.refresh_model_catalog().await.unwrap();
    assert_ne!(fake.pid().await, first);
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("high"));
    prompt(&fake.provider, &[Message::user("after-refresh")], "system")
        .await
        .unwrap();
    let refreshed = fake.pid().await;
    prompt(
        &fake.provider,
        &[Message::user("after-refresh"), Message::user("rebuilt")],
        "new-system",
    )
    .await
    .unwrap();
    assert_ne!(fake.pid().await, refreshed);
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("high"));
    let fork = fake.provider.fork();
    prompt(fork.as_ref(), &[Message::user("fork")], "")
        .await
        .unwrap();
    assert_eq!(fork.reasoning_effort().as_deref(), Some("high"));
    let rebuilt = fake.pid().await;
    fake.provider
        .session
        .lock()
        .await
        .as_mut()
        .unwrap()
        .child
        .kill()
        .await
        .unwrap();
    prompt(&fake.provider, &[Message::user("after-death")], "")
        .await
        .unwrap();
    assert_ne!(fake.pid().await, rebuilt);
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("high"));
    let efforts = effort_requests(&fake);
    assert_eq!(efforts.len(), 5);
    assert!(efforts.iter().all(|v| v["params"]["value"] == "high"));
}

#[tokio::test]
async fn same_model_restores_effort_from_snapshot_before_model_response_publication() {
    let fake = confirmed_high().await;
    fake.write(
        "new.json",
        &config(
            "future-b-2099",
            &["future-a-2099", "future-b-2099"],
            &["low", "high"],
            "low",
        ),
    );
    fake.provider.refresh_model_catalog().await.unwrap();
    assert_eq!(fake.provider.model(), "future-a-2099");
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("high"));
    assert_eq!(effort_requests(&fake).len(), 2);
    assert!(
        fake.requests()
            .iter()
            .any(|v| v["params"]["configId"] == "model" && v["params"]["value"] == "future-a-2099")
    );
}

#[tokio::test]
async fn stale_confirmed_effort_uses_fresh_default_without_reapplying_or_failing() {
    for selector in [true, false] {
        let fake = confirmed_high().await;
        let mut fresh = config("future-a-2099", &["future-a-2099"], &["medium"], "medium");
        if !selector {
            fresh["configOptions"].as_array_mut().unwrap().truncate(1);
        }
        fake.write("new.json", &fresh);
        fake.provider.refresh_model_catalog().await.unwrap();
        assert_eq!(
            fake.provider.reasoning_effort().as_deref(),
            selector.then_some("medium")
        );
        assert_eq!(effort_requests(&fake).len(), 1);
    }
}

#[tokio::test]
async fn changed_model_does_not_inherit_confirmed_effort_even_when_still_supported() {
    for requested in [true, false] {
        let fake = confirmed_high().await;
        let models: &[&str] = if requested {
            &["future-a-2099", "future-b-2099"]
        } else {
            &["future-b-2099"]
        };
        let fresh = config("future-b-2099", models, &["low", "high"], "low");
        fake.write("new.json", &fresh);
        fake.write("model.json", &fresh);
        if requested {
            fake.provider.set_model("future-b-2099").unwrap();
        }
        fake.provider.refresh_model_catalog().await.unwrap();
        assert_eq!(fake.provider.model(), "future-b-2099");
        assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("low"));
        assert_eq!(effort_requests(&fake).len(), 1);
    }
}

#[tokio::test]
async fn explicit_pending_effort_overrides_fallback_and_unsupported_pending_stays_strict() {
    let fake = confirmed_high().await;
    fake.provider.set_reasoning_effort("low").unwrap();
    fake.provider.refresh_model_catalog().await.unwrap();
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("low"));
    assert_eq!(effort_requests(&fake).len(), 1);
    let pid = fake.pid().await;
    fake.provider.set_reasoning_effort("high").unwrap();
    fake.write(
        "new.json",
        &config("future-a-2099", &["future-a-2099"], &["low"], "low"),
    );
    let error = fake.provider.refresh_model_catalog().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("does not advertise effort 'high'")
    );
    assert_eq!(fake.pid().await, pid);
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("low"));
    assert_eq!(effort_requests(&fake).len(), 1);
}

#[tokio::test]
async fn workspace_rebind_does_not_restore_previous_workspace_confirmed_effort() {
    let fake = confirmed_high().await;
    let workspace = fake.root.path().join("other-workspace");
    std::fs::create_dir(&workspace).unwrap();
    fake.provider.set_working_dir(&workspace);
    fake.provider.refresh_model_catalog().await.unwrap();
    assert_eq!(fake.provider.model(), "future-a-2099");
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("low"));
    assert_eq!(effort_requests(&fake).len(), 1);
}

#[tokio::test]
async fn cached_prefetch_keeps_pid_history_and_fork_scope_but_forced_refresh_requeries() {
    let fake = FakeAgent::new();
    fake.provider.prefetch_models().await.unwrap();
    let pid = fake.pid().await;
    assert_eq!(fake.provider.model_routes()[0].model, "future-a-2099");
    prompt(&fake.provider, &[Message::user("first-turn")], "system")
        .await
        .unwrap();
    fake.provider.prefetch_models().await.unwrap();
    assert_eq!(fake.pid().await, pid);
    prompt(
        &fake.provider,
        &[
            Message::user("first-turn"),
            Message::assistant_text("answer"),
            Message::user("second-turn"),
        ],
        "system",
    )
    .await
    .unwrap();
    assert_eq!(fake.pid().await, pid);
    let requests = fake.requests();
    let prompts: Vec<_> = requests
        .iter()
        .filter(|v| v["method"] == "session/prompt")
        .collect();
    assert_eq!(prompts.len(), 2);
    let second = prompts[1]["params"]["prompt"][0]["text"].as_str().unwrap();
    assert!(second.contains("second-turn"));
    assert!(!second.contains("first-turn"));
    let fork = fake.provider.fork();
    fork.prefetch_models().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(fake.root.path().join("pids"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    prompt(fork.as_ref(), &[Message::user("fork-turn")], "")
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(fake.root.path().join("pids"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    fake.write(
        "new.json",
        &config("future-c-2100", &["future-c-2100"], &["cosmic"], "cosmic"),
    );
    fake.provider.refresh_model_catalog().await.unwrap();
    assert_ne!(fake.pid().await, pid);
    assert_eq!(
        fake.provider.available_models_for_switching(),
        ["future-c-2100"]
    );
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("cosmic"));
}

#[tokio::test]
async fn unsuccessful_refreshes_retain_last_good_catalog_and_warm_process() {
    let fake = FakeAgent::new();
    fake.provider.prefetch_models().await.unwrap();
    let pid = fake.pid().await;
    for mode in ["empty", "malformed", "error"] {
        std::fs::write(fake.root.path().join("mode"), mode).unwrap();
        assert!(
            fake.provider.refresh_model_catalog().await.is_err(),
            "{mode}"
        );
        assert_eq!(fake.pid().await, pid, "{mode}");
        assert_eq!(
            fake.provider.available_models_display(),
            ["future-a-2099", "future-b-2099"],
            "{mode}"
        );
        assert_eq!(fake.provider.model(), "future-a-2099");
    }
    prompt(&fake.provider, &[Message::user("still-usable")], "")
        .await
        .unwrap();
    assert_eq!(fake.pid().await, pid);
}

#[tokio::test]
async fn dependent_effort_is_revalidated_after_model_and_live_snapshots_are_authoritative() {
    let fake = FakeAgent::new();
    fake.provider.prefetch_models().await.unwrap();
    let pid = fake.pid().await;
    fake.write(
        "model.json",
        &config(
            "future-b-2099",
            &["future-a-2099", "future-b-2099"],
            &["low", "cosmic"],
            "low",
        ),
    );
    fake.provider.set_model("future-b-2099").unwrap();
    fake.provider.set_reasoning_effort("high").unwrap();
    assert!(
        prompt(&fake.provider, &[Message::user("rejected-effort")], "")
            .await
            .is_err()
    );
    assert_eq!(fake.pid().await, pid);
    assert_eq!(fake.provider.model(), "future-b-2099");
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("low"));
    assert!(
        !fake
            .requests()
            .iter()
            .any(|v| v["params"]["configId"] == "thinking")
    );
    fake.write(
        "effort.json",
        &config(
            "future-b-2099",
            &["future-a-2099", "future-b-2099"],
            &["low", "cosmic"],
            "cosmic",
        ),
    );
    fake.provider.set_reasoning_effort("cosmic").unwrap();
    prompt(&fake.provider, &[Message::user("valid-effort")], "")
        .await
        .unwrap();
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("cosmic"));
    fake.write(
        "update.json",
        &config("future-a-2099", &["future-a-2099"], &["medium"], "medium"),
    );
    prompt(
        &fake.provider,
        &[Message::user("valid-effort"), Message::user("live-update")],
        "",
    )
    .await
    .unwrap();
    assert_eq!(fake.provider.model(), "future-a-2099");
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("medium"));
    assert_eq!(fake.provider.available_models_display(), ["future-a-2099"]);
    assert!(fake.provider.set_model("future-b-2099").is_err());
}

#[tokio::test]
async fn system_change_rebuilds_session_and_resends_full_history() {
    let fake = FakeAgent::new();
    prompt(&fake.provider, &[Message::user("first")], "original-system")
        .await
        .unwrap();
    let pid = fake.pid().await;
    prompt(
        &fake.provider,
        &[Message::user("first"), Message::user("second")],
        "changed-system",
    )
    .await
    .unwrap();
    assert_ne!(fake.pid().await, pid);
    let requests = fake.requests();
    let prompts: Vec<_> = requests
        .iter()
        .filter(|v| v["method"] == "session/prompt")
        .collect();
    let second = prompts[1]["params"]["prompt"][0]["text"].as_str().unwrap();
    assert!(second.contains("changed-system"));
    assert!(second.contains("first"));
    assert!(second.contains("second"));
    assert!(!second.contains("original-system"));
}

#[tokio::test]
async fn changed_workspace_rebinds_child_and_session_without_changing_fork_workspace() {
    let fake = FakeAgent::new();
    let first = fake.root.path().join("workspace-a");
    let second = fake.root.path().join("workspace-b");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    fake.provider.set_working_dir(&first);
    fake.provider.prefetch_models().await.unwrap();
    let pid = fake.pid().await;
    let fork = fake.provider.fork();
    let old_publication = fake.provider.published();
    fake.provider.set_working_dir(&second);
    assert!(fake.provider.available_models_display().is_empty());
    old_publication.publish(
        &fake.provider.session.lock().await.as_ref().unwrap().catalog,
        true,
    );
    assert_eq!(fake.provider.model(), "unknown");
    fake.write(
        "new.json",
        &config("workspace-b-model", &["workspace-b-model"], &["low"], "low"),
    );
    prompt(&fake.provider, &[Message::user("workspace-b")], "")
        .await
        .unwrap();
    assert_ne!(fake.pid().await, pid);
    assert_eq!(
        fake.provider.available_models_display(),
        ["workspace-b-model"]
    );
    assert_eq!(
        fork.available_models_display(),
        ["future-a-2099", "future-b-2099"]
    );
    let requests = fake.requests();
    let sessions: Vec<_> = requests
        .iter()
        .filter(|v| v["method"] == "session/new")
        .collect();
    assert_eq!(sessions[0]["params"]["cwd"], first.to_str().unwrap());
    assert_eq!(sessions[1]["params"]["cwd"], second.to_str().unwrap());
    let cwds = std::fs::read_to_string(fake.root.path().join("cwds")).unwrap();
    let cwds: Vec<_> = cwds.lines().collect();
    assert_eq!(
        std::fs::canonicalize(cwds[0]).unwrap(),
        std::fs::canonicalize(first).unwrap()
    );
    assert_eq!(
        std::fs::canonicalize(cwds[1]).unwrap(),
        std::fs::canonicalize(second).unwrap()
    );
}

#[tokio::test]
async fn permission_resolver_is_live_and_explicit_environment_override_wins() {
    let _guard = tests::tests_shared_state_mutex().lock().await;
    struct RestoreEnv(Option<std::ffi::OsString>);
    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(value) => std::env::set_var("JCODE_CURSOR_ACP_PERMISSION", value),
                    None => std::env::remove_var("JCODE_CURSOR_ACP_PERMISSION"),
                }
            }
        }
    }
    let _restore = RestoreEnv(std::env::var_os("JCODE_CURSOR_ACP_PERMISSION"));
    unsafe {
        std::env::remove_var("JCODE_CURSOR_ACP_PERMISSION");
    }
    let mut fake = FakeAgent::new();
    std::fs::write(fake.root.path().join("permission"), "").unwrap();
    let mode = Arc::new(RwLock::new("yolo".to_string()));
    let live_mode = mode.clone();
    fake.provider = fake
        .provider
        .with_permission_mode_resolver(move || live_mode.read().unwrap().clone());
    let mut history = vec![Message::user("first")];
    prompt(&fake.provider, &history, "").await.unwrap();
    let pid = fake.pid().await;
    *mode.write().unwrap() = "jcode".into();
    history.push(Message::user("safe-after-reload"));
    prompt(&fake.provider, &history, "").await.unwrap();
    assert_eq!(fake.pid().await, pid);
    unsafe {
        std::env::set_var("JCODE_CURSOR_ACP_PERMISSION", "yolo");
    }
    history.push(Message::user("explicit-override"));
    prompt(&fake.provider, &history, "").await.unwrap();
    unsafe {
        std::env::remove_var("JCODE_CURSOR_ACP_PERMISSION");
    }
    prompt(
        fake.provider.fork().as_ref(),
        &[Message::user("fork-safe")],
        "",
    )
    .await
    .unwrap();
    let replies = std::fs::read_to_string(fake.root.path().join("replies")).unwrap();
    let replies: Vec<Value> = replies
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies[0]["result"]["outcome"]["optionId"], "allow-always");
    assert_eq!(replies[1]["result"]["outcome"]["outcome"], "cancelled");
    assert_eq!(replies[2]["result"]["outcome"]["optionId"], "allow-always");
    assert_eq!(replies[3]["result"]["outcome"]["outcome"], "cancelled");
}

#[tokio::test]
async fn dropping_stream_releases_session_mutex_while_subprocess_stdin_is_blocked() {
    let fake = FakeAgent::new();
    std::fs::write(fake.root.path().join("block"), "").unwrap();
    fake.provider.prefetch_models().await.unwrap();
    let stream = fake
        .provider
        .complete(
            &[Message::user(&"x".repeat(4 * 1024 * 1024))],
            &[],
            "",
            None,
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(fake.provider.session.try_lock().is_err());
    drop(stream);
    let guard = timeout(Duration::from_secs(2), fake.provider.session.lock())
        .await
        .expect("cancelled blocked write must release the session mutex");
    assert!(guard.is_none());
}

#[tokio::test]
async fn endpoint_identity_is_captured_and_isolates_fresh_shared_catalogs() {
    let _guard = tests::tests_shared_state_mutex().lock().await;
    struct RestoreEndpoint(Option<std::ffi::OsString>);
    impl Drop for RestoreEndpoint {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(value) => std::env::set_var("CURSOR_API_ENDPOINT", value),
                    None => std::env::remove_var("CURSOR_API_ENDPOINT"),
                }
            }
        }
    }
    let _restore = RestoreEndpoint(std::env::var_os("CURSOR_API_ENDPOINT"));
    unsafe {
        std::env::set_var("CURSOR_API_ENDPOINT", "endpoint-a");
    }
    let fake = FakeAgent::new();
    fake.provider.prefetch_models().await.unwrap();
    unsafe {
        std::env::set_var("CURSOR_API_ENDPOINT", "endpoint-b");
    }
    let other = CursorAcpProvider::with_command(fake.provider.command.clone());
    assert!(other.available_models_display().is_empty());
    fake.write(
        "new.json",
        &config("endpoint-b-model", &["endpoint-b-model"], &["low"], "low"),
    );
    other.prefetch_models().await.unwrap();
    assert_eq!(other.available_models_display(), ["endpoint-b-model"]);
    assert_eq!(
        fake.provider.available_models_display(),
        ["future-a-2099", "future-b-2099"]
    );
    fake.provider.refresh_model_catalog().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(fake.root.path().join("endpoints")).unwrap(),
        "a\nb\na\n"
    );
}

#[tokio::test]
async fn agent_config_response_wins_over_requested_model() {
    let fake = FakeAgent::new();
    fake.provider.prefetch_models().await.unwrap();
    fake.provider.set_model("future-b-2099").unwrap();
    prompt(
        &fake.provider,
        &[Message::user("authoritative-selection")],
        "",
    )
    .await
    .unwrap();
    assert!(
        fake.requests()
            .iter()
            .any(|v| v["method"] == "session/set_config_option"
                && v["params"]["value"] == "future-b-2099")
    );
    assert_eq!(fake.provider.model(), "future-a-2099");
}

#[tokio::test]
async fn rejected_model_discards_paired_effort_and_leaves_warm_session_usable() {
    let fake = FakeAgent::new();
    fake.provider.prefetch_models().await.unwrap();
    let pid = fake.pid().await;
    std::fs::write(fake.root.path().join("reject-config"), "").unwrap();
    fake.provider.set_model("future-b-2099").unwrap();
    fake.provider.set_reasoning_effort("high").unwrap();
    assert!(
        prompt(&fake.provider, &[Message::user("rejected-pair")], "")
            .await
            .is_err()
    );
    assert_eq!(fake.pid().await, pid);
    assert_eq!(fake.provider.model(), "future-a-2099");
    assert!(fake.provider.requested_effort.read().unwrap().is_none());
    prompt(
        &fake.provider,
        &[Message::user("prior-model-still-usable")],
        "",
    )
    .await
    .unwrap();
    assert_eq!(fake.pid().await, pid);
    assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("low"));
    assert_eq!(
        fake.requests()
            .iter()
            .filter(|v| v["method"] == "session/set_config_option")
            .count(),
        1
    );
}

#[tokio::test]
async fn malformed_config_response_discards_process_and_next_prompt_recovers() {
    let fake = FakeAgent::new();
    fake.provider.prefetch_models().await.unwrap();
    let pid = fake.pid().await;
    std::fs::write(fake.root.path().join("malformed-config"), "").unwrap();
    fake.provider.set_model("future-b-2099").unwrap();
    assert!(
        prompt(&fake.provider, &[Message::user("malformed-config")], "")
            .await
            .is_err()
    );
    assert!(fake.provider.session.lock().await.is_none());
    prompt(&fake.provider, &[Message::user("recovered")], "")
        .await
        .unwrap();
    assert_ne!(fake.pid().await, pid);
}

#[tokio::test]
async fn legacy_effort_selects_only_exact_advertised_bracket_variants() {
    for key in ["reasoning", "effort"] {
        let fake = FakeAgent::new();
        let low = format!("future[context=272k,{key}=low]");
        let high = format!("future[context=272k,{key}=high]");
        let mut initial = config(&low, &[&low, &high], &[], "");
        initial["configOptions"].as_array_mut().unwrap().truncate(1);
        fake.write("new.json", &initial);
        let mut selected = initial.clone();
        selected["configOptions"][0]["currentValue"] = json!(high);
        fake.write("model.json", &selected);
        fake.provider.prefetch_models().await.unwrap();
        assert_eq!(fake.provider.available_efforts(), ["low", "high"]);
        fake.provider.set_reasoning_effort("high").unwrap();
        assert_eq!(fake.provider.model(), high);
        prompt(
            &fake.provider,
            &[Message::user("supported-legacy-effort")],
            "",
        )
        .await
        .unwrap();
        assert_eq!(fake.provider.model(), high);
        assert_eq!(fake.provider.reasoning_effort().as_deref(), Some("high"));
        assert!(fake.provider.set_reasoning_effort("cosmic").is_err());
        prompt(
            &fake.provider,
            &[
                Message::user("supported-legacy-effort"),
                Message::user("still-usable"),
            ],
            "",
        )
        .await
        .unwrap();
        assert_eq!(fake.provider.model(), high);
    }
}
