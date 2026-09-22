use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};

fn append_log(value: &Value) {
    let path = std::env::var("JCODE_FAKE_GROK_ACP_LOG").expect("fake log path");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open fake log");
    writeln!(file, "{value}").expect("write fake log");
}

fn send(value: Value) {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    writeln!(stdout, "{value}").expect("write response");
    stdout.flush().expect("flush response");
}

fn response(id: Value, result: Value) {
    send(json!({"jsonrpc":"2.0", "id":id, "result":result}));
}

fn tool_update(
    session_update: &str,
    id: &str,
    status: &str,
    old: &str,
    new: &str,
    path: Option<&str>,
) -> Value {
    json!({
        "jsonrpc":"2.0",
        "method":"session/update",
        "params":{
            "sessionId":"fake-session-new",
            "update":{
                "sessionUpdate": session_update,
                "toolCallId": id,
                "status": status,
                "title": "search_replace",
                "rawInput":{
                    "file_path": path.unwrap_or("src/lib.rs"),
                    "old_string": old,
                    "new_string": new
                },
                "_meta":{
                    "x.ai/tool":{
                        "name":"search_replace",
                        "kind":"edit",
                        "label":"Edit"
                    }
                }
            }
        }
    })
}

fn main() {
    let stdin = std::io::stdin();
    for line in BufReader::new(stdin.lock()).lines() {
        let line = line.expect("read request");
        let value: Value = serde_json::from_str(&line).expect("valid JSON-RPC request");
        append_log(&value);
        let Some(method) = value.get("method").and_then(Value::as_str) else {
            continue;
        };
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => response(
                id,
                json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {
                        "loadSession": true,
                        "sessionCapabilities": {"resume": {}}
                    },
                    "authMethods": [
                        {"id":"xai.api_key", "name":"xAI API key"},
                        {"id":"grok.com", "name":"Grok.com"},
                        {"id":"cached_token", "name":"Cached token"}
                    ],
                    "agentInfo": {"name":"fake-grok", "version":"1.0.0"},
                    "_meta": {
                        "modelState": {
                            "currentModelId":"grok-4.5",
                            "availableModels":[
                                {"modelId":"grok-4.5", "name":"Grok 4.5"},
                                {"modelId":"grok-code-fast-1", "name":"Grok Code Fast"}
                            ]
                        }
                    }
                }),
            ),
            "authenticate" => response(id, json!({})),
            "session/new" => response(
                id,
                json!({
                    "sessionId":"fake-session-new",
                    "models": {
                        "currentModelId":"grok-4.5",
                        "availableModels":[
                            {"modelId":"grok-4.5", "name":"Grok 4.5"},
                            {"modelId":"grok-code-fast-1", "name":"Grok Code Fast"}
                        ]
                    }
                }),
            ),
            "session/resume" => {
                if let Ok(kind) = std::env::var("JCODE_FAKE_GROK_ACP_RESUME_ERROR") {
                    let error = match kind.as_str() {
                        "missing" => json!({
                            "code": -32000,
                            "message": "Path not found.",
                            "data": {
                                "code": "FS_NOT_FOUND",
                                "detail": "No such file or directory (os error 2)"
                            }
                        }),
                        "auth" => json!({
                            "code": -32000,
                            "message": "authentication failed",
                            "data": {"code": "UNAUTHENTICATED"}
                        }),
                        "transport" => json!({
                            "code": -32603,
                            "message": "connection reset by peer"
                        }),
                        other => panic!("unknown resume error kind: {other}"),
                    };
                    send(json!({"jsonrpc":"2.0", "id": id, "error": error}));
                    continue;
                }
                response(
                    id,
                    json!({
                        "models": {
                            "currentModelId":"grok-code-fast-1",
                            "availableModels":[
                                {"modelId":"grok-4.5", "name":"Grok 4.5"},
                                {"modelId":"grok-code-fast-1", "name":"Grok Code Fast"}
                            ]
                        }
                    }),
                );
            }
            "session/set_model" => response(id, json!({})),
            "session/prompt" => {
                if std::env::var_os("JCODE_FAKE_GROK_ACP_HANG").is_some() {
                    continue;
                }
                if std::env::var_os("JCODE_FAKE_GROK_ACP_PAYMENT_REQUIRED").is_some() {
                    eprintln!(
                        "Error: Internal error: {{\"message\":\"API error (status 402 Payment Required): Grok Build usage balance exhausted\",\"http_status\":402}}"
                    );
                    response(id, json!({"stopReason":"end_turn"}));
                    continue;
                }
                send(json!({
                    "jsonrpc":"2.0",
                    "method":"_x.ai/settings/update",
                    "params":{"ignored":true}
                }));
                send(json!({
                    "jsonrpc":"2.0",
                    "method":"session/update",
                    "params":{
                        "sessionId":"fake-session-new",
                        "update":{
                            "sessionUpdate":"agent_thought_chunk",
                            "content":{"type":"text", "text":"thinking"}
                        }
                    }
                }));
                if std::env::var_os("JCODE_FAKE_GROK_ACP_EDIT_STAGES").is_some() {
                    send(tool_update(
                        "tool_call",
                        "edit-1",
                        "in_progress",
                        "fn old() {}\n",
                        "fn mid() {}\n",
                        None,
                    ));
                    send(tool_update(
                        "tool_call_update",
                        "edit-1",
                        "in_progress",
                        "fn old() {}\n",
                        "fn new() {}\n",
                        None,
                    ));
                    send(tool_update(
                        "tool_call_update",
                        "edit-1",
                        "completed",
                        "fn old() {}\n",
                        "fn new() {}\n",
                        None,
                    ));
                } else if std::env::var_os("JCODE_FAKE_GROK_ACP_EDIT_FAIL").is_some() {
                    send(tool_update(
                        "tool_call",
                        "edit-fail",
                        "in_progress",
                        "fn old() {}\n",
                        "fn new() {}\n",
                        None,
                    ));
                    send(json!({
                        "jsonrpc":"2.0",
                        "method":"session/update",
                        "params":{
                            "sessionId":"fake-session-new",
                            "update":{
                                "sessionUpdate":"tool_call_update",
                                "toolCallId":"edit-fail",
                                "status":"failed",
                                "content":[{
                                    "type":"content",
                                    "content":{"type":"text", "text":"write failed"}
                                }]
                            }
                        }
                    }));
                } else if std::env::var_os("JCODE_FAKE_GROK_ACP_WRITE").is_some() {
                    send(tool_update(
                        "tool_call",
                        "write-1",
                        "completed",
                        "",
                        "fn created() {}\n",
                        Some("src/new.rs"),
                    ));
                } else if std::env::var_os("JCODE_FAKE_GROK_ACP_EDIT").is_some() {
                    send(tool_update(
                        "tool_call",
                        "edit-1",
                        "completed",
                        "fn old() {}\n",
                        "fn new() {}\n",
                        None,
                    ));
                }
                send(json!({
                    "jsonrpc":"2.0",
                    "method":"session/update",
                    "params":{
                        "sessionId":"fake-session-new",
                        "update":{
                            "sessionUpdate":"agent_message_chunk",
                            "content":{"type":"text", "text":"AUTH_TEST_OK"}
                        }
                    }
                }));
                response(id, json!({"stopReason":"end_turn"}));
            }
            "session/cancel" => break,
            other => panic!("unexpected ACP method: {other}"),
        }
    }
}
