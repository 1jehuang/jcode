//! Control-channel answers (CLI -> host requests) and the in-process `jcode`
//! SDK MCP server.
//!
//! Everything here is pure: the turn driver feeds parsed requests in and
//! writes the returned JSON back to the child's stdin. `tools/call` is the
//! only asynchronous request and is surfaced as [`McpAction::CallTool`].

use jcode_message_types::ToolDefinition;
use serde_json::{Value, json};

use crate::translate::JCODE_MCP_PREFIX;

/// Name of the SDK MCP server jcode registers with Claude Code.
pub const JCODE_MCP_SERVER: &str = "jcode";

/// jcode tools Claude Code already provides natively. They are never exposed
/// over MCP so the model does not see two shells, two file readers, etc.
pub const NATIVE_DUPLICATE_TOOLS: &[&str] = &[
    "bash",
    "read",
    "write",
    "edit",
    "multiedit",
    "glob",
    "grep",
    "ls",
    "webfetch",
    "websearch",
    "todo",
    "todowrite",
    "todoread",
    "subagent",
    "task",
    "apply_patch",
    "patch",
    "replace",
    "notebook",
    "notebookedit",
    "batch",
    "invalid",
];

/// Message returned when Claude tries to leave plan mode on its own.
pub const PLAN_CAPTURED_MESSAGE: &str = "The client captured your proposed plan. Stop here and wait for the user's feedback before implementing anything.";

/// Message returned when Claude asks an interactive multiple-choice question.
pub const ASK_USER_DENIED_MESSAGE: &str = "jcode cannot show interactive questions in Claude Code mode. Ask the user the question directly in your reply and stop so they can answer.";

/// Whether a jcode tool should be offered to Claude Code over MCP.
pub fn is_exposed_tool(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    !lower.is_empty() && !NATIVE_DUPLICATE_TOOLS.contains(&lower.as_str())
}

/// jcode tools to offer over MCP, in the order jcode passed them.
pub fn exposed_tools(tools: &[ToolDefinition]) -> Vec<&ToolDefinition> {
    tools.iter().filter(|t| is_exposed_tool(&t.name)).collect()
}

/// `{"type":"control_request",...}` written by the host.
pub fn control_request(request_id: &str, request: Value) -> Value {
    json!({"type": "control_request", "request_id": request_id, "request": request})
}

/// Successful control response.
pub fn control_success(request_id: &str, response: Value) -> Value {
    json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": request_id, "response": response}
    })
}

/// Failed control response.
pub fn control_error(request_id: &str, error: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {"subtype": "error", "request_id": request_id, "error": error}
    })
}

/// `initialize` control request body.
pub fn initialize_request(expose_mcp: bool, append_system_prompt: &str) -> Value {
    let mut request = json!({"subtype": "initialize"});
    if expose_mcp {
        request["sdkMcpServers"] = json!([JCODE_MCP_SERVER]);
    }
    if !append_system_prompt.trim().is_empty() {
        request["appendSystemPrompt"] = json!(append_system_prompt);
    }
    request
}

/// Outcome of a `can_use_tool` request.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionDecision {
    pub allow: bool,
    /// Response body for the control response.
    pub response: Value,
    /// Short reason for the log.
    pub reason: &'static str,
    /// Plan markdown captured from `ExitPlanMode`, shown to the user.
    pub captured_plan: Option<String>,
}

fn allow(input: &Value, reason: &'static str) -> PermissionDecision {
    PermissionDecision {
        allow: true,
        response: json!({"behavior": "allow", "updatedInput": input}),
        reason,
        captured_plan: None,
    }
}

fn deny(message: &str, reason: &'static str) -> PermissionDecision {
    PermissionDecision {
        allow: false,
        response: json!({"behavior": "deny", "message": message}),
        reason,
        captured_plan: None,
    }
}

/// Decide a `can_use_tool` request.
///
/// Claude Code only asks for tools its own settings did not pre-approve.
/// jcode has no interactive approval hook here, so the configured permission
/// mode decides: `dontAsk` and `plan` deny, every other mode allows.
pub fn decide_permission(
    permission_mode: &str,
    tool_name: &str,
    input: &Value,
) -> PermissionDecision {
    if tool_name.starts_with(JCODE_MCP_PREFIX) {
        return allow(input, "jcode tool (jcode applies its own policy)");
    }
    match tool_name {
        "ExitPlanMode" => {
            let mut decision = deny(PLAN_CAPTURED_MESSAGE, "plan captured");
            decision.captured_plan = input
                .get("plan")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|p| !p.trim().is_empty());
            return decision;
        }
        "AskUserQuestion" => return deny(ASK_USER_DENIED_MESSAGE, "interactive question"),
        _ => {}
    }
    match crate::settings::normalize_permission_mode(permission_mode) {
        "dontAsk" => deny(
            &format!("{tool_name} is not pre-approved and permission_mode is dontAsk."),
            "dontAsk mode",
        ),
        "plan" => deny(
            &format!("{tool_name} is not allowed in plan mode. Present a plan instead."),
            "plan mode",
        ),
        "bypassPermissions" => allow(input, "bypassPermissions mode"),
        "acceptEdits" => allow(input, "acceptEdits mode"),
        "auto" => allow(input, "auto mode"),
        _ => allow(input, "default mode (no interactive approval available)"),
    }
}

/// What the driver must do with an SDK MCP message.
#[derive(Debug, Clone, PartialEq)]
pub enum McpAction {
    /// Reply immediately with this JSON-RPC message.
    Reply(Value),
    /// Run a jcode tool, then reply via [`mcp_tool_result`].
    CallTool {
        jsonrpc_id: Value,
        tool_name: String,
        arguments: Value,
    },
}

fn jsonrpc_result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn jsonrpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Handle one JSON-RPC message for the `jcode` SDK MCP server.
pub fn handle_mcp_message(message: &Value, tools: &[ToolDefinition]) -> McpAction {
    let id = message.get("id").cloned().unwrap_or(json!(0));
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-06-18");
            McpAction::Reply(jsonrpc_result(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": JCODE_MCP_SERVER, "version": env!("CARGO_PKG_VERSION")},
                }),
            ))
        }
        m if m.starts_with("notifications/") => McpAction::Reply(jsonrpc_result(&id, json!({}))),
        "ping" => McpAction::Reply(jsonrpc_result(&id, json!({}))),
        "tools/list" => {
            let list: Vec<Value> = exposed_tools(tools)
                .into_iter()
                .map(|tool| {
                    let schema = if tool.input_schema.is_object() {
                        tool.input_schema.clone()
                    } else {
                        json!({"type": "object", "properties": {}})
                    };
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": schema,
                    })
                })
                .collect();
            McpAction::Reply(jsonrpc_result(&id, json!({"tools": list})))
        }
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !exposed_tools(tools).iter().any(|t| t.name == name) {
                return McpAction::Reply(jsonrpc_result(
                    &id,
                    json!({
                        "content": [{"type": "text", "text": format!("Unknown jcode tool: {name}")}],
                        "isError": true,
                    }),
                ));
            }
            McpAction::CallTool {
                jsonrpc_id: id,
                tool_name: name,
                arguments: params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            }
        }
        other => McpAction::Reply(jsonrpc_error(
            &id,
            -32601,
            &format!("Method not found: {other}"),
        )),
    }
}

/// JSON-RPC reply for a finished `tools/call`.
pub fn mcp_tool_result(jsonrpc_id: &Value, text: &str, is_error: bool) -> Value {
    jsonrpc_result(
        jsonrpc_id,
        json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
    )
}

/// Control response body wrapping an MCP reply.
pub fn mcp_response_body(message: Value) -> Value {
    json!({"mcp_response": message})
}

/// Immediate answer for control requests that need no driver state, or
/// `None` for `can_use_tool` and `mcp_message`.
pub fn simple_control_answer(subtype: &str) -> Option<Result<Value, String>> {
    match subtype {
        "elicitation" => Some(Ok(json!({"action": "decline"}))),
        "request_user_dialog" => Some(Ok(json!({"behavior": "cancelled"}))),
        "hook_callback" => Some(Ok(json!({}))),
        "can_use_tool" | "mcp_message" => None,
        other => Some(Err(format!("Unsupported control request: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: format!("{name} tool"),
            input_schema: json!({"type":"object","properties":{"text":{"type":"string"}}}),
            defer_loading: false,
        }
    }

    #[test]
    fn exposes_only_jcode_specific_tools() {
        let tools = vec![
            tool("bash"),
            tool("Read"),
            tool("memory"),
            tool("todo"),
            tool("swarm"),
        ];
        let names: Vec<&str> = exposed_tools(&tools)
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec!["memory", "swarm"]);
    }

    #[test]
    fn mcp_initialize_echoes_protocol_version() {
        let msg = json!({"method":"initialize","params":{"protocolVersion":"2025-11-25"},"jsonrpc":"2.0","id":0});
        let McpAction::Reply(reply) = handle_mcp_message(&msg, &[]) else {
            panic!("expected reply");
        };
        assert_eq!(reply["id"], 0);
        assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(reply["result"]["serverInfo"]["name"], "jcode");
    }

    #[test]
    fn mcp_notifications_and_tools_list() {
        let note = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert!(
            matches!(handle_mcp_message(&note, &[]), McpAction::Reply(r) if r["result"] == json!({}))
        );
        let list = json!({"method":"tools/list","jsonrpc":"2.0","id":1});
        let McpAction::Reply(reply) = handle_mcp_message(&list, &[tool("bash"), tool("echo")])
        else {
            panic!("expected reply");
        };
        let tools = reply["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "echo");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn mcp_tools_call_from_fixture_shape() {
        let call = json!({"method":"tools/call","params":{"name":"echo","arguments":{"text":"MCPOK"},
            "_meta":{"progressToken":2}},"jsonrpc":"2.0","id":2});
        assert_eq!(
            handle_mcp_message(&call, &[tool("echo")]),
            McpAction::CallTool {
                jsonrpc_id: json!(2),
                tool_name: "echo".into(),
                arguments: json!({"text":"MCPOK"}),
            }
        );
        let unknown = json!({"method":"tools/call","params":{"name":"bash"},"id":3});
        assert!(matches!(handle_mcp_message(&unknown, &[tool("bash")]),
            McpAction::Reply(r) if r["result"]["isError"] == true));
        let reply = mcp_tool_result(&json!(2), "MCPOK", false);
        assert_eq!(reply["result"]["content"][0]["text"], "MCPOK");
        assert_eq!(reply["result"]["isError"], false);
    }

    #[test]
    fn unknown_mcp_method_is_jsonrpc_error() {
        let msg = json!({"method":"resources/list","id":4});
        assert!(
            matches!(handle_mcp_message(&msg, &[]), McpAction::Reply(r) if r["error"]["code"] == -32601)
        );
    }

    #[test]
    fn permission_decisions() {
        let input = json!({"file_path":"/tmp/x","content":"y"});
        let d = decide_permission("default", "mcp__jcode__memory", &input);
        assert!(d.allow);
        assert_eq!(d.response["updatedInput"], input);
        assert!(decide_permission("default", "Write", &input).allow);
        assert!(decide_permission("bypassPermissions", "Bash", &input).allow);
        assert!(decide_permission("acceptEdits", "Edit", &input).allow);
        assert!(!decide_permission("dontAsk", "Write", &input).allow);
        assert!(!decide_permission("plan", "Write", &input).allow);
        let plan = decide_permission("default", "ExitPlanMode", &json!({"plan":"1. do it"}));
        assert!(!plan.allow);
        assert_eq!(plan.captured_plan.as_deref(), Some("1. do it"));
        assert_eq!(plan.response["message"], PLAN_CAPTURED_MESSAGE);
        let ask = decide_permission("bypassPermissions", "AskUserQuestion", &json!({}));
        assert!(!ask.allow);
        assert_eq!(ask.response["behavior"], "deny");
    }

    #[test]
    fn simple_answers() {
        assert_eq!(
            simple_control_answer("elicitation"),
            Some(Ok(json!({"action":"decline"})))
        );
        assert_eq!(
            simple_control_answer("request_user_dialog"),
            Some(Ok(json!({"behavior":"cancelled"})))
        );
        assert_eq!(simple_control_answer("hook_callback"), Some(Ok(json!({}))));
        assert!(simple_control_answer("can_use_tool").is_none());
        assert!(matches!(simple_control_answer("bogus"), Some(Err(_))));
    }

    #[test]
    fn control_envelopes() {
        let ok = control_success("r1", json!({"a":1}));
        assert_eq!(ok["response"]["subtype"], "success");
        assert_eq!(ok["response"]["request_id"], "r1");
        let err = control_error("r2", "nope");
        assert_eq!(err["response"]["error"], "nope");
        let init = initialize_request(true, "note");
        assert_eq!(init["sdkMcpServers"], json!(["jcode"]));
        assert_eq!(init["appendSystemPrompt"], "note");
        assert!(initialize_request(false, "").get("sdkMcpServers").is_none());
    }
}
