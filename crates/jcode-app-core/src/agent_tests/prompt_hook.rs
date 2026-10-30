use super::*;
use crate::hooks::PromptOrigin;

#[derive(Clone)]
struct RecordingProvider(Arc<std::sync::Mutex<Vec<Vec<Message>>>>);

#[async_trait]
impl Provider for RecordingProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
        _: &str,
        _: Option<&str>,
    ) -> Result<EventStream> {
        self.0.lock().unwrap().push(messages.to_vec());
        Ok(Box::pin(futures::stream::iter(vec![
            Ok(StreamEvent::TextDelta("done".into())),
            Ok(StreamEvent::MessageEnd {
                stop_reason: Some("end_turn".into()),
            }),
        ])))
    }
    fn name(&self) -> &str {
        "prompt-hook-test"
    }
    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
    fn set(vars: &[(&'static str, String)]) -> Self {
        let previous = vars
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in vars {
            crate::env::set_var(key, value);
        }
        crate::config::invalidate_config_cache();
        Self(previous)
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

fn request_text(calls: &[Vec<Message>]) -> String {
    calls
        .iter()
        .flatten()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n---\n")
}

fn new_agent(calls: &Arc<std::sync::Mutex<Vec<Vec<Message>>>>) -> Agent {
    let mut agent = Agent::new_with_session(
        Arc::new(RecordingProvider(Arc::clone(calls))),
        Registry::empty(),
        Session::create(None, None),
        None,
    );
    agent.memory_enabled = false;
    agent
}

#[cfg(unix)]
#[tokio::test]
async fn prompt_hook_receives_stdin_json_adds_context_and_blocks() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let record = home.path().join("stdin.json");
    let hook = script(
        home.path(),
        "hook.sh",
        &format!(
            "#!/bin/sh\ncat > {rec}\nif grep -q BLOCKME {rec}; then echo 'not allowed' >&2; exit 2; fi\necho 'hook-context-marker'\nenv | grep -q 'BLOCKME' && echo LEAK\nexit 0\n",
            rec = record.display()
        ),
    );
    let _env = EnvGuard::set(&[
        ("JCODE_HOME", home.path().display().to_string()),
        ("JCODE_HOOK_USER_PROMPT_SUBMIT", hook),
    ]);
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut agent = new_agent(&calls);

    agent.run_once_capture("hello there").await.unwrap();
    let stdin: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    assert_eq!(stdin["hook_event_name"], "UserPromptSubmit");
    assert_eq!(stdin["prompt"], "hello there");
    assert_eq!(stdin["origin"]["kind"], "person");
    assert_eq!(stdin["jcode"]["origin_detail"], "user");
    let text = request_text(&calls.lock().unwrap());
    assert!(text.contains("hook-context-marker"), "{text}");
    assert!(!text.contains("LEAK"));

    // A pending origin declared by the server is reported and consumed.
    crate::hooks::set_pending_prompt_origin(agent.session_id(), PromptOrigin::SwarmSpawn);
    agent.run_once_capture("worker prompt").await.unwrap();
    let stdin: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    assert_eq!(stdin["origin"]["kind"], "host");
    assert_eq!(stdin["jcode"]["origin_detail"], "swarm_spawn");

    // A blocked prompt is never appended and never reaches the provider.
    let before_messages = agent.messages().len();
    let before_calls = calls.lock().unwrap().len();
    let error = agent.run_once_capture("please BLOCKME").await.unwrap_err();
    assert!(error.to_string().contains("not allowed"), "{error}");
    assert_eq!(agent.messages().len(), before_messages);
    assert_eq!(calls.lock().unwrap().len(), before_calls);
}

#[cfg(unix)]
#[tokio::test]
async fn prompt_hook_fails_open_on_timeout_and_odd_exit() {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let slow = script(home.path(), "slow.sh", "#!/bin/sh\nsleep 30\n");
    let odd = script(home.path(), "odd.sh", "#!/bin/sh\necho ignored\nexit 7\n");
    for (hook, timeout) in [(slow, "300"), (odd, "5000")] {
        let _env = EnvGuard::set(&[
            ("JCODE_HOME", home.path().display().to_string()),
            ("JCODE_HOOK_USER_PROMPT_SUBMIT", hook),
            (
                "JCODE_HOOK_USER_PROMPT_SUBMIT_TIMEOUT_MS",
                timeout.to_string(),
            ),
        ]);
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut agent = new_agent(&calls);
        let started = std::time::Instant::now();
        agent.run_once_capture("go on").await.unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert!(!request_text(&calls.lock().unwrap()).contains("ignored"));
    }
}
