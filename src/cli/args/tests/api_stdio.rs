#[cfg(unix)]
#[test]
fn api_stdio_accepts_alias_and_daemon_socket_but_not_api_socket() {
    for command in ["api", "api-bridge"] {
        let args = Args::try_parse_from([
            "jcode",
            "--no-update",
            "--socket",
            "/isolated/daemon.sock",
            command,
            "--stdio",
        ])
        .expect("stdio bridge must accept the API alias and daemon override");
        assert_eq!(args.socket.as_deref(), Some("/isolated/daemon.sock"));
        assert!(args.no_update);
        assert!(matches!(
            args.command,
            Some(Command::ApiBridge {
                api_socket: None,
                stdio: true
            })
        ));
        assert!(
            Args::try_parse_from([
                "jcode",
                command,
                "--stdio",
                "--api-socket",
                "/isolated/api.sock",
            ])
            .is_err(),
            "stdio must not silently ignore an API socket override"
        );
    }
}
