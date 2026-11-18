use super::*;
#[path = "searchable_text_fixtures.rs"]
mod fixtures;
#[path = "searchable_text_legacy.rs"]
mod oracle;

#[test]
fn borrowed_projection_matches_legacy() {
    let cases = fixtures::cases();
    for (name, msg, _) in &cases {
        for include_tools in [false, true] {
            assert_eq!(
                searchable_message_text(msg, include_tools),
                oracle::legacy(msg, include_tools),
                "{name}/{include_tools}"
            );
        }
    }
    let blocks: Vec<_> = cases
        .iter()
        .filter(|(name, _, _)| !name.starts_with("large") && *name != "many-text")
        .flat_map(|(_, msg, _)| msg.content.iter().cloned())
        .collect();
    for left in &blocks {
        for right in &blocks {
            let msg = fixtures::message(vec![left.clone(), right.clone()]);
            for tools in [false, true] {
                assert_eq!(
                    searchable_message_text(&msg, tools),
                    oracle::legacy(&msg, tools)
                );
            }
        }
    }
}

#[test]
fn single_plain_payload_is_borrowed() {
    for (name, msg, tools) in fixtures::cases() {
        if matches!(name, "small" | "large-text" | "large-result") {
            let text = searchable_message_text(&msg, tools);
            assert!(matches!(text, Cow::Borrowed(_)), "{name}");
            let original = match &msg.content[0] {
                ContentBlock::Text { text, .. } => text,
                ContentBlock::ToolResult { content, .. } => content,
                _ => unreachable!(),
            };
            assert_eq!(text.as_ptr(), original.as_ptr());
        }
    }
}
