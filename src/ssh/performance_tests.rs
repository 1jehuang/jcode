#[cfg(test)]
mod performance_benchmarks {
    use super::*;
    use std::time::{Duration, Instant};
    use std::path::PathBuf;
    use std::fs;

    // ==========================================
    // Performance Benchmark Infrastructure
    // ==========================================

    struct BenchmarkResult {
        name: String,
        iterations: usize,
        total_duration: Duration,
        avg_duration: Duration,
        min_duration: Duration,
        max_duration: Duration,
        ops_per_second: f64,
        memory_usage_bytes: Option<u64>,
    }

    impl std::fmt::Display for BenchmarkResult {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "\n[PERF] {}\n  Iterations: {}\n  Total Time: {:?}\n  Avg/Op: {:?} (±{:?})\n  Ops/sec: {:.2}\n  Memory: {}",
                self.name,
                self.iterations,
                self.total_duration,
                self.avg_duration,
                self.max_duration - self.min_duration,
                self.ops_per_second,
                match self.memory_usage_bytes {
                    Some(bytes) => format!("{:.1} KB", bytes as f64 / 1024.0),
                    None => "N/A".to_string(),
                }
            )
        }
    }

    fn run_benchmark<F>(name: &str, iterations: usize, operation: F) -> BenchmarkResult
    where
        F: Fn() -> (),
    {
        let mut durations = Vec::with_capacity(iterations);
        
        for _ in 0..iterations {
            let start = Instant::now();
            operation();
            durations.push(start.elapsed());
        }

        durations.sort();

        let total: Duration = durations.iter().sum();
        let avg = total / iterations as u32;
        let min = *durations.first().unwrap_or(&Duration::ZERO);
        let max = *durations.last().unwrap_or(&Duration::ZERO);

        BenchmarkResult {
            name: name.to_string(),
            iterations,
            total_duration: total,
            avg_duration: avg,
            min_duration: min,
            max_duration: max,
            ops_per_second: if total.as_secs_f64() > 0.0 {
                iterations as f64 / total.as_secs_f64()
            } else {
                0.0
            },
            memory_usage_bytes: None,  // Would need custom allocator tracking
        }
    }

    // ==========================================
    // SSH Session Creation Benchmarks
    // ==========================================

    #[test]
    fn benchmark_session_creation() {
        let result = run_benchmark("SshSession::new", 10000, || {
            let _session = SshSession::new(SshConfig::default());
        });
        eprintln!("{}", result);
        
        assert!(result.avg_duration < Duration::from_millis(1), 
            "Session creation should be < 1ms, got {:?}", result.avg_duration);
        assert!(result.ops_per_second > 1000.0, 
            "Should create >1000 sessions/sec");
    }

    #[test]
    fn benchmark_config_parsing() {
        let config_content = r#"
Host production
    HostName prod.example.com
    User deployer
    Port 2222
    IdentityFile ~/.ssh/prod_key
    LocalForward 8080 localhost:80
    ServerAliveInterval 60

Host staging
    HostName staging.example.com
    User deployer
    LocalForward 8080 localhost:3000
"#;

        let result = run_benchmark("ConfigParser::parse_content", 5000, || {
            let _ = ConfigParser::parse_content(config_content);
        });
        eprintln!("{}", result);
        
        assert!(result.avg_duration < Duration::from_millis(5),
            "Config parsing should be fast");
    }

    #[test]
    fn benchmark_ssh_args_building() {
        let complex_config = SshConfig {
            compression: true,
            control_master: true,
            jump_hosts: vec![
                JumpHost { host: "bastion1".to_string(), user: "j1".to_string(), port: 22, identity_file: None },
                JumpHost { host: "bastion2".to_string(), user: "j2".to_string(), port: 2222, identity_file: None },
            ],
            local_forwards: vec![
                PortForward { local_port: 8080, remote_host: "localhost".to_string(), remote_port: 3000, bind_address: Some("127.0.0.1".to_string()) },
            ],
            environment: {
                let mut env = HashMap::new();
                env.insert("TEST".to_string(), "value".to_string());
                env
            },
            ..SshConfig::default()
        };

        let result = run_benchmark("SshConfig::build_ssh_args", 20000, || {
            let _args = complex_config.build_ssh_args();
        });
        eprintln!("{}", result);
        
        assert!(result.ops_per_second > 5000.0,
            "Should build >5000 arg sets/sec");
    }

    // ==========================================
    // Connection Pool Benchmarks
    // ==========================================

    #[test]
    fn benchmark_pool_operations() {
        let pool = SshConnectionPool::with_config(PoolConfig {
            max_connections: 20,
            min_idle_connections: 5,
            ..PoolConfig::default()
        });

        let result = run_benchmark("ConnectionPool lifecycle", 5000, || {
            let key = format!("host-{}", rand_random_u32() % 10);
            let _result = pool.get_session(&key);
            // Don't actually return to avoid cleanup overhead in benchmark
        });
        eprintln!("{}", result);
    }

    // ==========================================
    // Audit Logging Throughput
    // ==========================================

    #[test]
    fn benchmark_audit_logging_throughput() {
        let logger = SshAuditLogger::with_config(AuditConfig {
            max_events_in_memory: 100000,
            enabled: true,
            log_to_file: false,  // Disable file I/O for pure logging test
            ..AuditConfig::default()
        });

        let result = run_benchmark("AuditLogger::log", 50000, || {
            logger.log(SshAuditEvent {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp: chrono::Utc::now(),
                event_type: SshEventType::CommandExecutionComplete,
                session_id: Some(format!("sess-{}", rand_random_u32() % 100)),
                user: Some("bench-user".to_string()),
                host: Some("bench-host".to_string()),
                details: AuditDetails {
                    description: "Benchmark event".to_string(),
                    command: Some("echo test".to_string()),
                    exit_code: Some(0),
                    duration_ms: Some(1),
                    ..Default::default()
                },
                severity: EventSeverity::Info,
                source_ip: None,
                metadata: None,
            });
        });
        eprintln!("{}", result);
        
        assert!(result.ops_per_second > 100000.0,
            "Audit logging should handle >100K events/sec");
        
        let stats = logger.get_statistics();
        assert_eq!(stats.total_events, 50000, 
            "Should have logged exactly 50k events");
    }

    #[test]
    fn benchmark_audit_query_performance() {
        let logger = SshAuditLogger::with_config(AuditConfig {
            max_events_in_memory: 100000,
            ..AuditConfig::default()
        });

        // Pre-populate with events
        for i in 0..10000 {
            logger.log_connection_attempt("user", "host", &format!("sess-{}", i));
        }

        let result = run_benchmark("AuditLogger::get_recent_events(100)", 1000, || {
            let _events = logger.get_recent_events(100);
        });
        eprintln!("{}", result);

        let result2 = run_benchmark("AuditLogger::get_events_by_session", 1000, || {
            let _events = logger.get_events_by_session("sess-5000");
        });
        eprintln!("{}", result2);

        let result3 = run_benchmark("AuditLogger::get_events_by_type", 1000, || {
            let _events = logger.get_events_by_type(&SshEventType::ConnectionAttempt);
        });
        eprintln!("{}", result3);

        let result4 = run_benchmark("AuditLogger::get_statistics", 1000, || {
            let _stats = logger.get_statistics();
        });
        eprintln!("{}", result4);
    }

    // ==========================================
    // Resilience Mechanism Overhead
    // ==========================================

    #[test]
    fn benchmark_retry_handler_overhead() {
        let policy = RetryPolicy {
            max_attempts: 5,
            strategy: ReconnectStrategy::ExponentialBackoff {
                initial_delay: Duration::from_millis(10),
                max_delay: Duration::from_secs(1),
                multiplier: 2.0,
                jitter: false,
            },
            ..RetryPolicy::default()
        };

        let handler = SmartRetryHandler::new(policy);

        let result = run_benchmark("SmartRetryHandler operations", 100000, || {
            let delay = handler.calculate_delay(2);  // Simulate 3rd attempt
            let classification = handler.classify_error("Connection reset by peer");
            let retryable = handler.is_retryable(&classification);
            
            // Prevent unused variable warnings
            if delay == Duration::ZERO && !retryable {
                unreachable!();
            }
        });
        eprintln!("{}", result);
        
        assert!(result.ops_per_second > 500000.0,
            "Retry handler should handle >500K ops/sec");
    }

    #[test]
    fn benchmark_circuit_breaker_overhead() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig::default());

        let result = run_benchmark("CircuitBreaker operations", 100000, || {
            let allowed = breaker.allow_request();
            if allowed {
                breaker.record_success();
            } else {
                breaker.record_failure();
            }
        });
        eprintln!("{}", result);
    }

    // ==========================================
    // Memory Usage Benchmarks
    // ==========================================

    #[test]
    fn benchmark_memory_usage_scaling() {
        eprintln!("\n[MEMORY] Scaling test with increasing event counts:");
        
        for count in [1000, 10000, 50000, 100000].iter() {
            let logger = SshAuditLogger::with_config(AuditConfig {
                max_events_in_memory: *count,
                ..AuditConfig::default()
            });

            let start = Instant::now();
            for i in 0..*count {
                logger.log(SshAuditEvent {
                    id: uuid::Uuid::new_v4().to_string(),
                    timestamp: chrono::Utc::now(),
                    event_type: SshEventType::ConnectionAttempt,
                    session_id: Some(format!("mem-test-{}", i)),
                    user: Some("testuser".to_string()),
                    host: Some("testhost".to_string()),
                    details: AuditDetails {
                        description: format!("Memory test event {}", i),
                        ..Default::default()
                    },
                    severity: EventSeverity::Info,
                    source_ip: None,
                    metadata: Some(serde_json::json!({
                        "index": i,
                        "data": "x".repeat(100)  // ~100 bytes of metadata per event
                    })),
                });
            }
            let duration = start.elapsed();
            
            let stats = logger.get_statistics();
            assert_eq!(stats.total_events, *count);
            
            eprintln!(
                "  Holding {} events: {:?} ({:.2} MB estimated)",
                count,
                duration,
                (*count as f64 * 200.0) / (1024.0 * 1024.0)
            );
        }
    }

    // ==========================================
    // Comparative Benchmarks (vs native tools)
    // ==========================================

    #[test]
    fn comparative_scp_vs_enhanced_scp_overhead() {
        eprintln!("\n[COMPARATIVE] EnhancedSCP vs Native SCP Overhead:");

        // Measure our SCP wrapper creation time
        let scp_creation_time = run_benchmark("EnhancedScp::new", 10000, || {
            let _scp = EnhancedScp::new("user", "localhost", 22);
        });
        eprintln!("  {}", scp_creation_time);

        // Measure argument building time
        let scp = EnhancedScp::new("user", "host", 22)
            .preserve_permissions(true)
            .compress(true)
            .bandwidth_limit(1024 * 1024)  // 1MB/s
            .verify_checksum(true, ChecksumAlgo::Sha256);

        let args_build_time = run_benchmark("EnhancedScp option chain", 5000, || {
            let configured = EnhancedScp::new("user", "host", 22)
                .preserve_permissions(true)
                .symlink_behavior(SymlinkBehavior::Follow)
                .bandwidth_limit(1024 * 1024);
            // Don't actually execute, just measure configuration cost
        });
        eprintln!("  {}", args_build_time);

        eprintln!("\n  Analysis:");
        eprintln!("    - Object creation: {:?} (should be < 100μs)", scp_creation_time.avg_duration);
        eprintln!("    - Option chaining: {:?} (should be < 50μs)", args_build_time.avg_duration);
        eprintln!("    - Expected overhead vs native scp: < 1ms per invocation");
    }

    #[test]
    fn comparative_sftp_vs_rsync_feature_comparison() {
        eprintln!("\n[FEATURE MATRIX] SFTP vs Rsync Feature Comparison:");

        let features = vec![
            ("Recursive directory sync", true, true),
            ("Resume interrupted transfers", true, true),
            ("Bandwidth limiting", true, true),
            ("Progress tracking", true, true),
            ("Checksum verification", true, true),
            ("Permission preservation", true, true),
            ("Symbolic link handling", true, true),
            ("Compression support", true, true),
            ("Delete mode (--delete)", false, true),
            ("Exclude patterns", false, true),
            ("Hard link preservation", false, true),
            ("ACL preservation", false, true),
            ("Cross-platform", true, true),
            ("Interactive prompts", false, false),  // Both batch-mode capable
        ];

        for (feature, sftp_has, rsync_has) in features {
            let sftp_marker = if sftp_has { "✅" } else { "❌" };
            let rsync_marker = if rsync_has { "✅" } else { "❌" };
            eprintln!("  {} {}: SFTP {} | Rsync {}", feature, sftp_marker, rsync_marker);
        }
    }

    // ==========================================
    // Stress Tests
    // ==========================================

    #[test]
    fn stress_test_concurrent_sessions() {
        eprintln!("\n[STRESS] Concurrent Session Management:");
        
        let pool = Arc::new(SshConnectionPool::with_config(PoolConfig {
            max_connections: 100,
            connection_timeout: Duration::from_millis(100),
            health_check_interval: Duration::from_secs(1),
            enable_auto_reconnect: false,  // Disable auto-reconnect for pure pool test
            ..PoolConfig::default()
        }));

        let handles: Vec<_> = (0..50).map(|i| {
            let pool_clone = Arc::clone(&pool);
            std::thread::spawn(move || {
                let start = Instant::now();
                
                // Simulate rapid session acquire/release cycles
                for cycle in 0..100 {
                    let key = format!("stress-host-{}-{}", i, cycle);
                    
                    // Try to get session (will fail since no real server, but tests pool logic)
                    let _result = pool_clone.get_session(&key);
                    
                    // Small sleep to simulate work
                    std::thread::sleep(Duration::from_micros(100));
                }
                
                start.elapsed()
            })
        }).collect();

        let total_start = Instant::now();
        let results: Vec<Duration> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let total_elapsed = total_start.elapsed();

        let avg_per_thread = results.iter().sum::<Duration>() / results.len() as u32;
        let max_thread = results.iter().max().unwrap_or(&Duration::ZERO).clone();
        let min_thread = results.iter().min().unwrap_or(&Duration::ZERO).clone();

        eprintln!("  Threads: 50");
        eprintln!("  Cycles/thread: 100");
        eprintln!("  Total wall time: {:?}", total_elapsed);
        eprintln!("  Avg thread time: {:?}", avg_per_thread);
        eprintln!("  Max thread time: {:?}", max_thread);
        eprintln!("  Min thread time: {:?}", min_thread);
        eprintln!("  Pool stats: active={}, idle={}", 
            pool.active_count(), pool.idle_count());
    }

    #[test]
    fn stress_test_audit_log_high_volume() {
        eprintln!("\n[STRESS] High-volume Audit Logging:");

        let logger = SshAuditLogger::with_config(AuditConfig {
            max_events_in_memory: 1000000,  // 1M events
            ..AuditConfig::default()
        });

        let start = Instant::now();
        let target_events = 250000;  // 250K events
        
        // Spawn multiple threads to generate events concurrently
        let handles: Vec<_> = (0..4).map(|thread_id| {
            let logger_ref = unsafe { &*(&logger as *const _) as &SshAuditLogger };  // Unsafe but ok for benchmark
            std::thread::spawn(move || {
                for i in 0..(target_events / 4) {
                    logger_ref.log(SshAuditEvent {
                        id: uuid::Uuid::new_v4().to_string(),
                        timestamp: chrono::Utc::now(),
                        event_type: match i % 5 {
                            0 => SshEventType::ConnectionAttempt,
                            1 => SshEventType::CommandExecutionComplete,
                            2 => SshEventType::FileUploadComplete,
                            3 => SshEventType::FileDownloadComplete,
                            _ => SshEventType::Disconnection,
                        },
                        session_id: Some(format!("stress-thread{}-{}", thread_id, i)),
                        user: Some("stress-user".to_string()),
                        host: Some(format!("stress-host-{}.example.com", i % 100)),
                        details: AuditDetails {
                            description: format!("Stress test event from thread {}", thread_id),
                            command: Some(format!("command_{}", i)),
                            exit_code: Some(if i % 10 == 0 { 1 } else { 0 }),
                            duration_ms: Some((i % 1000) as u64),
                            ..Default::default()
                        },
                        severity: if i % 20 == 0 { EventSeverity::Warning } else { EventSeverity::Info },
                        source_ip: Some(format!("192.168.1.{}", i % 256)),
                        metadata: Some(serde_json::json!({
                            "thread": thread_id,
                            "sequence": i,
                            "payload": "x".repeat(50)
                        })),
                    });
                }
            })
        }).collect();

        for handle in handles {
            handle.join().expect("Thread panicked");
        }

        let duration = start.elapsed();
        let final_stats = logger.get_statistics();

        eprintln!("  Target events: {}", target_events);
        eprintln!("  Actual logged: {}", final_stats.total_events);
        eprintln!("  Total time: {:?}", duration);
        eprintln!("  Throughput: {:.2} events/sec", 
            target_events as f64 / duration.as_secs_f64().max(0.001));
        eprintln!("  Memory estimate: {:.1} MB", 
            (target_events as f64 * 300.0) / (1024.0 * 1024.0));  // ~300 bytes/event

        assert_eq!(final_stats.total_events, target_events,
            "All events should be logged successfully");
    }

    // Helper function to generate pseudo-random numbers (for benchmarks only)
    fn rand_random_u32() -> u32 {
        use std::time::SystemTime;
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u32
    }
}

// Note: These benchmarks are designed to run quickly and provide relative performance data.
// For absolute performance numbers, use `cargo bench` or criterion.
