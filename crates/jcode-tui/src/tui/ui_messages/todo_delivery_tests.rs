#[test]
fn render_todos_message_shows_delivery_state_under_intent() {
    let msg = DisplayMessage::todos(
        serde_json::json!({
            "todos": [{
                "id": "1",
                "content": "Ship delivery state",
                "status": "in_progress",
                "priority": "high",
                "group": "ui"
            }],
            "plan": {
                "user_intention": "Show delivery state under intent",
                "understands_user_intent": "clear",
            },
            "goals": [
                {"group": "ui", "delivery_state": "outcome_delivered"},
                {"group": "tests", "delivery_state": "integrated"}
            ],
        })
        .to_string(),
    );
    let lines = render_todos_message(&msg, 80, crate::config::DiffDisplayMode::Off);
    let text_lines: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect();
    let intent_idx = text_lines
        .iter()
        .position(|line| line.contains("Intent clear: Show delivery state under intent"))
        .expect("intent line should render");
    assert!(
        text_lines
            .get(intent_idx + 1)
            .is_some_and(|line| line.contains("Delivery integrated")),
        "delivery state should render immediately under intent (using lowest goal state): {text_lines:?}"
    );
    for narrow in [16u16, 20, 28] {
        let narrow_lines = render_todos_message(&msg, narrow, crate::config::DiffDisplayMode::Off);
        for line in &narrow_lines {
            assert!(
                line.width() <= narrow as usize,
                "row exceeded width {narrow}: {:?}",
                line
            );
        }
    }
}
