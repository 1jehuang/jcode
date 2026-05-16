//! # CarpAI Admin CLI — 管理命令行工具
//!
//! 用于管理企业版服务器，无需浏览器访问管理后台。
//!
//! ## 使用示例
//!
//! ```bash
//! # 查看帮助
//! carpai-admin-cli --help
//!
//! # 注册新组织
//! carpai-admin-cli register --org "我的公司" --admin admin@company.com --password ****
//!
//! # 列出所有用户
//! carpai-admin-cli users list
//!
//! # 创建新用户
//! carpai-admin-cli users create --email dev@company.com --role developer
//!
//! # 查看用量统计
//! carpai-admin-cli usage --days 7
//!
//! # 查看节点状态
//! carpai-admin-cli nodes
//!
//! # 生成 API Key
//! carpai-admin-cli api-key generate
//! ```

use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();
    let server_url = env::var("CARPAI_SERVER").unwrap_or_else(|_| "http://localhost:8000".into());
    let client = reqwest::Client::new();

    if args.len() < 2 {
        print_help();
        return Ok(());
    }

    match args[1].as_str() {
        "register" => {
            // carpai-admin-cli register --org "My Org" --admin admin@org.com --password ****
            let mut org_name = String::new();
            let mut email = String::new();
            let mut password = String::new();

            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--org" => { i += 1; org_name = args[i].clone(); }
                    "--admin" => { i += 1; email = args[i].clone(); }
                    "--password" => { i += 1; password = args[i].clone(); }
                    _ => {}
                }
                i += 1;
            }

            if org_name.is_empty() || email.is_empty() || password.is_empty() {
                println!("错误: --org, --admin, --password 都不能为空");
                return Ok(());
            }

            let payload = serde_json::json!({
                "org_name": org_name,
                "admin_email": email,
                "admin_password": password,
                "plan": "enterprise",
            });

            match client.post(format!("{}/admin/auth/register", server_url))
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    println!("注册结果: {}", serde_json::to_string_pretty(&body).unwrap());
                }
                Err(e) => eprintln!("请求失败: {}", e),
            }
        }
        "users" if args.len() >= 3 && args[2] == "list" => {
            match client.get(format!("{}/admin/users", server_url)).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    println!("用户列表: {}", serde_json::to_string_pretty(&body).unwrap());
                }
                Err(e) => eprintln!("请求失败: {}", e),
            }
        }
        "users" if args.len() >= 3 && args[2] == "create" => {
            let mut email = String::new();
            let mut role = "developer".to_string();

            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--email" => { i += 1; email = args[i].clone(); }
                    "--role" => { i += 1; role = args[i].clone(); }
                    _ => {}
                }
                i += 1;
            }

            let payload = serde_json::json!({
                "email": email,
                "name": email.split('@').next().unwrap_or("user"),
                "password": "changeme123",
                "role": role,
            });

            match client.post(format!("{}/admin/users", server_url))
                .json(&payload)
                .send()
                .await
            {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    println!("创建用户: {}", serde_json::to_string_pretty(&body).unwrap());
                }
                Err(e) => eprintln!("请求失败: {}", e),
            }
        }
        "usage" => {
            let days = args.iter().position(|a| a == "--days")
                .and_then(|i| args.get(i + 1))
                .and_then(|v| v.parse().ok())
                .unwrap_or(7);

            match client.get(format!("{}/admin/usage?days={}", server_url, days)).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    println!("用量统计: {}", serde_json::to_string_pretty(&body).unwrap());
                }
                Err(e) => eprintln!("请求失败: {}", e),
            }
        }
        "nodes" => {
            match client.get(format!("{}/admin/nodes", server_url)).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    println!("节点状态: {}", serde_json::to_string_pretty(&body).unwrap());
                }
                Err(e) => eprintln!("请求失败: {}", e),
            }
        }
        "api-key" if args.len() >= 3 && args[2] == "generate" => {
            match client.post(format!("{}/admin/api-keys", server_url)).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    println!("API Key: {}", serde_json::to_string_pretty(&body).unwrap());
                }
                Err(e) => eprintln!("请求失败: {}", e),
            }
        }
        "metrics" => {
            match client.get(format!("{}/admin/metrics", server_url)).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    println!("系统指标: {}", serde_json::to_string_pretty(&body).unwrap());
                }
                Err(e) => eprintln!("请求失败: {}", e),
            }
        }
        "models" => {
            match client.get(format!("{}/admin/models", server_url)).send().await {
                Ok(resp) => {
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    println!("已配置模型: {}", serde_json::to_string_pretty(&body).unwrap());
                }
                Err(e) => eprintln!("请求失败: {}", e),
            }
        }
        "--help" | "-h" => { print_help(); }
        _ => {
            println!("未知命令: {}", args[1]);
            print_help();
        }
    }

    Ok(())
}

fn print_help() {
    println!("CarpAI Enterprise Admin CLI");
    println!("用法: carpai-admin-cli <子命令> [选项]");
    println!();
    println!("子命令:");
    println!("  register      注册新组织");
    println!("    --org <名称>         组织名称");
    println!("    --admin <邮箱>       管理员邮箱");
    println!("    --password <密码>    管理员密码");
    println!("  users          用户管理");
    println!("    list                 列出用户");
    println!("    create               创建用户");
    println!("      --email <邮箱>     用户邮箱");
    println!("      --role <角色>      角色 (admin/developer/viewer)");
    println!("  usage          用量统计");
    println!("    --days <天数>        查询天数 (默认7)");
    println!("  nodes          查看节点状态");
    println!("  api-key        管理 API Key");
    println!("    generate             生成新的 API Key");
    println!("  metrics        系统指标");
    println!("  models         查看已配置模型");
    println!();
    println!("环境变量:");
    println!("  CARPAI_SERVER  服务器地址 (默认 http://localhost:8000)");
}
