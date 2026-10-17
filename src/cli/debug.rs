use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::server;

pub async fn run_debug_command(
    command: &str,
    arg: &str,
    session_id: Option<String>,
    socket_path: Option<String>,
    _wait: bool,
) -> Result<()> {
    match command {
        "list" => return debug_list_servers().await,
        "start" => return debug_start_server(arg, socket_path).await,
        _ => {}
    }

    let debug_socket = if let Some(ref path) = socket_path {
        let main_path = std::path::PathBuf::from(path);
        let filename = main_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("kcode.sock");
        let debug_filename = filename.replace(".sock", "-debug.sock");
        main_path.with_file_name(debug_filename)
    } else {
        server::debug_socket_path()
    };

    if !crate::transport::is_socket_path(&debug_socket) {
        eprintln!("Debug socket not found at {:?}", debug_socket);
        eprintln!("\nMake sure:");
        eprintln!("  1. A jcode server is running (jcode or jcode serve)");
        eprintln!("  2. debug_socket is enabled in ~/.kcode/config.toml");
        eprintln!("     [display]");
        eprintln!("     debug_socket = true");
        eprintln!("\nOr use 'jcode debug start' to start a server.");
        eprintln!("Use 'jcode debug list' to see running servers.");
        anyhow::bail!("Debug socket not available");
    }

    let stream = server::connect_socket(&debug_socket).await?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let debug_cmd = if arg.is_empty() {
        command.to_string()
    } else {
        format!("{}:{}", command, arg)
    };

    let request = serde_json::json!({
        "type": "debug_command",
        "id": 1,
        "command": debug_cmd,
        "session_id": session_id,
    });

    let mut json = serde_json::to_string(&request)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;

    let mut line = String::new();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        anyhow::bail!("Server disconnected before sending response");
    }

    let response: serde_json::Value = serde_json::from_str(&line)?;

    match response.get("type").and_then(|v| v.as_str()) {
        Some("debug_response") => {
            let ok = response
                .get("ok")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let output = response
                .get("output")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if ok {
                println!("{}", output);
            } else {
                eprintln!("Error: {}", output);
                std::process::exit(1);
            }
        }
        Some("error") => {
            let message = response
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            eprintln!("Error: {}", message);
            std::process::exit(1);
        }
        _ => {
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
    }

    Ok(())
}

async fn debug_list_servers() -> Result<()> {
    let mut servers = Vec::new();

    let runtime_dir = crate::storage::runtime_dir();
    let mut scan_dirs = vec![runtime_dir.clone()];
    let temp_dir = std::env::temp_dir();
    if temp_dir != runtime_dir {
        scan_dirs.push(temp_dir);
    }

    for dir in scan_dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && name.starts_with("jcode")
                    && name.ends_with(".sock")
                    && !name.contains("-debug")
                {
                    servers.push(path);
                }
            }
        }
    }

    if servers.is_empty() {
        println!("No running jcode servers found.");
        println!("\nStart one with: jcode debug start");
        return Ok(());
    }

    println!("Running jcode servers:\n");

    for socket_path in servers {
        let debug_socket = {
            let filename = socket_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("kcode.sock");
            let debug_filename = filename.replace(".sock", "-debug.sock");
            socket_path.with_file_name(debug_filename)
        };

        let alive = crate::transport::Stream::connect(&socket_path)
            .await
            .is_ok();

        let debug_enabled = if crate::transport::is_socket_path(&debug_socket) {
            crate::transport::Stream::connect(&debug_socket)
                .await
                .is_ok()
        } else {
            false
        };

        let session_info = if debug_enabled {
            get_server_info(&debug_socket).await.unwrap_or_default()
        } else {
            String::new()
        };

        let status = if alive {
            if debug_enabled {
                format!("✓ running, debug: enabled{}", session_info)
            } else {
                "✓ running, debug: disabled".to_string()
            }
        } else {
            "✗ not responding (stale socket?)".to_string()
        };

        println!("  {} ({})", socket_path.display(), status);
    }

    println!("\nUse -s/--socket to target a specific server:");
    println!("  jcode debug -s /path/to/socket.sock sessions");

    Ok(())
}

async fn get_server_info(debug_socket: &std::path::Path) -> Result<String> {
    use crate::transport::Stream;

    let stream = Stream::connect(debug_socket).await?;
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let request = serde_json::json!({
        "type": "debug_command",
        "id": 1,
        "command": "sessions",
    });
    let mut json = serde_json::to_string(&request)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;

    let mut line = String::new();
    let n = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        reader.read_line(&mut line),
    )
    .await??;
    if n == 0 {
        return Ok(String::new());
    }

    let response: serde_json::Value = serde_json::from_str(&line)?;
    if let Some(output) = response.get("output").and_then(|v| v.as_str())
        && let Ok(sessions) = serde_json::from_str::<Vec<String>>(output)
    {
        return Ok(format!(", sessions: {}", sessions.len()));
    }

    Ok(String::new())
}

async fn debug_start_server(arg: &str, socket_path: Option<String>) -> Result<()> {
    let socket = socket_path.unwrap_or_else(|| {
        if !arg.is_empty() {
            arg.to_string()
        } else {
            server::socket_path().to_string_lossy().to_string()
        }
    });

    let socket_pathbuf = std::path::PathBuf::from(&socket);

    if crate::transport::is_socket_path(&socket_pathbuf)
        && crate::transport::Stream::connect(&socket_pathbuf)
            .await
            .is_ok()
    {
        eprintln!("Server already running at {}", socket);
        eprintln!("Use 'jcode debug list' to see all servers.");
        return Ok(());
    }

    let debug_socket = {
        let filename = socket_pathbuf
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("kcode.sock");
        let debug_filename = filename.replace(".sock", "-debug.sock");
        socket_pathbuf.with_file_name(debug_filename)
    };
    let _ = std::fs::remove_file(&debug_socket);

    eprintln!("Starting jcode server...");

    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("serve");

    if socket != server::socket_path().to_string_lossy() {
        cmd.arg("--socket").arg(&socket);
    }

    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > std::time::Duration::from_secs(10) {
            anyhow::bail!("Server failed to start within 10 seconds");
        }
        if crate::transport::is_socket_path(&socket_pathbuf)
            && crate::transport::Stream::connect(&socket_pathbuf)
                .await
                .is_ok()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    eprintln!("✓ Server started at {}", socket);

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    if crate::transport::is_socket_path(&debug_socket) {
        eprintln!("✓ Debug socket at {}", debug_socket.display());
    } else {
        eprintln!("⚠ Debug socket not enabled. Add to ~/.kcode/config.toml:");
        eprintln!("  [display]");
        eprintln!("  debug_socket = true");
    }

    Ok(())
}
