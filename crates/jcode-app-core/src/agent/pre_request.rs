use super::*;

/// Provider-bound payload after the `pre_request` transform stage.
pub(super) struct OutgoingRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub system_static: String,
    pub system_dynamic: String,
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
pub(super) async fn apply_pre_request_transform(
    session_id: &str,
    working_dir: Option<&str>,
    messages: &[Message],
    tools: &[ToolDefinition],
    system_static: &str,
    system_dynamic: &str,
) -> OutgoingRequest {
    let passthrough = || OutgoingRequest {
        messages: messages.to_vec(),
        tools: tools.to_vec(),
        system_static: system_static.to_string(),
        system_dynamic: system_dynamic.to_string(),
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
    let new_static = if out.system_static.is_empty() {
        system_static.to_string()
    } else {
        out.system_static
    };
    let new_dynamic = if out.system_dynamic.is_empty() {
        system_dynamic.to_string()
    } else {
        out.system_dynamic
    };

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

    // An echo (cat-style hook) is not a rewrite: only flag — and re-account —
    // when the wire bytes actually differ.
    let rewritten = messages_value != request_json["messages"]
        || tools_value != request_json["tools"]
        || new_static != system_static
        || new_dynamic != system_dynamic;
    OutgoingRequest {
        messages,
        tools,
        system_static: new_static,
        system_dynamic: new_dynamic,
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
    async fn echo_counts_as_unmodified() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let script = write_script(temp.path(), "cat.sh", "#!/bin/sh\ncat\n");
        let _env = HookEnv::set(Some(&script.to_string_lossy()));
        let messages = vec![Message::user("hi")];
        let out = apply_pre_request_transform("ses_x", None, &messages, &[], "s", "d").await;
        assert!(!out.rewritten);
        assert_eq!(out.messages.len(), 1);
    }
}
