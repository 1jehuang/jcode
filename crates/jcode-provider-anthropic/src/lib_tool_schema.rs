use super::*;

/// Normalize a tool schema for Anthropic's `input_schema`.
///
/// Anthropic accepts JSON Schema combinators inside object properties but
/// rejects `oneOf`/`anyOf`/`allOf` at the top level, and requires an object
/// schema with a `properties` map. The subset and the rewrites live in
/// `jcode-schema-dialect` so every provider shares one implementation and one
/// set of regression tests.
///
/// Widening a top-level combiner loses the per-branch constraint, which is
/// intended: runtime tool deserialization remains the authority on which
/// combination is actually valid.
pub(super) fn anthropic_input_schema(schema: &Value) -> Value {
    jcode_schema_dialect::normalize(schema, &jcode_schema_dialect::registry::ANTHROPIC)
}

/// Convert tool definitions to Anthropic API format.
///
/// Every provider and auth route advertises the same registry tools with the
/// same schemas. OAuth (subscription) only renames a few tools to their
/// Claude-Code builtin names (`bash` -> `Bash`, ...); the definition itself is
/// never replaced. Hand-curated OAuth schemas drifted from the real tools and
/// silently dropped options like `intent` (#706, #1223).
pub fn format_tools(tools: &[ToolDefinition], is_oauth: bool, cache_ttl_1h: bool) -> Vec<ApiTool> {
    let out = tools
        .iter()
        .map(|tool| ApiTool {
            name: if is_oauth {
                map_tool_name_for_oauth(&tool.name)
            } else {
                tool.name.clone()
            },
            description: tool.description.clone(),
            input_schema: anthropic_input_schema(&tool.input_schema),
            cache_control: None,
            defer_loading: tool.defer_loading,
        })
        .collect();
    finish_tool_list(out, cache_ttl_1h)
}

/// Order eager tools before deferred ones and put the prompt-cache breakpoint
/// on the last eager tool.
///
/// Deferred tools stay out of the cached system-prompt prefix, so they must
/// neither carry `cache_control` (the API rejects that with a 400) nor sit
/// between cached tools, where adding or removing one would shift the prefix.
/// The API also requires at least one non-deferred tool, so a list of only
/// deferred tools is sent eagerly instead.
pub(super) fn finish_tool_list(tools: Vec<ApiTool>, cache_ttl_1h: bool) -> Vec<ApiTool> {
    let (mut out, mut deferred): (Vec<ApiTool>, Vec<ApiTool>) =
        tools.into_iter().partition(|tool| !tool.defer_loading);
    if out.is_empty() {
        for tool in &mut deferred {
            tool.defer_loading = false;
        }
        std::mem::swap(&mut out, &mut deferred);
    }
    if let Some(last) = out.last_mut() {
        last.cache_control = Some(CacheControlParam::ephemeral(cache_ttl_1h));
    }
    out.extend(deferred);
    out
}
