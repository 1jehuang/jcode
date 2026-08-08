// Issue #533: direct OpenAI-compatible chat streams must request the terminal
// usage chunk, and extra_body must be able to override generated options.

#[test]
fn direct_openai_compatible_chat_request_requests_usage_chunk() {
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let provider = OpenRouterProvider {
        api_base,
        supports_provider_features: false,
        supports_model_catalog: false,
        send_openrouter_headers: false,
        ..make_custom_compatible_provider()
    };

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            event.expect("stream event should parse");
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.contains(r#""stream_options":{"include_usage":true}"#),
        "direct compatible stream must request the terminal usage chunk: {request}"
    );
}

#[test]
fn direct_openai_compatible_chat_request_allows_stream_options_override() {
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let provider = OpenRouterProvider {
        api_base,
        supports_provider_features: false,
        supports_model_catalog: false,
        send_openrouter_headers: false,
        extra_body: Some(serde_json::Map::from_iter([(
            "stream_options".to_string(),
            serde_json::json!({"include_usage": false}),
        )])),
        ..make_custom_compatible_provider()
    };

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }];
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("fake chat request should start");
        while let Some(event) = stream.next().await {
            event.expect("stream event should parse");
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    assert!(
        request.contains(r#""stream_options":{"include_usage":false}"#),
        "extra_body must override the generated stream options: {request}"
    );
}
