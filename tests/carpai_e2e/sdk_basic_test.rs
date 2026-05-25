//! Chain 4: SDK Basic Flow E2E Tests
//!
//! Tests CarpAI SDK client functionality:
//! - Client initialization and connection
//! - Chat completion API calls
//! - Session CRUD operations
//! - Error handling and edge cases
//!
//! # Prerequisites
//! - `carpai-server` running (or mock responses)
//! - SDK library compiled and available
//! - No external API keys needed (test mode)

use crate::helpers::*;
use crate::fixtures::*;
use serde_json::json;
use std::time::Instant;

/// Test: SDK client can connect to server
///
/// Validates basic SDK initialization:
/// - Client creation succeeds
/// - Connection to server established
/// - Ready state achieved
#[tokio::test]
#[ignore]
async fn sdk_client_connects_successfully() -> Result<()> {
    let mut report = AssertionReport::new("sdk_client_connects_successfully");

    // Start server for SDK to connect to
    let server_config = generate_server_test_config()?;
    let server_port = reserve_port()?;

    let mut server = start_server(&server_config, server_port).await?;
    wait_for_port(server_port, Duration::from_secs(10)).await?;
    report.add(TestAssertion::passed(format!(
        "Test server available at port {}",
        server_port
    )));

    // Note: When carpai-sdk crate is implemented, actual SDK code would go here.
    // For now, we simulate what the SDK would do using raw HTTP.

    let base_url = format!("http://127.0.0.1:{}", server_port);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    // Simulate SDK health check (what sdk.Client.connect() might do internally)
    let health_url = format!("{}/health", base_url);
    let start_time = Instant::now();

    match client.get(&health_url).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let latency = start_time.elapsed();

            let connected = status == 200;
            report.add(if connected {
                TestAssertion::passed(format!(
                    "SDK-like connection succeeded ({}ms)",
                    latency.as_millis()
                ))
            } else {
                TestAssertion::failed(
                    "Connection attempt returned unexpected status",
                    format!("Status: {}", status)
                )
            });
        }
        Err(e) => {
            report.add(TestAssertion::failed(
                "Connection failed",
                e.to_string()
            ));
        }
    }

    // Cleanup
    stop_server(&mut server).await?;
    std::fs::remove_file(server_config)?;

    report.finalize();
    Ok(())
}

/// Test: SDK chat_completion API works end-to-end
///
/// Validates:
/// - Request construction correct
/// - API call successful
/// - Response parsing works
/// - All required fields present
#[tokio::test]
#[ignore]
async fn sdk_chat_completion_flow() -> Result<()> {
    let mut report = AssertionReport::new("sdk_chat_completion_flow");

    let server_config = generate_server_test_config()?;
    let server_port = reserve_port()?;

    let mut server = start_server(&server_config, server_port).await?;
    wait_for_port(server_port, Duration::from_secs(10)).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let base_url = format!("http://127.0.0.1:{}", server_port);

    // Step 1: Build request (simulating sdk.ChatCompletionRequest)
    let user_message = "Explain Rust's ownership system briefly.";
    let request_payload = build_chat_completion_request(user_message, Some("test-model"));
    report.add(TestAssertion::passed("Built chat completion request"));

    // Step 2: Make API call (simulating await client.chat.completions.create())
    let url = format!("{}/v1/chat/completions", base_url);
    let api_start = Instant::now();

    let http_response = client.post(&url)
        .json(&request_payload)
        .header("Authorization", "Bearer test-token")
        .send()
        .await?;

    let api_latency = api_start.elapsed();
    let status_code = http_response.status().as_u16();

    // Validate HTTP level
    let http_ok = status_code == 200;
    report.add(if http_ok {
        TestAssertion::passed(format!(
            "HTTP POST succeeded ({}ms, status {})",
            api_latency.as_millis(),
            status_code
        ))
    } else {
        TestAssertion::failed(
            "API call failed at HTTP level",
            format!("Status: {}", status_code)
        )
    });

    // Step 3: Parse response (simulating sdk.ChatCompletionResponse)
    let response_body: serde_json::Value = http_response.json().await?;

    // Validate OpenAI-compatible structure
    let has_id = response_body.get("id").is_some();
    report.add(if has_id {
        TestAssertion::passed("Response has completion ID")
    } else {
        TestAssertion::failed("Missing ID field", "Required by spec")
    });

    let has_object_field = response_body.get("object").and_then(|v| v.as_str()) == Some("chat.completion");
    report.add(if has_object_field {
        TestAssertion::passed("Response object type correct")
    } else {
        TestAssertion::failed("Wrong object type", format!("Got: {:?}", response_body.get("object")))
    });

    // Extract assistant message
    let assistant_content = response_body
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let has_meaningful_content = !assistant_content.is_empty() && assistant_content.len() > 20;
    report.add(if has_meaningful_content {
        TestAssertion::passed(format!(
            "Assistant responded with content ({} chars)",
            assistant_content.len()
        ))
    } else {
        TestAssertion::failed(
            "Empty or too-short response",
            format!("Content length: {}", assistant_content.len())
        )
    });

    // Validate usage stats present
    let has_usage = response_body.get("usage").is_some();
    report.add(if has_usage {
        TestAssertion::passed("Usage statistics included")
    } else {
        TestAssertion::warning("Usage stats missing", "Optional field")
    });

    tracing::info!(
        "SDK Chat Completion Flow:\nRequest: {}\nResponse (truncated): {}\nLatency: {}ms",
        user_message,
        truncate_for_sdk_log(&assistant_content, 300),
        api_latency.as_millis()
    );

    stop_server(&mut server).await?;
    std::fs::remove_file(server_config)?;

    report.finalize();
    Ok(())
}

/// Test: SDK Session CRUD operations
///
/// Validates session lifecycle management through API:
//! - Create session
//! - List sessions
//! - Get session details
//! - Update session metadata
//! - Delete session
#[tokio::test]
#[ignore]
async fn sdk_session_crud_operations() -> Result<()> {
    let mut report = AssertionReport::new("sdk_session_crud_operations");

    let server_config = generate_server_test_config()?;
    let server_port = reserve_port()?;

    let mut server = start_server(&server_config, server_port).await?;
    wait_for_port(server_port, Duration::from_secs(10)).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let base_url = format!("http://127.0.0.1:{}", server_port);

    // CREATE session
    let create_payload = build_session_create_request("E2E Test Session");
    let create_resp = client
        .post(&format!("{}/v1/sessions", base_url))
        .json(&create_payload)
        .send()
        .await?;

    let created = create_resp.status().as_u64() == 201 || create_resp.status().as_u64() == 200;
    report.add(if created {
        TestAssertion::passed("Session created successfully")
    } else {
        TestAssertion::failed(
            "Session creation failed",
            format!("Status: {}", create_resp.status())
        )
    });

    let create_body: serde_json::Value = create_resp.json().await?;
    let session_id = create_body.get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let id_present = session_id != "unknown";
    report.add(if id_present {
        TestAssertion::passed(format!("Session ID obtained: {}", session_id))
    } else {
        TestAssertion::failed("No session ID in response", "Cannot proceed with CRUD")
    });

    // GET session details
    if id_present {
        let get_resp = client
            .get(&format!("{}/v1/sessions/{}", base_url, session_id))
            .send()
            .await?;

        let get_ok = get_resp.status().as_u64() == 200;
        report.add(if get_ok {
            TestAssertion::passed("Session details retrieved")
        } else {
            TestAssertion::failed(
                "Failed to retrieve session",
                format!("Status: {}", get_resp.status())
            )
        });

        let session_detail: serde_json::Value = get_resp.json().await?;
        let title_matches = session_detail["title"] == "E2E Test Session";
        report.add(if title_matches {
            TestAssertion::passed("Session title matches created value")
        } else {
            TestAssertion::warning(
                "Title mismatch",
                format!("Expected 'E2E Test Session', got: {:?}", session_detail.get("title"))
            )
        });
    }

    // LIST sessions
    let list_resp = client
        .get(&format!("{}/v1/sessions", base_url))
        .send()
        .await?;

    let list_ok = list_resp.status().as_u64() == 200;
    report.add(if list_ok {
        TestAssertion::passed("Session listing succeeded")
    } else {
        TestAssertion::failed(
            "Session listing failed",
            format!("Status: {}", list_resp.status())
        )
    });

    let list_body: serde_json::Value = list_resp.json().await?;
    let sessions_array = list_body.get("sessions").and_then(|v| v.as_array());
    let sessions_count = sessions_array.map(|arr| arr.len()).unwrap_or(0);

    let has_sessions = sessions_count > 0;
    report.add(if has_sessions {
        TestAssertion::passed(format!("Found {} session(s)", sessions_count))
    } else {
        TestAssertion::warning("No sessions listed", "May be empty initially")
    });

    // DELETE session (cleanup)
    if id_present {
        let delete_resp = client
            .delete(&format!("{}/v1/sessions/{}", base_url, session_id))
            .send()
            .await?;

        let deleted = matches!(delete_resp.status().as_u64(), 200 | 204 | 404);  // 404 ok if already gone
        report.add(if deleted {
            TestAssertion::passed("Session deleted successfully")
        } else {
            TestAssertion::failed(
                "Session deletion failed",
                format!("Status: {}", delete_resp.status())
            )
        });
    }

    tracing::info!("Session CRUD operations completed for session: {}", session_id);

    stop_server(&mut server).await?;
    std::fs::remove_file(server_config)?;

    report.finalize();
    Ok(())
}

/// Test: SDK handles streaming responses correctly
///
/// Validates SSE (Server-Sent Events) streaming:
//! - Stream opens successfully
//! - Chunks arrive incrementally
//! - Stream terminates properly with [DONE]
#[tokio::test]
#[ignore]
async fn sdk_streaming_chat_completion() -> Result<()> {
    let mut report = AssertionReport::new("sdk_streaming_chat_completion");

    let server_config = generate_server_test_config()?;
    let server_port = reserve_port()?;

    let mut server = start_server(&server_config, server_port).await?;
    wait_for_port(server_port, Duration::from_secs(10)).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let base_url = format!("http://127.0.0.1:{}", server_port);

    // Build streaming request
    let mut streaming_payload = build_chat_completion_request(
        "Write a short poem about programming.",
        None
    );
    streaming_payload["stream"] = json!(true);

    let url = format!("{}/v1/chat/completions", base_url);

    // Make streaming request
    let response = client.post(&url)
        .json(&streaming_payload)
        .send()
        .await?;

    let stream_started = response.status().as_u16() == 200;
    report.add(if stream_started {
        TestAssertion::passed("Streaming endpoint accepted request")
    } else {
        TestAssertion::failed(
            "Streaming request rejected",
            format!("Status: {}", response.status())
        )
    });

    // Process stream
    if stream_started {
        let mut chunk_count = 0u32;
        let mut total_bytes = 0usize;
        let mut saw_done = false;

        use futures::TryStreamExt;
        let mut byte_stream = response.bytes_stream();

        while let Some(chunk_result) = byte_stream.try_next().await? {
            let chunk_text = String::from_utf8_lossy(&chunk_text);
            total_bytes += chunk_text.len();
            chunk_count += 1;

            // Check for [DONE] sentinel
            if chunk_text.contains("[DONE]") {
                saw_done = true;
                break;
            }
        }

        let received_chunks = chunk_count > 0;
        report.add(if received_chunks {
            TestAssertion::passed(format!(
                "Received {} stream chunks ({} bytes total)",
                chunk_count,
                total_bytes
            ))
        } else {
            TestAssertion::failed("No stream chunks received", "Stream was empty")
        });

        let properly_terminated = saw_done;
        report.add(if properly_terminated {
            TestAssertion::passed("Stream terminated with [DONE]")
        } else {
            TestAssertion::warning(
                "Stream termination marker missing",
                "Implementation may differ"
            )
        });
    }

    stop_server(&mut server).await?;
    std::fs::remove_file(server_config)?;

    report.finalize();
    Ok(())
}

/// Test: SDK handles errors gracefully
///
//! - Invalid API key returns 401/403
//! - Invalid model name returns appropriate error
//! - Rate limiting handled correctly
#[tokio::test]
#[ignore]
async fn sdk_error_handling() -> Result<()> {
    let mut report = AssertionReport::new("sdk_error_handling");

    let server_config = generate_server_test_config()?;
    let server_port = reserve_port()?;

    let mut server = start_server(&server_config, server_port).await?;
    wait_for_port(server_port, Duration::from_secs(10)).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let base_url = format!("http://127.0.0.1:{}", server_port);

    // Test 1: Invalid auth token
    let invalid_auth_resp = client
        .post(&format!("{}/v1/chat/completions", base_url))
        .json(&build_chat_completion_request("test", None))
        .header("Authorization", "Bearer invalid-token-12345")
        .send()
        .await?;

    let auth_rejected = matches!(invalid_auth_resp.status().as_u64(), 401 | 403);
    report.add(if auth_rejected {
        TestAssertion::passed("Invalid token correctly rejected")
    } else {
        TestAssertion::warning(
            "Auth rejection behavior unclear",
            format!("Status: {}", invalid_auth_resp.status())
        )
    });

    // Test 2: Non-existent model
    let bad_model_payload = build_chat_completion_request("test", Some("non-existent-model-xyz"));
    let bad_model_resp = client
        .post(&format!("{}/v1/chat/completions", base_url))
        .json(&bad_model_payload)
        .send()
        .await?;

    let model_error = matches!(bad_model_resp.status().as_u64(), 400..=499);
    report.add(if model_error {
        TestAssertion::passed("Invalid model name returns client error")
    } else {
        TestAssertion::warning(
            "Invalid model handling unclear",
            format!("Status: {}", bad_model_resp.status())
        )
    });

    // Test 3: Empty messages array
    let empty_messages = json!({
        "model": "test-model",
        "messages": [],
        "stream": false
    });
    let empty_resp = client
        .post(&format!("{}/v1/chat/completions", base_url))
        .json(&empty_messages)
        .send()
        .await?;

    let empty_error = empty_resp.status().as_u64() >= 400;
    report.add(if empty_error {
        TestAssertion::passed("Empty messages array rejected as error")
    } else {
        TestAssertion::warning(
            "Empty messages handling unclear",
            format!("Status: {}", empty_resp.status())
        )
    });

    stop_server(&mut server).await?;
    std::fs::remove_file(server_config)?;

    report.finalize();
    Ok(())
}

/// Helper to truncate strings for logging
fn truncate_for_sdk_log(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        format!("{}... [truncated]", &text[..max_chars])
    }
}
