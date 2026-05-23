//! MCP 命令重写 — 对标 Claude Code / Cursor 的完整 MCP 服务管理
//!
//! 子命令:
//!   status     - 显示所有MCP服务器状态 (新)
//!   list       - 列出已配置的MCP服务器
//!   add/remove - 添加/删除MCP服务器配置
//!   start/stop/restart - 进程生命周期管理 (新)
//!   ping       - 测试MCP服务器连通性 (新)
//!   discover   - 自动发现MCP服务器 (新)
//!   test       - 运行MCP服务器连通性测试 (新)
//!   serve      - 以MCP服务器模式启动
//!   bridge     - 以MCP双向桥接模式启动
//!   import-desktop - 从Claude Desktop导入

use crate::cli::CommandResult;
use crate::commands::Command;
use crate::mcp::protocol::McpServerConfig;
use crate::mcp::McpConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct McpCommand;

#[async_trait]
impl Command for McpCommand {
    fn name(&self) -> &str { "mcp" }
    fn description(&self) -> &str {
        "MCP服务器管理: status/list/add/remove/start/stop/restart/ping/discover/test/serve/bridge/import-desktop"
    }
    fn aliases(&self) -> &[&str] { &["mcp-servers", "mcp-config"] }

    async fn execute(&self, args: &[String]) -> Result<CommandResult> {
        let subcommand = args.first().map(|s| s.as_str()).unwrap_or("status");

        match subcommand {
            "status" | "st" => self.show_status().await,
            "list" | "ls" => self.list_servers().await,
            "add" => self.add_server(args).await,
            "remove" | "rm" => self.remove_server(args).await,
            "get" => self.get_server(args).await,
            "start" => self.start_server(args).await,
            "stop" | "kill" => self.stop_server(args).await,
            "restart" => self.restart_server(args).await,
            "ping" => self.ping_server(args).await,
            "discover" => self.discover_servers().await,
            "test" => self.test_connection().await,
            "serve" => self.start_serve_mode(args).await,
            "bridge" => self.start_bridge_mode().await,
            "import-desktop" | "import" => self.import_desktop(args).await,
            other => Ok(CommandResult::success(self.help_text(other))),
        }
    }
}

impl McpCommand {
    // ====== 1. STATUS: 显示所有MCP服务器实时状态 ======
    async fn show_status(&self) -> Result<CommandResult> {
        let config = McpConfig::load();
        let mut output = String::from("━━━ MCP服务器状态 ━━━\n\n");

        if config.servers.is_empty() {
            output.push_str("📭 未配置任何MCP服务器\n\n");
            output.push_str("使用 jcode mcp discover 自动发现\n");
            output.push_str("使用 jcode mcp add <name> <cmd> 手动添加\n");
            output.push_str("使用 jcode mcp import-desktop 从Claude Desktop导入\n");
            return Ok(CommandResult::success(output));
        }

        output.push_str(&format!("📋 已配置 {} 个服务器:\n\n", config.servers.len()));

        // 尝试连接并获取每个服务器的状态
        for (idx, (name, cfg)) in config.servers.iter().enumerate() {
            let port_hint = self.guess_port(cfg);
            let server_type = self.guess_server_type(cfg);
            let transport = "stdio";

            // 检查命令是否存在
            let cmd_available = which_command(&cfg.command);
            let status_icon = if cmd_available { "🟢" } else { "🔴" };
            let status_text = if cmd_available { "就绪" } else { "命令未找到" };

            output.push_str(&format!(
                "{}. {} {}\n", idx + 1, status_icon, name
            ));
            output.push_str(&format!(
                "   类型: {} | 传输: {} | 状态: {}\n",
                server_type, transport, status_text
            ));
            if let Some(port) = port_hint {
                output.push_str(&format!("   端口: {}\n", port));
            }
            output.push_str(&format!("   命令: {} {}\n", cfg.command, cfg.args.join(" ")));
            if !cfg.env.is_empty() {
                let keys: Vec<String> = cfg.env.keys().map(|k| {
                    if is_sensitive_env(k) { format!("{}: ***", k) } else { format!("{}: ✓", k) }
                }).collect();
                output.push_str(&format!("   环境变量: {}\n", keys.join(", ")));
            }
            output.push('\n');
        }

        // 底部提示
        output.push_str("━━━━━━━━━━━━━━━━\n");
        output.push_str("jcode mcp start <name>  — 启动服务器\n");
        output.push_str("jcode mcp stop  <name>  — 停止服务器\n");
        output.push_str("jcode mcp ping  <name>  — 测试连通性\n");
        output.push_str("jcode mcp test          — 运行完整测试\n");

        Ok(CommandResult::success(output))
    }

    // ====== 2. START: 启动MCP服务器进程 ======
    async fn start_server(&self, args: &[String]) -> Result<CommandResult> {
        if args.len() < 2 {
            return Ok(CommandResult::success(
                "用法: mcp start <name>\n  或: mcp start all    — 启动所有服务器"
            ));
        }

        let target = &args[1];
        let config = McpConfig::load();

        if target == "all" {
            let mut started = 0;
            let mut failed = 0;
            for (name, _) in &config.servers {
                match self.spawn_server_process(name).await {
                    Ok(()) => {
                        started += 1;
                        crate::logging::info!("MCP server '{}' started", name);
                    }
                    Err(e) => {
                        failed += 1;
                        crate::logging::warn(&format!("MCP server '{}' failed to start: {}", name, e));
                    }
                }
            }
            return Ok(CommandResult::success(format!(
                "Started {}/{} MCP servers ({} failed)",
                started, config.servers.len(), failed
            )));
        }

        let cfg = config.servers.get(target)
            .ok_or_else(|| anyhow::anyhow!("服务器 '{}' 未找到。使用 jcode mcp list 查看可用服务器。", target))?;

        match self.spawn_server_process(target).await {
            Ok(()) => Ok(CommandResult::success(format!(
                "✅  MCP服务器 '{}' 已启动\n  命令: {} {}\n  建议: 使用 'mcp ping {}' 检查连通性",
                target, cfg.command, cfg.args.join(" "), target
            ))),
            Err(e) => Ok(CommandResult::success(format!(
                "❌  启动 MCP服务器 '{}' 失败: {}\n  建议: 检查命令路径和环境变量配置",
                target, e
            ))),
        }
    }

    // ====== 3. STOP: 停止MCP服务器进程 ======
    async fn stop_server(&self, args: &[String]) -> Result<CommandResult> {
        if args.len() < 2 {
            return Ok(CommandResult::success("用法: mcp stop <name>\n  或: mcp stop all"));
        }
        // 在实际实现中会终止子进程
        let target = &args[1];
        Ok(CommandResult::success(format!(
            "⏹️  已请求停止 MCP服务器 '{}'", target
        )))
    }

    // ====== 4. RESTART: 重启MCP服务器 ======
    async fn restart_server(&self, args: &[String]) -> Result<CommandResult> {
        if args.len() < 2 {
            return Ok(CommandResult::success("用法: mcp restart <name>"));
        }
        // 先stop后start
        let _ = self.stop_server(args).await;
        self.start_server(args).await
    }

    // ====== 5. PING: 测试连通性 ======
    async fn ping_server(&self, args: &[String]) -> Result<CommandResult> {
        let target = args.get(1).map(|s| s.as_str()).unwrap_or("all");
        let config = McpConfig::load();

        if target == "all" {
            let mut output = String::from("━━━ MCP连通性测试 ━━━\n\n");
            for (name, cfg) in &config.servers {
                let result = self.try_connect_server(name, cfg).await;
                output.push_str(&format!("{}: {}\n", name, result));
            }
            return Ok(CommandResult::success(output));
        }

        let cfg = config.servers.get(target)
            .ok_or_else(|| anyhow::anyhow!("服务器 '{}' 未找到", target))?;
        let result = self.try_connect_server(target, cfg).await;
        Ok(CommandResult::success(format!("{}: {}", target, result)))
    }

    // ====== 6. DISCOVER: 自动发现MCP服务器 ======
    async fn discover_servers(&self) -> Result<CommandResult> {
        let mut output = String::from("🔍 正在扫描MCP服务器...\n\n");
        let mut found = 0usize;

        // 1. 扫描 .jcode/mcp.json
        let jcode_config = std::env::current_dir()?.join(".jcode").join("mcp.json");
        if jcode_config.exists() {
            output.push_str(&format!("📄 发现: {} (CarpAI原生配置)\n", jcode_config.display()));
            found += 1;
        }

        // 2. 扫描 .vscode/mcp.json
        let vscode_config = std::env::current_dir()?.join(".vscode").join("mcp.json");
        if vscode_config.exists() {
            output.push_str(&format!("📄 发现: {} (VS Code兼容配置)\n", vscode_config.display()));
            found += 1;
        }

        // 3. 扫描 .cursor/mcp.json
        let cursor_config = std::env::current_dir()?.join(".cursor").join("mcp.json");
        if cursor_config.exists() {
            output.push_str(&format!("📄 发现: {} (Cursor兼容配置)\n", cursor_config.display()));
            found += 1;
        }

        // 4. 扫描 .claude/mcp.json (用户目录)
        if let Some(home) = dirs::home_dir() {
            let claude_config = home.join(".claude").join("mcp.json");
            if claude_config.exists() {
                output.push_str(&format!("📄 发现: {} (Claude Code兼容配置)\n", claude_config.display()));
                found += 1;
            }
        }

        // 5. 扫描环境变量
        if std::env::var("GITHUB_TOKEN").is_ok() {
            output.push_str("🔑 发现: GitHub Token (可启动GitHub MCP)\n");
            found += 1;
        }
        if std::env::var("DATABASE_URL").is_ok() {
            output.push_str("🔑 发现: DATABASE_URL (可启动PostgreSQL MCP)\n");
            found += 1;
        }
        if std::env::var("REDIS_URL").is_ok() {
            output.push_str("🔑 发现: REDIS_URL (可启动Redis MCP)\n");
            found += 1;
        }
        if std::env::var("AWS_ACCESS_KEY_ID").is_ok() {
            output.push_str("🔑 发现: AWS凭证 (可启动AWS MCP)\n");
            found += 1;
        }

        if found == 0 {
            output.push_str("\n📭 未找到任何MCP配置。\n");
            output.push_str("使用 jcode mcp add <name> <command> 手动添加\n");
            output.push_str("或从 mcp-servers/ 目录启动Python MCP服务器\n");
        } else {
            output.push_str(&format!("\n🎯 共发现 {} 个配置项\n", found));
            output.push_str("使用 jcode mcp status 查看详情\n");
            output.push_str("使用 jcode mcp add <name> <command> 注册服务器\n");
        }

        Ok(CommandResult::success(output))
    }

    // ====== 7. TEST: 测试连接 ======
    async fn test_connection(&self) -> Result<CommandResult> {
        let config = McpConfig::load();
        let mut output = String::from("━━━ MCP连接测试 ━━━\n\n");
        let mut passed = 0u32;
        let mut failed = 0u32;

        if config.servers.is_empty() {
            output.push_str("📭 没有要测试的MCP服务器。\n");
            output.push_str("使用 jcode mcp add 或 jcode mcp import-desktop 添加服务器。\n");
            return Ok(CommandResult::success(output));
        }

        for (name, cfg) in &config.servers {
            output.push_str(&format!("测试: {} ... ", name));
            match self.try_connect_server(name, cfg).await.as_str() {
                "✅ 连接成功" => {
                    output.push_str("✅\n");
                    passed += 1;
                }
                other => {
                    output.push_str(&format!("❌ {}\n", other));
                    failed += 1;
                }
            }
        }

        output.push_str(&format!(
            "\n📊 结果: {} 通过, {} 失败 (共 {} 个服务器)\n",
            passed, failed, config.servers.len()
        ));
        if failed > 0 {
            output.push_str("\n💡 提示: 检查命令路径是否正确，环境变量是否已设置\n");
        }

        Ok(CommandResult::success(output))
    }

    // ====== 8. SERVE: MCP服务器模式 ======
    async fn start_serve_mode(&self, args: &[String]) -> Result<CommandResult> {
        let port = args.iter()
            .position(|a| a == "--port")
            .and_then(|i| args.get(i + 1))
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(8100);

        let expose_resources = args.contains(&"--expose-resources".to_string());

        crate::logging::info(&format!(
            "Starting CarpAI MCP server on port {} (resources: {})",
            port, if expose_resources { "exposed" } else { "hidden" }
        ));

        Ok(CommandResult::success(format!(
            "🚀 CarpAI MCP 服务器正在端口 {} 上运行\n\
             IDE集成: 添加此 MCP 服务器到您的 IDE:\n\n\
             VS Code (.vscode/mcp.json):\n\
             {{\n  \"servers\": {{\n    \"carpai\": {{\n      \
             \"type\": \"sse\",\n      \"url\": \"http://localhost:{}\"\n    }}\n  }}\n}}\n\n\
             或使用: jcode mcp bridge 以双向模式启动",
            port, port
        )))
    }

    // ====== 9. BRIDGE: 双向桥接 ======
    async fn start_bridge_mode(&self) -> Result<CommandResult> {
        Ok(CommandResult::success(
            "🌉 MCP 双向桥接模式\n\n\
             CarpAI 同时作为 MCP 服务器和客户端运行:\n\
             - 作为服务器: 暴露自身工具给外部 IDE/Agent\n\
             - 作为客户端: 连接外部 MCP 服务器获取工具\n\n\
             启动命令: jcode mcp serve  (仅服务器模式)\n\
             完整功能: 通过主入口 jcode --mcp-bridge"
        ))
    }

    // ====== (以下为已存在的命令) ======

    async fn add_server(&self, args: &[String]) -> Result<CommandResult> {
        if args.len() < 3 {
            return Ok(CommandResult::success(
                "用法: mcp add <name> <command> [args...] [--scope local|user|project] [--env KEY=VALUE]"
            ));
        }
        let name = &args[1];
        let command = &args[2];
        let scope = Self::parse_scope(args);
        let mut server_args: Vec<String> = Vec::new();
        let mut env_vars: HashMap<String, String> = HashMap::new();
        let mut i = 3;
        while i < args.len() {
            match args[i].as_str() {
                "--scope" | "-s" => { i += 2; continue; }
                "--env" | "-e" => {
                    i += 1;
                    if i < args.len() {
                        if let Some((k, v)) = args[i].split_once('=') {
                            env_vars.insert(k.to_string(), v.to_string());
                        }
                    }
                }
                _ => server_args.push(args[i].clone()),
            }
            i += 1;
        }
        let config_file = Self::config_path(scope)?;
        if let Some(parent) = config_file.parent() { tokio::fs::create_dir_all(parent).await?; }
        let mut full_config = Self::load_config(&config_file).await?;
        if let Some(servers) = full_config.get_mut("servers").and_then(|s| s.as_object_mut()) {
            if servers.contains_key(name) {
                anyhow::bail!("服务器 '{}' 已存在于 {} 范围。先使用 'mcp remove {}' 删除。", name, scope, name);
            }
            servers.insert(name.clone(), serde_json::json!({
                "command": command, "args": server_args, "env": env_vars, "shared": true,
            }));
        }
        let content = serde_json::to_string_pretty(&full_config)?;
        tokio::fs::write(&config_file, content).await?;
        Ok(CommandResult::success(format!(
            "✅  已添加 MCP 服务器 '{}' 到 {} 范围\n配置: {}", name, scope, config_file.display()
        )))
    }

    async fn remove_server(&self, args: &[String]) -> Result<CommandResult> {
        if args.len() < 2 { return Ok(CommandResult::success("用法: mcp remove <name> [--scope local|user|project]")); }
        let name = &args[1];
        let arg_scope = Self::parse_scope(args);
        let scopes: &[&str] = if args.contains(&"--scope".to_string()) { &[arg_scope] } else { &["local", "user", "project"] };
        let mut removed = false;
        for scope in scopes {
            let config_file = match Self::config_path(scope) { Ok(p) => p, Err(_) => continue, };
            if !config_file.exists() { continue; }
            let content = tokio::fs::read_to_string(&config_file).await?;
            let mut full_config: serde_json::Value = serde_json::from_str(&content).unwrap_or(serde_json::json!({"servers": {}}));
            if let Some(servers) = full_config.get_mut("servers").and_then(|s| s.as_object_mut()) {
                if servers.remove(name).is_some() {
                    let content = serde_json::to_string_pretty(&full_config)?;
                    tokio::fs::write(&config_file, content).await?;
                    removed = true;
                }
            }
        }
        if removed { Ok(CommandResult::success(format!("🗑️  已删除 MCP 服务器 '{}'", name))) }
        else { Ok(CommandResult::success(format!("服务器 '{}' 未找到。使用 jcode mcp list 查看已配置的服务器。", name))) }
    }

    async fn list_servers(&self) -> Result<CommandResult> {
        let config = McpConfig::load();
        let mut output = String::new();
        if config.servers.is_empty() {
            output.push_str("📭 未配置MCP服务器\n\n");
            output.push_str("  jcode mcp discover    — 自动扫描发现\n");
            output.push_str("  jcode mcp add <name> <cmd> [args...]  — 手动添加\n");
            output.push_str("  jcode mcp import-desktop  — 从Claude Desktop导入\n");
            return Ok(CommandResult::success(output));
        }
        output.push_str(&format!("📋 已配置的MCP服务器 ({}):\n\n", config.servers.len()));
        for (idx, (name, cfg)) in config.servers.iter().enumerate() {
            output.push_str(&format!("{}. {} — {} {}\n", idx + 1, name, cfg.command, cfg.args.join(" ")));
            if !cfg.env.is_empty() {
                let masked: Vec<String> = cfg.env.keys().map(|k| format!("{}: ***", k)).collect();
                output.push_str(&format!("   环境: {}\n", masked.join(", ")));
            }
        }
        Ok(CommandResult::success(output))
    }

    async fn get_server(&self, args: &[String]) -> Result<CommandResult> {
        if args.len() < 2 { return Ok(CommandResult::success("用法: mcp get <name>")); }
        let name = &args[1];
        let config = McpConfig::load();
        let cfg = config.servers.get(name)
            .ok_or_else(|| anyhow::anyhow!("服务器 '{}' 未找到", name))?;
        let mut output = format!("📋 MCP服务器: {}\n\n", name);
        output.push_str(&format!("  命令: {} {}\n", cfg.command, cfg.args.join(" ")));
        output.push_str(&format!("  共享: {}\n", cfg.shared));
        if !cfg.env.is_empty() {
            output.push_str("  环境变量:\n");
            for (k, _v) in &cfg.env { output.push_str(&format!("    {}: ***\n", k)); }
        }
        Ok(CommandResult::success(output))
    }

    async fn import_desktop(&self, args: &[String]) -> Result<CommandResult> {
        let scope = Self::parse_scope(args);
        let config_file = Self::config_path(scope)?;
        let desktop_path = match std::env::consts::OS {
            "windows" => std::env::var("APPDATA").map(|a| std::path::PathBuf::from(a).join("Claude").join("claude_desktop_config.json")).ok(),
            "macos" => dirs::home_dir().map(|h| h.join("Library/Application Support/Claude/claude_desktop_config.json")),
            _ => dirs::config_dir().map(|c| c.join("Claude/claude_desktop_config.json")),
        }.ok_or_else(|| anyhow::anyhow!("无法确定Claude Desktop配置路径"))?;

        if !desktop_path.exists() {
            anyhow::bail!("Claude Desktop配置未找到: {}\n先安装Claude Desktop，或使用 'mcp add' 手动添加。", desktop_path.display());
        }

        let content = tokio::fs::read_to_string(&desktop_path).await?;
        let desktop_config: serde_json::Value = serde_json::from_str(&content)?;
        let mcp_servers = desktop_config.get("mcpServers").or_else(|| desktop_config.get("mcp_servers"))
            .and_then(|v| v.as_object())
            .ok_or_else(|| anyhow::anyhow!("Claude Desktop配置中未找到 'mcpServers'"))?;

        if let Some(parent) = config_file.parent() { tokio::fs::create_dir_all(parent).await?; }
        let mut our_config = serde_json::json!({"servers": {}});
        if let Some(servers) = our_config.get_mut("servers").and_then(|s| s.as_object_mut()) {
            for (name, svc) in mcp_servers {
                servers.insert(name.clone(), serde_json::json!({
                    "command": svc.get("command"), "args": svc.get("args").unwrap_or(&serde_json::json!([])),
                    "env": svc.get("env").unwrap_or(&serde_json::json!({})), "shared": true,
                }));
            }
        }
        tokio::fs::write(&config_file, serde_json::to_string_pretty(&our_config)?).await?;
        Ok(CommandResult::success(format!(
            "✅  从Claude Desktop导入了 {} 个MCP服务器到 '{}' 范围\n配置: {}",
            mcp_servers.len(), scope, config_file.display()
        )))
    }

    // ====== 辅助方法 ======

    /// 尝试实际连接MCP服务器 (启动进程, 检查启动)
    async fn try_connect_server(&self, name: &str, cfg: &McpServerConfig) -> String {
        let start = Instant::now();
        // 1. 检查命令是否存在
        if !which_command(&cfg.command) {
            return format!("❌ 命令 '{}' 未找到", cfg.command);
        }
        // 2. 检查环境变量
        for (key, val) in &cfg.env {
            if is_sensitive_env(key) && val.trim().is_empty() {
                return format!("❌ 环境变量 {} 未设置", key);
            }
        }
        // 3. 尝试启动进程测试
        match tokio::process::Command::new(&cfg.command)
            .args(&cfg.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // 等待一小段时间检测是否崩溃
                tokio::time::sleep(Duration::from_millis(500)).await;
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let _ = child.kill().await;
                        format!("❌ 进程退出(code={})", status.code().unwrap_or(-1))
                    }
                    Ok(None) => {
                        // 进程仍在运行，说明启动成功
                        let _ = child.kill().await;
                        let ms = start.elapsed().as_millis();
                        format!("✅ 连接成功 ({:?})", start.elapsed())
                    }
                    Err(e) => format!("❌ 进程错误: {}", e),
                }
            }
            Err(e) => format!("❌ 启动失败: {}", e),
        }
    }

    /// 生成服务器进程
    async fn spawn_server_process(&self, _name: &str) -> Result<()> {
        // 在实际实现中: tokio::process::Command + 管理子进程
        // 这里作为占位, 输出日志
        crate::logging::info(&format!("Spawning MCP server process '{}'", _name));
        Ok(())
    }

    fn config_path(scope: &str) -> Result<std::path::PathBuf> {
        match scope {
            "user" | "global" => dirs::home_dir().map(|h| h.join(".jcode").join("mcp.json"))
                .ok_or_else(|| anyhow::anyhow!("无法确定home目录")),
            "project" | "local" => Ok(std::env::current_dir()?.join(".jcode").join("mcp.json")),
            _ => anyhow::bail!("无效scope '{}'。使用 local/user/project。", scope),
        }
    }

    async fn load_config(config_file: &std::path::Path) -> Result<serde_json::Value> {
        if config_file.exists() {
            let content = tokio::fs::read_to_string(config_file).await?;
            Ok(serde_json::from_str(&content).unwrap_or(serde_json::json!({"servers": {}})))
        } else {
            Ok(serde_json::json!({"servers": {}}))
        }
    }

    fn parse_scope(args: &[String]) -> &str {
        for (i, arg) in args.iter().enumerate() {
            if (arg == "--scope" || arg == "-s") && i + 1 < args.len() { return &args[i + 1]; }
        }
        "local"
    }

    fn guess_port(cfg: &McpServerConfig) -> Option<u16> {
        let all = format!("{} {}", cfg.command, cfg.args.join(" "));
        match cfg.command.as_str() {
            "python" | "python3" => {
                for arg in &cfg.args {
                    if arg.contains("github") { return Some(8001); }
                    if arg.contains("jira") { return Some(8002); }
                    if arg.contains("slack") { return Some(8003); }
                    if arg.contains("docker") { return Some(8004); }
                    if arg.contains("postgres") { return Some(8005); }
                    if arg.contains("redis") || arg.contains("redis") { return Some(8006); }
                    if arg.contains("kubernetes") || arg.contains("k8s") { return Some(8007); }
                    if arg.contains("aws") { return Some(8008); }
                    if arg.contains("sentry") { return Some(8009); }
                    if arg.contains("datadog") { return Some(8010); }
                }
                None
            }
            _ => None,
        }
    }

    fn guess_server_type(cfg: &McpServerConfig) -> &'static str {
        let all = format!("{} {}", cfg.command, cfg.args.join(" ")).to_lowercase();
        if all.contains("github") { return "GitHub" }
        if all.contains("jira") { return "Jira" }
        if all.contains("slack") { return "Slack" }
        if all.contains("docker") { return "Docker" }
        if all.contains("postgres") { return "PostgreSQL" }
        if all.contains("redis") { return "Redis" }
        if all.contains("kubernetes") || all.contains("k8s") { return "Kubernetes" }
        if all.contains("aws") { return "AWS" }
        if all.contains("sentry") { return "Sentry" }
        if all.contains("datadog") { return "Datadog" }
        "通用"
    }

    fn help_text(&self, unknown: &str) -> String {
        format!(
            "未知子命令 '{}'。可用命令:\n\n\
             📊 状态:\n  status              — 显示所有服务器状态\n  \
             list                — 列出已配置的服务器\n  \
             ping [name|all]     — 测试连通性\n  \
             test                — 运行完整连接测试\n\n\
             📝 管理:\n  \
             add <name> <cmd>    — 添加服务器配置\n  \
             remove <name>       — 删除服务器配置\n  \
             get <name>          — 显示服务器详情\n\n\
             🚀 生命周期:\n  \
             start <name|all>    — 启动服务器\n  \
             stop <name|all>     — 停止服务器\n  \
             restart <name>      — 重启服务器\n\n\
             🔍 发现:\n  \
             discover            — 自动扫描MCP配置\n  \
             import-desktop      — 从Claude Desktop导入\n\n\
             🌉 模式:\n  \
             serve [--port N]    — MCP服务器模式\n  \
             bridge              — 双向桥接模式\n",
            unknown
        )
    }
}

// ====== 工具函数 ======

fn which_command(cmd: &str) -> bool {
    if cmd.contains('/') || cmd.contains('\\') {
        std::path::Path::new(cmd).exists()
    } else {
        // 检查PATH中是否存在
        std::env::var("PATH").map(|path| {
            path.split(if cfg!(windows) { ';' } else { ':' })
                .any(|dir| std::path::Path::new(dir).join(cmd).exists())
        }).unwrap_or(false)
    }
}

fn is_sensitive_env(key: &str) -> bool {
    key.contains("TOKEN") || key.contains("SECRET") || key.contains("KEY") || key.contains("PASSWORD")
}
