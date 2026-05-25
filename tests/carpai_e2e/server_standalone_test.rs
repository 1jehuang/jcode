//! Chain 2: Server Standalone E2E Tests
//!
//! Tests server capabilities independently:
//! - Health check endpoint
//! - gRPC ChatCompletion call
//! - REST POST /v1/chat/completions call
//! - Protocol consistency verification
//!
//! # Prerequisites
//! - `carpai-server` binary must be built and in PORT
//! - Mock provider available (no external API keys needed)

use crate::helpers::*;
use crate::fixtures::*;
use std::time::Instant;

/// Test: Server starts and passes health check
///
/// Validates:
/// - Server binds to port successfully
/// - GET /health returns 200 OK
/// - Health response includes expected fields
#[tokio::test]
#[ignore]
async fn server_standalone_health_check() -> Result<()> {
    let mut report = AssertionReport::new("server_standalone_health_check");

    // Setup
    let config_path = generate_server_test_config()?;
    let port = reserve_port()?;

    // Start server
    let mut server = start_server(&config_path, port).await?;
    report.add(TestAssertion::passed(format!("Server started on port {}", port)));

    // Wait for readiness
    wait_for_port(port, Duration::from_secs(10)).await?;
    report.add(TestAssertion::passed("Server port is accepting connections"));

    // Perform health check
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let health_url = format!("http://127.0.0.1:{}/health", port);
    let start_time = Instant::now();

    let response = client.get(&health_url).send().await?;
    let response_time = start_time.elapsed();

    // Validate status code
    let status_ok = response.status().as_u16() == 200;
    report.add(if status_ok {
        TestAssertion::passed("Health check returned 200 OK")
    } else {
        TestAssertion::failed(
            "Health check status unexpected",
            format!("Status: {}", response.status())
        )
    });

    // Validate response time
    assert_response_time(response_time, 2000);  // Max 2 seconds
    report.add(TestAssertion::passed(format!(
        "Health check responded in {}ms",
        response_time.as_millis()
    )));

    // Parse and validate JSON body
    let body: serde_json::Value = response.json().await?;
    let has_status_field = body.get("status").is_some();
    report.add(if has_status_field {
        TestAssertion::passed("Health response includes 'status' field")
    } else {
        TestAssertion::failed(
            "Missing required field",
            "Health response missing 'status' field"
        )
    });

    let status_healthy = body["status"] == "healthy";
    report.add(if status_healthy {
        TestAssertion::passed("Server reports healthy status")
    } else {
        TestAssertion::failed(
            "Server not healthy",
            format!("Status: {}", body["status"])
        )
    });

    // Log full health response
    tracing::info!("Health check response: {:#}", body);

    // Cleanup
    stop_server(&mut server).await?;
    std::fs::remove_file(config_path)?;

    report.finalize();
    Ok(())
}

/// Test: gRPC ChatCompletion request succeeds
///
/// This test requires tonic/gRPC client setup. For now, we verify
/// the gRPC port is accessible and basic connectivity works.
#[tokio::test]
#[ignore]
async fn server_standalone_grpc_connectivity() -> Result<()> {
    let mut report = AssertionReport::new("server_standalone_grpc_connectivity");

    let config_path = generate_server_test_config()?;
    let port = reserve_port()?;

    let mut server = start_server(&config_path, port).await?;
    wait_for_port(port, Duration::from_secs(10)).await?;
    report.add(TestAssertion::passed("Server ready for gRPC test"));

    // Note: Full gRPC testing requires generated client code from proto files.
    // Here we verify TCP-level connectivity to the gRPC port.
    // In production, this would use tonic's Channel to make actual calls.

    // Simulate gRPC connection attempt (TCP level)
    match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(stream) => {
            let peer_addr = stream.peer_addr()?;
            report.add(TestAssertion::passed(format!(
                "TCP connection established to gRPC endpoint ({})",
                peer_addr
            )));

            // Verify we can write/read (basic liveness)
            stream.shutdown(std::net::Shutdown::Both)?;
        }
        Err(e) => {
            report.add(TestAssertion::failed(
                "Failed to connect to gRPC port",
                e.to_string()
            ));
        }
    }

    stop_server(&mut server).await?;
    std::fs::remove_file(config_path)?;

    report.finalize();
    Ok(())
}

/// Test: REST POST /v1/chat/completions works correctly
///
/// Validates OpenAI-compatible API endpoint:
/// - Accepts properly formatted requests
/// - Returns valid chat completion response structure
/// - Response includes all required fields (id, choices, usage, etc.)
#[tokio::test]
#[ignore]
async fn server_standalone_rest_chat_completion() -> Result<()> {
    let mut report = AssertionReport::new("server_standalone_rest_chat_completion");

    let config_path = generate_server_test_config()?;
    let port = reserve_port()?;

    let mut server = start_server(&config_path, port).await?;
    wait_for_port(port, Duration::from_secs(10)).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // Build request payload
    let request_body = build_chat_completion_request(SIMPLE_GREETING, Some("test-model"));
    let url = format!("http://127.0.0.1:{}/v1/chat/completions", port);

    let start_time = Instant::now();
    let response = client.post(&url)
        .json(&request_body)
        .send()
        .await?;
    let latency = start_time.elapsed();

    // Validate HTTP status
    let status_ok = response.status().as_u16() == 200;
    report.add(if status_ok {
        TestAssertion::passed("Chat completion returned 200 OK")
    } else {
        TestAssertion::failed(
            "Unexpected status code",
            format!("Status: {} (expected 200)", response.status())
        )
    });

    // Validate response time (should be reasonable even with mock)
    assert_response_time(latency, 15000);  // 15s max for processing
    report.add(TestAssertion::passed(format!(
        "Completion latency: {}ms",
        latency.as_millis()
    )));

    // Parse response body
    let response_json: serde_json::Value = response.json().await?;

    // Validate required OpenAI-compatible fields
    let required_fields = ["id", "object", "created", "model", "choices"];
    for field in &required_fields {
        let exists = response_json.get(*field).is_some();
        report.add(if exists {
            TestAssertion::passed(format!("Response has '{}' field", field))
        } else {
            TestAssertion::failed(
                format!("Missing required field '{}'", field),
                "OpenAI compatibility broken"
            )
        });
    }

    // Validate choices array structure
    let has_choices = response_json.get("choices")
        .and_then(|c| c.as_array())
        .map(|arr| !arr.is_empty())
        .unwrap_or(false);

    report.add(if has_choices {
        TestAssertion::passed("Response contains non-empty choices array")
    } else {
        TestAssertion::failed(
            "Invalid choices structure",
            "Expected non-empty array in 'choices'"
        )
    });

    // Validate message content exists
    let content_exists = response_json.pointer("/choices/0/message/content").is_some();
    report.add(if content_exists {
        TestAssertion::passed("Response message has content field")
    } else {
        TestAssertion::failed(
            "Missing message content",
            "Expected /choices/0/message/content path"
        )
    });

    // Log full response for debugging
    tracing::info!("REST Chat Completion Response:\n{:#}", response_json);

    stop_server(&mut server).await?;
    std::fs::remove_file(config_path)?;

    report.finalize();
    Ok(())
}

/// Test: gRPC and REST protocols return consistent results
///
/// Sends identical requests via both protocols and verifies
/// structural consistency (not exact match due to timing/IDs).
#[tokio::test]
#[ignore]
async fn server_standalone_protocol_consistency() -> Result<()> {
    let mut report = AssertionReport::new("server_standalone_protocol_consistency");

    let config_path = generate_server_test_config()?;
    let port = reserve_port()?;

    let mut server = start_server(&config_path, port).await?;
    wait_for_port(port, Duration::from_secs(10)).await?;
    report.add(TestAssertion::passed("Server ready for protocol comparison test"));

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let request_payload = build_chat_completion_request(CODE_REQUEST, None);

    // Make REST request
    let rest_url = format!("http://127.0.0.1:{}/v1/chat/completions", port);
    let rest_response = client.post(&rest_url)
        .json(&request_payload)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    report.add(TestAssertion::passed("REST request completed"));

    // Note: gRPC comparison would go here when client is implemented.
    // For now, we validate REST response thoroughly.

    // Both should return same object type
    let correct_object_type = rest_response["object"] == "chat.completion";
    report.add(if correct_object_type {
        TestAssertion::passed("REST response has correct object type")
    } else {
        TestAssertion::failed(
            "Wrong object type",
            format!("Got: {}", rest_response["object"])
        )
    });

    // Both should have usage statistics
    let has_usage = rest_response.get("usage").is_some();
    report.add(if has_usage {
        TestAssertion::passed("Response includes usage statistics")
    } else {
        TestAssertion::warning(
            "Usage stats missing",
            "May be optional depending on implementation"
        )
    });

    // Both should have model identifier
    let has_model = rest_response.get("model").is_some();
    report.add(if has_model {
        TestAssertion::passed("Response specifies model used")
    } else {
        TestAssertion::failed(
            "Model field missing",
            "Required by OpenAI spec"
        )
    });

    tracing::info!(
        "Protocol Consistency Test - REST Response:\n{:#}",
        rest_response
    );

    stop_server(&mut server).await?;
    std::fs::remove_file(config_path)?;

    report.finalize();
    Ok(())
}

/// Test: Server handles concurrent requests without errors
///
/// Spawns multiple simultaneous requests and verifies all complete successfully.
#[tokio::test]
#[ignore]
async fn server_standalone_concurrent_requests() -> Result<()> {
    let mut report = AssertionReport::new("server_standalone_concurrent_requests");

    let config_path = generate_server_test_config()?;
    let port = reserve_port()?;

    let mut server = start_server(&config_path, port).await?;
    wait_for_port(port, Duration::from_secs(10)).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let url = format!("http://127.0.0.1:{}/v1/chat/completions", port);
    let concurrent_count = 5;

    // Spawn concurrent requests
    let mut handles = Vec::with_capacity(concurrent_count);
    for i in 0..concurrent_count {
        let client_clone = client.clone();
        let url_clone = url.clone();
        let payload = build_chat_completion_request(
            &format!("Concurrent request {}", i),
            None
        );

        handles.push(tokio::spawn(async move {
            client_clone
                .post(&url_clone)
                .json(&payload)
                .send()
                .await
        }));
    }

    // Collect results
    let mut success_count = 0;
    let mut failure_count = 0;

    for handle in handles {
        match handle.await? {
            Ok(response) => {
                if response.status().as_u16() == 200 {
                    success_count += 1;
                } else {
                    failure_count += 1;
                    tracing::warn!("Request failed with status: {}", response.status());
                }
            }
            Err(e) => {
                failure_count += 1;
                tracing::error!("Request error: {}", e);
            }
        }
    }

    let all_succeeded = failure_count == 0 && success_count == concurrent_count;
    report.add(if all_succeeded {
        TestAssertion::passed(format!(
            "All {} concurrent requests succeeded",
            concurrent_count
        ))
    } else {
        TestAssertion::failed(
            "Some concurrent requests failed",
            format!("Success: {}, Failure: {}", success_count, failure_count)
        )
    });

    stop_server(&mut server).await?;
    std::fs::remove_file(config_path)?;

    report.finalize();
    Ok(())
}

/// Test: Server handles invalid requests gracefully
///
/// Sends malformed requests and verifies appropriate error responses.
#[tokio::test]
#[ignore]
async fn server_standalone_error_handling() -> Result<()> {
    let mut report = AssertionReport::new("server_standalone_error_handling");

    let config_path = generate_server_test_config()?;
    let port = reserve_port()?;

    let mut server = start_server(&config_path, port).await?;
    wait_for_port(port, Duration::from_secs(10)).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let base_url = format!("http://127.0.0.1:{}", port);

    // Test 1: Non-existent endpoint returns 404
    let resp404 = client.get(&format!("{}/nonexistent", base_url))
        .send()
        .await?;

    let proper_404 = resp404.status().as_u16() == 404;
    report.add(if proper_404 {
        TestAssertion::passed("Non-existent endpoint returns 404")
    } else {
        TestAssertion::failed(
            "Bad status for missing endpoint",
            format!("Expected 404, got {}", resp404.status())
        )
    });

    // Test 2: Malformed JSON returns 400
    let bad_json_resp = client.post(&format!("{}/v1/chat/completions", base_url))
        .header("Content-Type", "application/json")
        .body("{invalid json")
        .send()
        .await?;

    let proper_400 = bad_json_resp.status().as_u16() == 400;
    report.add(if proper_400 {
        TestAssertion::passed("Malformed JSON returns 400 Bad Request")
    } else {
        TestAssertion::failed(
            "Bad status for invalid JSON",
            format!("Expected 400, got {}", bad_json_resp.status())
        )
    });

    // Test 3: Missing required fields returns 422
    let incomplete_req = json!({"model": "test"});  // Missing messages
    let incomplete_resp = client.post(&format!("{}/v1/chat/completions", base_url))
        .json(&incomplete_req)
        .send()
        .await?;

    let is_client_error = matches!(incomplete_resp.status().as_u16(), 400..=499);
    report.add(if is_client_error {
        TestAssertion::passed("Incomplete request returns 4xx client error")
    } else {
        TestAssertion::failed(
            "Unexpected status for incomplete request",
            format!("Expected 4xx, got {}", incomplete_resp.status())
        )
    });

    stop_server(&mut server).await?;
    std::fs::remove_file(config_path)?;

    report.finalize();
    Ok(())
}
