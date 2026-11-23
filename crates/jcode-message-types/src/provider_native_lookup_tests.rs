//! Lookup oracle frozen from 3de375bd4, provider_native.rs:323-329.
//! Display conversion remains the independent semantic authority.
use super::*;
use serde_json::json;

fn original(items: &ProviderNativeItems, id: &str) -> Value {
    items
        .items
        .iter()
        .filter_map(|(_, provider, item)| provider_native_display(provider, item))
        .find(|display| display.id == id && display.output.is_none())
        .and_then(|display| display.input)
        .unwrap_or(Value::Null)
}

#[test]
fn frozen_lookup_oracle_malformed_duplicates_and_order() {
    let mut candidates = vec![
        Value::Null,
        json!(1),
        json!([]),
        json!({}),
        json!({"type":3}),
    ];
    for kind in [
        "server_tool_use",
        "web_search_tool_result",
        "web_search_call",
        "unknown",
    ] {
        for id in [Value::Null, json!(false), json!(""), json!("same")] {
            for status in [
                Value::Null,
                json!(3),
                json!("completed"),
                json!("failed"),
                json!("unknown_future_status"),
                json!("in_progress"),
                json!("searching"),
            ] {
                for action in [
                    Value::Null,
                    json!({"query":"", "queries":["ignored"]}),
                    json!({"query":2,"queries":["a",3,"b"],"url":"ignored"}),
                    json!({"queries":[],"url":"ignored"}),
                    json!({"url":"page"}),
                ] {
                    candidates.push(json!({"type":kind,"id":id,"tool_use_id":id,"status":status,"action":action,"input":{"query":"first"}}));
                }
            }
        }
    }
    candidates.extend([
        json!({"type":"server_tool_use","id":"same"}),
        json!({"type":"server_tool_use","id":"same","input":false}),
        json!({"type":"web_search_call","status":"searching"}),
    ]);
    for provider in ["anthropic", "openai", "unknown", ""] {
        for item in &candidates {
            // Direct storage deliberately exercises live OpenAI snapshots that push
            // filters today, guarding lookup semantics independently of ingestion.
            for reversed in [false, true] {
                let first = (0, provider.to_owned(), item.clone());
                let second = (
                    0,
                    "anthropic".into(),
                    json!({"type":"server_tool_use","id":"same","input":"later"}),
                );
                let items = ProviderNativeItems {
                    items: if reversed {
                        vec![second, first]
                    } else {
                        vec![first, second]
                    },
                };
                for id in ["same", "", "missing"] {
                    assert_eq!(
                        items.call_input(id),
                        original(&items, id),
                        "{provider} {item} reversed={reversed} id={id}"
                    );
                }
            }
        }
    }
}
