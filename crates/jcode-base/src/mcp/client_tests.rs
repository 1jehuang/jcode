#[cfg(all(test, unix))]
mod tests {
    use super::{
        McpClient, configure_mcp_command_env, is_sensitive_inherited_env_key, mcp_child_env,
    };
    use crate::mcp::protocol::McpServerConfig;
    use std::collections::HashMap;

    #[test]
    fn inherited_mcp_env_scrubs_provider_credentials() {
        for key in [
            "ANTHROPIC_API_KEY",
            "openai_api_key",
            "CURSOR_ACCESS_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "GOOGLE_APPLICATION_CREDENTIALS",
        ] {
            assert!(is_sensitive_inherited_env_key(key), "must scrub {key}");
        }
        for key in ["PATH", "HOME", "RUST_LOG", "JCODE_OPENROUTER_API_KEY_NAME"] {
            assert!(!is_sensitive_inherited_env_key(key), "must preserve {key}");
        }
    }

    #[test]
    fn explicit_mcp_env_can_opt_a_credential_back_in() {
        let inherited = HashMap::from([
            ("PATH".into(), "/bin".into()),
            ("ANTHROPIC_API_KEY".into(), "daemon-secret".into()),
        ]);
        let explicit = HashMap::from([(
            "ANTHROPIC_API_KEY".to_string(),
            "server-specific-secret".to_string(),
        )]);

        let env = mcp_child_env(inherited, &explicit);
        assert_eq!(
            env.get(std::ffi::OsStr::new("PATH")).map(|v| v.as_os_str()),
            Some(std::ffi::OsStr::new("/bin"))
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("ANTHROPIC_API_KEY"))
                .map(|v| v.as_os_str()),
            Some(std::ffi::OsStr::new("server-specific-secret"))
        );
    }

    #[test]
    fn child_environment_scrubs_inherited_secrets_and_preserves_non_unicode() {
        use std::os::unix::ffi::OsStringExt;
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "mcp::client::tests::child_environment_fixture",
                "--ignored",
            ])
            .env("INHERITED_API_KEY", "synthetic-secret")
            .env("EXPLICIT_API_KEY", "old-secret")
            .env(
                "OPAQUE_VALUE",
                std::ffi::OsString::from_vec(vec![0xff, b'x']),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }

    #[tokio::test]
    #[ignore]
    async fn child_environment_fixture() {
        let inherited = std::env::vars_os().collect();
        let explicit = HashMap::from([("EXPLICIT_API_KEY".to_string(), "opted-in".to_string())]);
        let env = mcp_child_env(inherited, &explicit);
        let mut command = tokio::process::Command::new("/usr/bin/env");
        configure_mcp_command_env(&mut command, &env);
        let output = command.output().await.unwrap();
        assert!(output.status.success());
        let lines: Vec<_> = output.stdout.split(|b| *b == b'\n').collect();
        assert!(
            !lines
                .iter()
                .any(|line| line.starts_with(b"INHERITED_API_KEY="))
        );
        assert!(lines.contains(&b"EXPLICIT_API_KEY=opted-in".as_slice()));
        assert!(lines.contains(&b"OPAQUE_VALUE=\xffx".as_slice()));
    }

    /// A minimal fake stdio MCP server (shell script) that reports its own
    /// process cwd as the serverInfo name.
    fn fake_server_config() -> McpServerConfig {
        let script = r#"
while IFS= read -r line; do
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"%s","version":"0"}}}\n' "$PWD"
      ;;
    *'"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}\n'
      ;;
  esac
done
"#;
        McpServerConfig {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            env: Default::default(),
            shared: false,
            transport: None,
            url: None,
            headers: std::collections::HashMap::new(),
            enabled: None,
            disabled: None,
            timeout_secs: None,
        }
    }

    #[tokio::test]
    async fn connect_fails_fast_when_server_exits_before_initialize() {
        // A server that prints to stderr and exits before answering
        // `initialize` must fail connect promptly, even with a huge
        // `timeout_secs` (previously the pending request waited it out).
        let config = McpServerConfig {
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "echo boom >&2; exit 1".to_string()],
            timeout_secs: Some(86_400),
            ..fake_server_config()
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            McpClient::connect("dead".to_string(), &config),
        )
        .await
        .expect("connect must not hang on a server that exited");
        let err = format!("{:#}", result.err().expect("connect must fail"));
        assert!(err.contains("exited"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn connect_in_dir_sets_subprocess_cwd() {
        // Issue #557: owned MCP servers must run in the session project dir.
        let dir = tempfile::tempdir().expect("tempdir");
        let expected = dir.path().canonicalize().expect("canonicalize");

        let client = McpClient::connect_in_dir(
            "cwd-test".to_string(),
            &fake_server_config(),
            Some(dir.path()),
        )
        .await
        .expect("connect");

        let reported = client.server_info().expect("server info").name;
        assert_eq!(
            std::path::Path::new(&reported)
                .canonicalize()
                .expect("canonicalize reported"),
            expected
        );
    }

    #[tokio::test]
    async fn connect_in_dir_missing_dir_falls_back_to_inherited_cwd() {
        let client = McpClient::connect_in_dir(
            "cwd-fallback-test".to_string(),
            &fake_server_config(),
            Some(std::path::Path::new("/nonexistent/jcode-557")),
        )
        .await
        .expect("connect should fall back to inherited cwd");

        let reported = client.server_info().expect("server info").name;
        assert!(!reported.is_empty());
    }
}

#[cfg(all(test, windows))]
mod windows_env_tests {
    use super::mcp_child_env;
    use std::collections::HashMap;

    #[test]
    fn malformed_explicit_environment_keys_fail_without_panic() {
        for invalid in ["", "A=B", "BAD\0KEY"] {
            let inherited = HashMap::from([("Path".into(), "fixture".into())]);
            let explicit = HashMap::from([(invalid.to_string(), "value".to_string())]);
            let env = mcp_child_env(inherited, &explicit);
            let result = super::super::windows_native_env::environment_block(env.into_iter());
            assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
        }
    }
    #[test]
    fn windows_explicit_path_controls_lookup_and_child_environment() {
        use std::ffi::{OsStr, OsString};
        use std::os::windows::ffi::OsStringExt;
        let inherited_dir = tempfile::tempdir().unwrap();
        let explicit_dir = tempfile::tempdir().unwrap();
        let name = "jcode-path-precedence-fixture.exe";
        std::fs::write(inherited_dir.path().join(name), b"inherited").unwrap();
        std::fs::write(explicit_dir.path().join(name), b"explicit").unwrap();
        let non_unicode = OsString::from_wide(&[0xd800]);
        let inherited = HashMap::from([
            (
                "Path".into(),
                inherited_dir.path().as_os_str().to_os_string(),
            ),
            ("JCODE_HOME".into(), non_unicode.clone()),
        ]);
        let explicit_path = explicit_dir.path().to_str().unwrap().to_string();
        let explicit = HashMap::from([("PATH".to_string(), explicit_path.clone())]);
        let env = mcp_child_env(inherited, &explicit);
        assert_eq!(env.get(OsStr::new("JCODE_HOME")), Some(&non_unicode));
        let path = super::super::windows_native_command::child_path(&env);
        assert_eq!(path, Some(OsStr::new(&explicit_path)));
        assert_eq!(
            super::super::windows_native_resolve::resolve(OsStr::new(name), path).unwrap(),
            explicit_dir.path().join(name)
        );
        let block = super::super::windows_native_env::environment_block(
            env.iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v))),
        )
        .unwrap();
        let entries: Vec<String> = block
            .split(|c| *c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect();
        assert!(entries.contains(&format!("PATH={explicit_path}")));
        assert_eq!(entries.len(), 2);
    }
}
