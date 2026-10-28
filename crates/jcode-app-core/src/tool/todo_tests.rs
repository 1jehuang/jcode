#[test]
fn hierarchy_preserves_declared_order_and_rejects_empty_ids() {
    let item = |id: &str| json!({"id":id,"content":id,"status":"pending","priority":"high"});
    let input = json!({"phases":[
        {"phase":2,"lanes":[{"lane":"one","items":[item("a"),item("b")]},{"lane":"two","items":[item("c")]}]},
        {"phase":1,"lanes":[{"lane":"one","items":[item("d")]}]}
    ]});
    let todos = parse_todo_input(input).unwrap().todos.unwrap();
    assert_eq!(
        todos.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["a", "b", "c", "d"]
    );
    assert_eq!(
        todos.iter().map(|t| t.stage).collect::<Vec<_>>(),
        [Some(2), Some(2), Some(2), Some(1)]
    );
    for id in ["", " "] {
        assert!(
            parse_todo_input(
                json!({"phases":[{"phase":1,"lanes":[{"lane":"work","items":[item(id)]}]}]})
            )
            .is_err()
        );
    }
    let patched = apply_todo_patch(
        json!({"add":[{
            "id":"a","content":"A","status":"pending","priority":"high",
            "phase":1,"stage":1,"lane":"work","group":"work"
        }]}),
        || json!([]),
    )
    .unwrap();
    assert_eq!(
        parse_todo_input(patched).unwrap().todos.unwrap()[0].stage,
        Some(1)
    );
}

#[tokio::test]
async fn hierarchy_invalid_writes_are_atomic() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = "hierarchy-atomic";
    let tool = TodoTool::new();
    let valid = json!({"phases":[{"phase":1,"lanes":[{"lane":"x","items":[
        {"id":"a","content":"original","status":"pending","priority":"high"}
    ]}]}]});
    tool.execute(valid.clone(), test_ctx(session))
        .await
        .unwrap();
    let before = load_todos(session).unwrap();
    let mut invalid = valid;
    invalid["phases"][0]["lanes"][0]["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"a"}));
    for input in [
        invalid,
        json!({"update":[{"id":"a","content":"changed"}],"add":[{"id":"b","phase":0,"lane":"x"}]}),
        json!({"update":[{"id":"a","content":"changed"},{"id":"missing","status":"pending"}]}),
    ] {
        assert!(tool.execute(input, test_ctx(session)).await.is_err());
        assert_eq!(load_todos(session).unwrap(), before);
    }
    tool.execute(json!({"phases":[]}), test_ctx(session))
        .await
        .unwrap();
    assert!(load_todos(session).unwrap().is_empty());
    match previous_home {
        Some(home) => crate::env::set_var("JCODE_HOME", home),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}
#[test]
fn hierarchy_flattens_and_clears() {
    let input = json!({"phases":[{"phase":2,"lanes":[{"lane":"parser","items":[
        {"id":"a","content":"A","status":"pending","priority":"high"}
    ]}]}]});
    let parsed = parse_todo_input(input.clone()).unwrap().todos.unwrap();
    assert_eq!(parsed[0].stage, Some(2));
    assert_eq!(parsed[0].group.as_deref(), Some("parser"));
    assert_eq!(todo_write_mode(&input), "full");
    assert!(
        parse_todo_input(json!({"phases":[]}))
            .unwrap()
            .todos
            .unwrap()
            .is_empty()
    );
    let flattened =
        apply_todo_patch(input, || panic!("replacement must not load stored list")).unwrap();
    assert!(flattened.get("phases").is_none());
    assert_eq!(flattened["todos"][0]["stage"], 2);
}

#[test]
fn hierarchy_rejects_malformed_structures() {
    let item = json!({"id":"a","content":"A","status":"pending","priority":"high"});
    let lane = json!({"lane":"x","items":[item]});
    let phase = json!({"phase":1,"lanes":[lane]});
    for bad in [
        json!(null),
        json!({}),
        json!([null]),
        json!([{"phase":0,"lanes":[lane]}]),
        json!([{"phase":1.5,"lanes":[lane]}]),
        json!([{"phase":1}]),
        json!([{"phase":1,"lanes":[]}]),
        json!([{"phase":1,"lanes":{}}]),
        json!([{"phase":1,"lanes":[{"lane":" ","items":[item]}]}]),
        json!([{"phase":1,"lanes":[{"lane":"x"}]}]),
        json!([{"phase":1,"lanes":[{"lane":"x","items":[]}]}]),
        json!([{"phase":1,"lanes":[{"lane":"x","items":[null]}]}]),
        json!([phase, phase]),
        json!([{"phase":1,"lanes":[lane,lane]}]),
        json!([{"phase":1,"lanes":[lane,{"lane":"y","items":[item]}]}]),
    ] {
        assert!(
            parse_todo_input(json!({"phases":bad})).is_err(),
            "accepted {bad}"
        );
    }
    for key in ["phase", "stage", "wave", "lane", "group"] {
        let mut item = item.clone();
        item[key] = json!(1);
        assert!(
            parse_todo_input(json!({"phases":[{"phase":1,"lanes":[{"lane":"x","items":[item]}]}]}))
                .is_err()
        );
    }
    for key in ["todos", "update", "add", "remove"] {
        let mut input = json!({"phases":[]});
        input[key] = json!([]);
        assert!(apply_todo_patch(input, || json!([])).is_err());
    }
}

#[test]
fn hierarchy_patch_inherits_and_preserves_membership() {
    let stored = json!([{"id":"a","content":"A","status":"pending","priority":"high","stage":2,"group":"x"}]);
    let result = apply_todo_patch(
        json!({"update":[{"id":"a","content":"changed"}],
        "add":[{"id":"b","content":"B","status":"pending","priority":"high","after":"a"}]}),
        || stored.clone(),
    )
    .unwrap();
    let todos = parse_todo_input(result).unwrap().todos.unwrap();
    assert_eq!(todos.len(), 2);
    for todo in todos {
        assert_eq!(todo.stage, Some(2));
        assert_eq!(todo.group.as_deref(), Some("x"));
    }
    for patch in [
        json!({"add":[{"id":"b"}]}),
        json!({"add":[{"id":"b","after":"missing"}]}),
        json!({"update":[{"id":"a","phase":0}]}),
        json!({"update":[{"id":"a","lane":""}]}),
        json!({"update":[{"id":"a","phase":1,"stage":2}]}),
        json!({"update":[{"id":"a","lane":"x","group":"y"}]}),
        json!({"update":[{"id":"a","items":[]}]}),
        json!({"remove":[42]}),
    ] {
        assert!(apply_todo_patch(patch, || stored.clone()).is_err());
        assert_eq!(stored[0]["content"], "A");
    }
}
use super::*;

#[test]
fn tool_is_named_todo() {
    assert_eq!(TodoTool::new().name(), "todo");
}

#[test]
fn add_schema_declares_all_todo_fields_and_conforms_to_gemini() {
    let schema = TodoTool::new().parameters_schema();
    let add = &schema["properties"]["add"]["items"];
    for (name, property) in schema["properties"]["phases"]["items"]["properties"]["lanes"]["items"]
        ["properties"]["items"]["items"]["properties"]
        .as_object()
        .unwrap()
    {
        assert_eq!(&add["properties"][name], property, "{name}");
    }
    assert_eq!(add["properties"]["after"]["type"], "string");
    let errors = jcode_schema_dialect::must_not_contain_unsupported_constructs(
        &jcode_schema_dialect::dialect::apply(&schema, &jcode_schema_dialect::registry::GEMINI),
        &jcode_schema_dialect::registry::GEMINI,
    );
    assert!(errors.is_empty(), "{errors:?}");
    for name in add["required"].as_array().unwrap() {
        assert!(add["properties"].get(name.as_str().unwrap()).is_some());
    }
}

#[test]
fn schema_advertises_intent_and_todos() {
    let schema = TodoTool::new().parameters_schema();
    let props = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("todo schema should have properties");
    assert_eq!(props.len(), 7);
    assert!(props.contains_key("intent"));
    assert!(props.contains_key("phases"));
    assert!(!props.contains_key("todos"));
    assert!(props.contains_key("plan"));
    assert!(props.contains_key("goals"));
    for patch in ["update", "add", "remove"] {
        assert!(
            props.contains_key(patch),
            "{patch} patch mode should be offered"
        );
    }

    let item = props["phases"]["items"]["properties"]["lanes"]["items"]["properties"]["items"]
        .get("items")
        .and_then(|v| v.as_object())
        .expect("todos should describe item objects");
    let required = item
        .get("required")
        .and_then(|v| v.as_array())
        .expect("todo item should advertise required fields");
    assert!(required.iter().any(|v| v == "confidence"));
    let item_props = item
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("todo item should advertise properties");
    assert!(!item_props.contains_key("phase"));
    assert!(!item_props.contains_key("lane"));
    assert!(!item_props.contains_key("stage"));
    assert!(!item_props.contains_key("group"));
    assert!(item_props.contains_key("confidence"));
    assert!(item_props.contains_key("completion_confidence"));
    assert!(!item_props.contains_key("closed_feedback_loop"));
    assert_eq!(
        item_props["confidence"]["description"],
        "Evidence state that this todo can be completed correctly; reassess as evidence accumulates."
    );

    let plan_props = props["plan"]
        .get("properties")
        .and_then(|v| v.as_object())
        .expect("plan should describe properties");
    assert!(plan_props.contains_key("user_intention"));
    assert!(plan_props.contains_key("understands_user_intent"));
    assert!(!plan_props.contains_key("alignment_score"));
    assert!(!plan_props.contains_key("user_intention_alignment"));
    assert_eq!(plan_props.len(), 2);
    let plan_required = props["plan"]["required"]
        .as_array()
        .expect("plan should advertise required fields");
    assert!(plan_required.iter().any(|value| value == "user_intention"));
    assert!(
        plan_required
            .iter()
            .any(|value| value == "understands_user_intent")
    );

    let goal_props = props["goals"]
        .get("items")
        .and_then(|v| v.get("properties"))
        .and_then(|v| v.as_object())
        .expect("goals should describe item objects");
    assert!(goal_props.contains_key("group"));
    assert!(goal_props.contains_key("closed_feedback_loop"));
    assert!(goal_props.contains_key("feedback_loop"));
    assert!(goal_props.contains_key("feedback_loop_relevance"));
    assert!(goal_props.contains_key("feedback_loop_coverage"));
    assert!(goal_props.contains_key("feedback_loop_traceability"));
    assert!(goal_props.contains_key("delivery_state"));
    assert!(goal_props.contains_key("difficulty"));
    assert!(goal_props.contains_key("autonomy"));
    assert!(goal_props.contains_key("iteration_maturity"));
    assert!(goal_props.contains_key("stopping_evidence"));
    assert!(!goal_props.contains_key("end_to_end_ownership"));
    // Intent lives on the plan, not per goal.
    assert!(!goal_props.contains_key("user_intention"));
    assert!(!goal_props.contains_key("alignment_score"));
    assert!(!goal_props.contains_key("objective"));
    assert_eq!(goal_props.len(), 11);
    assert_eq!(
        goal_props["feedback_loop_relevance"]["enum"],
        json!([
            "indirect",
            "synthetic",
            "representative",
            "acceptance_blocked",
            "acceptance_aligned"
        ])
    );
    let relevance_description = goal_props["feedback_loop_relevance"]["description"]
        .as_str()
        .expect("feedback-loop relevance should explain every state");
    for required_concept in [
        "custom harnesses",
        "real public interfaces",
        "external constraint",
        "Substitute-only validation is never acceptance_aligned",
    ] {
        assert!(relevance_description.contains(required_concept));
    }

    let goal_required = props["goals"]["items"]["required"]
        .as_array()
        .expect("goals should advertise required fields");
    assert!(
        goal_required
            .iter()
            .any(|value| value == "closed_feedback_loop")
    );
    assert!(goal_required.iter().any(|value| value == "feedback_loop"));
    assert!(
        goal_required
            .iter()
            .any(|value| value == "feedback_loop_relevance")
    );
    assert!(
        goal_required
            .iter()
            .any(|value| value == "feedback_loop_coverage")
    );
    assert!(
        goal_required
            .iter()
            .any(|value| value == "feedback_loop_traceability")
    );

    let alignment_description = plan_props["understands_user_intent"]
        .get("description")
        .and_then(Value::as_str)
        .expect("alignment score should describe representation coverage");
    assert!(alignment_description.contains("what the user wants"));
    assert!(alignment_description.contains("when guessing"));
    // Deferred continuation messages are intentionally concise. Preserve
    // their actionable guidance without requiring the legacy long rubric.
    for required_concept in [
        "Understand the user's intent better",
        "Try to avoid asking the user",
        "Make sure the todo is up to date",
    ] {
        assert!(
            crate::todo::TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE.contains(required_concept),
            "intent gate message omitted {required_concept}"
        );
    }
    let feedback_description = goal_props["feedback_loop"]
        .get("description")
        .and_then(Value::as_str)
        .expect("feedback loop should describe requirement-to-check coverage");
    // Case-insensitive: the description opens the sentence with
    // "Requirement-to-check", so a case-sensitive match broke when the
    // wording moved to the front of the string (issue #730).
    let feedback_description_lower = feedback_description.to_ascii_lowercase();
    assert!(
        feedback_description_lower.contains("requirement-to-check"),
        "feedback_loop description omitted the requirement-to-check framing: {feedback_description}"
    );
    assert!(
        feedback_description_lower.contains("explicit observation or check"),
        "feedback_loop description omitted per-requirement check coverage: {feedback_description}"
    );
    for required_concept in [
        "Your feedback loop isn't good enough",
        "Think about what feedback loops you need",
        "Make sure the todo is up to date",
    ] {
        assert!(
            crate::todo::TODO_CLOSED_FEEDBACK_LOOP_CONTINUATION_MESSAGE.contains(required_concept),
            "feedback gate message omitted {required_concept}"
        );
    }
    assert!(
        !alignment_description
            .to_ascii_lowercase()
            .contains("threshold")
    );

    let ownership_description = goal_props["delivery_state"]
        .get("description")
        .and_then(Value::as_str)
        .expect("delivery state should have a neutral description");
    assert!(ownership_description.contains("toward the user's outcome"));
    assert!(!ownership_description.contains("90"));
    assert!(
        !ownership_description
            .to_ascii_lowercase()
            .contains("threshold")
    );

    let loop_description = goal_props["closed_feedback_loop"]
        .get("description")
        .and_then(Value::as_str)
        .expect("closed feedback loop should describe the assessment neutrally");
    assert!(!loop_description.to_ascii_lowercase().contains("threshold"));

    let model_visible_schema = serde_json::to_string(&schema)
        .expect("todo schema should serialize")
        .to_ascii_lowercase();
    for disclosure in [
        "threshold",
        "quality gate",
        "internal quality check",
        "not jump",
        "test that passes",
        "isn't high enough",
    ] {
        assert!(
            !model_visible_schema.contains(disclosure),
            "model-visible todo schema disclosed calibration wording: {disclosure}"
        );
    }
    for required_guidance in [
        "public interfaces",
        "integration boundaries",
        "edge cases",
        "packaging",
        "likely failure modes",
    ] {
        assert!(
            model_visible_schema.contains(required_guidance),
            "todo schema omitted generic validation guidance: {required_guidance}"
        );
    }
    for domain_hint in [
        "visual quality",
        "screenshot",
        "browser",
        "viewport",
        "console error",
    ] {
        assert!(
            !model_visible_schema.contains(domain_hint),
            "model-visible todo schema biased visual-work feedback: {domain_hint}"
        );
    }
}

fn parse(input: Value) -> Result<TodoInput> {
    parse_todo_input(input)
}

#[test]
fn accepts_stringified_todos_array() {
    let input = json!({
        "todos": "[{\"content\":\"a\",\"status\":\"pending\",\"priority\":\"high\",\"id\":\"1\",\"confidence\":90}]"
    });
    let parsed = parse(input).expect("stringified todos array should parse");
    let todos = parsed.todos.expect("todos present");
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].content, "a");
    assert_eq!(
        todos[0].confidence,
        Some(crate::todo::ConfidenceState::Plausible)
    );
}

#[test]
fn accepts_stringified_todo_items_and_string_confidence() {
    let input = json!({
        "todos": [
            "{\"content\":\"b\",\"status\":\"completed\",\"priority\":\"low\",\"id\":\"2\",\"confidence\":\"85\",\"completion_confidence\":\"95\"}",
            {"content": "c", "status": "pending", "priority": "high", "id": "3", "confidence": "70"}
        ]
    });
    let parsed = parse(input).expect("string-coerced items should parse");
    let todos = parsed.todos.expect("todos present");
    assert_eq!(todos.len(), 2);
    assert_eq!(
        todos[0].confidence,
        Some(crate::todo::ConfidenceState::Plausible)
    );
    assert_eq!(
        todos[0].completion_confidence,
        Some(crate::todo::ConfidenceState::Plausible)
    );
    assert_eq!(
        todos[1].confidence,
        Some(crate::todo::ConfidenceState::Plausible)
    );
}

#[test]
fn normalizes_natural_and_case_varied_todo_statuses() {
    let parsed = parse(json!({
        "todos": [
            {"content": "a", "status": "done", "priority": "high", "id": "1", "confidence": "verified"},
            {"content": "b", "status": " Finished ", "priority": "low", "id": "2", "confidence": "validated"},
            {"content": "c", "status": "Canceled", "priority": "low", "id": "3", "confidence": "plausible"}
        ]
    }))
    .expect("status synonyms should parse");
    let statuses: Vec<_> = parsed
        .todos
        .expect("todos present")
        .into_iter()
        .map(|todo| todo.status)
        .collect();
    assert_eq!(statuses, ["completed", "completed", "cancelled"]);
}

#[test]
fn rejects_unknown_todo_statuses_with_valid_vocabulary() {
    let error = parse(json!({
        "todos": [
            {"content": "a", "status": "blocked", "priority": "high", "id": "1", "confidence": "plausible"}
        ]
    }))
    .err()
    .expect("unknown status should be rejected");
    let message = error.to_string();
    assert!(message.contains("invalid todo status \"blocked\""));
    assert!(message.contains("pending, in_progress, completed, cancelled"));
}

#[test]
fn accepts_float_confidence_and_empty_string_as_none() {
    let input = json!({
        "todos": [
            {"content": "d", "status": "pending", "priority": "high", "id": "4", "confidence": 90.0, "completion_confidence": ""}
        ]
    });
    let parsed = parse(input).expect("float confidence should parse");
    let todos = parsed.todos.expect("todos present");
    assert_eq!(
        todos[0].confidence,
        Some(crate::todo::ConfidenceState::Plausible)
    );
    assert_eq!(todos[0].completion_confidence, None);
}

#[test]
fn empty_string_todos_means_read() {
    let parsed = parse(json!({"todos": ""})).expect("empty string should parse");
    assert!(parsed.todos.is_none());
}

#[test]
fn native_input_still_parses() {
    let input = json!({
        "todos": [
            {"content": "e", "status": "pending", "priority": "high", "id": "5", "confidence": 80}
        ]
    });
    let parsed = parse(input).expect("native input should parse");
    assert_eq!(
        parsed.todos.expect("todos present")[0].confidence,
        Some(crate::todo::ConfidenceState::Plausible)
    );
}

#[test]
fn accepts_goals_and_plan_including_string_coercion() {
    let input = json!({
        "plan": {"user_intention": "make repository search feel instant", "understands_user_intent": "97"},
        "goals": [
            {"group": "optimize grep", "closed_feedback_loop": "95", "feedback_loop": "run the grep benchmark and compare p50"},
            {"closed_feedback_loop": 20}
        ]
    });
    let parsed = parse(input).expect("goals and plan should parse");
    let plan = parsed.plan.expect("plan present");
    assert_eq!(
        plan.understands_user_intent,
        Some(crate::todo::IntentUnderstanding::Clear)
    );
    assert_eq!(
        plan.user_intention.as_deref(),
        Some("make repository search feel instant")
    );
    let goals = parsed.goals.expect("goals present");
    assert_eq!(
        goals[0].closed_feedback_loop,
        Some(crate::todo::FeedbackLoopState::Strong)
    );
    assert_eq!(
        goals[0].feedback_loop.as_deref(),
        Some("run the grep benchmark and compare p50")
    );
    // Runtime parsing remains backward-compatible with stored or older
    // provider payloads even though the advertised schema requires the field.
    assert_eq!(goals[1].feedback_loop, None);
    assert_eq!(goals[1].group, None);
}

#[test]
fn stringified_plan_object_is_accepted() {
    let parsed = parse(json!({
        "plan": "{\"user_intention\":\"ship it\",\"understands_user_intent\":\"96\"}"
    }))
    .expect("stringified plan should parse");
    let plan = parsed.plan.expect("plan present");
    assert_eq!(plan.user_intention.as_deref(), Some("ship it"));
    assert_eq!(
        plan.understands_user_intent,
        Some(crate::todo::IntentUnderstanding::Clear)
    );
}

#[test]
fn accepts_legacy_plan_alignment_key_but_serializes_the_new_name() {
    let parsed = parse(json!({
        "plan": {"user_intention_alignment": "97"}
    }))
    .expect("legacy alignment key should remain readable");
    let plan = parsed.plan.expect("plan present");
    assert_eq!(
        plan.understands_user_intent,
        Some(crate::todo::IntentUnderstanding::Clear)
    );

    let serialized = serde_json::to_value(plan).expect("plan should serialize");
    assert_eq!(serialized["understands_user_intent"], "clear");
    assert!(serialized.get("user_intention_alignment").is_none());

    let legacy_field: TodoPlanField = serde_json::from_str("\"user_intention_alignment\"")
        .expect("legacy plan-change field should deserialize");
    assert_eq!(legacy_field, TodoPlanField::UnderstandsUserIntent);
    assert_eq!(
        serde_json::to_string(&legacy_field).expect("plan field should serialize"),
        "\"understands_user_intent\""
    );
}

fn goal(group: Option<&str>, state: crate::todo::FeedbackLoopState) -> TodoGoal {
    TodoGoal {
        group: group.map(str::to_string),
        closed_feedback_loop: Some(state),
        feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
        feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
        feedback_loop_traceability: Some(crate::todo::FeedbackLoopTraceability::Complete),
        ..Default::default()
    }
}

/// A plan whose intent assessment clears the private gate, so goal-level
/// tests observe only closed feedback loop behavior.
fn aligned_plan() -> TodoPlan {
    TodoPlan {
        user_intention: Some("understood".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::Complete),
        understands_user_intent_history: vec![crate::todo::IntentUnderstanding::Complete],
    }
}

fn todo_in_group(group: Option<&str>, id: &str) -> TodoItem {
    TodoItem {
        content: format!("task {id}"),
        status: "pending".to_string(),
        priority: "medium".to_string(),
        id: id.to_string(),
        group: group.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn todo_telemetry_derives_lifecycle_groups_and_score_summaries() {
    let mut pending = todo_in_group(Some("build"), "pending");
    pending.confidence = Some(crate::todo::ConfidenceState::Plausible);
    let mut removed = todo_in_group(Some("build"), "removed");
    removed.status = "in_progress".to_string();
    removed.confidence = Some(crate::todo::ConfidenceState::Plausible);
    let previous = vec![pending.clone(), removed];

    pending.status = "completed".to_string();
    pending.completion_confidence = Some(crate::todo::ConfidenceState::Validated);
    let mut created = todo_in_group(Some("verify"), "created");
    created.confidence = Some(crate::todo::ConfidenceState::Plausible);
    let current = vec![pending, created];
    let goals = vec![
        TodoGoal {
            group: Some("build".to_string()),
            closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Strong),
            feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::Representative),
            feedback_loop_coverage: Some(crate::todo::FeedbackLoopCoverage::MainPaths),
            delivery_state: Some(crate::todo::DeliveryState::OutcomeDelivered),
            ..Default::default()
        },
        TodoGoal {
            group: Some("verify".to_string()),
            closed_feedback_loop: Some(crate::todo::FeedbackLoopState::Strong),
            feedback_loop_relevance: Some(crate::todo::FeedbackLoopRelevance::AcceptanceAligned),
            feedback_loop_coverage: Some(
                crate::todo::FeedbackLoopCoverage::EdgeAndIntegrationPaths,
            ),
            delivery_state: Some(crate::todo::DeliveryState::OutcomeDelivered),
            ..Default::default()
        },
    ];
    let plan = TodoPlan {
        understands_user_intent: Some(crate::todo::IntentUnderstanding::Partial),
        ..Default::default()
    };

    let update = todo_telemetry_update(&previous, &current, &goals, &plan);
    assert_eq!(update.todos_created, 1);
    assert_eq!(update.todos_completed, 1);
    assert_eq!(update.todos_abandoned, 1);
    assert_eq!(update.current_incomplete, 1);
    assert_eq!(update.list_size, 2);
    assert_eq!(update.groups_completed, 1);
    assert_eq!(update.groups_total, 2);
    assert_eq!(update.confidence.min, Some(80));
    assert_eq!(update.confidence.mean, Some(80.0));
    assert_eq!(update.confidence.count, 2);
    assert_eq!(update.completion_confidence.min, Some(96));
    assert_eq!(update.completion_confidence.count, 1);
    assert_eq!(update.understands_user_intent.min, Some(80));
    assert_eq!(update.closed_feedback_loop.min, Some(88));
    assert_eq!(update.closed_feedback_loop.mean, Some(88.0));
    assert_eq!(update.feedback_loop_relevance.min, Some(75));
    assert_eq!(update.feedback_loop_relevance.count, 2);
    assert_eq!(update.feedback_loop_coverage.min, Some(75));
    assert_eq!(update.feedback_loop_coverage.count, 2);
    assert_eq!(update.end_to_end_ownership.min, Some(98));
    assert_eq!(update.end_to_end_ownership.mean, Some(98.0));
}

#[test]
fn todo_telemetry_regrouping_does_not_create_or_abandon_items() {
    let mut completed = todo_in_group(Some("old"), "a");
    completed.status = "completed".to_string();
    let pending = todo_in_group(Some("old"), "b");
    let previous = vec![completed.clone(), pending.clone()];

    completed.group = Some("done".to_string());
    let mut pending = pending;
    pending.group = Some("remaining".to_string());
    let current = vec![completed, pending];

    let update = todo_telemetry_update(&previous, &current, &[], &TodoPlan::default());
    assert_eq!(update.todos_created, 0);
    assert_eq!(update.todos_completed, 0);
    assert_eq!(update.todos_abandoned, 0);
    assert_eq!(update.groups_completed, 1);
    assert_eq!(update.groups_total, 2);
}

#[test]
fn todo_telemetry_zero_state_is_all_zero_and_has_no_scores() {
    let update = todo_telemetry_update(&[], &[], &[], &TodoPlan::default());
    assert_eq!(update, crate::telemetry::TodoTelemetryUpdate::default());
}

/// Issue #695: after the agent moves to a new task and replaces the todo
/// list, goals from the finished task must not keep showing in the panel.
#[test]
fn prune_orphaned_goals_drops_goals_without_live_todos() {
    let goals = vec![
        goal(Some("old task"), crate::todo::FeedbackLoopState::Weak),
        goal(Some("new task"), crate::todo::FeedbackLoopState::Strong),
    ];
    let todos = vec![todo_in_group(Some("new task"), "1")];

    let pruned = prune_orphaned_goals(goals, &todos);

    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].group.as_deref(), Some("new task"));
}

#[test]
fn prune_orphaned_goals_keeps_ungrouped_goal_for_flat_list() {
    let goals = vec![goal(None, crate::todo::FeedbackLoopState::Usable)];
    let todos = vec![todo_in_group(None, "1")];

    assert_eq!(prune_orphaned_goals(goals, &todos).len(), 1);
}

#[test]
fn prune_orphaned_goals_keeps_everything_when_todo_list_is_empty() {
    // A goals-only write with no stored todos must not lose assessments.
    let goals = vec![
        goal(Some("a"), crate::todo::FeedbackLoopState::Absent),
        goal(None, crate::todo::FeedbackLoopState::Weak),
    ];
    assert_eq!(prune_orphaned_goals(goals, &[]).len(), 2);
}

#[test]
fn merge_goals_retains_unmentioned_goals() {
    let stored = vec![
        goal(Some("a"), crate::todo::FeedbackLoopState::Weak),
        goal(Some("b"), crate::todo::FeedbackLoopState::Strong),
    ];
    // Rewrite goal 'a', leave 'b' alone.
    let merged = merge_goals(
        &stored,
        Some(vec![goal(
            Some(" a "),
            crate::todo::FeedbackLoopState::Weak,
        )]),
    );
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].group.as_deref(), Some("a"));
    assert_eq!(
        merged[0].closed_feedback_loop,
        Some(crate::todo::FeedbackLoopState::Weak)
    );
    assert_eq!(merged[1].group.as_deref(), Some("b"));
    // No incoming goals: stored goals unchanged.
    assert_eq!(merge_goals(&stored, None).len(), 2);
}

#[test]
fn merge_plan_retains_stored_intent_when_update_omits_fields() {
    let stored = TodoPlan {
        user_intention: Some("make search feel instant".to_string()),
        understands_user_intent: Some(crate::todo::IntentUnderstanding::Partial),
        understands_user_intent_history: vec![crate::todo::IntentUnderstanding::Partial],
    };

    let merged = merge_plan(
        &stored,
        Some(TodoPlan {
            user_intention: None,
            understands_user_intent: Some(crate::todo::IntentUnderstanding::Partial),
            ..Default::default()
        }),
    );
    assert_eq!(
        merged.user_intention.as_deref(),
        Some("make search feel instant")
    );
    assert_eq!(
        merged.understands_user_intent,
        Some(crate::todo::IntentUnderstanding::Partial)
    );

    // An omitted plan leaves the stored assessment untouched.
    assert_eq!(merge_plan(&stored, None), stored);
}

#[test]
fn plan_change_reports_only_updated_intent_fields() {
    let before = aligned_plan();
    let after = TodoPlan {
        user_intention: Some("understood better".to_string()),
        ..before.clone()
    };

    let change = plan_change(&before, &after).expect("intent change should be reported");
    assert_eq!(change.fields, vec![TodoPlanField::UserIntention]);
    assert_eq!(change.before.as_ref(), Some(&before));
    assert_eq!(change.after.as_ref(), Some(&after));
    assert!(plan_change(&before, &before).is_none());
}

fn open_todo(group: Option<&str>) -> TodoItem {
    TodoItem {
        id: "t1".to_string(),
        content: "work".to_string(),
        status: "in_progress".to_string(),
        priority: "high".to_string(),
        group: group.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn ownership_gate_output_preserves_the_saved_todo_card() {
    let todos = vec![open_todo(Some("ship"))];
    let plan = aligned_plan();
    let goals = vec![goal(Some("ship"), crate::todo::FeedbackLoopState::Closed)];
    let output = build_todo_output(
        todos.clone(),
        plan.clone(),
        goals.clone(),
        None,
        None,
        [crate::todo::TODO_OWNERSHIP_CONTINUATION_MESSAGE.to_string()],
    )
    .expect("ownership gate should produce a structured todo result");

    assert_eq!(output.title.as_deref(), Some("1 todos"));
    assert!(output.output.starts_with('['));
    assert!(output.output.contains("\"status\": \"in_progress\""));
    assert!(
        output
            .output
            .contains(crate::todo::TODO_OWNERSHIP_CONTINUATION_MESSAGE)
    );
    assert_eq!(
        output.metadata,
        Some(json!({"todos": todos, "plan": plan, "goals": goals}))
    );
}

#[path = "todo_execution_tests.rs"]
mod execution;

fn test_ctx(session_id: &str) -> ToolContext {
    ToolContext {
        session_id: session_id.to_string(),
        message_id: session_id.to_string(),
        tool_call_id: "call".to_string(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}
