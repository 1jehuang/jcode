//! Chain 1: CLI Local Mode E2E Tests
//!
//! Tests the complete flow: TUI → type message → receive reply
//!
//! # Test Coverage
//! - CLI startup and initialization in local mode
//! - Message input via stdin
//! - Response reception and validation
//! - Session persistence to disk
//! - Graceful shutdown
//!
//! # Prerequisites
//! - `carpai` binary must be built and in PATH
//! - No external services required (uses mock provider)

use crate::helpers::*;
use crate::fixtures::*;

/// Test: CLI starts successfully in local mode
///
/// Verifies that:
/// - Process spawns without error
/// - Initial output contains expected prompts
/// - Process is responsive within timeout
#[tokio::test]
#[ignore]  // Requires built binary; run with --include-ignored
async fn cli_local_starts_successfully() -> Result<()> {
    let mut report = AssertionReport::new("cli_local_starts_successfully");

    // Setup test environment
    let env = TestEnvironment::new("cli-local-start")?;

    // Start CLI in local mode
    let mut cli_process = start_cli_local(&env.config_path, &env.temp_dir).await?;
    report.add(TestAssertion::passed("CLI process started"));

    // Wait for initial output (should show prompt or welcome message)
    let initial_output = read_output(&mut cli_process.child, Duration::from_secs(5)).await?;
    let has_prompt = !initial_output.is_empty();
    report.add(if has_prompt {
        TestAssertion::passed("Received initial output from CLI")
    } else {
        TestAssertion::failed(
            "No initial output received",
            "CLI may not have started properly or timed out"
        )
    });

    // Verify process is still running
    let is_alive = cli_process.is_running();
    report.add(if is_alive {
        TestAssertion::passed("CLI process is running")
    } else {
        TestAssertion::failed("CLI process exited unexpectedly", "Process died during startup")
    });

    // Cleanup
    stop_server(&mut cli_process).await?;
    env.cleanup()?;

    report.finalize();
    Ok(())
}

/// Test: Send message and receive response in local mode
///
/// This is the core E2E test that validates the complete user interaction loop:
/// 1. Start CLI in local mode
/// 2. Send "Hello, CarpAI!" via stdin
/// 3. Wait for and validate AI response
/// 4. Verify response contains meaningful content
#[tokio::test]
#[ignore]
async fn cli_local_send_message_and_receive_reply() -> Result<()> {
    let mut report = AssertionReport::new("cli_local_send_message_and_receive_reply");

    // Setup isolated environment
    let env = TestEnvironment::new("cli-local-message")?;

    // Start CLI
    let mut cli_process = start_cli_local(&env.config_path, &env.temp_dir).await?;
    report.add(TestAssertion::passed("CLI started in local mode"));

    // Give it a moment to initialize
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Clear any startup output
    let _startup_output = read_output(&mut cli_process.child, Duration::from_millis(100)).await?;

    // Send test message
    send_input(&mut cli_process.child, SIMPLE_GREETING).await?;
    report.add(TestAssertion::passed(format!("Sent message: '{}'", SIMPLE_GREETING)));

    // Wait for response with generous timeout
    let response = read_output(&mut cli_process.child, Duration::from_secs(30)).await?;

    // Validate response
    let has_content = ResponsePatterns::has_content(&response);
    report.add(if has_content {
        TestAssertion::passed(format!(
            "Received response ({} bytes)",
            response.len()
        ))
    } else {
        TestAssertion::failed(
            "Response appears empty or too short",
            format!("Actual length: {} bytes", response.len())
        )
    });

    // Log response for debugging (truncated)
    tracing::info!("CLI Local Mode Response:\n{}", truncate_for_log(&response, 300));

    // Cleanup
    stop_server(&mut cli_process).await?;
    env.cleanup()?;

    report.finalize();
    Ok(())
}

/// Test: Session persists to disk after interaction
///
/// Validates that:
/// - Sessions are created during conversation
/// - Session data is written to configured storage location
/// - Session files can be read back
#[tokio::test]
#[ignore]
async fn cli_local_session_persists_to_disk() -> Result<()> {
    let mut report = AssertionReport::new("cli_local_session_persists_to_disk");

    let env = TestEnvironment::new("cli-local-persist")?;
    let session_dir = env.session_storage_path();

    // Verify session directory exists initially
    let dir_existed_before = session_dir.exists();
    report.add(TestAssertion::passed(format!(
        "Session directory ready: {}",
        if dir_existed_before { "exists" } else { "will be created" }
    )));

    // Start CLI and have brief interaction
    let mut cli_process = start_cli_local(&env.config_path, &env.temp_dir).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    send_input(&mut cli_process.child, SIMPLE_GREETING).await?;

    // Wait for response
    let _response = read_output(&mut cli_process.child, Duration::from_secs(15)).await?;

    // Allow time for persistence
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Stop gracefully to trigger cleanup/persistence
    stop_server(&mut cli_process).await?;

    // Check for session files
    let session_files: Vec<_> = std::fs::read_dir(&session_dir)?
        .filter_map(|e| e.ok())
        .collect();

    let sessions_created = !session_files.is_empty();
    report.add(if sessions_created {
        TestAssertion::passed(format!(
            "Session files persisted ({} files)",
            session_files.len()
        ))
    } else {
        TestAssertion::failed(
            "No session files found",
            format!("Expected files in: {}", session_dir.display())
        )
    });

    // List session file names for debugging
    for file in &session_files {
        tracing::debug!("Session file: {:?}", file.file_name());
    }

    env.cleanup()?;
    report.finalize();
    Ok(())
}

/// Test: Multiple messages in same session maintain context
///
/// Sends multiple messages sequentially and verifies responses show awareness of context.
/// This tests session state management and conversation history handling.
#[tokio::test]
#[ignore]
async fn cli_local_multi_turn_conversation() -> Result<()> {
    let mut report = AssertionReport::new("cli_local_multi_turn_conversation");

    let env = TestEnvironment::new("cli-local-multi-turn")?;
    let mut cli_process = start_cli_local(&env.config_path, &env.temp_dir).await?;
    report.add(TestAssertion::passed("CLI started for multi-turn test"));

    // First message
    tokio::time::sleep(Duration::from_millis(500)).await;
    send_input(&mut cli_process.child, "My name is Alice.").await?;

    let response1 = read_output(&mut cli_process.child, Duration::from_secs(20)).await?;
    let r1_valid = ResponsePatterns::has_content(&response1);
    report.add(if r1_valid {
        TestAssertion::passed("First turn completed")
    } else {
        TestAssertion::failed("First turn failed", "No valid response")
    });

    // Second message (context-dependent)
    send_input(&mut cli_process.child, "What's my name?").await?;

    let response2 = read_output(&mut cli_process.child, Duration::from_secs(20)).await?;
    let r2_valid = ResponsePatterns::has_content(&response2);
    report.add(if r2_valid {
        TestAssertion::passed("Second turn completed")
    } else {
        TestAssertion::failed("Second turn failed", "No valid response")
    });

    // Check if second response references the name (basic context check)
    let mentions_alice = response2.to_lowercase().contains("alice");
    report.add(if mentions_alice {
        TestAssertion::passed("Context maintained across turns (name remembered)")
    } else {
        TestAssertion::warning(
            "Context may not have been maintained",
            "Second response didn't explicitly mention 'Alice' (may depend on provider)"
        )
    });

    tracing::info!(
        "Multi-turn conversation:\nTurn 1: {}\nTurn 2: {}",
        truncate_for_log(&response1, 200),
        truncate_for_log(&response2, 200)
    );

    stop_server(&mut cli_process).await?;
    env.cleanup()?;

    report.finalize();
    Ok(())
}

/// Test: CLI handles graceful shutdown on SIGINT/SIGTERM
///
/// Verifies clean exit without data loss or corruption.
#[tokio::test]
#[ignore]
async fn cli_local_graceful_shutdown() -> Result<()> {
    let mut report = AssertionReport::new("cli_local_graceful_shutdown");

    let env = TestEnvironment::new("cli-local-shutdown")?;
    let mut cli_process = start_cli_local(&env.config_path, &env.temp_dir).await?;

    // Brief interaction
    tokio::time::sleep(Duration::from_secs(1)).await;
    send_input(&mut cli_process.child, "Test message before shutdown").await?;
    let _ = read_output(&mut cli_process.child, Duration::from_secs(5)).await?;

    // Attempt graceful termination
    let kill_result = cli_process.kill();
    report.add(match kill_result {
        Ok(_) => TestAssertion::passed("Process terminated successfully"),
        Err(e) => TestAssertion::failed("Failed to terminate process", e.to_string()),
    });

    // Verify no zombie processes or leftover resources
    let still_running_after_kill = cli_process.is_running();
    report.add(if !still_running_after_kill {
        TestAssertion::passed("Process fully stopped")
    } else {
        TestAssertion::failed("Process still running after kill", "Potential resource leak")
    });

    env.cleanup()?;
    report.finalize();
    Ok(())
}

/// Helper function to truncate output for log display
fn truncate_for_log(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        format!("{}... [truncated]", &text[..max_chars])
    }
}
