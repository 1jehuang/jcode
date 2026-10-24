use crate::todo::ConfidenceState as CS;

fn history_todo(id: &str, confidence: Option<CS>, history: Vec<CS>) -> TodoItem {
    TodoItem {
        id: id.to_string(),
        content: format!("todo {id}"),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        confidence,
        confidence_history: history,
        ..Default::default()
    }
}

#[test]
fn confidence_history_appends_changes_and_skips_repeats() {
    let previous = vec![history_todo("1", Some(CS::Plausible), vec![CS::Plausible])];
    // Same confidence again: no new entry.
    let mut incoming = vec![history_todo("1", Some(CS::Plausible), Vec::new())];
    merge_confidence_history(&previous, &mut incoming);
    assert_eq!(incoming[0].confidence_history, vec![CS::Plausible]);
    // Raised confidence: appended.
    let mut incoming = vec![history_todo("1", Some(CS::Validated), Vec::new())];
    merge_confidence_history(&previous, &mut incoming);
    assert_eq!(
        incoming[0].confidence_history,
        vec![CS::Plausible, CS::Validated]
    );
}

#[test]
fn confidence_history_records_completion_confidence() {
    let previous = vec![history_todo("1", Some(CS::Plausible), vec![CS::Plausible])];
    let mut done = history_todo("1", Some(CS::Verified), Vec::new());
    done.status = "completed".to_string();
    done.completion_confidence = Some(CS::Verified);
    let mut incoming = vec![done];
    merge_confidence_history(&previous, &mut incoming);
    // 75 (planning) -> 100 (final bulk stamp): the spike stays visible.
    assert_eq!(
        incoming[0].confidence_history,
        vec![CS::Plausible, CS::Verified]
    );
}

#[test]
fn completion_write_contributes_only_one_final_confidence_observation() {
    let previous = vec![history_todo("1", Some(CS::Plausible), vec![CS::Plausible])];
    let mut done = history_todo("1", Some(CS::Plausible), Vec::new());
    done.status = "completed".to_string();
    done.completion_confidence = Some(CS::Verified);

    let mut incoming = vec![done];
    merge_confidence_history(&previous, &mut incoming);

    assert_eq!(
        incoming[0].confidence_history,
        vec![CS::Plausible, CS::Verified]
    );
}

#[test]
fn confidence_history_seeds_legacy_todos_before_completion() {
    let previous = vec![history_todo("1", Some(CS::Plausible), Vec::new())];
    let mut done = history_todo("1", Some(CS::Plausible), Vec::new());
    done.status = "completed".to_string();
    done.completion_confidence = Some(CS::Verified);

    let mut incoming = vec![done];
    merge_confidence_history(&previous, &mut incoming);

    assert_eq!(
        incoming[0].confidence_history,
        vec![CS::Plausible, CS::Verified]
    );
}

#[test]
fn confidence_history_ignores_model_supplied_history_for_new_todos() {
    let mut incoming = vec![history_todo(
        "9",
        Some(CS::Plausible),
        vec![CS::Speculative, CS::Verified],
    )];
    merge_confidence_history(&[], &mut incoming);
    assert_eq!(incoming[0].confidence_history, vec![CS::Plausible]);
}
