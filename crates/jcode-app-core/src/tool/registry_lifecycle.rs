//! Tool lifecycle telemetry, hooks, and feedback.

use super::*;

#[cfg(test)]
#[path = "registry_lifecycle_tests.rs"]
mod tests;

impl Registry {
    pub(super) fn tool_lifecycle_fields(
        phase: &str,
        requested_name: &str,
        resolved_name: &str,
        input: &Value,
        ctx: &ToolContext,
    ) -> Vec<(String, String)> {
        let cwd = ctx
            .working_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string());
        // Telemetry needs only the length, not another owned copy of the input.
        let input_json_bytes = crate::process_memory::estimate_json_bytes(input);
        let mut fields = vec![
            ("phase".to_string(), phase.to_string()),
            ("tool_name".to_string(), requested_name.to_string()),
            ("resolved_tool_name".to_string(), resolved_name.to_string()),
            ("session_id".to_string(), ctx.session_id.clone()),
            ("message_id".to_string(), ctx.message_id.clone()),
            ("tool_call_id".to_string(), ctx.tool_call_id.clone()),
            (
                "execution_mode".to_string(),
                format!("{:?}", ctx.execution_mode),
            ),
            ("cwd".to_string(), cwd),
            ("input_json_bytes".to_string(), input_json_bytes.to_string()),
        ];

        if let Some(object) = input.as_object() {
            let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
            keys.sort();
            fields.push(("input_keys".to_string(), keys.join(",")));

            let touched_paths = Self::touched_paths(input, ctx)
                .into_iter()
                .map(|(key, path)| format!("{key}:{}", path.display()))
                .collect::<Vec<_>>();
            if !touched_paths.is_empty() {
                fields.push(("touched_paths".to_string(), touched_paths.join(",")));
                fields.push((
                    "touched_path_count".to_string(),
                    touched_paths.len().to_string(),
                ));
            }

            for text_key in ["command", "prompt", "task", "query", "content"] {
                if let Some(text) = object.get(text_key).and_then(Value::as_str) {
                    fields.push((format!("{text_key}_bytes"), text.len().to_string()));
                    fields.push((
                        format!("{text_key}_chars"),
                        text.chars().count().to_string(),
                    ));
                }
            }
        }

        fields
    }

    /// Event payload shared by the `post_tool` observer and the
    /// `post_tool_feedback` hook.
    pub(super) fn post_tool_event(
        event_name: &'static str,
        resolved_name: &str,
        input: &Value,
        ctx: &ToolContext,
        result: std::result::Result<&ToolOutput, &anyhow::Error>,
        latency_ms: u64,
    ) -> crate::hooks::HookEvent {
        let mut unique_paths: Vec<String> = Vec::new();
        for (_, path) in Self::touched_paths(input, ctx) {
            let path = path.display().to_string();
            if !unique_paths.contains(&path) {
                unique_paths.push(path);
            }
        }
        let mut event = crate::hooks::HookEvent::new(event_name)
            .session_id(ctx.session_id.clone())
            .field("TOOL_NAME", resolved_name)
            .field("TOOL_CALL_ID", ctx.tool_call_id.clone())
            .field("TOOL_INPUT", crate::hooks::tool_input_json_prefix(input))
            .field("TOUCHED_PATHS", unique_paths.join("\n"))
            .field("STATUS", if result.is_ok() { "ok" } else { "error" })
            .field("DURATION_MS", latency_ms.to_string());
        if let Some(dir) = &ctx.working_dir {
            event = event.cwd(dir.display().to_string());
        }
        match result {
            Ok(output) => {
                event = event.field("OUTPUT_BYTES", output.output.len().to_string());
            }
            Err(error) => {
                const ERROR_LIMIT: usize = 1000;
                let message: String = error.to_string().chars().take(ERROR_LIMIT).collect();
                event = event.field("ERROR", message);
            }
        }
        event
    }

    /// Fire the `post_tool` observer hook with tool outcome metadata.
    /// No-op (without building the payload) when the hook is not configured.
    pub(super) fn fire_post_tool_hook(
        resolved_name: &str,
        input: &Value,
        ctx: &ToolContext,
        result: &Result<ToolOutput>,
        latency_ms: u64,
    ) {
        if !crate::hooks::hook_configured("post_tool") {
            return;
        }
        crate::hooks::dispatch_observer(Self::post_tool_event(
            "post_tool",
            resolved_name,
            input,
            ctx,
            result.as_ref(),
            latency_ms,
        ));
    }

    /// Run the synchronous `post_tool_feedback` hooks after a successful tool
    /// call and append whatever they print to the tool result, so linters and
    /// checkers can feed findings straight back to the model.
    pub(super) async fn apply_post_tool_feedback(
        resolved_name: &str,
        input: &Value,
        ctx: &ToolContext,
        latency_ms: u64,
        output: &mut ToolOutput,
    ) {
        if !crate::hooks::hook_configured("post_tool_feedback") {
            return;
        }
        let event = Self::post_tool_event(
            "post_tool_feedback",
            resolved_name,
            input,
            ctx,
            Ok(output),
            latency_ms,
        );
        if let Some(feedback) =
            crate::hooks::run_post_tool_feedback(&event, &input.to_string()).await
        {
            if !output.output.is_empty() && !output.output.ends_with('\n') {
                output.output.push('\n');
            }
            output.output.push_str("\n[post_tool_feedback]\n");
            output.output.push_str(&feedback);
        }
    }
}
