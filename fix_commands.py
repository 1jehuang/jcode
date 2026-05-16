import re

path = r"d:\studying\Codecargo\CarpAI\src\cli\commands.rs"

with open(path, "r", encoding="utf-8") as f:
    content = f.read()

# 1. Fix Vec<&&str> join issue (line 3748)
content = content.replace(
    "let selected: Vec<&&str> = lines[start_idx..end_idx].iter().collect();",
    "let selected: Vec<&str> = lines[start_idx..end_idx].iter().copied().collect();"
)

# 2. Add Serialize derive to DiffFile struct (line 3980)
content = content.replace(
    "struct DiffFile {",
    "#[derive(Debug, Clone, Serialize)]\nstruct DiffFile {"
)

# 3. Add AsyncReadExt in dap_request function - after AsyncWriteExt import
content = content.replace(
    "    use tokio::io::AsyncWriteExt;\n    let header = format!(\"Content-Length:",
    "    use tokio::io::AsyncWriteExt;\n    use tokio::io::AsyncReadExt;\n    let header = format!(\"Content-Length:"
)

# 4. Add AsyncReadExt in dap_request_internal - after AsyncBufReadExt import
#    "    use tokio::io::AsyncBufReadExt;\n    let mut header_line"
content = content.replace(
    "    use tokio::io::AsyncBufReadExt;\n    let mut header_line = String::new();\n    let mut content_length = 0usize;\n    loop {\n        header_line.clear();\n        if stdout.read_line(&mut header_line).await? == 0 {\n            anyhow::bail!(\"Debug adapter closed connection\");\n        }\n        let trimmed = header_line.trim();\n        if trimmed.is_empty() { break; }\n        if trimmed.to_ascii_lowercase().starts_with(\"content-length:\") {\n            let len_str = trimmed.split(':').nth(1).unwrap_or(\"0\").trim();\n            content_length = len_str.parse().unwrap_or(0);\n        }\n    }\n\n    // Read body\n    let mut body_buf = vec![0u8; content_length];\n    let mut offset = 0;\n    while offset < content_length {\n        let n = stdout.read(&mut body_buf[offset..]).await?;",
    "    use tokio::io::AsyncBufReadExt;\n    use tokio::io::AsyncReadExt;\n    let mut header_line = String::new();\n    let mut content_length = 0usize;\n    loop {\n        header_line.clear();\n        if stdout.read_line(&mut header_line).await? == 0 {\n            anyhow::bail!(\"Debug adapter closed connection\");\n        }\n        let trimmed = header_line.trim();\n        if trimmed.is_empty() { break; }\n        if trimmed.to_ascii_lowercase().starts_with(\"content-length:\") {\n            let len_str = trimmed.split(':').nth(1).unwrap_or(\"0\").trim();\n            content_length = len_str.parse().unwrap_or(0);\n        }\n    }\n\n    // Read body\n    let mut body_buf = vec![0u8; content_length];\n    let mut offset = 0;\n    while offset < content_length {\n        let n = stdout.read(&mut body_buf[offset..]).await?;"
)

# 5. Add AsyncReadExt in poll_dap_event function
#    At line 4291: "    use tokio::io::AsyncBufReadExt;"
#    This is in a different function
content = content.replace(
    "    use tokio::io::AsyncBufReadExt;\n    // Try to read a header line without blocking",
    "    use tokio::io::AsyncBufReadExt;\n    use tokio::io::AsyncReadExt;\n    // Try to read a header line without blocking"
)

# 6-12. Fix Result indexing: add ? after .await on dap_request_internal calls
# 6. DebugCommand::Stack - line 4643-4645
content = content.replace(
    'let resp = dap_request_internal(session, "stackTrace",\n                        Some(serde_json::json!({ "threadId": tid, "levels": 20 })),\n                    ).await;',
    'let resp = dap_request_internal(session, "stackTrace",\n                        Some(serde_json::json!({ "threadId": tid, "levels": 20 })),\n                    ).await?;'
)

# 7. DebugCommand::Variables - stack (line 4676-4678)
content = content.replace(
    'let stack = dap_request_internal(session, "stackTrace",\n                        Some(serde_json::json!({ "threadId": tid, "levels": 1 })),\n                    ).await;',
    'let stack = dap_request_internal(session, "stackTrace",\n                        Some(serde_json::json!({ "threadId": tid, "levels": 1 })),\n                    ).await?;'
)

# 8. DebugCommand::Variables - vars (line 4682-4684)
content = content.replace(
    'let vars = dap_request_internal(session, "scopes",\n                            Some(serde_json::json!({ "frameId": frame_id })),\n                        ).await;',
    'let vars = dap_request_internal(session, "scopes",\n                            Some(serde_json::json!({ "frameId": frame_id })),\n                        ).await?;'
)

# 9. DebugCommand::Variables - variable_response (line 4692-4694)
content = content.replace(
    'let variable_response = dap_request_internal(session, "variables",\n                                            Some(serde_json::json!({ "variablesReference": var_ref })),\n                                        ).await;',
    'let variable_response = dap_request_internal(session, "variables",\n                                            Some(serde_json::json!({ "variablesReference": var_ref })),\n                                        ).await?;'
)

# 10. DebugCommand::Evaluate - stack (line 4727-4729)
content = content.replace(
    'let stack = dap_request_internal(session, "stackTrace",\n                        Some(serde_json::json!({ "threadId": session.active_thread_id, "levels": 1 })),\n                    ).await;',
    'let stack = dap_request_internal(session, "stackTrace",\n                        Some(serde_json::json!({ "threadId": session.active_thread_id, "levels": 1 })),\n                    ).await?;'
)

# 11. DebugCommand::Evaluate - resp (line 4732-4738)
content = content.replace(
    'let resp = dap_request_internal(session, "evaluate",\n                        Some(serde_json::json!({\n                            "expression": expression,\n                            "frameId": frame_id,\n                            "context": "repl",\n                        })),\n                    ).await;',
    'let resp = dap_request_internal(session, "evaluate",\n                        Some(serde_json::json!({\n                            "expression": expression,\n                            "frameId": frame_id,\n                            "context": "repl",\n                        })),\n                    ).await?;'
)

# 12. DebugCommand::Modules - resp (line 4800-4801)
content = content.replace(
    'let resp = dap_request_internal(session, "modules", None,\n                    ).await;',
    'let resp = dap_request_internal(session, "modules", None,\n                    ).await?;'
)

# 13. DebugCommand::Threads - resp (line 4824-4825)
content = content.replace(
    'let resp = dap_request_internal(session, "threads", None,\n                    ).await;',
    'let resp = dap_request_internal(session, "threads", None,\n                    ).await?;'
)

# 14. DebugCommand::Stop - change Some(mut session) to Some(ref mut session) (line 4924)
content = content.replace(
    'if let Some(mut session) = guard.take() {\n                // Send disconnect request',
    'if let Some(ref mut session) = guard.take() {\n                // Send disconnect request'
)

# 15. Fix get() on resp Result in Evaluate - line 4741
#     "resp.get(\"success\")" - but resp is now Result after ?. Wait, I already fixed with ? above.
#     So get() should work since resp is now Value, not Result.

with open(path, "w", encoding="utf-8") as f:
    f.write(content)

print("Done! Fixed cli/commands.rs")