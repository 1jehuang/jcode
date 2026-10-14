use super::*;

fn intent_observation(state: Option<IntentUnderstanding>) -> GateObservation {
    GateObservation {
        kind: GateObservationKind::IntentUnderstanding,
        group: None,
        state: state.map(|state| state.as_str().to_string()),
    }
}

fn loop_observation(group: Option<&str>, state: Option<FeedbackLoopState>) -> GateObservation {
    GateObservation {
        kind: GateObservationKind::ClosedFeedbackLoop,
        group: group.map(str::to_string),
        state: state.map(|state| state.as_str().to_string()),
    }
}

#[test]
fn substitute_and_blocked_checks_do_not_pass_involved_acceptance_gate() {
    let goal = |relevance| TodoGoal {
        difficulty: Some(Difficulty::Involved),
        feedback_loop_relevance: Some(relevance),
        ..Default::default()
    };

    assert!(!feedback_loop_relevance_passes(&goal(
        FeedbackLoopRelevance::Synthetic
    )));
    assert!(!feedback_loop_relevance_passes(&goal(
        FeedbackLoopRelevance::Representative
    )));
    assert!(!feedback_loop_relevance_passes(&goal(
        FeedbackLoopRelevance::AcceptanceBlocked
    )));
    assert!(feedback_loop_relevance_passes(&goal(
        FeedbackLoopRelevance::AcceptanceAligned
    )));
}

/// A score that climbed only after work was underway still gets raised, and
/// is described as the coverage gap it is. Suppressing it would let an agent
/// clear the gate by writing a good assessment at the end, after the work it
/// was supposed to govern was already done and booked.
#[test]
fn digest_still_raises_a_point_whose_score_climbed_late() {
    let observations = vec![
        intent_observation(Some(IntentUnderstanding::Uncertain)),
        intent_observation(Some(IntentUnderstanding::Partial)),
        intent_observation(Some(IntentUnderstanding::Partial)),
    ];
    let climbed = TodoPlan {
        understands_user_intent: Some(IntentUnderstanding::Clear),
        understands_user_intent_history: vec![
            IntentUnderstanding::Uncertain,
            IntentUnderstanding::Partial,
            IntentUnderstanding::Clear,
        ],
        ..Default::default()
    };
    let digest = build_gate_digest(&observations, &climbed, &[])
        .expect("a late climb must still be raised, not silently dropped");
    assert_eq!(digest.matches("\n- ").count(), 1);
    // Worded as "you started without understanding", not "you never
    // understood": the latter would contradict the passing final state and
    // invite the model to argue with the reminder instead of acting on it.
    assert!(digest.contains("started this work without understanding"));
    assert!(digest.contains("(flagged 3 times this turn)"));
    assert!(!digest.contains("never became solid"));
}

/// The late-climb wording is per point, so one turn can carry both a goal
/// that closed its loop late and a goal that never closed one at all.
#[test]
fn digest_words_late_climbs_and_never_closed_goals_differently() {
    let observations = vec![
        loop_observation(Some("closed late"), Some(FeedbackLoopState::Usable)),
        loop_observation(Some("never closed"), Some(FeedbackLoopState::Usable)),
    ];
    let goals = vec![
        TodoGoal {
            group: Some("closed late".to_string()),
            closed_feedback_loop: Some(FeedbackLoopState::Closed),
            ..Default::default()
        },
        TodoGoal {
            group: Some("never closed".to_string()),
            closed_feedback_loop: Some(FeedbackLoopState::Usable),
            ..Default::default()
        },
    ];
    let digest = build_gate_digest(&observations, &TodoPlan::default(), &goals)
        .expect("both goals should be surfaced");
    assert_eq!(digest.matches("\n- ").count(), 2);
    assert!(digest.contains(
        "the goal for \"closed late\" was worked on before its feedback loop was closed"
    ));
    assert!(digest.contains("the goal for \"never closed\" never closed its feedback loop"));
}

#[test]
fn digest_directs_weak_relevance_and_coverage_to_real_failure_surfaces() {
    let observations = vec![
        GateObservation {
            kind: GateObservationKind::FeedbackLoopRelevance,
            group: Some("release".to_string()),
            state: Some("indirect".to_string()),
        },
        GateObservation {
            kind: GateObservationKind::FeedbackLoopCoverage,
            group: Some("release".to_string()),
            state: Some("narrow".to_string()),
        },
    ];
    let digest = build_gate_digest(&observations, &TodoPlan::default(), &[])
        .expect("weak feedback-loop dimensions should be surfaced");

    for guidance in [
        "public interfaces",
        "integration boundaries",
        "custom harness",
        "externally blocked",
        "main workflows",
        "edge cases",
        "packaging",
        "likely failure modes",
    ] {
        assert!(
            digest.contains(guidance),
            "digest omitted {guidance}: {digest}"
        );
    }
    assert!(!digest.to_ascii_lowercase().contains("threshold"));
    assert!(!digest.contains("representative+main_paths"));
}

#[test]
fn digest_reports_a_point_that_never_resolved() {
    let observations = vec![intent_observation(Some(IntentUnderstanding::Partial))];
    let unresolved = TodoPlan {
        understands_user_intent: Some(IntentUnderstanding::Partial),
        understands_user_intent_history: vec![IntentUnderstanding::Partial],
        ..Default::default()
    };
    let digest = build_gate_digest(&observations, &unresolved, &[])
        .expect("an unresolved point should be surfaced");
    assert!(digest.starts_with(TODO_GATE_DIGEST_PREFIX));
    assert!(digest.contains("what the user actually wants"));
    // Framed as verification, since by turn end the work is already done.
    assert!(digest.contains("double-check"));
    // Private calibration stays private.
    assert!(!digest.to_ascii_lowercase().contains("threshold"));
}

/// A long iterative turn flags the same point on every write. The digest
/// must collapse those into one line, not a wall of duplicates.
#[test]
fn digest_collapses_repeats_and_counts_them() {
    let observations: Vec<GateObservation> = (0..9)
        .map(|_| loop_observation(Some("utf16 transcode"), Some(FeedbackLoopState::Strong)))
        .collect();
    let goals = vec![TodoGoal {
        group: Some("utf16 transcode".to_string()),
        closed_feedback_loop: Some(FeedbackLoopState::Strong),
        ..Default::default()
    }];
    let digest = build_gate_digest(&observations, &TodoPlan::default(), &goals)
        .expect("an unresolved goal should be surfaced");
    assert_eq!(
        digest.matches("\n- ").count(),
        1,
        "nine identical flags should collapse to one line: {digest}"
    );
    assert!(digest.contains("flagged 9 times"));
    assert!(digest.contains("utf16 transcode"));
}

/// Each goal gets its own line, named by group, so a multi-goal turn does
/// not blur two different problems into one instruction.
#[test]
fn digest_separates_goals_by_group() {
    let observations = vec![
        loop_observation(Some("transcode"), Some(FeedbackLoopState::Usable)),
        loop_observation(Some("render"), Some(FeedbackLoopState::Usable)),
        loop_observation(Some("render"), Some(FeedbackLoopState::Usable)),
    ];
    let goals = vec![
        TodoGoal {
            group: Some("transcode".to_string()),
            closed_feedback_loop: Some(FeedbackLoopState::Usable),
            ..Default::default()
        },
        TodoGoal {
            group: Some("render".to_string()),
            closed_feedback_loop: Some(FeedbackLoopState::Usable),
            ..Default::default()
        },
    ];
    let digest = build_gate_digest(&observations, &TodoPlan::default(), &goals)
        .expect("both goals should be surfaced");
    assert_eq!(digest.matches("\n- ").count(), 2);
    assert!(digest.contains("transcode"));
    assert!(digest.contains("render"));
    // The repeated goal is collapsed and counted, the single one is not.
    assert!(digest.contains("(flagged 2 times this turn)"));
}

/// An ungrouped goal has no label to name, so the line must still read
/// cleanly rather than rendering an empty quoted string.
#[test]
fn digest_handles_the_ungrouped_goal() {
    let digest = build_gate_digest(
        &[loop_observation(None, Some(FeedbackLoopState::Absent))],
        &TodoPlan::default(),
        &[TodoGoal {
            closed_feedback_loop: Some(FeedbackLoopState::Absent),
            ..Default::default()
        }],
    )
    .expect("ungrouped goal should be surfaced");
    assert!(!digest.contains("\"\""));
    assert!(digest.contains("the goal never closed its feedback loop"));
}

#[test]
fn digest_is_empty_without_observations() {
    assert_eq!(build_gate_digest(&[], &TodoPlan::default(), &[]), None);
}

/// The digest is persisted as a user-role message so the model treats it as
/// a continuation, so reload must not re-render it as a user prompt.
#[test]
fn digest_is_recognized_as_a_synthetic_message() {
    let digest = build_gate_digest(
        &[intent_observation(Some(IntentUnderstanding::Partial))],
        &TodoPlan::default(),
        &[],
    )
    .expect("digest");
    assert!(is_auto_poke_message(&digest));
}

#[test]
fn built_auto_poke_messages_are_detected() {
    assert!(is_auto_poke_message(&build_auto_poke_message(1)));
    assert!(is_auto_poke_message(&build_auto_poke_message(3)));
    assert!(is_auto_poke_message(
        TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE
    ));
    assert!(is_auto_poke_message(
        LEGACY_TODO_ALIGNMENT_CONTINUATION_MESSAGE
    ));
    assert!(is_auto_poke_message(
        TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE
    ));
    assert!(is_auto_poke_message(TODO_OWNERSHIP_CONTINUATION_MESSAGE));
    assert!(is_auto_poke_message(TODO_COMPLETION_CONTINUATION_MESSAGE));
    assert!(is_auto_poke_message(
        TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE
    ));
    assert!(is_auto_poke_message(
        TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE
    ));
    assert_eq!(
        auto_poke_display_summary(TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE),
        Some("✅ Preparing the final response...")
    );
    assert!(is_auto_poke_message(LEGACY_TODO_CONFIDENCE_SUMMARY_PREFIX));
    assert!(is_auto_poke_message(LABELED_TODO_GATE_DIGEST_PREFIX));
}

#[test]
fn quality_continuations_are_actionable_without_private_calibration() {
    for (message, category) in [
        (
            TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE,
            "feedback loop isn't good enough",
        ),
        (
            TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE,
            "understand the user's intent better",
        ),
        (TODO_OWNERSHIP_CONTINUATION_MESSAGE, "continue the work"),
        (TODO_COMPLETION_CONTINUATION_MESSAGE, "more validation"),
        (
            TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE,
            "confidence jump",
        ),
    ] {
        let lower = message.to_ascii_lowercase();
        assert!(lower.contains(category));
        assert!(!message.chars().any(|ch| ch.is_ascii_digit()));
        for disclosure in ["threshold", "percent", "quality gate"] {
            assert!(
                !lower.contains(disclosure),
                "category-only continuation disclosed {disclosure}: {message}"
            );
        }
        if category != "alignment score" {
            assert!(
                !lower.contains("score"),
                "category-only continuation disclosed score: {message}"
            );
        }
    }

    assert!(TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE.contains("Think about"));
    assert!(TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE.contains("Try to avoid asking"));
    for message in [
        TODO_OWNERSHIP_CONTINUATION_MESSAGE,
        TODO_COMPLETION_CONTINUATION_MESSAGE,
    ] {
        let lower = message.to_ascii_lowercase();
        for evaluator_term in ["gate", "flagged", "failed", "threshold", "confidence"] {
            assert!(
                !lower.contains(evaluator_term),
                "disclosed {evaluator_term}: {message}"
            );
        }
    }
    let spike = TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE.to_ascii_lowercase();
    for evaluator_term in ["gate", "flagged", "failed", "threshold", "score"] {
        assert!(
            !spike.contains(evaluator_term),
            "disclosed {evaluator_term}"
        );
    }
}

#[test]
fn static_quality_gate_messages_stay_within_token_budget() {
    for (name, message) in [
        ("long session review", TODO_LONG_SESSION_REVIEW_MESSAGE),
        (
            "intent understanding",
            TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE,
        ),
        (
            "closed feedback loop",
            TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE,
        ),
        ("ownership", TODO_OWNERSHIP_CONTINUATION_MESSAGE),
        ("completion", TODO_COMPLETION_CONTINUATION_MESSAGE),
        (
            "confidence jump",
            TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE,
        ),
        ("final response", TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE),
        ("turn digest", TODO_GATE_DIGEST_PREFIX),
    ] {
        assert!(
            message.starts_with("[auto] "),
            "{name} quality gate does not use the compact automation prefix: {message}"
        );
        let tokens = jcode_core::util::estimate_tokens(message);
        assert!(
            tokens <= TODO_QUALITY_GATE_MAX_APPROX_TOKENS,
            "{name} quality-gate message is about {tokens} tokens; budget is {TODO_QUALITY_GATE_MAX_APPROX_TOKENS}: {message}"
        );
    }
}

#[test]
fn working_quality_gates_remind_the_model_to_update_todos() {
    for (name, message) in [
        ("long session review", TODO_LONG_SESSION_REVIEW_MESSAGE),
        (
            "intent understanding",
            TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE,
        ),
        (
            "closed feedback loop",
            TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE,
        ),
        ("ownership", TODO_OWNERSHIP_CONTINUATION_MESSAGE),
        ("completion", TODO_COMPLETION_CONTINUATION_MESSAGE),
        (
            "confidence jump",
            TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE,
        ),
    ] {
        assert!(
            message.to_ascii_lowercase().contains("todo"),
            "{name} quality gate does not remind the model to update the todo: {message}"
        );
    }
}

/// The model must be told which items it should recheck, otherwise the
/// nudge is a guess-the-weak-spot puzzle. Scores stay private.
#[test]
fn completion_continuation_names_the_failing_todos() {
    let mut missing = todo("write the parser", "completed", None);
    missing.completion_confidence = None;
    let mut weak = todo("wire up the CLI flag", "completed", None);
    weak.completion_confidence = Some(ConfidenceState::Plausible);
    let mut strong = todo("rename the module", "completed", None);
    strong.completion_confidence = Some(ConfidenceState::Validated);
    let open = todo("ship it", "in_progress", None);

    let message =
        build_todo_completion_continuation_message(&[missing, weak, strong.clone(), open]);
    assert!(message.starts_with(TODO_COMPLETION_CONTINUATION_MESSAGE));
    assert!(message.contains("\"write the parser\""));
    assert!(message.contains("\"wire up the CLI flag\""));
    // Passing and unfinished todos are not what the gate is asking about.
    assert!(!message.contains("\"rename the module\""));
    assert!(!message.contains("\"ship it\""));
    for disclosure in ["70", "99", "threshold"] {
        assert!(!message.contains(disclosure), "disclosed {disclosure}");
    }

    // Only the weighted average failed: no individual item is nameable, so
    // the fallback still tells the model what to do.
    let average_only = build_todo_completion_continuation_message(&[strong]);
    assert!(average_only.starts_with(TODO_COMPLETION_CONTINUATION_MESSAGE));
    assert!(average_only.contains("Validate further:"));
    assert!(average_only.contains("rename the module"));
}

#[test]
fn spike_continuation_names_the_spiked_todos() {
    let mut spiked = todo("port the tests", "completed", None);
    spiked.completion_confidence = Some(ConfidenceState::Verified);
    spiked.confidence_history = vec![ConfidenceState::Speculative, ConfidenceState::Verified];
    let mut steady = todo("update docs", "completed", None);
    steady.completion_confidence = Some(ConfidenceState::Validated);
    steady.confidence_history = vec![ConfidenceState::Plausible, ConfidenceState::Validated];

    let message = build_todo_confidence_spike_continuation_message(&[spiked, steady]);
    assert!(message.starts_with(TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE));
    assert!(message.contains("\"port the tests\""));
    assert!(!message.contains("\"update docs\""));
}

/// A 40-item plan must not turn one gate nudge into a wall of text.
#[test]
fn completion_continuation_caps_named_todos() {
    let todos: Vec<TodoItem> = (0..40)
        .map(|index| {
            let mut item = todo(&format!("task {index}"), "completed", None);
            item.completion_confidence = None;
            item
        })
        .collect();
    let message = build_todo_completion_continuation_message(&todos);
    assert!(message.contains("\"task 0\""));
    assert!(!message.contains("\"task 30\""));
    assert!(message.contains(&format!("(and {} more)", 40 - GATE_NAMED_TODO_LIMIT)));
}

#[test]
fn detailed_gate_continuations_are_not_mistaken_for_user_messages() {
    let mut item = todo("do the thing", "completed", None);
    item.completion_confidence = Some(ConfidenceState::Speculative);
    item.confidence_history = vec![ConfidenceState::Speculative];
    let todos = [item];
    assert!(is_auto_poke_message(
        &build_todo_completion_continuation_message(&todos)
    ));
    assert!(is_auto_poke_message(
        &build_todo_confidence_spike_continuation_message(&todos)
    ));
}

/// The transcript must not replay model-facing gate instructions at the
/// user; they get a short "we are checking" line instead.
#[test]
fn gate_continuations_have_short_user_facing_summaries() {
    let mut item = todo("do the thing", "completed", None);
    item.completion_confidence = Some(ConfidenceState::Speculative);
    item.confidence_history = vec![ConfidenceState::Speculative];
    let todos = [item];

    for message in [
        build_todo_completion_continuation_message(&todos),
        build_todo_confidence_spike_continuation_message(&todos),
        TODO_OWNERSHIP_CONTINUATION_MESSAGE.to_string(),
        TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE.to_string(),
        TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE.to_string(),
    ] {
        let summary = auto_poke_display_summary(&message).expect("summary");
        assert!(summary.chars().count() < 70, "too long: {summary}");
        assert!(!summary.contains("do the thing"));
        assert!(!summary.contains("completion_confidence"));
    }

    // The incomplete-todos poke is already short and its count is useful.
    assert!(auto_poke_display_summary(&build_auto_poke_message(3)).is_none());
    // Real user prompts are never rewritten.
    assert!(auto_poke_display_summary("fix the login bug").is_none());
}

#[test]
fn confidence_spike_classifier_distinguishes_bulk_stamp_from_stepped_rise() {
    let mut bulk = todo("bulk", "completed", None);
    bulk.confidence = Some(ConfidenceState::Speculative);
    bulk.completion_confidence = Some(ConfidenceState::Verified);
    bulk.confidence_history = vec![ConfidenceState::Speculative, ConfidenceState::Verified];

    let mut stepped = todo("stepped", "completed", None);
    stepped.confidence = Some(ConfidenceState::Verified);
    stepped.completion_confidence = Some(ConfidenceState::Verified);
    stepped.confidence_history = vec![
        ConfidenceState::Plausible,
        ConfidenceState::Validated,
        ConfidenceState::Verified,
    ];

    let todos = [bulk, stepped];
    let spiked = spike_completed_todos(&todos);
    assert_eq!(spiked.len(), 1);
    assert_eq!(spiked[0].content, "bulk");
}

#[test]
fn confidence_spike_classifier_requires_extreme_recorded_jump() {
    let mut boundary = todo("boundary", "completed", None);
    boundary.confidence = Some(ConfidenceState::Speculative);
    boundary.completion_confidence = Some(ConfidenceState::Verified);
    boundary.confidence_history = vec![ConfidenceState::Speculative, ConfidenceState::Verified];

    let mut legacy = todo("legacy", "completed", None);
    legacy.confidence = Some(ConfidenceState::Speculative);
    legacy.completion_confidence = Some(ConfidenceState::Verified);

    let todos = [boundary, legacy];
    let spiked = spike_completed_todos(&todos);
    assert_eq!(
        spiked
            .iter()
            .map(|todo| todo.content.as_str())
            .collect::<Vec<_>>(),
        vec!["boundary"]
    );
}

#[test]
fn confidence_spikes_ignore_normal_validation_gains_and_stale_history() {
    use ConfidenceState::*;
    for (history, completion) in [
        (vec![Plausible, Verified], Some(Verified)),
        (vec![Speculative, Validated], Some(Validated)),
        (vec![Speculative, Validated, Verified], Some(Verified)),
        (vec![Verified], Some(Verified)),
        (vec![], Some(Verified)),
        (vec![Speculative, Verified], Some(Validated)),
        (vec![Speculative, Verified], None),
        (vec![Verified, Speculative], Some(Speculative)),
    ] {
        let mut item = todo("validated normally", "completed", None);
        item.confidence = Some(Speculative);
        item.completion_confidence = completion;
        item.confidence_history = history;
        assert!(spike_completed_todos(&[item]).is_empty());
    }
    for status in ["pending", "in_progress", "cancelled"] {
        let mut item = todo("not completed", status, None);
        item.completion_confidence = Some(Verified);
        item.confidence_history = vec![Speculative, Verified];
        assert!(spike_completed_todos(&[item]).is_empty());
    }
}

#[test]
fn real_user_prompts_are_not_detected_as_pokes() {
    assert!(!is_auto_poke_message("fix the login bug"));
    assert!(!is_auto_poke_message(
        "You have 2 incomplete todos. Continue working, or update the todo tool.\n\nalso please fix the tests"
    ));
    assert!(!is_auto_poke_message(""));
}

#[test]
fn final_handoff_preserves_limitations_and_legacy_transcript_classification() {
    assert!(TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE.contains("limitations or blockers"));
    assert!(!TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE.contains("checks passed"));
    for message in [
        TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE,
        PRE_NARROWED_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE,
        PRE_COMPACT_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE,
        PRE_BUDGET_TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE,
    ] {
        assert!(is_auto_poke_message(message));
        assert_eq!(
            auto_poke_display_summary(message),
            Some("✅ Preparing the final response...")
        );
    }
}

fn todo(content: &str, status: &str, group: Option<&str>) -> TodoItem {
    TodoItem {
        content: content.to_string(),
        status: status.to_string(),
        priority: "high".to_string(),
        id: content.to_ascii_lowercase().replace(' ', "-"),
        group: group.map(str::to_string),
        confidence: None,
        completion_confidence: None,
        confidence_history: Vec::new(),
        blocked_by: Vec::new(),
        assigned_to: None,
    }
}

fn delivery_goal(group: Option<&str>, delivery: Option<DeliveryState>) -> TodoGoal {
    TodoGoal {
        group: group.map(str::to_string),
        delivery_state: delivery,
        autonomy: Some(Autonomy::NecessaryFollowthrough),
        iteration_maturity: Some(IterationMaturity::OutcomeReached),
        feedback_loop_relevance: Some(FeedbackLoopRelevance::Representative),
        feedback_loop_coverage: Some(FeedbackLoopCoverage::MainPaths),
        feedback_loop_traceability: Some(FeedbackLoopTraceability::Complete),
        ..Default::default()
    }
}

#[test]
fn newly_completed_group_requires_sufficient_delivery() {
    let previous = vec![todo("work", "in_progress", Some("ship"))];
    let completed = vec![todo("work", "completed", Some("ship"))];

    for delivery in [
        None,
        Some(DeliveryState::ChangeMade),
        Some(DeliveryState::Integrated),
    ] {
        assert!(!newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[delivery_goal(Some("ship"), delivery)],
        ));
    }
    assert!(newly_completed_groups_have_sufficient_delivery(
        &previous,
        &completed,
        &[delivery_goal(
            Some("ship"),
            Some(DeliveryState::WorkflowValidated)
        )],
    ));
}

/// Difficulty never raises the delivery-state bar by itself. Operational
/// outcome delivery comes from the request, not implementation difficulty.
#[test]
fn difficulty_is_descriptive_and_never_raises_the_delivery_bar() {
    let previous = vec![todo("work", "in_progress", Some("ship"))];
    let completed = vec![todo("work", "completed", Some("ship"))];

    let mut hard = delivery_goal(Some("ship"), Some(DeliveryState::WorkflowValidated));
    hard.difficulty = Some(Difficulty::Hard);
    hard.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceAligned);
    hard.feedback_loop_coverage = Some(FeedbackLoopCoverage::EdgeAndIntegrationPaths);
    assert!(newly_completed_groups_have_sufficient_delivery(
        &previous,
        &completed,
        &[hard.clone()],
    ));
    hard.delivery_state = Some(DeliveryState::OutcomeDelivered);
    assert!(newly_completed_groups_have_sufficient_delivery(
        &previous,
        &completed,
        &[hard],
    ));

    // A trivial goal, and one with no difficulty at all, pass at
    // workflow_validated: absent difficulty is never punished.
    for difficulty in [None, Some(Difficulty::Trivial), Some(Difficulty::Routine)] {
        let mut goal = delivery_goal(Some("ship"), Some(DeliveryState::WorkflowValidated));
        goal.difficulty = difficulty;
        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[goal],
        ));
    }

    assert_eq!(
        required_delivery_state(Some(Difficulty::Involved)),
        DeliveryState::WorkflowValidated
    );
    assert_eq!(
        required_delivery_state(None),
        DeliveryState::WorkflowValidated
    );
}

#[test]
fn feedback_loop_completion_bar_scales_at_involved_difficulty() {
    let mut ordinary = delivery_goal(None, Some(DeliveryState::WorkflowValidated));
    assert!(delivery_state_passes(&ordinary));
    ordinary.feedback_loop_relevance = Some(FeedbackLoopRelevance::Indirect);
    assert!(!delivery_state_passes(&ordinary));
    ordinary.feedback_loop_relevance = Some(FeedbackLoopRelevance::Representative);
    ordinary.feedback_loop_coverage = Some(FeedbackLoopCoverage::Narrow);
    assert!(!delivery_state_passes(&ordinary));

    let mut involved = delivery_goal(None, Some(DeliveryState::WorkflowValidated));
    involved.difficulty = Some(Difficulty::Involved);
    assert!(!delivery_state_passes(&involved));
    involved.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceAligned);
    involved.feedback_loop_coverage = Some(FeedbackLoopCoverage::EdgeAndIntegrationPaths);
    assert!(delivery_state_passes(&involved));

    involved.feedback_loop_relevance = None;
    assert!(!delivery_state_passes(&involved));
    involved.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceAligned);
    involved.feedback_loop_coverage = None;
    assert!(!delivery_state_passes(&involved));
}

#[test]
fn feedback_loop_traceability_scales_at_involved_difficulty() {
    let mut goal = delivery_goal(None, Some(DeliveryState::WorkflowValidated));
    goal.feedback_loop_traceability = Some(FeedbackLoopTraceability::Partial);
    assert!(delivery_state_passes(&goal));

    goal.difficulty = Some(Difficulty::Involved);
    goal.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceAligned);
    goal.feedback_loop_coverage = Some(FeedbackLoopCoverage::EdgeAndIntegrationPaths);
    assert!(!delivery_state_passes(&goal));

    goal.feedback_loop_traceability = Some(FeedbackLoopTraceability::Complete);
    assert!(delivery_state_passes(&goal));
    goal.feedback_loop_traceability = None;
    assert!(!delivery_state_passes(&goal));
}

#[test]
fn research_completion_requires_stopping_evidence() {
    let previous = vec![todo("work", "in_progress", Some("ship"))];
    let completed = vec![todo("work", "completed", Some("ship"))];
    let mut goal = delivery_goal(Some("ship"), Some(DeliveryState::WorkflowValidated));
    goal.difficulty = Some(Difficulty::Research);
    goal.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceAligned);
    goal.feedback_loop_coverage = Some(FeedbackLoopCoverage::EdgeAndIntegrationPaths);
    goal.iteration_maturity = Some(IterationMaturity::PlateauConfirmed);

    assert!(!newly_completed_groups_have_sufficient_delivery(
        &previous,
        &completed,
        &[goal.clone()],
    ));
    goal.stopping_evidence =
        Some("Three materially different approaches plateaued at the same score".to_string());
    assert!(newly_completed_groups_have_sufficient_delivery(
        &previous,
        &completed,
        &[goal],
    ));
}

#[test]
fn autonomy_requires_necessary_followthrough_at_completion() {
    let previous = vec![todo("work", "in_progress", Some("ship"))];
    let completed = vec![todo("work", "completed", Some("ship"))];
    for autonomy in [None, Some(Autonomy::RequestedOnly)] {
        let mut goal = delivery_goal(Some("ship"), Some(DeliveryState::WorkflowValidated));
        goal.autonomy = autonomy;
        assert!(!newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[goal],
        ));
    }
    for autonomy in [
        Autonomy::NecessaryFollowthrough,
        Autonomy::Proactive,
        Autonomy::Stewardship,
    ] {
        let mut goal = delivery_goal(Some("ship"), Some(DeliveryState::WorkflowValidated));
        goal.autonomy = Some(autonomy);
        assert!(newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[goal],
        ));
    }
}

#[test]
fn iteration_maturity_requires_terminal_basis_and_supporting_evidence() {
    let previous = vec![todo("work", "in_progress", Some("search"))];
    let completed = vec![todo("work", "completed", Some("search"))];
    for maturity in [
        None,
        Some(IterationMaturity::NotStarted),
        Some(IterationMaturity::Exploring),
        Some(IterationMaturity::Improving),
        Some(IterationMaturity::PlateauUnproven),
    ] {
        let mut goal = delivery_goal(Some("search"), Some(DeliveryState::WorkflowValidated));
        goal.iteration_maturity = maturity;
        assert!(!newly_completed_groups_have_sufficient_delivery(
            &previous,
            &completed,
            &[goal],
        ));
    }

    let mut plateau = delivery_goal(Some("search"), Some(DeliveryState::WorkflowValidated));
    plateau.iteration_maturity = Some(IterationMaturity::PlateauConfirmed);
    assert!(!newly_completed_groups_have_sufficient_delivery(
        &previous,
        &completed,
        &[plateau.clone()],
    ));
    plateau.stopping_evidence =
        Some("Two distinct post-best approaches regressed under the same benchmark".to_string());
    assert!(newly_completed_groups_have_sufficient_delivery(
        &previous,
        &completed,
        &[plateau],
    ));

    let outcome = delivery_goal(Some("search"), Some(DeliveryState::WorkflowValidated));
    assert!(newly_completed_groups_have_sufficient_delivery(
        &previous,
        &completed,
        &[outcome],
    ));
}

#[test]
fn delivery_is_not_required_before_group_completion() {
    let previous = vec![todo("work", "pending", Some("ship"))];
    let in_progress = vec![todo("work", "in_progress", Some("ship"))];

    assert!(newly_completed_groups_have_sufficient_delivery(
        &previous,
        &in_progress,
        &[],
    ));
}

#[test]
fn delivery_gate_normalizes_groups_and_supports_ungrouped_work() {
    let previous = vec![todo("work", "in_progress", Some(" ship "))];
    let completed = vec![todo("work", "completed", Some("ship"))];
    assert!(newly_completed_groups_have_sufficient_delivery(
        &previous,
        &completed,
        &[delivery_goal(
            Some(" ship"),
            Some(DeliveryState::OutcomeDelivered)
        )],
    ));

    let previous = vec![todo("work", "in_progress", None)];
    let completed = vec![todo("work", "completed", None)];
    assert!(newly_completed_groups_have_sufficient_delivery(
        &previous,
        &completed,
        &[delivery_goal(None, Some(DeliveryState::OutcomeDelivered))],
    ));
}

/// The turn-finish gate must tell the model how to clear it without implying
/// that the todo write which triggered the check was discarded.
#[test]
fn ownership_message_names_the_field_that_must_be_raised() {
    assert!(TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("Continue the work below"));
    for private_calibration in [
        "necessary_followthrough",
        "outcome_reached",
        "plateau_confirmed",
        "budget_exhausted",
    ] {
        assert!(!TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains(private_calibration));
    }
    assert!(
        TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("Keep the todo up to date"),
        "the ownership nudge must say how to update the assessment"
    );
    assert!(
        !TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("rejected")
            && !TODO_OWNERSHIP_CONTINUATION_MESSAGE.contains("unchanged"),
        "the turn-finish nudge must not claim the already-saved write was discarded"
    );
    assert!(TODO_COMPLETION_CONTINUATION_MESSAGE.contains("more validation"));
}

#[test]
fn ownership_continuation_identifies_each_failing_field_and_goal() {
    let todos = vec![todo("work", "completed", Some("ship"))];
    let mut goal = delivery_goal(Some("ship"), Some(DeliveryState::Integrated));
    goal.autonomy = Some(Autonomy::RequestedOnly);
    goal.iteration_maturity = Some(IterationMaturity::PlateauConfirmed);
    goal.stopping_evidence = None;

    let message = build_todo_ownership_continuation_message(&todos, &[goal]);
    assert!(message.contains("Goal \"ship\""));
    assert!(message.contains("complete workflow"));
    assert!(!message.contains("ownership of the necessary follow-through"));
    assert!(message.contains("evidence about whether the work should stop"));
    assert!(!message.contains("workflow_validated"));
    assert!(!message.contains("necessary_followthrough"));
    assert!(!message.contains("outcome_reached"));
    // PlateauConfirmed is terminal, so it must not also be diagnosed as an
    // iteration_maturity failure. Its missing evidence is the exact defect.
    assert!(!message.contains("remaining hypotheses"));
    assert!(is_auto_poke_message(&message));
}

#[test]
fn ownership_continuation_does_not_invent_work_for_missing_assessments() {
    let todos = vec![todo("work", "completed", Some("ship"))];
    let message = build_todo_ownership_continuation_message(&todos, &[]);
    assert_eq!(message, TODO_OWNERSHIP_CONTINUATION_MESSAGE);
}

#[test]
fn completed_groups_require_sufficient_delivery_at_turn_finish() {
    let incomplete = vec![todo("work", "in_progress", Some("ship"))];
    assert!(completed_groups_have_sufficient_delivery(&incomplete, &[]));

    let completed = vec![todo("work", "completed", Some("ship"))];
    for delivery in [
        Some(DeliveryState::ChangeMade),
        Some(DeliveryState::Integrated),
    ] {
        assert!(!completed_groups_have_sufficient_delivery(
            &completed,
            &[delivery_goal(Some("ship"), delivery)],
        ));
    }
    assert!(completed_groups_have_sufficient_delivery(
        &completed,
        &[delivery_goal(
            Some("ship"),
            Some(DeliveryState::WorkflowValidated)
        )],
    ));
}

#[test]
fn ownership_gate_does_not_turn_assessment_metadata_into_more_work() {
    let todos = [todo("work", "completed", None)];
    assert!(completed_groups_have_sufficient_delivery(&todos, &[]));
    assert!(completed_groups_have_sufficient_delivery(
        &todos,
        &[TodoGoal::default()]
    ));
    let mut goal = delivery_goal(None, Some(DeliveryState::WorkflowValidated));
    goal.autonomy = Some(Autonomy::RequestedOnly);
    goal.difficulty = Some(Difficulty::Hard);
    goal.feedback_loop_traceability = Some(FeedbackLoopTraceability::Partial);
    assert!(
        !delivery_state_passes(&goal),
        "strict quality assessment is preserved"
    );
    assert!(completed_groups_have_sufficient_delivery(
        &todos,
        &[goal.clone()]
    ));
    assert_eq!(
        build_todo_ownership_continuation_message(&todos, &[goal]),
        TODO_OWNERSHIP_CONTINUATION_MESSAGE
    );
}

#[test]
fn ownership_gate_respects_documented_stops_but_not_unsupported_ones() {
    let todos = [todo("work", "completed", None)];
    for maturity in [
        IterationMaturity::ConstraintsExhausted,
        IterationMaturity::BudgetExhausted,
        IterationMaturity::PlateauConfirmed,
    ] {
        let mut goal = delivery_goal(None, Some(DeliveryState::Integrated));
        goal.iteration_maturity = Some(maturity);
        goal.feedback_loop_relevance = Some(FeedbackLoopRelevance::AcceptanceBlocked);
        for evidence in [None, Some("".into()), Some("  ".into())] {
            goal.stopping_evidence = evidence;
            assert!(!completed_groups_have_sufficient_delivery(
                &todos,
                &[goal.clone()]
            ));
            assert!(
                build_todo_ownership_continuation_message(&todos, &[goal.clone()])
                    .contains("whether the work should stop")
            );
        }
        goal.stopping_evidence =
            Some("Available checks ran, the remaining path is unavailable.".into());
        assert!(completed_groups_have_sufficient_delivery(
            &todos,
            &[goal.clone()]
        ));
        assert_eq!(
            build_todo_ownership_continuation_message(&todos, &[goal]),
            TODO_OWNERSHIP_CONTINUATION_MESSAGE
        );
    }
}

#[test]
fn ownership_gate_still_names_explicit_unfinished_work_only() {
    let todos = [
        todo("work", "completed", Some(" unfinished ")),
        todo("done", "completed", Some("done")),
        todo("open", "in_progress", Some("open")),
    ];
    let mut unfinished = delivery_goal(Some("unfinished"), None);
    unfinished.iteration_maturity = Some(IterationMaturity::Improving);
    let goals = [
        unfinished,
        delivery_goal(Some("done"), Some(DeliveryState::WorkflowValidated)),
        delivery_goal(Some("open"), Some(DeliveryState::ChangeMade)),
    ];
    assert!(!completed_groups_have_sufficient_delivery(&todos, &goals));
    let message = build_todo_ownership_continuation_message(&todos, &goals);
    assert!(message.contains("Goal \"unfinished\": keep iterating"));
    assert!(!message.contains("Goal \"done\""));
    assert!(!message.contains("Goal \"open\""));
    assert!(!message.contains("complete workflow"));
}

#[test]
fn ownership_followup_is_stable_across_todo_order_and_evidence_wording() {
    let mut todos = [
        todo("b", "completed", Some("b")),
        todo("a", "completed", Some("a")),
    ];
    let mut goals = [
        delivery_goal(Some("b"), Some(DeliveryState::Integrated)),
        delivery_goal(Some("a"), Some(DeliveryState::ChangeMade)),
    ];
    let message = build_todo_ownership_continuation_message(&todos, &goals);
    todos.reverse();
    goals.reverse();
    goals[0].stopping_evidence = Some("Rephrased the same assessment".into());
    assert_eq!(
        build_todo_ownership_continuation_message(&todos, &goals),
        message
    );
}

#[test]
fn delivery_gate_grandfathers_preexisting_completed_groups() {
    let completed = vec![todo("legacy", "completed", Some("legacy"))];
    assert!(newly_completed_groups_have_sufficient_delivery(
        &completed,
        &completed,
        &[],
    ));
}

#[test]
fn session_title_prefers_in_progress_todo_group() {
    let todos = vec![
        todo("old task", "pending", Some("Older goal")),
        todo("current task", "in_progress", Some("Fix resume names")),
        todo("later task", "pending", Some("Later goal")),
    ];

    assert_eq!(
        derive_session_title(&todos, &TodoPlan::default()).as_deref(),
        Some("Fix resume names")
    );
}

#[test]
fn session_title_uses_latest_incomplete_group_when_nothing_is_active() {
    let todos = vec![
        todo("finished", "completed", Some("Old goal")),
        todo("next", "pending", Some("Current goal")),
    ];

    assert_eq!(
        derive_session_title(&todos, &TodoPlan::default()).as_deref(),
        Some("Current goal")
    );
}

#[test]
fn ungrouped_session_title_prefers_plan_intention_then_item_content() {
    let todos = vec![todo("Run targeted tests", "in_progress", None)];
    let plan = TodoPlan {
        user_intention: Some("Keep resumed work easy to identify".to_string()),
        understands_user_intent: Some(IntentUnderstanding::Clear),
        ..Default::default()
    };

    assert_eq!(
        derive_session_title(&todos, &plan).as_deref(),
        Some("Keep resumed work easy to identify")
    );
    assert_eq!(
        derive_session_title(&todos, &TodoPlan::default()).as_deref(),
        Some("Run targeted tests")
    );
}
