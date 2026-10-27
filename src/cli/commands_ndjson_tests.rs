use super::*;

#[test]
fn ndjson_preserves_reasoning_separately_from_the_final_answer() {
    let mut state = NdjsonRunState::default();
    let mut output = Vec::new();
    for event in [
        crate::protocol::ServerEvent::ReasoningDelta {
            text: "Verify the multiplication.\n".into(),
        },
        crate::protocol::ServerEvent::ReasoningDelta {
            text: "Check once more.".into(),
        },
        crate::protocol::ServerEvent::TextDelta {
            text: "1813".into(),
        },
    ] {
        emit_ndjson_event(&mut output, &mut state, event).unwrap();
    }
    let events: Vec<serde_json::Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0],
        serde_json::json!({
            "type": "reasoning_delta", "text": "Verify the multiplication.\n"
        })
    );
    assert_eq!(events[1]["type"], "reasoning_delta");
    assert_eq!(events[2]["type"], "text_delta");
    assert_eq!(state.text, "1813");
}

#[test]
fn ndjson_preserves_provider_reported_service_tier_without_inferring_it() {
    let mut state = NdjsonRunState::default();
    assert_eq!(state.served_service_tier, None);
    let mut output = Vec::new();
    for (requested, served) in [
        (Some("ultrafast"), "default"),
        (Some("priority"), "priority"),
        (None, "default"),
    ] {
        emit_ndjson_event(
            &mut output,
            &mut state,
            crate::protocol::ServerEvent::ServiceTierReported {
                requested: requested.map(str::to_owned),
                tier: served.to_owned(),
            },
        )
        .expect("emit tier metadata");
        assert_eq!(state.requested_service_tier.as_deref(), requested);
        assert_eq!(state.served_service_tier.as_deref(), Some(served));
    }
    let events: Vec<serde_json::Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0],
        serde_json::json!({
            "type": "service_tier", "requested": "ultrafast", "tier": "default",
            "report_status": "reported_mismatch"
        })
    );
    assert!(events[2]["requested"].is_null());
}

#[test]
fn ndjson_service_tier_status_never_infers_confirmation() {
    for (requested, reported, expected) in [
        (None, None, "not_reported"),
        (Some("ultrafast"), Some("auto"), "unresolved"),
        (Some("ultrafast"), Some("default"), "reported_mismatch"),
        (Some("ultrafast"), Some("ultrafast"), "reported_match"),
        (Some("fast"), Some("priority"), "reported_match"),
        (Some("off"), Some("standard"), "reported_match"),
        (None, Some("default"), "reported_match"),
        (Some("flex"), Some("default"), "reported_mismatch"),
        (Some("future-tier"), Some("default"), "reported_mismatch"),
    ] {
        let state = NdjsonRunState {
            requested_service_tier: requested.map(str::to_string),
            served_service_tier: reported.map(str::to_string),
            ..Default::default()
        };
        assert_eq!(state.service_tier_report_status(), expected);
        assert!(!state.service_tier_report_status().contains("confirmed"));
    }
}
