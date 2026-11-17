use super::*;

#[test]
fn cloud_lifecycle_commands_parse_their_contracts() {
    for command in ["send", "status", "wait", "logs", "inspect", "run-detached"] {
        let mut argv = vec!["jcode", "cloud", command, "--session", "session_test"];
        if matches!(command, "send" | "run-detached") {
            argv.push("continue the task");
        }
        assert!(Args::try_parse_from(argv).is_ok(), "{command}");
        assert!(
            Args::try_parse_from(["jcode", "cloud", command]).is_err(),
            "{command} requires a session"
        );
    }
    let args = Args::try_parse_from([
        "jcode",
        "cloud",
        "wait",
        "--session",
        "s",
        "--timeout",
        "0",
        "--messages",
        "3",
        "--json",
    ])
    .unwrap();
    assert!(
        matches!(args.command, Some(Command::Cloud(CloudCommand::Wait { session, timeout: 0, messages: 3, json: true })) if session == "s")
    );
    let args = Args::try_parse_from([
        "jcode",
        "cloud",
        "fork",
        "--session",
        "parent",
        "--prompt",
        "child task",
        "--host",
        "my-box",
        "--json",
    ])
    .unwrap();
    assert!(
        matches!(args.command, Some(Command::Cloud(CloudCommand::Fork { session: Some(parent), prompt: Some(prompt), target, json: true })) if parent == "parent" && prompt == "child task" && target.host.as_deref() == Some("my-box"))
    );
    assert!(Args::try_parse_from(["jcode", "cloud", "fork"]).is_ok());
    assert!(Args::try_parse_from(["jcode", "cloud", "doctor", "--json"]).is_ok());
    let args =
        Args::try_parse_from(["jcode", "cloud", "problems", "--limit", "0", "--json"]).unwrap();
    assert!(matches!(
        args.command,
        Some(Command::Cloud(CloudCommand::Problems {
            limit: 0,
            json: true
        }))
    ));
}
