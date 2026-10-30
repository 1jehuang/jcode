use super::*;

#[derive(Clone)]
struct CountingProvider(Arc<std::sync::Mutex<Vec<Vec<Message>>>>);

#[async_trait]
impl Provider for CountingProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        self.0.lock().unwrap().push(messages.to_vec());
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::TextDelta("answer".into())),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            }),
        ])))
    }
    fn name(&self) -> &str {
        "stop-hook-test"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

struct Env(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl Env {
    fn set(vars: &[(&'static str, String)]) -> Self {
        let previous = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var_os(k)))
            .collect();
        for (key, value) in vars {
            crate::env::set_var(key, value);
        }
        crate::config::invalidate_config_cache();
        Self(previous)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
        }
        crate::config::invalidate_config_cache();
    }
}

fn script(dir: &std::path::Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

fn all_text(calls: &[Message]) -> String {
    calls
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n---\n")
}

fn agent(calls: &Arc<std::sync::Mutex<Vec<Vec<Message>>>>) -> Agent {
    let mut agent = Agent::new_with_session(
        Arc::new(CountingProvider(Arc::clone(calls))),
        Registry::empty(),
        Session::create(None, None),
        None,
    );
    agent.memory_enabled = false;
    agent
}

/// Hook that continues until the file `count` holds `limit` lines.
fn counting_hook(dir: &std::path::Path, limit: u32) -> String {
    script(
        dir,
        "stop.sh",
        &format!(
            "#!/bin/sh\ncat >> {dir}/stdin.log\necho >> {dir}/stdin.log\necho x >> {dir}/count\nn=$(wc -l < {dir}/count)\nif [ \"$n\" -le {limit} ]; then echo \"keep going $n\" >&2; exit 2; fi\nexit 0\n",
            dir = dir.display(),
            limit = limit
        ),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn stop_hook_continues_in_turn_with_reminder_then_ends() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = Env::set(&[
        ("JCODE_HOME", home.path().display().to_string()),
        ("JCODE_HOOK_STOP", counting_hook(home.path(), 2)),
    ]);
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent = agent(&calls);
    agent.run_once_capture("start work").await.unwrap();

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 3, "two continuations plus the final answer");
    let last = all_text(calls.last().unwrap());
    assert!(last.contains("[stop hook] keep going 1"), "{last}");
    assert!(last.contains("[stop hook] keep going 2"), "{last}");
    // The continuation is a hidden reminder, not a new user prompt.
    assert_eq!(last.matches("start work").count(), 1);

    let log = std::fs::read_to_string(home.path().join("stdin.log")).unwrap();
    let docs: Vec<serde_json::Value> = log
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(docs.len(), 3);
    assert_eq!(docs[0]["hook_event_name"], "Stop");
    assert_eq!(docs[0]["stop_hook_active"], false);
    assert_eq!(docs[1]["stop_hook_active"], true);
    assert_eq!(docs[0]["last_assistant_message"], "answer");
}

#[cfg(unix)]
#[tokio::test]
async fn stop_hook_cap_ends_the_turn_and_a_new_prompt_resets_it() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = Env::set(&[
        ("JCODE_HOME", home.path().display().to_string()),
        ("JCODE_HOOK_STOP", counting_hook(home.path(), 1000)),
        ("JCODE_HOOK_STOP_MAX_CONTINUATIONS", "2".to_string()),
    ]);
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent = agent(&calls);
    agent.run_once_capture("one").await.unwrap();
    assert_eq!(calls.lock().unwrap().len(), 3, "cap of 2 continuations");
    assert_eq!(crate::hooks::stop_continuations(agent.session_id()), 2);

    // A new user message starts a fresh streak.
    agent.run_once_capture("two").await.unwrap();
    assert_eq!(calls.lock().unwrap().len(), 6);
}

#[cfg(unix)]
#[tokio::test]
async fn stop_hook_json_halt_wins_and_failures_fail_open() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let first = script(
        home.path(),
        "first.sh",
        "#!/bin/sh\ncat >/dev/null\necho '{\"continue\":false,\"stopReason\":\"done\"}'\n",
    );
    let second = script(
        home.path(),
        "second.sh",
        "#!/bin/sh\ncat >/dev/null\necho second >&2\nexit 2\n",
    );
    let _env = Env::set(&[
        ("JCODE_HOME", home.path().display().to_string()),
        ("JCODE_HOOK_STOP", format!("[\"{first}\", \"{second}\"]")),
    ]);
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent = agent(&calls);
    agent.run_once_capture("go").await.unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);

    let slow = script(home.path(), "slow.sh", "#!/bin/sh\nsleep 30\n");
    let _env2 = Env::set(&[
        ("JCODE_HOOK_STOP", slow),
        ("JCODE_HOOK_STOP_TIMEOUT_MS", "300".to_string()),
    ]);
    let calls2 = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent2 = self::agent(&calls2);
    agent2.run_once_capture("go").await.unwrap();
    assert_eq!(calls2.lock().unwrap().len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn stop_hook_continues_streaming_turns_and_turn_end_fires_once() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let turn_end = script(
        home.path(),
        "turn_end.sh",
        &format!(
            "#!/bin/sh\necho x >> {}/turn_end.log\n",
            home.path().display()
        ),
    );
    let _env = Env::set(&[
        ("JCODE_HOME", home.path().display().to_string()),
        ("JCODE_HOOK_STOP", counting_hook(home.path(), 2)),
        ("JCODE_HOOK_TURN_END", turn_end),
    ]);
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent = agent(&calls);
    let (tx, _rx) = tokio_mpsc::unbounded_channel();
    agent
        .run_once_streaming_mpsc("stream work", vec![], None, tx)
        .await
        .unwrap();
    assert_eq!(calls.lock().unwrap().len(), 3);
    let last = all_text(calls.lock().unwrap().last().unwrap());
    assert!(last.contains("[stop hook] keep going 2"), "{last}");
    assert_eq!(last.matches("stream work").count(), 1);
    for _ in 0..100 {
        if home.path().join("turn_end.log").exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    let fired = std::fs::read_to_string(home.path().join("turn_end.log")).unwrap();
    assert_eq!(
        fired.lines().count(),
        1,
        "turn_end fires once at the real end"
    );
}
