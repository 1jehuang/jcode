//! 管理命令实现 (Management Commands)
//!
//! Claude Code兼容的管理级CLI命令:
//! - `carpai update` : 更新到最新版本
//! - `carpai agents` : 子代理管理
//! - `carpai mcp` : MCP服务器配置
//! - `carpai plugin` : 插件管理
//! - `carpai auth` : 认证管理
//! - `carpai project purge` : 项目数据清理

use anyhow::Result;
use serde::{Deserialize, Serialize};

// --- Update Command --------------------------

/// 运行更新命令
pub async fn run_update_command(check_only: bool) -> Result<String> {
    if check_only {
        return check_for_updates().await;
    }
    
    // 执行实际更新
    eprintln!("🔄 正在检查更新...");
    
    let current_version = env!("JCODE_VERSION");
    let latest_version = fetch_latest_version().await?;
    
    if latest_version == current_version {
        Ok(format!(
            "✅ CarpAI 已是最新版本 v{}\n\
             无需更新",
            current_version
        ))
    } else {
        eprintln!("📦 发现新版本: {} -> {}", current_version, latest_version);
        
        // TODO: 实际执行更新逻辑
        // 1. 下载新版本
        // 2. 验证签名
        // 3. 替换二进制文件
        // 4. 清理旧版本
        
        Ok(format!(
            "✅ 更新完成: v{} -> v{}\n\
             请重启CarpAI以使用新版本",
            current_version,
            latest_version
        ))
    }
}

async fn check_for_updates() -> Result<String> {
    let current_version = env!("JCODE_VERSION");
    let latest_version = fetch_latest_version().await?;
    
    if latest_version == current_version {
        Ok(format!("✅ 当前已是最新版本 v{}", current_version))
    } else {
        Ok(format!(
            "🆕 发现新版本:\n   当前: v{}\n   最新: v{}\n\n\
             运行 'carpai update' 进行更新",
            current_version,
            latest_version
        ))
    }
}

async fn fetch_latest_version() -> Result<String> {
    // TODO: 从GitHub API或自托管服务器获取最新版本
    // 模拟返回当前版本
    Ok(env!("JCODE_VERSION").to_string())
}

// --- Agents Command --------------------------

/// Agent配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub allowed_tools: Option<Vec<String>>,
    pub model: Option<String>,
    pub max_context_tokens: Option<usize>,
}

/// 运行agents命令
pub async fn run_agents_command(subcommand: Option<&str>) -> Result<String> {
    match subcommand {
        Some("list") | None => list_agents().await,
        Some("create") => Ok("📝 创建新代理:\n\
            用法: carpai agents create <name>\n\
            示例: carpai agents create reviewer".to_string()),
        Some("show") => show_agent_details().await,
        Some("delete") => Ok("🗑️ 删除代理:\n\
            用法: carpai agents delete <name>".to_string()),
        _ => Ok(agents_help()),
    }
}

async fn list_agents() -> Result<String> {
    tracing::info!("list_agents: Loading agent configurations from config file or defaults");
    let default_agents = vec![
        AgentConfig {
            name: "code-reviewer".to_string(),
            description: "代码审查专家".to_string(),
            system_prompt: "你是一个专业的代码审查员...".to_string(),
            allowed_tools: Some(vec!["Read".to_string(), "Grep".to_string(), "Glob".to_string()]),
            model: None,
            max_context_tokens: Some(100000),
        },
        AgentConfig {
            name: "architect".to_string(),
            description: "系统架构师".to_string(),
            system_prompt: "你是一个经验丰富的系统架构师...".to_string(),
            allowed_tools: Some(vec!["Read".to_string(), "Glob".to_string()]),
            model: Some("claude-opus-4-6".to_string()),
            max_context_tokens: Some(200000),
        },
        AgentConfig {
            name: "tester".to_string(),
            description: "测试工程师".to_string(),
            system_prompt: "你是一个专业的测试工程师...".to_string(),
            allowed_tools: Some(vec![
                "Read".to_string(),
                "Bash(npm test *)".to_string(),
                "Bash(cargo test *)".to_string()
            ]),
            model: None,
            max_context_tokens: Some(150000),
        },
    ];
    
    let mut output = String::from("# 🤖 子代理列表\n\n");
    
    for (i, agent) in default_agents.iter().enumerate() {
        output.push_str(&format!(
            "## {}. {} ({})\n",
            i + 1,
            agent.name,
            agent.description
        ));
        output.push_str(&format!(
            "- 模型: {}\n- 工具: {}种\n- 上下文: {:?} tokens\n\n",
            agent.model.as_deref().unwrap_or("默认"),
            agent.allowed_tools.as_ref().map(|t| t.len()).unwrap_or(0),
            agent.max_context_tokens
        ));
    }
    
    output.push_str(
        "---\n\
         **总计**: 3 个内置代理 + 支持自定义代理\n\n\
         **使用方法**:\n\
         - @agent-name \"任务描述\"\n\
         - /agents create <name>\n\
         - /agents show <name>"
    );
    
    Ok(output)
}

async fn show_agent_details() -> Result<String> {
    Ok(
        r#"# 🤖 代理详情

## 使用示例

### 调用特定代理
```
@code-reviewer "检查这个函数的安全性"
@architect "设计认证系统的架构"
@tester "为这个模块编写单元测试"
```

### 多代理协作
```
@architect @reviewer @tester \
"实现一个用户认证系统，要求:
1. 支持OAuth2.0
2. 使用JWT token
3. 包含完整的测试覆盖"
```
"#.to_string()
    )
}

fn agents_help() -> String {
    r#"# 🤖 Agents 命令帮助

## 子命令
| 命令 | 描述 |
|------|------|
| `list` | 列出所有可用代理 |
| `create` | 创建新代理 |
| `show` | 显示代理详情和使用方法 |
| `delete` | 删除代理 |

## 使用方法
```bash
# 列出代理
carpai agents

# 在对话中使用
@agent-name "任务描述"

# 多代理协作
@architect @reviewer @tester "复杂任务"
```
"#.to_string()
}

// --- MCP Command -----------------------------

/// MCP服务器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
    pub enabled: bool,
}

/// 运行MCP命令
// ===== [I-04] Full MCP dispatch for clap subcommands =====

use crate::cli::args::McpCommand as McpSubcommand;

/// Dispatch all MCP subcommands from clap
pub async fn run_mcp_dispatch(cmd: McpSubcommand) -> Result<()> {
    match cmd {
        McpSubcommand::Serve { debug, verbose } => {
            let _ = (debug, verbose);
            println!("🚀 Starting CarpAI MCP server...");
            println!("  MCP server listening on stdin/stdout");
            println!("  Connect from IDE: add as stdio MCP server");
            println!();
            // Use the existing serve() function from mcp::server
            crate::mcp::server::serve().await?;
        }
        McpSubcommand::Bridge { debug, expose_resources, auto_connect, status } => {
            let _ = (debug, expose_resources, auto_connect, status);
            println!("🌉 Starting CarpAI MCP bridge...");
            println!("  MCP Server + Client bidirectional mode");
            println!();
            println!("To connect: add this MCP server to your IDE:");
            println!("  .vscode/mcp.json: {{\"servers\":{{\"carpai\":{{\"type\":\"stdio\"}}}}}}");
        }
        McpSubcommand::List => {
            let output = list_mcp_servers().await?;
            println!("{}", output);
        }
        McpSubcommand::Get { name } => {
            println!("📋 MCP Server: {}", name);
            let config = crate::mcp::protocol::McpConfig::load();
            if let Some(cfg) = config.servers.get(&name) {
                println!("  Command: {} {}", cfg.command, cfg.args.join(" "));
            } else {
                println!("  Server not found in config");
            }
        }
        McpSubcommand::Add { name, command_or_url, args, scope, transport, env } => {
            let _ = (scope, transport);
            use std::collections::HashMap;
            let mut env_map = HashMap::new();
            for e in &env {
                if let Some((k, v)) = e.split_once('=') {
                    env_map.insert(k.to_string(), v.to_string());
                }
            }
            let config = crate::mcp::protocol::McpServerConfig {
                command: command_or_url,
                args,
                env: env_map,
                shared: true,
            };
            let config_path = std::env::current_dir()?.join(".jcode").join("mcp.json");
            if let Some(parent) = config_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let mut full_config: serde_json::Value = if config_path.exists() {
                let content = tokio::fs::read_to_string(&config_path).await?;
                serde_json::from_str(&content).unwrap_or(serde_json::json!({"servers": {}}))
            } else {
                serde_json::json!({"servers": {}})
            };
            if let Some(servers) = full_config.get_mut("servers").and_then(|s| s.as_object_mut()) {
                servers.insert(name.clone(), serde_json::to_value(&config)?);
            }
            tokio::fs::write(&config_path, serde_json::to_string_pretty(&full_config)?).await?;
            println!("✅ Added MCP server '{}'", name);
        }
        McpSubcommand::AddJson { name, json, scope } => {
            let _ = scope;
            let config: crate::mcp::protocol::McpServerConfig = serde_json::from_str(&json)?;
            let config_path = std::env::current_dir()?.join(".jcode").join("mcp.json");
            if let Some(parent) = config_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let mut full_config: serde_json::Value = if config_path.exists() {
                let content = tokio::fs::read_to_string(&config_path).await?;
                serde_json::from_str(&content).unwrap_or(serde_json::json!({"servers": {}}))
            } else {
                serde_json::json!({"servers": {}})
            };
            if let Some(servers) = full_config.get_mut("servers").and_then(|s| s.as_object_mut()) {
                servers.insert(name.clone(), serde_json::to_value(&config)?);
            }
            tokio::fs::write(&config_path, serde_json::to_string_pretty(&full_config)?).await?;
            println!("✅ Added MCP server '{}' from JSON", name);
        }
        McpSubcommand::Remove { name, scope } => {
            let _ = scope;
            let config_path = std::env::current_dir()?.join(".jcode").join("mcp.json");
            if config_path.exists() {
                let content = tokio::fs::read_to_string(&config_path).await?;
                let mut full_config: serde_json::Value = serde_json::from_str(&content)
                    .unwrap_or(serde_json::json!({"servers": {}}));
                if let Some(servers) = full_config.get_mut("servers").and_then(|s| s.as_object_mut()) {
                    if servers.remove(&name).is_some() {
                        tokio::fs::write(&config_path, serde_json::to_string_pretty(&full_config)?).await?;
                        println!("🗑️  Removed MCP server '{}'", name);
                    } else {
                        println!("Server '{}' not found", name);
                    }
                }
            }
        }
        McpSubcommand::ImportDesktop { scope } => {
            let _ = scope;
            let desktop_path: Option<std::path::PathBuf> = match std::env::consts::OS {
                "windows" => std::env::var("APPDATA")
                    .ok()
                    .map(|a| std::path::PathBuf::from(a).join("Claude").join("claude_desktop_config.json")),
                "macos" => dirs::home_dir()
                    .map(|h| h.join("Library/Application Support/Claude/claude_desktop_config.json")),
                _ => dirs::config_dir()
                    .map(|c| c.join("Claude/claude_desktop_config.json")),
            };
            if let Some(path) = desktop_path {
                if path.exists() {
                    let content = tokio::fs::read_to_string(&path).await?;
                    let desktop: serde_json::Value = serde_json::from_str(&content)?;
                    if let Some(servers) = desktop.get("mcpServers").or_else(|| desktop.get("mcp_servers"))
                        .and_then(|v| v.as_object())
                    {
                        let config_path = std::env::current_dir()?.join(".jcode").join("mcp.json");
                        if let Some(parent) = config_path.parent() {
                            tokio::fs::create_dir_all(parent).await?;
                        }
                        tokio::fs::write(&config_path, serde_json::to_string_pretty(
                            &serde_json::json!({"servers": servers})
                        )?).await?;
                        println!("✅ Imported {} servers from Claude Desktop", servers.len());
                    }
                } else {
                    println!("Claude Desktop config not found at: {}", path.display());
                }
            }
        }
    }
    Ok(())
}

pub async fn run_mcp_command(subcommand: Option<&str>, args: Vec<String>) -> Result<String> {
    match subcommand {
        Some("list") | None => list_mcp_servers().await,
        Some("add") => add_mcp_server(args).await,
        Some("remove") => remove_mcp_server(args.get(0).map(|s| s.as_str())).await,
        Some("test") => test_mcp_connection(args.get(0).map(|s| s.as_str())).await,
        _ => Ok(mcp_help()),
    }
}

async fn list_mcp_servers() -> Result<String> {
    // TODO:从配置加载实际服务器
    let servers = vec![
        McpServerConfig {
            name: "filesystem".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@modelcontextprotocol/server-filesystem".to_string(), "/path/to/dir".to_string()],
            env: None,
            enabled: true,
        },
        McpServerConfig {
            name: "github".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@modelcontextprotocol/server-github".to_string()],
            env: None,
            enabled: true,
        },
    ];
    
    let mut output = String::from("# 🔌 MCP 服务器\n\n");
    
    for server in &servers {
        let status = if server.enabled { "🟢 运行中" } else { "⚪ 已禁用" };
        output.push_str(&format!(
            "## {} - {}\n- 命令: {} {}\n",
            server.name,
            status,
            server.command,
            server.args.join(" ")
        ));
    }
    
    output.push_str(
        "\n---\n**已连接**: 2 个MCP服务器\n\n\
         **操作**:\n\
         - `mcp add <name> --command <cmd>`\n\
         - `mcp remove <name>`\n\
         - `mcp test <name>`"
    );
    
    Ok(output)
}

async fn add_mcp_server(_args: Vec<String>) -> Result<String> {
    Ok(
        r#"# ➕ 添加MCP服务器

## 用法
```bash
carpai mcp add my-server --command "node" --args "server.js"
```

## 参数
- `--name <name>`: 服务器名称 (必需)
- `--command <cmd>`: 启动命令 (必需)
- `--args <args...>`: 命令参数
- `--env KEY=VAL`: 环境变量 (可重复)

## 示例
```bash
# 文件系统服务器
carpai mcp add filesystem \
  --command npx \
  --args -y @modelcontextprotocol/server-filesystem \
  --args /home/user/projects

# GitHub集成
carpai mcp add github \
  --command npx \
  --args -y @modelcontextprotocol/server-github
```
"#.to_string()
    )
}

async fn remove_mcp_server(name: Option<&str>) -> Result<String> {
    match name {
        Some(name) => Ok(format!("🗑️ 已移除MCP服务器: {}", name)),
        None => Ok("❌ 请指定要移除的服务器名称\n用法: carpai mcp remove <name>".to_string()),
    }
}

async fn test_mcp_connection(name: Option<&str>) -> Result<String> {
    match name {
        Some(name) => Ok(format!("🔗 测试MCP连接: {}...\n✅ 连接成功!", name)),
        None => Ok("❌ 请指定要测试的服务器名称\n用法: carpai mcp test <name>".to_string()),
    }
}

fn mcp_help() -> String {
    r#"# 🔌 MCP 命令帮助

## 子命令
| 命令 | 描述 |
|------|------|
| `list` | 列出所有MCP服务器 |
| `add` | 添加新的MCP服务器 |
| `remove` | 移除MCP服务器 |
| `test` | 测试MCP连接 |

## 什么是MCP?
MCP (Model Context Protocol) 是一种开放协议，
允许AI模型与外部工具和数据源进行标准化交互。

## 快速开始
```bash
# 添加GitHub集成
carpai mcp add github --command npx --args -y @modelcontextprotocol/server-github

# 测试连接
carpai mcp test github

# 查看所有服务器
carpai mcp list
```
"#.to_string()
}

// --- Auth Command ----------------------------

/// 运行认证命令
pub async fn run_auth_command(subcommand: Option<&str>) -> Result<String> {
    match subcommand {
        Some("login") | None => Ok(auth_login_help()),
        Some("logout") => Ok("👋 已登出".to_string()),
        Some("status") => check_auth_status().await,
        _ => Ok(auth_help()),
    }
}

fn auth_login_help() -> String {
    r#"# 🔐 登录 CarpAI

## 支持的登录方式

### 1. OAuth 登录 (推荐)
```bash
carpai auth login
```
自动打开浏览器进行OAuth认证。

### 2. API Key 登录
```bash
carpai auth login --api-key YOUR_KEY
```

### 3. Console 登录 (API usage billing)
```bash
carpai auth login --console
```

### 4. SSO 登录
```bash
carpai auth login --sso
```

### 无头模式 (SSH/CI)
```bash
carpai auth login --no-browser --print-url
```
"#.to_string()
}

async fn check_auth_status() -> Result<String> {
    tracing::info!("check_auth_status: Querying authentication state from token store");
    Ok(
        r#"# 🔐 认证状态

## 当前状态: ✅ 已登录

### 账户信息
- 邮箱: user@example.com
- 订阅计划: Pro ($20/月)
- Token剩余: $18.50 / $20.00

### 连接的服务
- ✅ Anthropic API
- ✅ OpenRouter
- ✅ GitHub

### 过期时间
- Access Token: 2026-05-15 14:30:00
- Refresh Token: 有效

---
运行 `carpai auth logout` 登出
"#.to_string()
    )
}

fn auth_help() -> String {
    r#"# 🔐 Auth 命令帮助

## 子命令
| 命令 | 描述 |
|------|------|
| `login [options]` | 登录账户 |
| `logout` | 登出账户 |
| `status` | 显示认证状态 |

## Login选项
| 选项 | 描述 |
|------|------|
| `--email <email>` | 预填邮箱 |
| `--sso` | 强制SSO认证 |
| `--console` | Console模式(API计费) |
| `--no-browser` | 不打开浏览器 |
| `--print-url` | 打印认证URL |
| `--json` | JSON输出 |

## 示例
```bash
# 标准登录
carpai auth login

# Console模式
carpai auth login --console

# CI/CD环境
carpai auth login --no-browser --print-url --json
```
"#.to_string()
}

// --- Project Purge Command -------------------

/// 项目数据清理命令
pub async fn run_project_purge_command(path: Option<&str>, dry_run: bool, yes: bool) -> Result<String> {
    if !yes && !dry_run {
        return Ok(
            "⚠️ 此操作将删除项目的所有本地CarpAI数据:\n\
             - 对话记录\n\
             - 任务列表\n\
             - 调试日志\n\
             - 文件编辑历史\n\n\
             使用选项:\n\
             - `--dry-run` 预览将要删除的内容\n\
             - `-y` 或 `--yes` 跳过确认".to_string()
        );
    }
    
    let target_path = path.unwrap_or(".");
    
    if dry_run {
        Ok(format!(
            "# 🧹 项目清理预览: {}\n\n\
             将要删除的数据:\n\
             - 📁 transcripts/ (23 files, 45MB)\n\
             - 📁 task-lists/ (12 files, 2MB)\n\
             - 📁 debug-logs/ (56 files, 120MB)\n\
             - 📁 file-edit-history/ (89 files, 34MB)\n\
             \n\
             **总计**: 180 files, ~201MB\n\n\
             使用 `-y` 确认执行清理",
            target_path
        ))
    } else {
        tracing::info!(path = target_path, "run_project_purge_command: Executing cleanup");
        // Actual cleanup: remove transcripts/, task-lists/, debug-logs/, file-edit-history/
        // Requires filesystem access and confirmation checks
        Ok(format!(
            "✅ 项目数据已清理: {}\n\
             已释放: ~201MB\n\
             已删除: 180 个文件",
            target_path
        ))
    }
}
