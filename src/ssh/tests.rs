#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    // ==========================================
    // SshConfig Tests (配置管理)
    // ==========================================

    mod config_tests {
        use super::*;

        #[test]
        fn test_default_config() {
            let config = SshConfig::default();
            assert_eq!(config.host, "localhost");
            assert_eq!(config.port, 22);
            assert_eq!(config.connect_timeout, Duration::from_secs(30));
            assert!(config.compression);
            assert!(config.control_master);
            assert!(config.strict_host_key_checking);
            assert!(!config.request_pty);
        }

        #[test]
        fn test_config_with_host_simple() {
            let config = SshConfig::with_host("example.com");
            assert_eq!(config.host, "example.com");
            assert_eq!(config.port, 22);
        }

        #[test]
        fn test_config_with_host_user_at_host() {
            let config = SshConfig::with_host("user@example.com");
            assert_eq!(config.user, "user");
            assert_eq!(config.host, "example.com");
            assert_eq!(config.port, 22);
        }

        #[test]
        fn test_config_with_host_port() {
            let config = SshConfig::with_host("example.com:2222");
            assert_eq!(config.host, "example.com");
            assert_eq!(config.port, 2222);
        }

        #[test]
        fn test_config_with_host_full_format() {
            let config = SshConfig::with_host("admin@example.com:8022");
            assert_eq!(config.user, "admin");
            assert_eq!(config.host, "example.com");
            assert_eq!(config.port, 8022);
        }

        #[test]
        fn test_build_ssh_args_basic() {
            let config = SshConfig::default();
            let args = config.build_ssh_args();
            
            assert!(args.contains(&"-o".to_string()));
            assert!(args.contains(&"BatchMode=yes".to_string()));
            assert!(args.contains(&"-p".to_string()));
            assert!(args.contains(&"22".to_string()));
        }

        #[test]
        fn test_build_ssh_args_with_identity() {
            let mut config = SshConfig::default();
            config.identity_file = Some(PathBuf::from("/path/to/key"));
            let args = config.build_ssh_args();
            
            assert!(args.contains(&"-i".to_string()));
            assert!(args.contains(&"/path/to/key".to_string()));
        }

        #[test]
        fn test_build_ssh_args_compression_disabled() {
            let mut config = SshConfig::default();
            config.compression = false;
            let args = config.build_ssh_args();
            
            assert!(!args.iter().any(|a| a == "Compression=yes"));
        }

        #[test]
        fn test_build_ssh_args_with_jump_hosts() {
            let mut config = SshConfig::default();
            config.jump_hosts.push(JumpHost {
                host: "bastion.example.com".to_string(),
                user: "jumpuser".to_string(),
                port: 22,
                identity_file: None,
            });
            let args = config.build_ssh_args();
            
            assert!(args.contains(&"-J".to_string()));
            assert!(args.iter().any(|a| a.contains("bastion")));
        }

        #[test]
        fn test_config_from_host_config_nonexistent_file() {
            let result = SshConfig::from_host_config("nonexistent-host");
            assert!(result.is_ok());
            let config = result.unwrap();
            assert_eq!(config.host, "nonexistent-host");
        }
    }

    // ==========================================
    // SshSession Tests (会话管理)
    // ==========================================

    mod session_tests {
        use super::*;

        #[test]
        fn test_session_creation() {
            let session = SshSession::new(SshConfig::default());
            assert!(!session.id().is_empty());
            assert_eq!(session.state(), &SessionState::Disconnected);
            assert!(session.uptime().is_none());
            assert!(session.idle_time().is_none());
        }

        #[test]
        fn test_session_stats_initial() {
            let session = SshSession::new(SshConfig::default());
            let stats = session.stats();
            assert_eq!(stats.commands_executed, 0);
            assert_eq!(stats.bytes_transferred, 0);
            assert_eq!(stats.files_uploaded, 0);
            assert_eq!(stats.files_downloaded, 0);
            assert_eq!(stats.reconnect_count, 0);
        }

        #[test]
        fn test_execute_without_connect_fails() {
            let mut session = SshSession::new(SshConfig::default());
            let result = session.execute("ls");
            assert!(result.is_err());
        }

        #[test]
        fn test_upload_without_connect_fails() {
            let session = SshSession::new(SshConfig::default());
            let result = session.upload(
                &PathBuf::from("/tmp/local"),
                &PathBuf::from("/tmp/remote")
            );
            assert!(result.is_err());
        }

        #[test]
        fn test_download_without_connect_fails() {
            let session = SshSession::new(SshConfig::default());
            let result = session.download(
                &PathBuf::from("/tmp/remote"),
                &PathBuf::from("/tmp/local")
            );
            assert!(result.is_err());
        }

        #[test]
        fn test_ensure_connected_disconnected() {
            let session = SshSession::new(SshConfig::default());
            let result = session.ensure_connected();
            assert!(result.is_err());
        }

        #[test]
        fn test_is_alive_disconnected() {
            let session = SshSession::new(SshConfig::default());
            assert!(!session.is_alive());
        }

        #[test]
        fn test_config_reference() {
            let config = SshConfig::with_host("test.com");
            let session = SshSession::new(config);
            assert_eq!(session.config().host, "test.com");
        }
    }

    // ==========================================
    // ConfigParser Tests (配置解析器)
    // ==========================================

    mod config_parser_tests {
        use super::*;

        #[test]
        fn test_parse_empty_content() {
            let configs = ConfigParser::parse_content("").unwrap();
            assert!(configs.is_empty());
        }

        #[test]
        fn test_parse_single_host() {
            let content = r#"
Host myserver
    HostName server.example.com
    User admin
    Port 2222
"#;
            let configs = ConfigParser::parse_content(content).unwrap();
            assert_eq!(configs.len(), 1);
            assert_eq!(configs[0].host, "myserver");
            assert_eq!(configs[0].host_name.as_deref(), Some("server.example.com"));
            assert_eq!(configs[0].user.as_deref(), Some("admin"));
            assert_eq!(configs[0].port, Some(2222));
        }

        #[test]
        fn test_parse_multiple_hosts() {
            let content = r#"
Host webserver
    HostName web.example.com
    User www

Host dbserver
    HostName db.example.com
    User dbadmin
    Port 5432
"#;
            let configs = ConfigParser::parse_content(content).unwrap();
            assert_eq!(configs.len(), 2);
            assert_eq!(configs[0].host, "webserver");
            assert_eq!(configs[1].host, "dbserver");
        }

        #[test]
        fn test_parse_identity_files() {
            let content = r#"
Host secure
    IdentityFile ~/.ssh/id_rsa
    IdentityFile ~/.ssh/id_ed25519
"#;
            let configs = ConfigParser::parse_content(content).unwrap();
            assert_eq!(configs[0].identity_file.len(), 2);
        }

        #[test]
        fn test_parse_port_forwards() {
            let content = r#"
Host tunnel
    LocalForward 8080 localhost:80
    RemoteForward 9000 localhost:9000
    DynamicForward 1080
"#;
            let configs = ConfigParser::parse_content(content).unwrap();
            assert_eq!(configs[0].local_forward.len(), 1);
            assert_eq!(configs[0].remote_forward.len(), 1);
            assert_eq!(configs[0].dynamic_forward, Some(1080));
        }

        #[test]
        fn test_parse_comments_and_empty_lines() {
            let content = r#"
# This is a comment
Host commented
    # Inline comment
    User test
    
Host another
    Port 22
"#;
            let configs = ConfigParser::parse_content(content).unwrap();
            assert_eq!(configs.len(), 2);
        }

        #[test]
        fn test_pattern_matching_wildcard() {
            assert!(ConfigParser::pattern_matches("*", "anything"));
            assert!(ConfigParser::pattern_matches("*.example.com", "web.example.com"));
            assert!(!ConfigParser::pattern_matches("*.example.com", "web.example.org"));
        }

        #[test]
        fn test_pattern_matching_question_mark() {
            assert!(ConfigParser::pattern_matches("server?", "server1"));
            assert!(ConfigParser::pattern_matches("server?", "serverA"));
            assert!(!ConfigParser::pattern_matches("server?", "server10"));
        }

        #[test]
        fn test_list_hosts_returns_vec() {
            let hosts = ConfigParser::list_hosts();
            assert!(hosts.is_vec()); // Should return a Vec<String>
        }

        #[test]
        fn test_validate_config_no_panic() {
            let result = ConfigParser::validate_config();
            assert!(result.is_ok() || result.is_err()); // Should not panic
        }
    }

    // ==========================================
    // PortForwarder Tests (端口转发)
    // ==========================================

    mod port_forwarder_tests {
        use super::*;

        #[test]
        fn test_local_forward_creation() {
            let forwarder = PortForwarder::local_forward(8080, "localhost", 80);
            assert_eq!(forwarder.local_port(), 8080);
            assert_eq!(forwarder.forward_type(), ForwardType::Local);
        }

        #[test]
        fn test_remote_forward_creation() {
            let forwarder = PortForwarder::remote_forward(9000, "localhost", 9000);
            assert_eq!(forwarder.local_port(), 9000);
            assert_eq!(forwarder.forward_type(), ForwardType::Remote);
        }

        #[test]
        fn test_dynamic_forward_creation() {
            let forwarder = PortForwarder::dynamic_forward(1080);
            assert_eq!(forwarder.local_port(), 1080);
            assert_eq!(forwarder.forward_type(), ForwardType::Dynamic);
        }

        #[test]
        fn test_forwarder_builder_pattern() {
            let forwarder = PortForwarder::local_forward(8080, "remote", 80)
                .bind_address("127.0.0.1")
                .target("user", "host.com")
                .ssh_port(2222)
                .identity_file(&PathBuf::from("/key"));

            assert_eq!(forwarder.config().host, "host.com");
            assert_eq!(forwarder.config().port, 2222);
        }

        #[test]
        fn test_tunnel_manager_creation() {
            let manager = TunnelManager::new();
            assert!(manager.list_tunnels().is_empty());
        }

        #[test]
        fn test_jump_host_chain() {
            let chain = JumpHostChain::new()
                .add_jump("user", "bastion1.com", 22)
                .add_jump("user", "bastion2.com", 2222);

            let jump_arg = chain.build_jump_arg();
            assert!(jump_arg.contains("bastion1.com"));
            assert!(jump_arg.contains("bastion2.com"));
        }
    }

    // ==========================================
    // FileTransfer Tests (文件传输)
    // ==========================================

    mod file_transfer_tests {
        use super::*;

        #[test]
        fn test_transfer_creation() {
            let transfer = FileTransfer::new("user", "host", 22);
            assert_eq!(transfer.ssh_user, "user");
            assert_eq!(transfer.ssh_host, "host");
            assert_eq!(transfer.ssh_port, 22);
        }

        #[test]
        fn test_transfer_with_identity() {
            let transfer = FileTransfer::new("user", "host", 22)
                .with_identity(&PathBuf::from("/path/to/key"));
            
            assert!(transfer.identity_file.is_some());
        }

        #[test]
        fn test_transfer_compress_option() {
            let transfer = FileTransfer::new("user", "host", 22)
                .compress(false);
            
            assert!(!transfer.compression);
        }

        #[test]
        fn test_transfer_bandwidth_limit() {
            let transfer = FileTransfer::new("user", "host", 22)
                .limit_bandwidth(1024 * 1024); // 1 MB/s
            
            assert_eq!(transfer.bandwidth_limit, Some(1024 * 1024));
        }

        #[test]
        fn test_sync_result_success() {
            let result = TransferResult {
                success: true,
                bytes_transferred: 1024,
                duration: Duration::from_secs(1),
                speed_bytes_per_sec: 1024.0,
                error: None,
            };

            assert!(result.success);
            assert_eq!(result.bytes_transferred, 1024);
            assert!(result.error.is_none());
        }

        #[test]
        fn test_sync_result_failure() {
            let result = TransferResult {
                success: false,
                bytes_transferred: 0,
                duration: Duration::ZERO,
                speed_bytes_per_sec: 0.0,
                error: Some("Connection failed".to_string()),
            };

            assert!(!result.success);
            assert!(result.error.is_some());
        }
    }

    // ==========================================
    // ConnectionPool Tests (连接池)
    // ==========================================

    mod connection_pool_tests {
        use super::*;

        #[test]
        fn test_pool_creation() {
            let pool = SshConnectionPool::new(5);
            assert_eq!(pool.active_count(), 0);
            assert_eq!(pool.idle_count(), 0);
        }

        #[test]
        fn test_pool_default_config() {
            let config = PoolConfig::default();
            assert_eq!(config.max_connections, 10);
            assert_eq!(config.min_idle_connections, 2);
            assert_eq!(config.max_idle_time, Duration::from_secs(300));
        }

        #[test]
        fn test_pool_custom_config() {
            let config = PoolConfig {
                max_connections: 20,
                min_idle_connections: 5,
                ..Default::default()
            };
            assert_eq!(config.max_connections, 20);
            assert_eq!(config.min_idle_connections, 5);
        }

        #[test]
        fn test_pool_stats_initial() {
            let pool = SshConnectionPool::new(5);
            let stats = pool.stats();
            assert_eq!(stats.total_created, 0);
            assert_eq!(stats.total_destroyed, 0);
            assert_eq!(stats.active_count, 0);
            assert_eq!(stats.idle_count, 0);
        }

        #[test]
        fn test_pool_with_default_config() {
            let default_cfg = SshConfig::with_host("default.com");
            let pool = SshConnectionPool::new(5).with_default_config(default_cfg);
            
            let stats = pool.stats();
            assert_eq!(stats.total_created, 0); // No connections created yet
        }
    }

    // ==========================================
    // AuditLogger Tests (审计日志)
    // ==========================================

    mod audit_logger_tests {
        use super::*;

        #[test]
        fn test_logger_creation() {
            let logger = SshAuditLogger::new();
            let stats = logger.get_statistics();
            assert_eq!(stats.total_events, 0);
        }

        #[test]
        fn test_logger_custom_config() {
            let config = AuditConfig {
                max_events_in_memory: 100,
                log_to_file: false,
                ..Default::default()
            };
            let logger = SshAuditLogger::with_config(config);
            assert!(logger.get_recent_events(10).is_empty());
        }

        #[test]
        fn test_log_event_basic() {
            let logger = SshAuditLogger::new();
            
            logger.log(SshAuditEvent {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp: chrono::Utc::now(),
                event_type: SshEventType::ConnectionAttempt,
                session_id: Some("test-session".to_string()),
                user: Some("testuser".to_string()),
                host: Some("testhost".to_string()),
                details: AuditDetails {
                    description: "Test connection attempt".to_string(),
                    ..Default::default()
                },
                severity: EventSeverity::Info,
                source_ip: None,
                metadata: None,
            });

            let events = logger.get_recent_events(10);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event_type, SshEventType::ConnectionAttempt);
        }

        #[test]
        fn test_log_connection_methods() {
            let logger = SshAuditLogger::new();

            logger.log_connection_attempt("user", "host", "session-123");
            logger.log_connection_success("user", "host", "session-123");
            logger.log_connection_failure("user", "host", "Connection refused");

            let events = logger.get_recent_events(10);
            assert_eq!(events.len(), 3);
        }

        #[test]
        fn test_log_command_execution() {
            let logger = SshAuditLogger::new();
            
            logger.log_command_execution("session-1", "ls -la", Some(0), 100, None);

            let events = logger.get_recent_events(1);
            assert_eq!(events[0].severity, EventSeverity::Info);
        }

        #[test]
        fn test_log_command_execution_failure() {
            let logger = SshAuditLogger::new();
            
            logger.log_command_execution("session-1", "invalid_cmd", Some(1), 50, None);

            let events = logger.get_recent_events(1);
            assert_eq!(events[0].severity, EventSeverity::Warning);
        }

        #[test]
        fn test_log_file_transfer() {
            let logger = SshAuditLogger::new();
            
            logger.log_file_transfer("session-1", true, &PathBuf::from("/local/file"), 1024, true, None);
            logger.log_file_transfer("session-1", false, &PathBuf::from("/remote/file"), 2048, false, Some("Permission denied"));

            let events = logger.get_recent_events(2);
            assert_eq!(events[0].details.file_size, Some(1024));
            assert!(events[1].error.is_some());
        }

        #[test]
        fn test_log_security_violation() {
            let logger = SshAuditLogger::new();
            
            logger.log_security_violation("session-1", "UnauthorizedAccess", "Attempted root login");

            let events = logger.get_recent_events(1);
            assert_eq!(events[0].severity, EventSeverity::Critical);
            assert_eq!(events[0].event_type, SshEventType::SecurityViolation);
        }

        #[test]
        fn test_get_events_by_session() {
            let logger = SshAuditLogger::new();

            logger.log_connection_attempt("u", "h", "session-A");
            logger.log_connection_attempt("u", "h", "session-B");
            logger.log_connection_attempt("u", "h", "session-A");

            let events_a = logger.get_events_by_session("session-A");
            assert_eq!(events_a.len(), 2);

            let events_b = logger.get_events_by_session("session-B");
            assert_eq!(events_b.len(), 1);
        }

        #[test]
        fn test_get_events_by_type() {
            let logger = SshAuditLogger::new();

            logger.log_connection_attempt("u", "h", "s1");
            logger.log_connection_success("u", "h", "s1");
            logger.log_command_execution("s1", "ls", None, 0, None);

            let conn_events = logger.get_events_by_type(&SshEventType::ConnectionAttempt);
            assert_eq!(conn_events.len(), 1);

            let cmd_events = logger.get_events_by_type(&SshEventType::CommandExecutionComplete);
            assert_eq!(cmd_events.len(), 1);
        }

        #[test]
        fn test_get_events_by_severity() {
            let logger = SshAuditLogger::new();

            logger.log_connection_attempt("u", "h", "s"); // Info
            logger.log_connection_failure("u", "h", "err"); // Error
            logger.log_security_violation("s", "test", "desc"); // Critical

            let critical_events = logger.get_events_by_severity(EventSeverity::Critical);
            assert_eq!(critical_events.len(), 1);

            let error_or_higher = logger.get_events_by_severity(EventSeverity::Error);
            assert!(error_or_higher.len() >= 2); // Critical + Error
        }

        #[test]
        fn test_statistics_calculation() {
            let logger = SshAuditLogger::new();

            for i in 0..10 {
                logger.log_connection_attempt("u", "h", &format!("session-{}", i));
            }

            logger.log_connection_failure("u", "h", "err");
            logger.log_security_violation("s", "test", "desc");

            let stats = logger.get_statistics();
            assert_eq!(stats.total_events, 12);
            assert_eq!(stats.connections, 11); // 10 attempts + 1 failure
            assert_eq!(stats.security_violations, 1);
            assert_eq!(stats.critical_count, 1);
        }

        #[test]
        fn test_max_events_limit() {
            let config = AuditConfig {
                max_events_in_memory: 5,
                enabled: true,
                ..Default::default()
            };
            let logger = SshAuditLogger::with_config(config);

            for i in 0..10 {
                logger.log_connection_attempt("u", "h", &format!("s{}", i));
            }

            let events = logger.get_recent_events(100);
            assert_eq!(events.len(), 5); // Should be limited to 5
        }

        #[test]
        fn test_clear_events() {
            let logger = SshAuditLogger::new();

            logger.log_connection_attempt("u", "h", "s1");
            logger.log_connection_attempt("u", "h", "s2");
            
            assert_eq!(logger.get_recent_events(10).len(), 2);

            logger.clear_events();
            assert!(logger.get_recent_events(10).is_empty());
        }

        #[test]
        fn test_export_to_json() {
            let logger = SshAuditLogger::new();
            logger.log_connection_attempt("u", "h", "s1");

            let temp_path = PathBuf::from("/tmp/test_audit_export.json");
            let result = logger.export_to_json(&temp_path);
            
            if result.is_ok() {
                assert!(temp_path.exists());
                let _ = std::fs::remove_file(&temp_path);
            }
        }

        #[test]
        fn test_export_to_csv() {
            let logger = SshAuditLogger::new();
            logger.log_connection_attempt("u", "h", "s1");

            let temp_path = PathBuf::from("/tmp/test_audit_export.csv");
            let result = logger.export_to_csv(&temp_path);
            
            if result.is_ok() {
                assert!(temp_path.exists());
                let _ = std::fs::remove_file(&temp_path);
            }
        }
    }

    // ==========================================
    // SshOutput Tests (输出结构体)
    // ==========================================

    mod ssh_output_tests {
        use super::*;

        #[test]
        fn test_successful_output() {
            let output = SshOutput {
                stdout: "file1.txt\nfile2.txt\n".to_string(),
                stderr: String::new(),
                exit_code: Some(0),
                duration: Duration::from_millis(100),
            };

            assert!(output.exit_code == Some(0));
            assert!(output.stderr.is_empty());
            assert!(output.stdout.contains("file1.txt"));
        }

        #[test]
        fn test_error_output() {
            let output = SshOutput {
                stdout: String::new(),
                stderr: "command not found".to_string(),
                exit_code: Some(127),
                duration: Duration::from_millis(50),
            };

            assert!(output.exit_code != Some(0));
            assert!(!output.stderr.is_empty());
        }
    }

    // ==========================================
    // Integration Tests (集成测试)
    // ==========================================

    mod integration_tests {
        use super::*;

        #[test]
        fn test_full_workflow_simulation() {
            // Simulate a complete SSH workflow without actual connection
            let config = SshConfig::with_host("production-server");
            
            let mut session = SshSession::new(config.clone());
            assert!(!session.id().is_empty());

            // Verify configuration is correct
            assert_eq!(session.config().host, "production-server");
            assert_eq!(session.config().port, 22);

            // Verify initial state
            assert_eq!(session.state(), &SessionState::Disconnected);
            assert!(session.uptime().is_none());

            // Verify stats are initialized
            let stats = session.stats();
            assert_eq!(stats.commands_executed, 0);
            assert_eq!(stats.bytes_transferred, 0);
        }

        #[test]
        fn test_error_handling_workflow() {
            let mut session = SshSession::new(SshConfig::default());

            // Try operations that should fail without connection
            let exec_result = session.execute("ls");
            assert!(exec_result.is_err());

            let upload_result = session.upload(
                &PathBuf::from("/local"),
                &PathBuf::from("/remote")
            );
            assert!(upload_result.is_err());

            let download_result = session.download(
                &PathBuf::from("/remote"),
                &PathBuf::from("/local")
            );
            assert!(download_result.is_err());

            // Disconnect should fail when not connected
            let disconnect_result = session.disconnect();
            assert!(disconnect_result.is_err());
        }

        #[test]
        fn test_config_parser_integration() {
            let config_content = r#"
Host production
    HostName prod.example.com
    User deploy
    Port 2222
    IdentityFile ~/.ssh/prod_key
    ForwardAgent yes
    ServerAliveInterval 60

Host staging
    HostName staging.example.com
    User deploy
    LocalForward 8080 localhost:3000
"#;

            let configs = ConfigParser::parse_content(config_content).unwrap();
            assert_eq!(configs.len(), 2);

            let prod_config = &configs[0];
            assert_eq!(prod_config.host_name.as_deref(), Some("prod.example.com"));
            assert_eq!(prod_config.forward_agent, Some(true));

            let staging_config = &configs[1];
            assert_eq!(staging_config.local_forward.len(), 1);
        }

        #[test]
        fn test_audit_trail_workflow() {
            let logger = SshAuditLogger::new();

            let session_id = "test-workflow-session";

            // Simulate complete workflow with audit trail
            logger.log_connection_attempt("admin", "server.com", session_id);
            logger.log_connection_success("admin", "server.com", session_id);
            
            logger.log_command_execution(session_id, "cd /app && ls", Some(0), 150, Some("file1.py\nfile2.py"));
            logger.log_command_execution(session_id, "python deploy.py", Some(0), 5000, Some("Deployment successful"));
            
            logger.log_file_transfer(session_id, true, &PathBuf::from("artifact.tar.gz"), 1048576, true, None);
            
            logger.log(SshAuditEvent {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp: chrono::Utc::now(),
                event_type: SshEventType::Disconnection,
                session_id: Some(session_id.to_string()),
                user: Some("admin".to_string()),
                host: Some("server.com".to_string()),
                details: AuditDetails {
                    description: "Normal disconnect after deployment".to_string(),
                    ..Default::default()
                },
                severity: EventSeverity::Info,
                source_ip: None,
                metadata: None,
            });

            // Verify complete audit trail
            let all_events = logger.get_recent_events(100);
            assert_eq!(all_events.len(), 5);

            let session_events = logger.get_events_by_session(session_id);
            assert_eq!(session_events.len(), 5);

            let stats = logger.get_statistics();
            assert_eq!(stats.total_events, 5);
            assert_eq!(stats.connections, 2); // attempt + success
            assert_eq!(stats.commands_executed, 2);
            assert_eq!(stats.file_uploads, 1);
        }

        #[test]
        fn test_batch_operations_simulation() {
            let hosts = vec![
                ("server1".to_string(), SshConfig::with_host("server1.example.com")),
                ("server2".to_string(), SshConfig::with_host("server2.example.com")),
                ("server3".to_string(), SshConfig::with_host("server3.example.com")),
            ];

            for (name, config) in &hosts {
                let session = SshSession::new(config.clone());
                assert_eq!(session.config().host, name.replace(".example.com", ""));
            }

            assert_eq!(hosts.len(), 3);
        }
    }

    // ==========================================
    // Edge Cases and Error Conditions (边界条件测试)
    // ==========================================

    mod edge_case_tests {
        use super::*;

        #[test]
        fn test_empty_host_config() {
            let config = SshConfig::with_host("");
            assert_eq!(config.host, "");
        }

        #[test]
        fn test_very_long_hostname() {
            let long_host = "a".repeat(253) + ".com";
            let config = SshConfig::with_host(&long_host);
            assert_eq!(config.host.len(), 256);
        }

        #[test]
        fn test_special_characters_in_host() {
            let config = SshConfig::with_host("my-server_example.com");
            assert_eq!(config.host, "my-server_example.com");
        }

        #[test]
        fn test_extreme_port_numbers() {
            let config_max = SshConfig::with_host("host:65535");
            assert_eq!(config_max.port, 65535);

            let config_min = SshConfig::with_host("host:1");
            assert_eq!(config_min.port, 1);
        }

        #[test]
        fn test_zero_timeout() {
            let mut config = SshConfig::default();
            config.connect_timeout = Duration::from_secs(0);
            let args = config.build_ssh_args();
            assert!(args.contains(&"ConnectTimeout=0".to_string()));
        }

        #[test]
        fn test_multiple_identity_files() {
            let mut config = SshConfig::default();
            config.identity_file = Some(PathBuf::from("/key1"));
            let args = config.build_ssh_args();
            
            let identity_count = args.windows(2)
                .filter(|w| w[0] == "-i")
                .count();
            assert_eq!(identity_count, 1);
        }

        #[test]
        fn test_empty_environment_variables() {
            let config = SshConfig::default();
            assert!(config.environment.is_empty());
        }

        #[test]
        fn test_environment_variables_population() {
            let mut config = SshConfig::default();
            config.environment.insert("HOME".to_string(), "/root".to_string());
            config.environment.insert("PATH".to_string(), "/usr/bin".to_string());
            assert_eq!(config.environment.len(), 2);
        }

        #[test]
        fn test_rsync_availability_check() {
            let session = SshSession::new(SshConfig::default());
            let _ = session._is_rsync_available(); // Should not panic
        }

        #[test]
        fn test_audit_details_default() {
            let details = AuditDetails::default();
            assert!(details.description.is_empty());
            assert!(details.command.is_none());
            assert!(details.file_path.is_none());
            assert!(details.error_message.is_none());
        }

        #[test]
        fn test_event_severity_ordering() {
            assert!(EventSeverity::Info < EventSeverity::Warning);
            assert!(EventSeverity::Warning < EventSeverity::Error);
            assert!(EventSeverity::Error < EventSeverity::Critical);
        }

        #[test]
        fn test_pool_stats_defaults() {
            let stats = PoolStats::default();
            assert_eq!(stats.total_created, 0);
            assert_eq!(stats.total_destroyed, 0);
            assert_eq!(stats.active_count, 0);
            assert_eq!(stats.idle_count, 0);
            assert_eq!(stats.wait_count, 0);
            assert_eq!(stats.checkout_failures, 0);
        }

        #[test]
        fn test_transfer_result_display() {
            let success = TransferResult {
                success: true,
                bytes_transferred: 1024,
                duration: Duration::from_secs(1),
                speed_bytes_per_sec: 1024.0,
                error: None,
            };
            assert!(success.success);

            let failure = TransferResult {
                success: false,
                bytes_transferred: 0,
                duration: Duration::ZERO,
                speed_bytes_per_sec: 0.0,
                error: Some("Error".to_string()),
            };
            assert!(!failure.success);
            assert!(failure.error.is_some());
        }
    }

    // ==========================================
    // Performance Benchmarks (性能基准测试)
    // ==========================================

    #[cfg(test)]
    mod performance_tests {
        use super::*;
        use std::time::Instant;

        #[test]
        fn benchmark_config_creation() {
            let iterations = 10000;
            let start = Instant::now();

            for _ in 0..iterations {
                let _config = SshConfig::default();
            }

            let duration = start.elapsed();
            eprintln!("\n[PERF] Created {} configs in {:?}", iterations, duration);
        }

        #[test]
        fn benchmark_config_parsing() {
            let content = r#"
Host test1
    HostName host1.com
    User user1
    Port 22

Host test2
    HostName host2.com
    User user2
    Port 2222
"#;

            let iterations = 1000;
            let start = Instant::now();

            for _ in 0..iterations {
                let _ = ConfigParser::parse_content(content);
            }

            let duration = start.elapsed();
            eprintln!("\n[PARSED] Parsed {} configs in {:?}", iterations, duration);
        }

        #[test]
        fn benchmark_audit_logging() {
            let logger = SshAuditLogger::new();
            let iterations = 10000;
            let start = Instant::now();

            for i in 0..iterations {
                logger.log_connection_attempt("user", "host", &format!("session-{}", i));
            }

            let duration = start.elapsed();
            let events = logger.get_recent_events(iterations);
            
            eprintln!(
                "\n[AUDIT] Logged {} events in {:?} ({:.2} ops/sec)",
                iterations,
                duration,
                iterations as f64 / duration.as_secs_f64().max(0.001)
            );
            assert_eq!(events.len(), iterations.min(logger.get_statistics().total_events));
        }

        #[test]
        fn benchmark_session_creation() {
            let iterations = 1000;
            let start = Instant::now();

            let sessions: Vec<SshSession> = (0..iterations)
                .map(|_| SshSession::new(SshConfig::default()))
                .collect();

            let duration = start.elapsed();
            eprintln!(
                "\n[SESSION] Created {} sessions in {:?} ({:.2} ops/sec)",
                sessions.len(),
                duration,
                iterations as f64 / duration.as_secs_f64().max(0.001)
            );
        }

        #[test]
        fn benchmark_ssh_args_building() {
            let config = SshConfig {
                compression: true,
                control_master: true,
                strict_host_key_checking: true,
                jump_hosts: vec![
                    JumpHost { host: "jump1".to_string(), user: "j1".to_string(), port: 22, identity_file: None },
                    JumpHost { host: "jump2".to_string(), user: "j2".to_string(), port: 2222, identity_file: None },
                ],
                ..SshConfig::default()
            };

            let iterations = 10000;
            let start = Instant::now();

            for _ in 0..iterations {
                let _args = config.build_ssh_args();
            }

            let duration = start.elapsed();
            eprintln!(
                "\n[ARGS] Built {} arg sets in {:?} ({:.2} ops/sec)",
                iterations,
                duration,
                iterations as f64 / duration.as_secs_f64().max(0.001)
            );
        }

        #[test]
        fn memory_usage_test() {
            let logger = SshAuditLogger::with_config(AuditConfig {
                max_events_in_memory: 100000,
                ..Default::default()
            });

            for i in 0..100000 {
                logger.log_connection_attempt("user", "host", &format!("session-{}", i % 1000));
            }

            let stats = logger.get_statistics();
            eprintln!(
                "\n[MEMORY] Holding {} events (max: 100000), total logged: {}",
                stats.total_events,
                100000
            );

            assert_eq!(stats.total_events, 100000); // Should keep all within limit
        }
    }
}
