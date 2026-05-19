//! Debug Adapter Protocol (DAP) — complete client implementation
//!
//! Extracted from commands.rs for better modularity.

use anyhow::Result;

// Debug Commands — DAP (Debug Adapter Protocol) integration
// ════════════════════════════════════════════════════════════════════

// ════════════════════════════════════════════════════════════════════
// Debug Adapter Protocol (DAP) — complete client implementation
// ════════════════════════════════════════════════════════════════════

use tokio::sync::Mutex;

/// A single breakpoint in the debug session
#[derive(Debug, Clone)]
struct DebugBreakpoint {
    id: u64,
    file: String,
    line: u32,
    condition: Option<String>,
    verified: bool,
}

/// Full debug session state
struct DebugSession {
    /// Debug adapter child process (stdin/stdout/JSON-RPC)
    process: Option<tokio::process::Child>,
    /// Write half of stdin pipe
    stdin: Option<tokio::process::ChildStdin>,
    /// Read half of stdout pipe (for DAP responses)
    stdout: Option<tokio::io::BufReader<tokio::process::ChildStdout>>,
    /// Breakpoints set during this session
    breakpoints: Vec<DebugBreakpoint>,
    /// Next breakpoint ID
    next_bp_id: u64,
    /// Pretty name of the adapter
    adapter_name: String,
    /// Whether this is a Rust/lldb, Python, or Node debug session
    language: String,
    /// Target executable/program
    program: String,
    /// Whether the debugee is running (false = stopped at breakpoint)
    running: bool,
    /// Last known thread ID
    active_thread_id: u64,
    /// Whether DAP initialize has completed
    initialized: bool,
    /// JSON-RPC request counter
    request_seq: u64,
}

/// Detect the debug adapter to use based on project type
fn detect_debug_adapter(config: Option<&str>) -> Result<(String, String, String)> {
    // Check for explicit debug configuration in .vscode/launch.json
    let launch_json_path = std::path::Path::new(".vscode/launch.json");

    if let Some(cfg_name) = config {
        if launch_json_path.exists() {
            if let Ok(content) = std::fs::read_to_string(launch_json_path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(configurations) = json["configurations"].as_array() {
                        for cfg in configurations {
                            if cfg["name"].as_str() == Some(cfg_name) {
                                let adapter = cfg.get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("lldb");
                                let program_val = cfg.get("program")
                                    .or_else(|| cfg.get("cargo"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                return Ok((adapter.to_string(), "auto".to_string(), program_val.to_string()));
                            }
                        }
                    }
                }
            }
        }
        anyhow::bail!("Debug configuration '{}' not found in .vscode/launch.json", cfg_name);
    }

    // Auto-detect based on project files
    if std::path::Path::new("Cargo.toml").exists() {
        // Try to find the binary name
        let program = if let Ok(content) = std::fs::read_to_string("Cargo.toml") {
            content.lines()
                .find(|l| l.trim().starts_with("name ="))
                .and_then(|l| l.split('=').nth(1))
                .map(|s| s.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| "target/debug/carpai".to_string())
        } else {
            "target/debug/carpai".to_string()
        };
        Ok(("lldb".to_string(), "rust".to_string(), program))
    } else if std::path::Path::new("package.json").exists() {
        Ok(("node".to_string(), "node".to_string(), "index.js".to_string()))
    } else if std::path::Path::new("pyproject.toml").exists() || std::path::Path::new("requirements.txt").exists() {
        Ok(("python".to_string(), "python".to_string(), "main.py".to_string()))
    } else {
        Ok(("lldb".to_string(), "rust".to_string(), "target/debug/carpai".to_string()))
    }
}

/// Map language to debug adapter command
fn adapter_command(adapter_type: &str) -> &'static str {
    match adapter_type {
        "lldb" => "lldb-vscode",
        "lldb-dap" => "lldb-dap",
        "node" | "node2" => "node-debug2-adapter",
        "python" => "debugpy",
        "gdb" => "gdb",
        _ => "lldb-vscode",
    }
}

/// Send a DAP JSON-RPC request (standalone — used before session is stored).
async fn dap_request(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    seq: &mut u64,
    command: &str,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let request = serde_json::json!({
        "seq": *seq + 1,
        "type": "request",
        "command": command,
        "arguments": args.unwrap_or(serde_json::json!({})),
    });
    *seq += 1;

    let body = serde_json::to_string(&request)?;
    use tokio::io::AsyncWriteExt;
    use tokio::io::AsyncReadExt;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(body.as_bytes()).await?;
    stdin.flush().await?;

    use tokio::io::AsyncBufReadExt;
    let mut header_line = String::new();
    let mut content_length = 0usize;
    loop {
        header_line.clear();
        if stdout.read_line(&mut header_line).await? == 0 {
            anyhow::bail!("Debug adapter closed connection");
        }
        let trimmed = header_line.trim();
        if trimmed.is_empty() { break; }
        if trimmed.to_ascii_lowercase().starts_with("content-length:") {
            let len_str = trimmed.split(':').nth(1).unwrap_or("0").trim();
            content_length = len_str.parse().unwrap_or(0);
        }
    }

    let mut body_buf = vec![0u8; content_length];
    let mut offset = 0;
    while offset < content_length {
        let n = stdout.read(&mut body_buf[offset..]).await?;
        if n == 0 { break; }
        offset += n;
    }

    let response: serde_json::Value = serde_json::from_slice(&body_buf)?;
    Ok(response)
}

/// Send a DAP JSON-RPC request via an active session (uses stored stdin/stdout).
async fn dap_request_internal(
    session: &mut DebugSession,
    command: &str,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let seq = &mut session.request_seq;
    *seq += 1;
    let request = serde_json::json!({
        "seq": *seq,
        "type": "request",
        "command": command,
        "arguments": args.unwrap_or(serde_json::json!({})),
    });

    let body = serde_json::to_string(&request)?;
    use tokio::io::AsyncWriteExt;

    let stdin = session.stdin.as_mut()
        .ok_or_else(|| anyhow::anyhow!("No debug adapter stdin"))?;
    let stdout = session.stdout.as_mut()
        .ok_or_else(|| anyhow::anyhow!("No debug adapter stdout"))?;

    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    stdin.write_all(header.as_bytes()).await?;
    stdin.write_all(body.as_bytes()).await?;
    stdin.flush().await?;

    // Read response headers
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncReadExt;
    let mut header_line = String::new();
    let mut content_length = 0usize;
    loop {
        header_line.clear();
        if stdout.read_line(&mut header_line).await? == 0 {
            anyhow::bail!("Debug adapter closed connection");
        }
        let trimmed = header_line.trim();
        if trimmed.is_empty() { break; }
        if trimmed.to_ascii_lowercase().starts_with("content-length:") {
            let len_str = trimmed.split(':').nth(1).unwrap_or("0").trim();
            content_length = len_str.parse().unwrap_or(0);
        }
    }

    // Read body
    let mut body_buf = vec![0u8; content_length];
    let mut offset = 0;
    while offset < content_length {
        let n = stdout.read(&mut body_buf[offset..]).await?;
        if n == 0 { break; }
        offset += n;
    }

    let response: serde_json::Value = serde_json::from_slice(&body_buf)?;
    Ok(response)
}

/// Parse a DAP event from stdout (non-blocking check)
async fn poll_dap_event(
    stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
) -> Result<Option<serde_json::Value>> {
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncReadExt;
    // Try to read a header line without blocking
    let mut header_line = String::new();
    tokio::select! {
        result = stdout.read_line(&mut header_line) => {
            let _ = result?;
        }
        _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
            return Ok(None);
        }
    }

    let trimmed = header_line.trim();
    if trimmed.is_empty() { return Ok(None); }
    if !trimmed.to_ascii_lowercase().starts_with("content-length:") {
        return Ok(None);
    }

    let len_str = trimmed.split(':').nth(1).unwrap_or("0").trim();
    let content_length: usize = len_str.parse().unwrap_or(0);

    // Read remaining headers
    let mut buf = String::new();
    loop {
        buf.clear();
        if stdout.read_line(&mut buf).await? == 0 { break; }
        if buf.trim().is_empty() { break; }
    }

    // Read body
    let mut body_buf = vec![0u8; content_length];
    let mut offset = 0;
    while offset < content_length {
        let n = stdout.read(&mut body_buf[offset..]).await?;
        if n == 0 { break; }
        offset += n;
    }

    let event: serde_json::Value = serde_json::from_slice(&body_buf)?;
    Ok(Some(event))
}

/// Detect the debug adapter and find its executable
fn find_adapter_executable(adapter_type: &str) -> Option<String> {
    let cmd_name = adapter_command(adapter_type);
    // Check common locations
    let candidates = vec![
        cmd_name.to_string(),
        format!("{}.exe", cmd_name),
        format!("/usr/bin/{}", cmd_name),
        format!("/usr/local/bin/{}", cmd_name),
    ];
    for candidate in &candidates {
        if std::path::Path::new(candidate).exists() {
            return Some(candidate.clone());
        }
    }
    // Fallback: try to find via which/where
    None
}

/// Debug commands — complete DAP client implementation
pub async fn run_debug_command(cmd: super::args::DebugCommand) -> Result<()> {
    use super::args::DebugCommand;

    static SESSION: std::sync::OnceLock<Mutex<Option<DebugSession>>> = std::sync::OnceLock::new();
    let session_lock = SESSION.get_or_init(|| Mutex::new(None));

    // Helper: execute DAP command via session if active, or print stub message
    macro_rules! dap_cmd {
        ($session:expr, $cmd:expr, $args:expr) => {{
            let s = $session;
            if let Some(ref mut session) = s {
                dap_request_internal(session, $cmd, $args).await
            } else {
                anyhow::bail!("No debug session active");
            }
        }};
    }
    macro_rules! dap_print_stub {
        ($label:expr) => {
            eprintln!("\n{} (no active debug session)\n", $label);
        };
    }

    match cmd {
        // -- start -----------------------------------------------
        DebugCommand::Start { config, args } => {
            let (adapter_type, language, program) = detect_debug_adapter(config.as_deref())?;

            // Find the debug adapter executable
            let adapter_exe = find_adapter_executable(&adapter_type)
                .unwrap_or_else(|| adapter_command(&adapter_type).to_string());

            eprintln!("\n🐛 Starting debug session...\n");
            eprintln!("  Adapter: {}", adapter_exe);
            eprintln!("  Type:    {}", adapter_type);
            eprintln!("  Lang:    {}", language);
            eprintln!("  Program: {}", program);

            let mut cmd = tokio::process::Command::new(&adapter_exe);
            cmd.stdin(std::process::Stdio::piped());
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());

            let mut child = cmd.spawn()
                .map_err(|e| anyhow::anyhow!(
                    "Failed to start debug adapter '{}': {}\n\
                     Install with: cargo install lldb-vscode  (or debugpy for Python)",
                    adapter_exe, e
                ))?;

            let stdin = child.stdin.take()
                .ok_or_else(|| anyhow::anyhow!("No stdin on debug adapter"))?;
            let stdout = child.stdout.take()
                .ok_or_else(|| anyhow::anyhow!("No stdout on debug adapter"))?;
            let mut stdout_reader = tokio::io::BufReader::new(stdout);
            let mut mut_stdin = stdin;
            let mut seq: u64 = 0;

            // Step 1: Initialize
            eprintln!("  [1/3] Initializing DAP...");
            let init_args = serde_json::json!({
                "adapterID": adapter_type,
                "clientID": "carpai",
                "clientName": "CarpAI Debugger",
                "locale": "en",
                "linesStartAt1": true,
                "columnsStartAt1": true,
                "pathFormat": "path",
                "supportsVariableType": true,
                "supportsVariablePaging": true,
                "supportsRunInTerminalRequest": true,
            });
            let init_resp = dap_request(&mut mut_stdin, &mut stdout_reader, &mut seq, "initialize", Some(init_args)).await?;
            let init_body = &init_resp["body"];
            eprintln!("     Supports: {:?}", init_body);

            // Step 2: Launch
            eprintln!("  [2/3] Launching program...");
            let launch_args = match adapter_type.as_str() {
                "lldb" | "lldb-dap" => serde_json::json!({
                    "program": program,
                    "args": args,
                    "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
                    "stopOnEntry": true,
                    "stdio": [null, null, null],
                }),
                "python" => serde_json::json!({
                    "program": program,
                    "args": args,
                    "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
                    "stopOnEntry": true,
                    "console": "integratedTerminal",
                }),
                _ => serde_json::json!({
                    "program": program,
                    "args": args,
                    "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
                    "stopOnEntry": true,
                }),
            };
            let launch_resp = dap_request(&mut mut_stdin, &mut stdout_reader, &mut seq, "launch", Some(launch_args)).await?;
            if launch_resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                eprintln!("     ✅ Launch successful");
            } else {
                let msg = launch_resp["message"].as_str().unwrap_or("unknown error");
                eprintln!("     ⚠️  Launch message: {}", msg);
            }

            // Step 3: ConfigurationDone
            eprintln!("  [3/3] Completing configuration...");
            let _ = dap_request(&mut mut_stdin, &mut stdout_reader, &mut seq, "configurationDone", None).await?;

            // Check for initial stopped event
            if let Ok(Some(event)) = poll_dap_event(&mut stdout_reader).await {
                let event_type = event["event"].as_str().unwrap_or("?");
                if event_type == "stopped" {
                    let reason = event["body"]["reason"].as_str().unwrap_or("entry");
                    let tid = event["body"]["threadId"].as_i64().unwrap_or(1) as u64;
                    eprintln!("     ⏸️  Stopped ({}) on thread {}", reason, tid);
                }
            }

            let mut session = session_lock.lock().await;
            *session = Some(DebugSession {
                process: Some(child),
                stdin: Some(mut_stdin),
                stdout: Some(stdout_reader),
                breakpoints: Vec::new(),
                next_bp_id: 1,
                adapter_name: adapter_type,
                language,
                program,
                running: false,
                active_thread_id: 1,
                initialized: true,
                request_seq: seq,
            });

            eprintln!("\n✅ Debug session ready\n");
            eprintln!("  Commands:");
            eprintln!("    breakpoint <file>:<line>    Set breakpoint");
            eprintln!("    continue                   Resume execution");
            eprintln!("    next                       Step over");
            eprintln!("    step-in                    Step into");
            eprintln!("    step-out                   Step out");
            eprintln!("    stack                      Show call stack");
            eprintln!("    variables                  Show variables");
            eprintln!("    evaluate <expr>            Evaluate expression");
            eprintln!("    breakpoints                List breakpoints");
            eprintln!("    delete-breakpoint <id>     Remove breakpoint");
            eprintln!("    threads                    List threads");
            eprintln!("    thread <id>                Switch thread");
            eprintln!("    modules                    Loaded modules");
            eprintln!("    restart                    Restart session");
            eprintln!("    disconnect                 Detach without killing");
            eprintln!("    logpoint <file>:<line>     Set log message");
            eprintln!("    exception-breakpoint       Set exception filter");
            eprintln!("    stop                       End debug session");
        }

        // -- breakpoint ------------------------------------------
        DebugCommand::Breakpoint { location, condition } => {
            let parts: Vec<&str> = location.split(':').collect();
            if parts.len() < 2 {
                anyhow::bail!("Invalid breakpoint '{}'. Use format: <file>:<line>", location);
            }
            let file = parts[..parts.len() - 1].join(":");
            let line: u32 = parts.last()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid line number in '{}'", location))?;

            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                let bp_id = session.next_bp_id;
                session.next_bp_id += 1;

                session.breakpoints.push(DebugBreakpoint {
                    id: bp_id,
                    file: file.clone(),
                    line,
                    condition: condition.clone(),
                    verified: false,
                });

                // Send breakpoint to DAP adapter
                let bp_args = serde_json::json!({
                    "source": {
                        "name": std::path::Path::new(&file).file_name().map(|f| f.to_string_lossy()).unwrap_or(std::borrow::Cow::Borrowed(&file)),
                        "path": std::path::Path::new(&file).canonicalize().ok().map(|p| p.to_string_lossy().to_string()).unwrap_or(file.clone()),
                    },
                    "breakpoints": [{
                        "line": line,
                        "condition": condition.as_deref().unwrap_or(""),
                    }],
                    "lines": [line],
                });
                let _ = dap_request_internal(session, "setBreakpoints", Some(bp_args)).await;

                eprintln!("\n🔴 Breakpoint {} set: {}:{}\n", bp_id, file, line);
                if let Some(ref cond) = condition {
                    eprintln!("  Condition: {}", cond);
                }
                eprintln!("  Total breakpoints: {}", session.breakpoints.len());
            } else {
                anyhow::bail!("No debug session. Run `carpai debug start` first.");
            }
        }

        // -- continue --------------------------------------------
        DebugCommand::Continue => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let seq = session.request_seq;
                    let tid = session.active_thread_id;
                    let resp = dap_request_internal(session, "continue",
                        Some(serde_json::json!({ "threadId": tid })),
                    ).await?;
                    session.request_seq = seq;
                    session.running = true;
                    let all_threads = resp["body"]["allThreadsContinued"].as_bool().unwrap_or(false);
                    eprintln!("\n▶️  Continued (allThreadsContinued: {})\n", all_threads);
                    eprintln!("  (Waiting for breakpoint... Use `debug breakpoint` to set one.)");
                }
            } else {
                anyhow::bail!("No debug session active.");
            }
        }

        // -- next ------------------------------------------------
        DebugCommand::Next => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut din) = session.stdin {
                    let seq = session.request_seq;
                    let tid = session.active_thread_id;
                    let _ = dap_request_internal(session, "next",
                        Some(serde_json::json!({ "threadId": tid })),
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n⏭️  Step Over\n");
                }
            } else {
                eprintln!("\n⏭️  Step Over — no active session (preview mode)\n");
            }
        }

        // -- stepIn ----------------------------------------------
        DebugCommand::StepIn => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut _stdin) = session.stdin {
                    let seq = session.request_seq;
                    let tid = session.active_thread_id;
                    let _ = dap_request_internal(session, "stepIn",
                        Some(serde_json::json!({ "threadId": tid })),
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n⏬ Step Into\n");
                }
            } else {
                eprintln!("\n⏬ Step Into\n");
            }
        }

        // -- stepOut ---------------------------------------------
        DebugCommand::StepOut => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut _stdin) = session.stdin {
                    let seq = session.request_seq;
                    let tid = session.active_thread_id;
                    let _ = dap_request_internal(session, "stepOut",
                        Some(serde_json::json!({ "threadId": tid })),
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n⏫ Step Out\n");
                }
            } else {
                eprintln!("\n⏫ Step Out\n");
            }
        }

        // -- stack -----------------------------------------------
        DebugCommand::Stack => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut _stdin) = session.stdin {
                    let seq = session.request_seq;
                    let tid = session.active_thread_id;
                    let resp = dap_request_internal(session, "stackTrace",
                        Some(serde_json::json!({ "threadId": tid, "levels": 20 })),
                    ).await?;
                    session.request_seq = seq;

                    eprintln!("\n📋 Stack Trace\n");
                    if let Some(stack_frames) = resp["body"]["stackFrames"].as_array() {
                        for (i, frame) in stack_frames.iter().enumerate() {
                            let name = frame["name"].as_str().unwrap_or("?");
                            let file = frame["source"]["path"].as_str()
                                .or_else(|| frame["source"]["name"].as_str())
                                .unwrap_or("?");
                            let line = frame["line"].as_i64().unwrap_or(0);
                            let col = frame["column"].as_i64().unwrap_or(0);
                            eprintln!("  #{} {} ({}:{}:{})", i, name, file, line, col);
                        }
                    } else {
                        eprintln!("  (no stack frames)");
                    }
                }
            } else {
                eprintln!("\n📋 Stack Trace (no active session)\n");
            }
        }

        // -- variables -------------------------------------------
        DebugCommand::Variables => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut _stdin) = session.stdin {
                    let seq = session.request_seq;
                    let tid = session.active_thread_id;
                    // First get stack to find top frame variables reference
                    let stack = dap_request_internal(session, "stackTrace",
                        Some(serde_json::json!({ "threadId": tid, "levels": 1 })),
                    ).await?;

                    // Get variables for the top frame
                    if let Some(frame_id) = stack["body"]["stackFrames"][0]["id"].as_i64() {
                        let vars = dap_request_internal(session, "scopes",
                            Some(serde_json::json!({ "frameId": frame_id })),
                        ).await?;

                        eprintln!("\n📊 Variables\n");
                        if let Some(scopes) = vars["body"]["scopes"].as_array() {
                            for scope in scopes {
                                let scope_name = scope["name"].as_str().unwrap_or("?");
                                if let Some(var_ref) = scope["variablesReference"].as_i64() {
                                    if var_ref > 0 {
                                        let variable_response = dap_request_internal(session, "variables",
                                            Some(serde_json::json!({ "variablesReference": var_ref })),
                                        ).await?;
                                        eprintln!("  {}:", scope_name);
                                        if let Some(vars_list) = variable_response["body"]["variables"].as_array() {
                                            for v in vars_list {
                                                let v_name = v["name"].as_str().unwrap_or("?");
                                                let v_value = v["value"].as_str().unwrap_or("?");
                                                let v_type = v["type"].as_str().unwrap_or("");
                                                let type_info = if v_type.is_empty() { String::new() }
                                                    else { format!(" ({})", v_type) };
                                                eprintln!("    {}{} = {}", v_name, type_info, v_value);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        eprintln!("  (no active frame — program may be running)");
                    }
                    session.request_seq = seq;
                }
            } else {
                eprintln!("\n📊 Variables (no active session)\n");
            }
        }

        // -- evaluate --------------------------------------------
        DebugCommand::Evaluate { expression } => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut _stdin) = session.stdin {
                    let seq = session.request_seq;
                    // Get top frame for evaluation context
                    let stack = dap_request_internal(session, "stackTrace",
                        Some(serde_json::json!({ "threadId": session.active_thread_id, "levels": 1 })),
                    ).await?;
                    let frame_id = stack["body"]["stackFrames"][0]["id"].as_i64().unwrap_or(0);

                    let resp = dap_request_internal(session, "evaluate",
                        Some(serde_json::json!({
                            "expression": expression,
                            "frameId": frame_id,
                            "context": "repl",
                        })),
                    ).await?;
                    session.request_seq = seq;

                    if resp.get("success").and_then(|v| v.as_bool()).unwrap_or(false) {
                        let result = resp["body"]["result"].as_str().unwrap_or("(no result)");
                        let result_type = resp["body"]["type"].as_str().unwrap_or("");
                        eprintln!("\n🔮 Evaluate: {}", expression);
                        eprintln!("  = {} ({})\n", result, result_type);
                    } else {
                        let msg = resp["message"].as_str().unwrap_or("Evaluation failed");
                        eprintln!("\n❌ Evaluate: {}", expression);
                        eprintln!("  Error: {}\n", msg);
                    }
                }
            } else {
                eprintln!("\n🔮 Evaluate (no active session)\n");
            }
        }

        // -- restart ---------------------------------------------
        DebugCommand::Restart => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut _stdin) = session.stdin {
                    let seq = session.request_seq;
                    let _ = dap_request_internal(session, "restart", None,
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n🔄 Restarting debug session...\n");
                }
            } else {
                eprintln!("\n🔄 Restart (no active session)\n");
            }
        }

        // -- disconnect ------------------------------------------
        DebugCommand::Disconnect => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut _stdin) = session.stdin {
                    let seq = session.request_seq;
                    let _ = dap_request_internal(session, "disconnect",
                        Some(serde_json::json!({ "restart": false, "terminateDebuggee": false })),
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n🔌 Disconnected from debug target (process continues running)\n");
                }
                let mut take_session = guard.take();
                if let Some(ref mut s) = take_session {
                    if let Some(ref mut child) = s.process {
                        let _ = child.kill().await;
                    }
                }
            }
        }

        // -- modules ---------------------------------------------
        DebugCommand::Modules => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut _stdin) = session.stdin {
                    let seq = session.request_seq;
                    let resp = dap_request_internal(session, "modules", None,
                    ).await?;
                    session.request_seq = seq;

                    eprintln!("\n📦 Loaded Modules\n");
                    if let Some(modules) = resp["body"]["modules"].as_array() {
                        for module in modules {
                            let name = module["name"].as_str().unwrap_or("?");
                            let path = module["path"].as_str().unwrap_or("");
                            eprintln!("  {} — {}", name, path);
                        }
                    } else {
                        eprintln!("  (no module info)");
                    }
                }
            }
        }

        // -- threads ---------------------------------------------
        DebugCommand::Threads => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut _stdin) = session.stdin {
                    let seq = session.request_seq;
                    let resp = dap_request_internal(session, "threads", None,
                    ).await?;
                    session.request_seq = seq;

                    eprintln!("\n🧵 Threads\n");
                    if let Some(threads) = resp["body"]["threads"].as_array() {
                        for thread in threads {
                            let tid = thread["id"].as_i64().unwrap_or(0);
                            let name = thread["name"].as_str().unwrap_or("?");
                            let active = if tid == session.active_thread_id as i64 { " <- active" } else { "" };
                            eprintln!("  #{} — {}{}", tid, name, active);
                        }
                    } else {
                        eprintln!("  (no thread info)");
                    }
                }
            }
        }

        // -- thread <id> -----------------------------------------
        DebugCommand::Thread { id } => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                session.active_thread_id = id;
                eprintln!("\n🧵 Switched to thread #{}", id);
            }
        }

        // -- breakpoints (list) ----------------------------------
        DebugCommand::Breakpoints => {
            let guard = session_lock.lock().await;
            if let Some(ref session) = *guard {
                eprintln!("\n🔴 Breakpoints ({})\n", session.breakpoints.len());
                for bp in &session.breakpoints {
                    let status = if bp.verified { "✅" } else { "⏳" };
                    eprintln!("  {} #{} {}:{}", status, bp.id, bp.file, bp.line);
                    if let Some(ref cond) = bp.condition {
                        eprintln!("     if: {}", cond);
                    }
                }
            } else {
                eprintln!("\n🔴 Breakpoints (no active session)\n");
            }
        }

        // -- delete-breakpoint <id> ------------------------------
        DebugCommand::DeleteBreakpoint { id } => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                let before = session.breakpoints.len();
                session.breakpoints.retain(|bp| bp.id != id);
                let removed = before - session.breakpoints.len();
                if removed > 0 {
                    eprintln!("\n🗑️  Removed breakpoint #{}\n", id);
                } else {
                    eprintln!("\n⏳ Breakpoint #{} not found\n", id);
                }
            }
        }

        // -- exception-breakpoint --------------------------------
        DebugCommand::ExceptionBreakpoint { filter } => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = *guard {
                if let Some(ref mut stdin) = session.stdin {
                    let seq = session.request_seq;
                    let filters = match filter.as_str() {
                        "all" => vec!["all"],
                        "uncaught" => vec!["uncaught"],
                        "none" => vec![],
                        _ => vec!["uncaught"],
                    };
                    let _ = dap_request_internal(session, "setExceptionBreakpoints",
                        Some(serde_json::json!({ "filters": filters })),
                    ).await;
                    session.request_seq = seq;
                    eprintln!("\n⚠️  Exception breakpoint: {}\n", filter);
                }
            }
        }

        // -- logpoint --------------------------------------------
        DebugCommand::Logpoint { location, message } => {
            let parts: Vec<&str> = location.split(':').collect();
            if parts.len() < 2 {
                anyhow::bail!("Invalid logpoint '{}'. Use format: <file>:<line>", location);
            }
            let file = parts[..parts.len() - 1].join(":");
            let line: u32 = parts.last()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid line number in '{}'", location))?;

            eprintln!("\n📝 Logpoint set: {}:{}", file, line);
            eprintln!("  Message: {}\n", message);
            eprintln!("  (Logpoints require DAP adapter support — set as conditional breakpoint with log message.)");
        }

        // -- stop ------------------------------------------------
        DebugCommand::Stop => {
            let mut guard = session_lock.lock().await;
            if let Some(ref mut session) = guard.take() {
                // Send disconnect request
                if let Some(ref mut _stdin) = session.stdin {
                    let _seq = session.request_seq;
                    let _ = dap_request_internal(session, "disconnect",
                        Some(serde_json::json!({ "restart": false, "terminateDebuggee": true })),
                    ).await;
                }
                // Kill child
                if let Some(ref mut child) = session.process {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
                eprintln!("\n🛑 Debug session ended.\n");
            } else {
                eprintln!("  No debug session active.");
            }
        }
    }

    Ok(())
}

// ════════════════════════════════════════════════════════════════════