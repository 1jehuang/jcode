use super::*;
use std::borrow::Cow;

/// Provider-bound payload after the `pre_request` transform stage.
///
/// Borrowed when nothing rewrote the request, so the hot path (no hook
/// configured) costs zero clones over the previous direct borrow.
pub(super) struct OutgoingRequest<'a> {
    pub messages: Cow<'a, [Message]>,
    pub tools: Cow<'a, [ToolDefinition]>,
    pub system_static: Cow<'a, str>,
    pub system_dynamic: Cow<'a, str>,
    /// True when a hook rewrote at least one part of the request. Drives
    /// cache re-accounting: the tracker's snapshot must follow what the
    /// provider actually received, not what the session holds.
    pub rewritten: bool,
}

/// Last-chance rewrite of the provider-bound request via the `pre_request`
/// hook (see `jcode-base::hooks::run_pre_request_transform`).
///
/// With no hook configured this returns the inputs untouched and spawns
/// nothing, so the hot path is a single branch. A hook that echoes its input
/// (e.g. `cat`) counts as unmodified: `rewritten` compares canonical JSON,
/// not hook exit status.
///
/// The static system prefix is core-owned and never rewritten: it anchors
/// the provider cache prefix (memory and context work belong in messages,
/// tools, and the dynamic part). A hook-returned `system_static` is ignored
/// with a warning.
pub(super) async fn apply_pre_request_transform<'a>(
    session_id: &str,
    working_dir: Option<&str>,
    messages: &'a [Message],
    tools: &'a [ToolDefinition],
    system_static: &'a str,
    system_dynamic: &'a str,
) -> OutgoingRequest<'a> {
    let passthrough = || OutgoingRequest {
        messages: Cow::Borrowed(messages),
        tools: Cow::Borrowed(tools),
        system_static: Cow::Borrowed(system_static),
        system_dynamic: Cow::Borrowed(system_dynamic),
        rewritten: false,
    };
    if !crate::hooks::hook_configured("pre_request") {
        return passthrough();
    }

    let request_json = serde_json::json!({
        "event": "pre_request",
        "session_id": session_id,
        "messages": messages,
        "tools": tools,
        "system_static": system_static,
        "system_dynamic": system_dynamic,
    });
    let request_str = request_json.to_string();
    let out = crate::hooks::run_pre_request_transform(session_id, working_dir, &request_str).await;
    if !out.changed {
        return passthrough();
    }

    let messages_value = if out.messages.is_null() {
        request_json["messages"].clone()
    } else {
        out.messages
    };
    let tools_value = if out.tools.is_null() {
        request_json["tools"].clone()
    } else {
        out.tools
    };
    if !out.system_static.is_empty() && out.system_static != system_static {
        crate::logging::warn(
            "Hook 'pre_request' returned system_static: the static prefix is core-owned, ignoring",
        );
    }
    let new_dynamic = out
        .system_dynamic
        .unwrap_or_else(|| system_dynamic.to_string());

    let messages: Vec<Message> = match serde_json::from_value(messages_value.clone()) {
        Ok(messages) => messages,
        Err(error) => {
            crate::logging::warn(&format!(
                "Hook 'pre_request' returned undecodable messages ({error}); sending original request"
            ));
            return passthrough();
        }
    };
    let tools: Vec<ToolDefinition> = match serde_json::from_value(tools_value.clone()) {
        Ok(tools) => tools,
        Err(error) => {
            crate::logging::warn(&format!(
                "Hook 'pre_request' returned undecodable tools ({error}); sending original request"
            ));
            return passthrough();
        }
    };
    // into_owned() only clones on the rewrite path; passthrough above stays
    // borrowed.

    // An echo (cat-style hook) is not a rewrite: only flag — and re-account —
    // when the wire bytes actually differ.
    let rewritten = messages_value != request_json["messages"]
        || tools_value != request_json["tools"]
        || new_dynamic != system_dynamic;
    OutgoingRequest {
        messages: Cow::Owned(messages),
        tools: Cow::Owned(tools),
        system_static: Cow::Borrowed(system_static),
        system_dynamic: Cow::Owned(new_dynamic),
        rewritten,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn write_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod script");
        path
    }

    #[cfg(unix)]
    static HOOK_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    #[cfg(unix)]
    struct HookEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev_hook: Option<std::ffi::OsString>,
        prev_timeout: Option<std::ffi::OsString>,
    }
    #[cfg(unix)]
    impl HookEnv {
        /// Serialize hook-env mutation across tests and force the config
        /// cache to re-read env: jcode-base is compiled without cfg(test)
        /// here, so without invalidation the 500ms cache would hide the vars.
        fn set(hook: Option<&str>) -> Self {
            let lock = HOOK_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let prev_hook = std::env::var_os("JCODE_HOOK_PRE_REQUEST");
            let prev_timeout = std::env::var_os("JCODE_HOOK_PRE_REQUEST_TIMEOUT_MS");
            match hook {
                Some(hook) => crate::env::set_var("JCODE_HOOK_PRE_REQUEST", hook),
                None => crate::env::remove_var("JCODE_HOOK_PRE_REQUEST"),
            }
            crate::env::set_var("JCODE_HOOK_PRE_REQUEST_TIMEOUT_MS", "5000");
            crate::config::invalidate_config_cache();
            HookEnv {
                _lock: lock,
                prev_hook,
                prev_timeout,
            }
        }
    }
    #[cfg(unix)]
    impl Drop for HookEnv {
        fn drop(&mut self) {
            match self.prev_hook.take() {
                Some(value) => crate::env::set_var("JCODE_HOOK_PRE_REQUEST", value),
                None => crate::env::remove_var("JCODE_HOOK_PRE_REQUEST"),
            }
            match self.prev_timeout.take() {
                Some(value) => crate::env::set_var("JCODE_HOOK_PRE_REQUEST_TIMEOUT_MS", value),
                None => crate::env::remove_var("JCODE_HOOK_PRE_REQUEST_TIMEOUT_MS"),
            }
            crate::config::invalidate_config_cache();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn passthrough_without_hook() {
        let _env = HookEnv::set(None);
        let messages = vec![Message::user("hi")];
        let out = apply_pre_request_transform("ses_x", None, &messages, &[], "s", "d").await;
        assert!(!out.rewritten);
        assert_eq!(out.messages.len(), 1);
        assert_eq!(out.system_static, "s");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn applies_rewrite_and_keeps_untouched_keys() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        // Appends a message, returns only the messages key.
        let script = write_script(
            temp.path(),
            "append.py",
            "#!/usr/bin/env python3\nimport json,sys\nreq=json.load(sys.stdin)\nreq['messages'].append({'role':'user','content':[{'type':'text','text':'tagged'}]})\njson.dump({'messages': req['messages']},sys.stdout)\n",
        );
        let _env = HookEnv::set(Some(&script.to_string_lossy()));
        let messages = vec![Message::user("hi")];
        let out = apply_pre_request_transform("ses_x", None, &messages, &[], "s", "d").await;
        assert!(out.rewritten);
        assert_eq!(out.messages.len(), 2);
        // tools/system fell back to the originals.
        assert!(out.tools.is_empty());
        assert_eq!(out.system_static, "s");
        assert_eq!(out.system_dynamic, "d");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn static_prefix_rewrite_is_ignored() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        // Rewrites only the static prefix: core-owned, must not apply.
        let script = write_script(
            temp.path(),
            "static.py",
            "#!/usr/bin/env python3\nimport json,sys\nreq=json.load(sys.stdin)\njson.dump({'system_static': 'hijacked'},sys.stdout)\n",
        );
        let _env = HookEnv::set(Some(&script.to_string_lossy()));
        let messages = vec![Message::user("hi")];
        let out = apply_pre_request_transform("ses_x", None, &messages, &[], "s", "d").await;
        assert!(!out.rewritten);
        assert_eq!(out.system_static, "s");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn echo_counts_as_unmodified() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let script = write_script(temp.path(), "cat.sh", "#!/bin/sh\ncat\n");
        let _env = HookEnv::set(Some(&script.to_string_lossy()));
        let messages = vec![Message::user("hi")];
        let out = apply_pre_request_transform("ses_x", None, &messages, &[], "s", "d").await;
        assert!(!out.rewritten);
        assert_eq!(out.messages.len(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn chained_partial_result_merges_instead_of_replacing() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let seen = temp.path().join("second_stdin.json");
        // First hook appends a message but returns messages only; the second
        // must still see tools and system keys (merge), not just what the
        // first repeated.
        let first = write_script(
            temp.path(),
            "first.py",
            "#!/usr/bin/env python3\nimport json,sys\nreq=json.load(sys.stdin)\nreq['messages'].append({'role':'user','content':[{'type':'text','text':'tagged'}]})\njson.dump({'messages': req['messages']},sys.stdout)\n",
        );
        let second = write_script(
            temp.path(),
            "second.py",
            &format!(
                "#!/usr/bin/env python3\nimport json,sys\nraw=sys.stdin.read()\nopen('{}','w').write(raw)\nsys.stdout.write(raw)\n",
                seen.display()
            ),
        );
        let chain = serde_json::to_string(&vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ])
        .expect("serialize chain");
        let _env = HookEnv::set(Some(&chain));
        let messages = vec![Message::user("hi")];
        let out = apply_pre_request_transform("ses_x", None, &messages, &[], "s", "d").await;
        assert!(out.rewritten);
        assert_eq!(out.messages.len(), 2);
        let recorded: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&seen).expect("second hook must record stdin"),
        )
        .expect("valid json");
        assert!(recorded.get("tools").is_some(), "merge keeps tools");
        assert!(
            recorded.get("system_dynamic").is_some(),
            "merge keeps system_dynamic"
        );
        assert_eq!(out.system_dynamic, "d");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn explicit_empty_dynamic_clears_context() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let script = write_script(
            temp.path(),
            "clear.py",
            "#!/usr/bin/env python3\nimport json,sys\njson.dump({'system_dynamic': ''},sys.stdout)\n",
        );
        let _env = HookEnv::set(Some(&script.to_string_lossy()));
        let messages = vec![Message::user("hi")];
        let out = apply_pre_request_transform("ses_x", None, &messages, &[], "s", "d").await;
        assert!(out.rewritten);
        assert_eq!(out.system_dynamic, "");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn blocked_stdin_and_runaway_stdout_fail_open_fast() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        // Never reads stdin: the write must hit the deadline, not the pipe.
        let stuck = write_script(temp.path(), "stuck.sh", "#!/bin/sh\nsleep 30\n");
        // Emits far over the 64 MiB cap: killed at the limit, not buffered.
        let gusher = write_script(
            temp.path(),
            "gusher.sh",
            "#!/bin/sh\nhead -c 70000000 /dev/zero | tr '\\0' 'x'\n",
        );
        let messages = vec![Message::user("hi")];
        let start = std::time::Instant::now();
        let _env = HookEnv::set(Some(&stuck.to_string_lossy()));
        // NOTE: HookEnv pins the timeout to 5000ms; re-apply the short deadline after.
        crate::env::set_var("JCODE_HOOK_PRE_REQUEST_TIMEOUT_MS", "300");
        crate::config::invalidate_config_cache();
        // Payload over the 64 KiB pipe buffer so the write genuinely blocks
        // against the non-reading child.
        let mut bulky = messages.clone();
        for _ in 0..64 {
            bulky.push(Message::user(&"y".repeat(4096)));
        }
        let out = apply_pre_request_transform("ses_x", None, &bulky, &[], "s", "d").await;
        assert!(!out.rewritten, "stuck hook must fail open");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(25),
            "blocked stdin must not outlive the deadline"
        );
        drop(_env);
        let _env2 = HookEnv::set(Some(&gusher.to_string_lossy()));
        crate::env::set_var("JCODE_HOOK_PRE_REQUEST_TIMEOUT_MS", "30000");
        crate::config::invalidate_config_cache();
        let out = apply_pre_request_transform("ses_x", None, &messages, &[], "s", "d").await;
        assert!(!out.rewritten, "runaway stdout must fail open");
    }
}
