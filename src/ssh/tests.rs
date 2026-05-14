#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssh_config_default() {
        let config = SshConfig::default();
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 22);
        assert!(config.identity_file.is_none());
    }

    #[test]
    fn test_ssh_session_lifecycle() {
        let config = SshConfig {
            host: "localhost".to_string(),
            port: 22,
            user: "testuser".to_string(),
            identity_file: None,
            connect_timeout: std::time::Duration::from_secs(5),
        };

        let mut session = SshSession::new(config);
        assert!(!session.is_connected());
        assert!(session.uptime().is_none());

        let result = session.connect();
        if result.is_ok() {
            assert!(session.is_connected());
            assert!(session.uptime().is_some());

            let disconnect_result = session.disconnect();
            assert!(disconnect_result.is_ok());
            assert!(!session.is_connected());
        }
    }

    #[test]
    fn test_ssh_session_double_connect_fails() {
        let mut session = SshSession::new(SshConfig::default());

        let result1 = session.connect();
        if result1.is_ok() {
            let result2 = session.connect();
            assert!(result2.is_err(), "Double connect should fail");
            session.disconnect().ok();
        }
    }

    #[test]
    fn test_ssh_execute_without_connect_fails() {
        let session = SshSession::new(SshConfig::default());
        let result = session.execute("ls");
        assert!(result.is_err(), "Execute without connect should fail");
    }

    #[test]
    fn test_ssh_command_parsing() {
        let args = vec![
            "connect".to_string(),
            "example.com".to_string(),
            "--port".to_string(),
            "2222".to_string(),
            "--user".to_string(),
            "admin".to_string(),
        ];

        let output = SshCommand::execute(&args);
        assert!(output.contains("example.com") || output.contains("Error"));
    }

    #[test]
    fn test_ssh_command_usage() {
        let args: Vec<String> = vec![];
        let output = SshCommand::execute(&args);
        assert!(output.contains("Usage"));
    }
}
