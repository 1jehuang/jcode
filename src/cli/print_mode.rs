//! Print Mode (-p flag) 实现
//!
//! Claude Code兼容: `claude -p "query"` 
//! 功能: 非交互式模式，执行单个查询后退出
//!
//! ## 使用示例
//! ```bash
//! carpai -p "解释这个函数"
//! carpai -p --json "分析代码质量"
//! cat file.txt | carpai -p "总结内容"
//! ```

use anyhow::Result;
use std::io::{self, IsTerminal, Read};

use crate::agent::Agent;
use crate::cli::provider_init::{init_provider_and_registry, ProviderChoice};

/// Print模式配置
#[derive(Debug, Clone)]
pub struct PrintModeConfig {
    /// 查询文本
    pub query: String,
    
    /// 是否输出JSON格式
    pub json_output: bool,
    
    /// 是否输出NDJSON (流式JSON)
    pub ndjson: bool,
    
    /// 附加的系统提示
    pub system_prompt: Option<String>,
    
    /// 工作目录
    pub cwd: Option<String>,
    
    /// 模型名称
    pub model: Option<String>,
    
    /// 最大Token数
    pub max_tokens: Option<usize>,
    
    /// 温度参数
    pub temperature: Option<f64>,

    /// 提供商选择
    pub provider_choice: Option<ProviderChoice>,
}

impl Default for PrintModeConfig {
    fn default() -> Self {
        Self {
            query: String::new(),
            json_output: false,
            ndjson: false,
            system_prompt: None,
            cwd: None,
            model: None,
            max_tokens: None,
            temperature: None,
            provider_choice: None,
        }
    }
}

/// 运行Print模式 (非交互式单次查询)
pub async fn run_print_mode(config: PrintModeConfig) -> Result<()> {
    // 1. 检查是否有管道输入
    let piped_content = read_piped_input();
    
    // 2. 构建完整查询
    let full_query = if let Some(content) = piped_content {
        if config.query.is_empty() {
            content
        } else {
            format!("{}\n\n[管道输入]\n{}", config.query, content)
        }
    } else {
        if config.query.is_empty() {
            anyhow::bail!("Print模式需要提供查询文本或管道输入。用法: carpai -p \"查询\" 或 echo 内容 | carpai -p");
        }
        config.query
    };
    
    // 3. 显示模式信息 (非quiet模式)
    if !config.json_output && !config.ndjson {
        eprintln!("🚀 CarpAI Print Mode (Claude Code兼容)");
        eprintln!("📝 查询: {}...", full_query.chars().take(50).collect::<String>());
        if let Some(model) = &config.model {
            eprintln!("🤖 模型: {}", model);
        }
        eprintln!();
    }
    
    // 4. 初始化Provider和Registry
    let provider_choice = config.provider_choice.as_ref().unwrap_or(&ProviderChoice::Jcode);
    let (provider, registry) = init_provider_and_registry(provider_choice, config.model.as_deref()).await?;
    
    // 5. 创建Agent并执行查询
    let mut agent = Agent::new(provider, registry);
    
    // 应用配置
    if let Some(cwd) = &config.cwd {
        agent.set_working_directory(cwd)?;
    }
    
    if let Some(model) = &config.model {
        agent.set_model_provider(model)?;
    }
    
    if let Some(prompt) = &config.system_prompt {
        agent.append_system_prompt(prompt)?;
    }
    
    if let Some(max_tokens) = config.max_tokens {
        agent.set_max_tokens(max_tokens as u32);
    }
    
    if let Some(temp) = config.temperature {
        agent.set_temperature(temp);
    }
    
    // 5. 执行查询
    if config.json_output {
        // JSON输出模式
        let result = agent.query_json(&full_query).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if config.ndjson {
        // NDJSON流式输出模式 - temporarily skip
        println!("{{\"status\":\"error\",\"message\":\"ndjson mode temporarily not implemented\"}}");
    } else {
        // 标准文本输出模式
        let response = agent.query(&full_query).await?;
        print!("{}", response);
    }
    
    Ok(())
}

/// 读取管道输入 (如果有)
fn read_piped_input() -> Option<String> {
    // 检查stdin是否是终端 (如果不是说明有管道输入)
    if std::io::stdin().is_terminal() {
        return None;
    }
    
    let mut content = String::new();
    match io::stdin().read_to_string(&mut content) {
        Ok(0) => None, // 空输入
        Ok(_) => {
            // 清理末尾换行
            while content.ends_with('\n') || content.ends_with('\r') {
                content.pop();
            }
            Some(content)
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_print_mode_requires_query_or_pipe() {
        let config = PrintModeConfig::default();
        let result = run_print_mode(config).await;
        
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("需要提供查询"));
    }
    
    #[test]
    fn test_read_piped_input_returns_none_for_tty() {
        // 在测试环境中stdin通常是TTY，所以应该返回None
        // 这个测试主要验证函数不会panic
        let result = read_piped_input();
        assert!(result.is_none()); // 大多数情况下
    }
}
