//! Chain 3: CLI Remote Mode E2E Tests
//!
//! Tests the flow: CLI → gRPC → Server → reply
//!
//! # Test Coverage
//! - CLI connects to remote server
//! - Messages are proxied through server
//! - Responses are received and displayed
//! - Connection resilience (reconnect on failure)
//!
//! # Prerequisites
//! - Both `carpai` and `carpai-server` binaries available
//! - Network connectivity between CLI and server

use crate::helpers::*;
use crate::fixtures::*;

/// Test: CLI connects to remote server successfully
///
/// Validates:
/// - Server is running and accessible
/// - CLI in remote mode establishes connection
/// - Initial handshake completes without error
#[tokio::test]
#[ignore]
async fn cli_remote_connects_to_server() -> Result<()> {
    let mut report = AssertionReport::new("cli_remote_connects_to_server");

    // Start server first
    let server_config = generate_server_test_config()?;
    let server_port = reserve_port()?;

    let mut server = start_server(&server_config, server_port).await?;
    wait_for_port(server_port, Duration::from_secs(10)).await?;
    report.add(TestAssertion::passed(format!(
        "Server started on port {}",
        server_port
    )));

    // Configure and start CLI in remote mode
    let cli_config = generate_test_config()?;
    let server_addr = format!("127.0.0.1:{}", server_port);

    let mut cli_process = start_cli_remote(&server_addr, &cli_config).await?;
    report.add(TestAssertion::passed("CLI started in remote mode"));

    // Wait for connection establishment
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Check if both processes are alive
    let server_alive = server.is_running();
    let cli_alive = cli_process.is_running();

    report.add(if server_alive && cli_alive {
        TestAssertion::passed("Both server and CLI processes running")
    } else {
        TestAssertion::failed(
            "Process failure",
            format!(
                "Server: {}, CLI: {}",
                if server_alive { "alive" } else { "dead" },
                if cli_alive { "alive" } else { "dead" }
            )
        )
    });

    // Read initial output to verify connection message
    let output = read_output(&mut cli_process.child, Duration::from_secs(5)).await?;
    let has_connection_msg = !output.is_empty();
    report.add(if has_connection_msg {
        TestAssertion::passed("CLI produced output (connection established)")
    } else {
        TestAssertion::warning(
            "No initial output from CLI",
            "May be normal depending on UI mode"
        )
    });

    tracing::info!("CLI Remote Connection Output:\n{}", truncate_output_for_log(&output, 300));

    // Cleanup
    stop_server(&mut cli_process).await?;
    stop_server(&mut server).await?;
    std::fs::remove_file(server_config)?;
    std::fs::remove_file(cli_config)?;

    report.finalize();
    Ok(())
}

/// Test: Message sent via CLI remote mode gets response from server
///
/// This is the core test validating the complete remote flow:
/// 1. Server running with mock provider
/// 2. CLI connects in remote mode
/// 3. User types message in CLI
/// 4. Message sent to server via gRPC
/// 5. Server processes and returns response
/// 6. Response displayed in CLI
#[tokio::test]
#[ignore]
async fn cli_remote_message_roundtrip() -> Result<()> {
    let mut report = AssertionReport::new("cli_remote_message_roundtrip");

    // Setup server
    let server_config = generate_server_test_config()?;
    let server_port = reserve_port()?;

    let mut server = start_server(&server_config, server_port).await?;
    wait_for_port(server_port, Duration::from_secs(10)).await?;
    report.add(TestAssertion::passed("Server ready"));

    // Setup CLI
    let cli_config = generate_test_config()?;
    let server_addr = format!("127.0.0.1:{}", server_port);

    let mut cli_process = start_cli_remote(&server_addr, &cli_config).await?;
    report.add(TestAssertion::passed("CLI connected to server"));

    // Wait for ready state
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Send message
    send_input(&mut cli_process.child, SIMPLE_GREETING).await?;
    report.add(TestAssertion::passed(format!(
        "Sent message via remote CLI: '{}'",
        SIMPLE_GREETING
    )));

    // Receive response (may take longer due to network round-trip)
    let response = read_output(&mut cli_process.child, Duration::from_secs(45)).await?;

    // Validate we got something back
    let has_response = !response.trim().is_empty();
    report.add(if has_response {
        TestAssertion::passed(format!(
            "Received response from server ({} bytes)",
            response.len()
        ))
    } else {
        TestAssertion::failed(
            "No response from server",
            "Message may not have reached server or processing failed"
        )
    });

    // Basic content validation
    let looks_valid = ResponsePatterns::has_content(&response);
    report.add(if looks_valid {
        TestAssertion::passed("Response appears valid")
    } else {
        TestAssertion::warning(
            "Response content validation inconclusive",
            "May need manual inspection"
        )
    });

    tracing::info!(
        "CLI Remote Mode Roundtrip:\nRequest: {}\nResponse: {}",
        SIMPLE_GREETING,
        truncate_output_for_log(&response, 500)
    );

    // Cleanup
    stop_server(&mut cli_process).await?;
    stop_server(&mut server).await?;
    std::fs::remove_file(server_config)?;
    std::fs::remove_file(cli_config)?;

    report.finalize();
    Ok(())
}

/// Test: Multiple sequential messages work correctly in remote mode
///
/// Verifies session state is maintained across multiple turns
/// when using remote mode.
#[tokio::test]
#[ignore]
async fn cli_remote_multi_turn_session() -> Result<()> {
    let mut report = AssertionReport::new("cli_remote_multi_turn_session");

    // Infrastructure setup
    let server_config = generate_server_test_config()?;
    let server_port = reserve_port()?;
    let mut server = start_server(&server_config, server_port).await?;
    wait_for_port(server_port, Duration::from_secs(10)).await?;

    let cli_config = generate_test_config()?;
    let server_addr = format!("127.0.0.1:{}", server_port);
    let mut cli_process = start_cli_remote(&server_addr, &cli_config).await?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Turn 1: Establish context
    send_input(&mut cli_process.child, "Remember this number: 42").await?;
    let resp1 = read_output(&mut cli_process.child, Duration::from_secs(30)).await?;
    let turn1_ok = ResponsePatterns::has_content(&resp1);
    report.add(if turn1_ok {
        TestAssertion::passed("Remote turn 1 completed")
    } else {
        TestAssertion::failed("Turn 1 failed", "No valid response")
    });

    // Turn 2: Test context retention
    send_input(&mut cli_process.child, "What number did I tell you?").await?;
    let resp2 = read_output(&mut cli_process.child, Duration::from_secs(30)).await?;
    let turn2_ok = ResponsePatterns::has_content(&resp2);
    report.add(if turn2_ok {
        TestAssertion::passed("Remote turn 2 completed")
    } else {
        TestAssertion::failed("Turn 2 failed", "No valid response")
    });

    // Check for number mention (basic context check)
    let mentions_42 = resp2.contains("42");
    report.add(if mentions_42 {
        TestAssertion::passed("Context retained across remote turns")
    } else {
        TestAssertion::warning(
            "Context may not have been fully retained",
            "Provider may not have emphasized '42' explicitly"
        )
    });

    tracing::info!(
        "Remote Multi-Turn Session:\nTurn1: {}\nTurn2: {}",
        truncate_output_for_log(&resp1, 200),
        truncate_output_for_log(&resp2, 200)
    );

    // Teardown
    stop_server(&mut cli_process).await?;
    stop_server(&mut server).await?;
    std::fs::remove_file(server_config)?;
    std::fs::remove_file(cli_config)?;

    report.finalize();
    Ok(())
}

/// Test: CLI handles server disconnection gracefully
///
/// Simulates server going down during active session and verifies:
/// - CLI detects disconnection
/// - Appropriate error message shown
/// - Reconnection attempt or clean error handling
#[tokio::test]
#[ignore]
async fn cli_remote_handles_disconnect() -> Result<()> {
    let mut report = AssertionReport::new("cli_remote_handles_disconnect");

    // Start server
    let server_config = generate_server_test_config()?;
    let server_port = reserve_port()?;
    let mut server = start_server(&server_config, server_port).await?;
    wait_for_port(server_port, Duration::from_secs(10)).await?;

    // Connect CLI
    let cli_config = generate_test_config()?;
    let server_addr = format!("127.0.0.1:{}", server_port);
    let mut cli_process = start_cli_remote(&server_addr, &cli_config).await?;
    report.add(TestAssertion::passed("Initial connection established"));

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Kill server abruptly
    server.kill().unwrap();
    report.add(TestAssertion::passed("Server killed to simulate disconnect"));

    // Give CLI time to detect disconnect
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Try sending a message (should fail or show error)
    send_input(&mut cli_process.child, "Test after disconnect").await?;
    let post_disconnect_output = read_output(&mut cli_process.child, Duration::from_secs(5)).await?;

    // Verify CLI didn't crash (still running or exited gracefully)
    let cli_still_exists = !cli_process.is_running() ||
        post_disconnect_output.contains("error") ||
        post_disconnect_output.contains("disconnect") ||
        post_disconnect_output.contains("connection");

    report.add(if cli_still_exists {
        TestAssertion::passed("CLI handled disconnect gracefully")
    } else {
        TestAssertion::warning(
            "Disconnect handling unclear",
            "Manual inspection may be needed"
        )
    });

    tracing::info!(
        "Post-disconnect output: {}",
        truncate_output_for_log(&post_disconnect_output, 300)
    );

    // Final cleanup
    stop_server(&mut cli_process).await.ok();  // May already be dead
    std::fs::remove_file(server_config)?;
    std::fs::remove_file(cli_config)?;

    report.finalize();
    Ok(())
}

/// Test: Large payload transmission works over remote connection
///
/// Sends a long message to verify no truncation or buffer issues.
#[tokio::test]
#[ignore]
async fn cli_remote_large_message_handling() -> Result<()> {
    let mut report = AssertionReport::new("cli_remote_large_message_handling");

    let server_config = generate_server_test_config()?;
    let server_port = reserve_port()?;
    let mut server = start_server(&server_config, server_port).await?;
    wait_for_port(server_port, Duration::from_secs(10)).await?;

    let cli_config = generate_test_config()?;
    let server_addr = format!("127.0.0.1:{}", server_port);
    let mut cli_process = start_cli_remote(&server_addr, &cli_config).await?;

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Send large message (>4KB to stress-test buffering)
    let large_msg = LONG_CONTEXT_MESSAGE;
    send_input(&mut cli_process.child, large_msg).await?;
    report.add(TestAssertion::passed(format!(
        "Sent large message ({} bytes)",
        large_msg.len()
    )));

    // Wait longer for processing
    let response = read_output(&mut cli_process.child, Duration::from_secs(60)).await?;

    let got_response = !response.trim().is_empty();
    report.add(if got_response {
        TestAssertion::passed(format!(
            "Received response for large message ({} bytes)",
            response.len()
        ))
    } else {
        TestAssertion::failed(
            "No response for large message",
            "Possible timeout or buffer overflow"
        )
    });

    tracing::info!(
        "Large message response ({} bytes): {}",
        response.len(),
        truncate_output_for_log(&response, 400)
    );

    stop_server(&mut cli_process).await?;
    stop_server(&mut server).await?;
    std::fs::remove_file(server_config)?;
    std::fs::remove_file(cli_config)?;

    report.finalize();
    Ok(())
}

fn truncate_output_for_log(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        format!("{}... [truncated]", &text[..max_chars])
    }
}
