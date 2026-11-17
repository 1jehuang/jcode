use super::*;
use futures::StreamExt;
use serde_json::json;

fn events(frames: Vec<Value>) -> Vec<StreamEvent> {
    let bytes = frames
        .into_iter()
        .map(|frame| Ok::<_, reqwest::Error>(Bytes::from(format!("data: {frame}\n\n"))))
        .collect::<Vec<_>>();
    futures::executor::block_on(
        OpenAIResponsesStream::new(futures::stream::iter(bytes))
            .map(Result::unwrap)
            .collect(),
    )
}

#[test]
fn resolved_model_uses_actual_metadata_and_dedupes_lifecycle_repeats() {
    let frames = vec![
        json!({"type":"response.created", "response":{"model":"gpt-actual-snapshot"}}),
        json!({"type":"response.in_progress", "response":{"model":"gpt-actual-snapshot"}}),
        json!({"type":"response.completed", "response":{"model":"gpt-actual-snapshot", "usage":{"input_tokens":2,"output_tokens":1}}}),
    ];
    // A new stream is a new attempt: the same upstream model must be reported again.
    for _attempt in 0..2 {
        let events = events(frames.clone());
        assert!(
            matches!(&events[0], StreamEvent::ResolvedModel { model } if model == "gpt-actual-snapshot")
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, StreamEvent::ResolvedModel { .. }))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event,
            StreamEvent::TokenUsage {
                input_tokens: Some(2),
                ..
            }
        )));
        assert!(matches!(
            events.last(),
            Some(StreamEvent::MessageEnd { .. })
        ));
    }
}

#[test]
fn resolved_model_ignores_missing_empty_and_nonstring_metadata() {
    let events = events(vec![
        json!({"type":"response.created", "model":"requested-alias", "response":{}}),
        json!({"type":"response.in_progress", "response":{"model":null}}),
        json!({"type":"response.in_progress", "response":{"model":"  "}}),
        json!({"type":"response.completed", "response":{"model":42}}),
    ]);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, StreamEvent::ResolvedModel { .. }))
    );
}

#[test]
fn resolved_model_preserves_changes_and_existing_created_metadata() {
    let events = events(vec![
        json!({"type":"response.created", "response":{"model":"actual-a", "service_tier":"priority"}}),
        json!({"type":"response.completed", "response":{"model":"actual-b"}}),
    ]);
    let models = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ResolvedModel { model } => Some(model.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(models, ["actual-a", "actual-b"]);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::ServiceTier { .. }))
    );
}
