use super::*;
use futures::StreamExt;
use serde_json::json;

const MARKER: &str = "\u{e200}cite\u{e202}turn0search12\u{e201}";

fn annotated_part(prefix: &str, title: &str) -> Value {
    json!({"type":"output_text", "text":format!("{prefix}{MARKER}"), "annotations":[{
        "type":"url_citation", "start_index":prefix.chars().count(),
        "end_index":prefix.chars().count() + MARKER.chars().count(),
        "url":"https://www.apple.com/legal/sla/docs/macOSTahoe.pdf", "title":title,
    }]})
}

fn done(parts: Vec<Value>) -> Value {
    json!({"type":"response.output_item.done", "item":{
        "id":"msg_1", "type":"message", "role":"assistant", "content":parts,
    }})
}

fn http_events(frames: Vec<Value>) -> Vec<StreamEvent> {
    // Exercise the public HTTP SSE stream, not just the rendering helper.
    let bytes = frames
        .into_iter()
        .map(|frame| Ok::<_, reqwest::Error>(Bytes::from(format!("data: {frame}\n\n"))));
    futures::executor::block_on(async {
        OpenAIResponsesStream::new(futures::stream::iter(bytes.collect::<Vec<_>>()))
            .map(|event| event.unwrap())
            .collect()
            .await
    })
}

#[test]
fn streamed_citation_is_corrected_from_completed_item_before_text_done() {
    let prefix = "许可 🦀. ";
    let raw = format!("{prefix}{MARKER}");
    let events = http_events(vec![
        json!({"type":"response.output_text.delta", "delta":prefix}),
        json!({"type":"response.output_text.delta", "delta":"\u{e200}ci"}),
        json!({"type":"response.output_text.delta", "delta":"te\u{e202}turn0search12\u{e201}"}),
        // Streaming annotation events are optional. The completed item is the
        // authoritative fallback even when this frame wasn't delivered.
        json!({"type":"response.output_text.annotation.added", "annotation":
            annotated_part(prefix, "macOS license")["annotations"][0]}),
        done(vec![annotated_part(prefix, "macOS license")]),
        json!({"type":"response.completed", "response":{"status":"completed"}}),
    ]);
    assert!(matches!(&events[..], [
        StreamEvent::TextDelta(a), StreamEvent::TextDelta(_), StreamEvent::TextDelta(_),
        StreamEvent::TextSuffixReplace { old_text, new_text },
        StreamEvent::TextDone, StreamEvent::MessageEnd { .. },
    ] if a == prefix && old_text == &raw && new_text ==
        "许可 🦀. [macOS license](<https://www.apple.com/legal/sla/docs/macOSTahoe.pdf>)"));
}

#[test]
fn fallback_text_and_per_content_part_character_offsets_are_supported() {
    let events = http_events(vec![done(vec![
        annotated_part("First. ", "A"),
        annotated_part(" 第二. ", "B"),
    ])]);
    assert!(
        matches!(&events[..], [StreamEvent::TextDelta(text), StreamEvent::TextDone]
        if text == "First. [A](<https://www.apple.com/legal/sla/docs/macOSTahoe.pdf>) 第二. [B](<https://www.apple.com/legal/sla/docs/macOSTahoe.pdf>)")
    );
}

#[test]
fn completed_snapshot_without_annotations_does_not_duplicate_or_hide_raw_text() {
    let events = http_events(vec![
        json!({"type":"response.output_text.delta", "delta":MARKER}),
        done(vec![
            json!({"type":"output_text", "text":MARKER, "annotations":[]}),
        ]),
    ]);
    assert!(
        matches!(&events[..], [StreamEvent::TextDelta(text), StreamEvent::TextDone]
        if text == MARKER)
    );
}

#[test]
fn sequential_messages_apply_late_metadata_to_only_their_own_suffix() {
    let events = http_events(vec![
        json!({"type":"response.output_text.delta", "delta":"Earlier commentary."}),
        done(vec![
            json!({"type":"output_text", "text":"Earlier commentary."}),
        ]),
        json!({"type":"response.output_item.done", "item":{
            "type":"web_search_call", "id":"ws_1", "status":"completed",
            "action":{"type":"open_page", "url":"https://www.apple.com/legal/sla/docs/macOSTahoe.pdf"},
        }}),
        json!({"type":"response.output_text.delta", "delta":format!("B. {MARKER}")}),
        done(vec![annotated_part("B. ", "B")]),
        json!({"type":"response.output_text.delta", "delta":format!("C. {MARKER}")}),
        done(vec![annotated_part("C. ", "C")]),
    ]);
    let mut text = String::new();
    let mut native = jcode_message_types::provider_native::ProviderNativeItems::default();
    for event in events {
        match event {
            StreamEvent::TextDelta(delta) => text.push_str(&delta),
            StreamEvent::TextSuffixReplace { old_text, new_text } => {
                assert!(native.replace_text_suffix(&mut text, &old_text, &new_text));
            }
            StreamEvent::ProviderNative { provider, item } => {
                native.push(text.len(), provider, item)
            }
            _ => {}
        }
    }
    let blocks = native.interleave(&text);
    assert!(matches!(&blocks[..], [
        jcode_message_types::ContentBlock::Text { text: a, .. },
        jcode_message_types::ContentBlock::ProviderNative { .. },
        jcode_message_types::ContentBlock::Text { text: bc, .. },
    ] if a == "Earlier commentary." && bc ==
        "B. [B](<https://www.apple.com/legal/sla/docs/macOSTahoe.pdf>)C. [C](<https://www.apple.com/legal/sla/docs/macOSTahoe.pdf>)"));
}
