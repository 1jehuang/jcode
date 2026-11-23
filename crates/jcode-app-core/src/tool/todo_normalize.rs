use serde_json::Value;

/// Leniently normalize raw todo-tool arguments before strict deserialization.
///
/// Some providers (notably Claude tool calling) intermittently emit tool
/// arguments as JSON *strings* instead of native types: the whole `todos`
/// array as one stringified JSON blob, individual items as stringified
/// objects, or numeric fields like `confidence` as `"90"`. Strict
/// `serde_json::from_value` rejects these with `invalid type: string ...`,
/// failing the entire call (issue #357; same provider quirk as #106).
pub(super) fn normalize_todo_input(mut input: Value) -> Value {
    let Some(obj) = input.as_object_mut() else {
        return input;
    };
    if let Some(plan) = obj.get_mut("plan") {
        if let Value::String(raw) = plan {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                *plan = Value::Null;
            } else if let Ok(parsed @ (Value::Object(_) | Value::Null)) =
                serde_json::from_str::<Value>(trimmed)
            {
                *plan = parsed;
            }
        }
        if let Some(fields) = plan.as_object_mut() {
            for key in [
                "alignment_score",
                "user_intention_alignment",
                "understands_user_intent",
            ] {
                if let Some(value) = fields.get_mut(key) {
                    coerce_empty_string_to_null(value);
                }
            }
        }
    }
    for key in ["todos", "goals"] {
        let Some(entries) = obj.get_mut(key) else {
            continue;
        };

        // Whole array sent as a stringified JSON blob.
        if let Value::String(raw) = entries {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                *entries = Value::Null;
            } else if let Ok(parsed @ (Value::Array(_) | Value::Null)) =
                serde_json::from_str::<Value>(trimmed)
            {
                *entries = parsed;
            }
        }

        if let Value::Array(items) = entries {
            for item in items.iter_mut() {
                // Individual item sent as a stringified JSON object.
                if let Value::String(raw) = item
                    && let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(raw.trim())
                {
                    *item = parsed;
                }
                let Some(fields) = item.as_object_mut() else {
                    continue;
                };
                if key == "todos"
                    && let Some(Value::String(status)) = fields.get_mut("status")
                    && let Some(canonical) = crate::todo::canonical_todo_status(status)
                {
                    *status = canonical.to_string();
                }
                for key in [
                    "confidence",
                    "completion_confidence",
                    "alignment_score",
                    "user_intention_alignment",
                    "closed_feedback_loop",
                    // Pre-rename aliases; some prompts and replayed transcripts
                    // still carry the old keys.
                    "hill_climbability",
                    "end_to_end_ownership",
                    "delivery_state",
                    "feedback_loop_relevance",
                    "feedback_loop_coverage",
                    "feedback_loop_traceability",
                    "difficulty",
                    "autonomy",
                ] {
                    if let Some(value) = fields.get_mut(key) {
                        coerce_empty_string_to_null(value);
                    }
                }
            }
        }
    }
    input
}

/// Coerce an empty or whitespace-only string to `null` so an omitted-but-sent
/// assessment reads as absent. Numeric legacy scores (ints, floats, numeric
/// strings) are handled by the semantic-state deserializers themselves, so no
/// numeric coercion is needed here anymore.
fn coerce_empty_string_to_null(value: &mut Value) {
    if let Value::String(raw) = value
        && raw.trim().is_empty()
    {
        *value = Value::Null;
    }
}
