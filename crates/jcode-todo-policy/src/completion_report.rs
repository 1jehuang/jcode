//! Display-only, uncapped completion reports. This module never schedules work.
use crate::*;
use std::fmt::Write;

// Encode structural punctuation, HTML, and line breaks without losing Unicode.
fn escape(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            ch if ch.is_ascii_punctuation() => {
                write!(out, "&#{};", ch as u32).unwrap();
            }
            ch if ch.is_control() => {
                write!(out, "&#{};", ch as u32).unwrap();
            }
            ch => out.push(ch),
        }
    }
    out
}

fn text(out: &mut String, label: &str, value: Option<&str>) {
    writeln!(
        out,
        "- **{label}:** {}",
        value.map(escape).unwrap_or_else(|| "not recorded".into())
    )
    .unwrap();
}

fn history(out: &mut String, label: &str, values: impl Iterator<Item = &'static str>) {
    let values: Vec<_> = values.map(escape).collect();
    writeln!(
        out,
        "- **{label} history (oldest first):** {}",
        if values.is_empty() {
            "not recorded".into()
        } else {
            values.join(" → ")
        }
    )
    .unwrap();
}

fn issue(out: &mut Vec<String>, label: &str, passes: bool) {
    if !passes {
        out.push(label.to_owned());
    }
}

fn goal_issues(goal: &TodoGoal) -> Vec<String> {
    let mut issues = Vec::new();
    // A permissive policy threshold is not proof of an accepted outcome.
    // Blockers and non-outcome stopping reasons stay visible in the report
    // even when they are an honest reason to stop automated follow-ups.
    if goal.feedback_loop_relevance == Some(FeedbackLoopRelevance::AcceptanceBlocked) {
        issues.push("acceptance path blocked".into());
    }
    if let Some(state) = goal.iteration_maturity
        && matches!(
            state,
            IterationMaturity::ConstraintsExhausted
                | IterationMaturity::BudgetExhausted
                | IterationMaturity::PlateauConfirmed
        )
    {
        issues.push(format!(
            "stopped at {} rather than outcome_reached",
            state.as_str()
        ));
    }
    issue(
        &mut issues,
        "feedback loop description missing",
        goal.feedback_loop
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty()),
    );
    issue(
        &mut issues,
        "feedback loop quality weak or missing",
        feedback_loop_passes(goal.closed_feedback_loop),
    );
    issue(
        &mut issues,
        "feedback loop relevance weak or missing",
        feedback_loop_relevance_passes(goal),
    );
    issue(
        &mut issues,
        "feedback loop coverage weak or missing",
        feedback_loop_coverage_passes(goal),
    );
    issue(
        &mut issues,
        "feedback loop traceability weak or missing",
        feedback_loop_traceability_passes(goal),
    );
    issue(
        &mut issues,
        "delivery state weak or missing",
        goal.delivery_state
            .is_some_and(|v| v >= required_delivery_state(goal.difficulty)),
    );
    issue(
        &mut issues,
        "difficulty missing (descriptive)",
        goal.difficulty.is_some(),
    );
    issue(
        &mut issues,
        "autonomy below policy threshold or missing (descriptive)",
        goal.autonomy
            .is_some_and(|v| v >= Autonomy::NecessaryFollowthrough),
    );
    issue(
        &mut issues,
        "iteration maturity nonterminal or missing",
        goal.iteration_maturity
            .is_some_and(IterationMaturity::permits_completion),
    );
    let needs_evidence = matches!(
        goal.iteration_maturity,
        Some(
            IterationMaturity::PlateauConfirmed
                | IterationMaturity::ConstraintsExhausted
                | IterationMaturity::BudgetExhausted
        )
    );
    issue(
        &mut issues,
        "required stopping evidence missing",
        !needs_evidence
            || goal
                .stopping_evidence
                .as_deref()
                .is_some_and(|v| !v.trim().is_empty()),
    );
    issues
}

/// Render all recorded todo, plan, and goal data as display-only Markdown.
/// Assessments are reported, not independently verified or used to trigger work.
pub fn render_completion_report(todos: &[TodoItem], plan: &TodoPlan, goals: &[TodoGoal]) -> String {
    let mut current = Vec::new();
    let mut earlier = Vec::new();
    issue(
        &mut current,
        "Plan: intent understanding weak or missing",
        intent_understanding_passes(plan.understands_user_intent),
    );
    issue(
        &mut current,
        "Plan: user intention missing",
        plan.user_intention
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty()),
    );
    for state in &plan.understands_user_intent_history {
        if !intent_understanding_passes(Some(*state)) {
            earlier.push(format!("Plan: intent understanding was {}", state.as_str()));
        }
    }
    for todo in todos {
        let label = format!("Todo {}", todo.id);
        issue(
            &mut current,
            &format!("{label}: confidence weak or missing"),
            completion_confidence_passes(todo.confidence),
        );
        if todo_status_is_completed(&todo.status) {
            issue(
                &mut current,
                &format!("{label}: completion confidence weak or missing"),
                completion_confidence_passes(todo.completion_confidence),
            );
        }
        for state in &todo.confidence_history {
            if !completion_confidence_passes(Some(*state)) {
                earlier.push(format!("{label}: confidence was {}", state.as_str()));
            }
        }
        if !goals.iter().any(|g| {
            normalized_group(g.group.as_deref()) == normalized_group(todo.group.as_deref())
        }) {
            current.push(format!("{label}: goal assessment missing"));
        }
    }
    for goal in goals {
        let label = goal.group.as_deref().unwrap_or("ungrouped");
        current.extend(
            goal_issues(goal)
                .into_iter()
                .map(|v| format!("Goal {label}: {v}")),
        );
        macro_rules! past {
            ($field:ident, $hist:ident, $check:expr) => {
                for state in &goal.$hist {
                    let mut prior = goal.clone();
                    prior.$field = Some(*state);
                    if !$check(&prior) {
                        earlier.push(format!(
                            "Goal {label}: {} was {}",
                            stringify!($field),
                            state.as_str()
                        ));
                    }
                }
            };
        }
        past!(
            closed_feedback_loop,
            closed_feedback_loop_history,
            |g: &TodoGoal| feedback_loop_passes(g.closed_feedback_loop)
        );
        past!(
            feedback_loop_relevance,
            feedback_loop_relevance_history,
            feedback_loop_relevance_passes
        );
        past!(
            feedback_loop_coverage,
            feedback_loop_coverage_history,
            feedback_loop_coverage_passes
        );
        past!(
            feedback_loop_traceability,
            feedback_loop_traceability_history,
            feedback_loop_traceability_passes
        );
        past!(delivery_state, delivery_state_history, |g: &TodoGoal| g
            .delivery_state
            .is_some_and(|v| v >= required_delivery_state(g.difficulty)));
    }
    let mut out = String::from(
        "# Todo completion report\n\nDisplay only. These are recorded descriptive self-assessments, not independently proven results. Status alone does not establish successful validation. Historical comparisons use current policy thresholds and current recorded difficulty.\n\n",
    );
    for (heading, issues) in [
        (
            "Current weaknesses, limitations, or missing assessments",
            current,
        ),
        (
            "Earlier weak states (including subsequently improved assessments)",
            earlier,
        ),
    ] {
        writeln!(out, "## {heading}\n").unwrap();
        if issues.is_empty() {
            out.push_str(
                "None identified in the recorded data. This is not independent verification.\n",
            );
        }
        for value in issues {
            writeln!(out, "- {}", escape(&value)).unwrap();
        }
        out.push('\n');
    }
    out.push_str("## Plan\n\n");
    text(&mut out, "User intention", plan.user_intention.as_deref());
    text(
        &mut out,
        "Current intent understanding",
        plan.understands_user_intent.map(|v| v.as_str()),
    );
    history(
        &mut out,
        "Intent understanding",
        plan.understands_user_intent_history
            .iter()
            .map(|v| v.as_str()),
    );
    writeln!(out, "\n## Todos ({})\n", todos.len()).unwrap();
    if todos.is_empty() {
        out.push_str("No todos recorded.\n");
    }
    for (index, todo) in todos.iter().enumerate() {
        writeln!(out, "\n### {}. {}\n", index + 1, escape(&todo.content)).unwrap();
        for (label, value) in [
            ("ID", Some(todo.id.as_str())),
            ("Status", Some(todo.status.as_str())),
            ("Group", todo.group.as_deref()),
            ("Priority", Some(todo.priority.as_str())),
            ("Details", todo.details.as_deref()),
            ("Assigned to", todo.assigned_to.as_deref()),
            ("Confidence", todo.confidence.map(|v| v.as_str())),
            (
                "Completion confidence",
                todo.completion_confidence.map(|v| v.as_str()),
            ),
        ] {
            text(&mut out, label, value);
        }
        text(
            &mut out,
            "Stage",
            todo.stage.map(|v| v.to_string()).as_deref(),
        );
        text(
            &mut out,
            "Blocked by",
            if todo.blocked_by.is_empty() {
                None
            } else {
                Some(todo.blocked_by.join(", "))
            }
            .as_deref(),
        );
        history(
            &mut out,
            "Confidence",
            todo.confidence_history.iter().map(|v| v.as_str()),
        );
    }
    writeln!(out, "\n## Goals ({})\n", goals.len()).unwrap();
    if goals.is_empty() {
        out.push_str("No goal assessments recorded.\n");
    }
    for goal in goals {
        writeln!(
            out,
            "\n### Goal: {}\n",
            escape(goal.group.as_deref().unwrap_or("ungrouped"))
        )
        .unwrap();
        text(&mut out, "Group", goal.group.as_deref());
        text(&mut out, "Feedback loop", goal.feedback_loop.as_deref());
        macro_rules! field {
            ($label:literal, $field:ident) => {
                text(&mut out, $label, goal.$field.map(|v| v.as_str()));
            };
        }
        macro_rules! hist {
            ($label:literal, $field:ident) => {
                history(&mut out, $label, goal.$field.iter().map(|v| v.as_str()));
            };
        }
        field!("Feedback loop quality", closed_feedback_loop);
        hist!("Feedback loop quality", closed_feedback_loop_history);
        field!("Feedback loop relevance", feedback_loop_relevance);
        hist!("Feedback loop relevance", feedback_loop_relevance_history);
        field!("Feedback loop coverage", feedback_loop_coverage);
        hist!("Feedback loop coverage", feedback_loop_coverage_history);
        field!("Feedback loop traceability", feedback_loop_traceability);
        hist!(
            "Feedback loop traceability",
            feedback_loop_traceability_history
        );
        field!("Delivery state", delivery_state);
        hist!("Delivery state", delivery_state_history);
        field!("Difficulty (descriptive)", difficulty);
        field!("Autonomy (descriptive)", autonomy);
        field!("Iteration maturity", iteration_maturity);
        text(
            &mut out,
            "Stopping evidence",
            goal.stopping_evidence.as_deref(),
        );
    }
    out
}

#[cfg(test)]
#[path = "completion_report_tests.rs"]
mod tests;
