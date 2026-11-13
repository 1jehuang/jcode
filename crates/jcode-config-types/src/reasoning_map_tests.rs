use super::*;

fn parse(json: &str) -> ReasoningEffortMapConfig {
    serde_json::from_str(json).expect("reasoning map should parse")
}

#[test]
fn reasoning_map_parses_opencode_style_levels() {
    let map = parse(
        r#"{
        "xhigh": {"reasoningEffort": "xhigh"},
        "high": {"reasoningEffort": "high"},
        "none": {"reasoningEffort": "none"},
        "medium": {"disabled": true},
        "low": {"disabled": true}
    }"#,
    );

    assert_eq!(map.resolve("xhigh").as_deref(), Some("xhigh"));
    assert_eq!(map.resolve("high").as_deref(), Some("high"));
    assert_eq!(map.resolve("none").as_deref(), Some("none"));
    // Listed-but-disabled rungs are rejected by resolve.
    assert_eq!(map.resolve("medium"), None);
    assert_eq!(map.resolve("low"), None);
    assert!(map.is_disabled("medium"));
    assert!(map.is_disabled("low"));
    assert!(!map.is_disabled("xhigh"));
    // Unlisted rungs are neither disabled nor resolvable: callers fall
    // back to their built-in ladder behavior for them.
    assert!(!map.is_disabled("minimal"));
    assert_eq!(map.resolve("minimal"), None);
    assert!(!map.is_disabled("max"));
    assert_eq!(map.resolve("max"), None);
    assert!(!map.is_empty());
    // rungs() lists all configured rungs in ladder order.
    let rung_keys: Vec<&str> = map.rungs().map(|(key, _)| key).collect();
    assert_eq!(rung_keys, vec!["none", "low", "medium", "high", "xhigh"]);
}

#[test]
fn reasoning_map_accepts_snake_case_alias_and_trims_wire_values() {
    let map = parse(r#"{"max": {"reasoning_effort": "  turbo  "}}"#);
    assert_eq!(map.resolve("max").as_deref(), Some("turbo"));
}

#[test]
fn reasoning_map_unknown_rung_key_is_rejected() {
    let result = serde_json::from_str::<ReasoningEffortMapConfig>(
        r#"{"turbo": {"reasoningEffort": "ultra"}}"#,
    );
    assert!(result.is_err(), "unknown rung key must fail parsing");
}

#[test]
fn reasoning_map_without_levels_is_empty() {
    let map: ReasoningEffortMapConfig = ReasoningEffortMapConfig::default();
    assert!(map.is_empty());
    let map = parse("{}");
    assert!(map.is_empty());
}

#[test]
fn reasoning_map_round_trips_through_serde() {
    let map = parse(
        r#"{
        "xhigh": {"reasoningEffort": "xhigh"},
        "medium": {"disabled": true}
    }"#,
    );
    let serialized = serde_json::to_string(&map).expect("serialize");
    let reparsed: ReasoningEffortMapConfig = serde_json::from_str(&serialized).expect("reparse");
    assert_eq!(reparsed, map);
}
