/// Plan-level assessment lines shown once above the todo groups.
fn push_todo_plan_details(
    lines: &mut Vec<Line<'static>>,
    plan: &crate::todo::TodoPlan,
    goals: &[crate::todo::TodoGoal],
    base_indent: &str,
    inner_width: usize,
    compact_details: bool,
) {
    let intention = plan
        .user_intention
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(state) = plan.understands_user_intent {
        let state_color = match state {
            crate::todo::IntentUnderstanding::Uncertain => todo_failure_color(),
            crate::todo::IntentUnderstanding::Partial => todo_warning_color(),
            crate::todo::IntentUnderstanding::Clear => todo_score_color(),
        };
        let mut spans = vec![
            Span::styled("Intent ", Style::default().fg(todo_label_color())),
            Span::styled(state.as_str().to_string(), Style::default().fg(state_color)),
            Span::styled(": ", Style::default().fg(todo_label_color())),
        ];
        if let Some(intention) = intention {
            spans.push(Span::styled(
                intention.to_string(),
                Style::default().fg(todo_meta_color()),
            ));
        }
        lines.push(todo_card_line(spans, base_indent, inner_width));
    } else if let Some(intention) = intention {
        push_todo_detail(
            lines,
            "Intent",
            intention,
            base_indent,
            inner_width,
            compact_details,
        );
    }
    if let Some(state) = goals.iter().filter_map(|goal| goal.delivery_state).min() {
        let state_color = match state {
            crate::todo::DeliveryState::ChangeMade => todo_meta_color(),
            crate::todo::DeliveryState::Integrated => todo_warning_color(),
            crate::todo::DeliveryState::WorkflowValidated
            | crate::todo::DeliveryState::OutcomeDelivered => todo_score_color(),
        };
        lines.push(todo_card_line(
            vec![
                Span::styled("Delivery ", Style::default().fg(todo_label_color())),
                Span::styled(state.as_str().to_string(), Style::default().fg(state_color)),
            ],
            base_indent,
            inner_width,
        ));
    }
}
