//! 三大核心功能 CLI 入口
//!   jcode agent fix     — 错误修复闭环 (FixEngine + AutoFixLoop)
//!   jcode agent plan    — 自主规划闭环 (AutonomousAgent)
//!   jcode agent refactor — 语义重构闭环 (AstRenamer + ImportManager)

use std::path::Path;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum CoreAgentCommand {
    /// 错误修复: 检测→LLM修复→重新编译→循环×3
    Fix {
        /// 工作区路径 (默认: 当前目录)
        #[arg(long)]
        workspace: Option<String>,
    },
    /// 自主规划: 创建Plan→LLM生成代码→编译验证→修复
    Plan {
        /// 目标描述
        goal: String,
        /// 工作区路径
        #[arg(long)]
        workspace: Option<String>,
    },
    /// 语义重构: Tree-sitter AST 精确重命名
    Refactor {
        /// 目标文件
        file: String,
        /// 旧符号名
        old_name: String,
        /// 新符号名
        new_name: String,
        /// 工作区路径
        #[arg(long)]
        workspace: Option<String>,
    },
}

/// 执行核心Agent命令
pub async fn run_core_agent(cmd: &CoreAgentCommand) -> Result<(), String> {
    match cmd {
        CoreAgentCommand::Fix { workspace } => {
            let root = workspace.as_ref().map(Path::new).unwrap_or_else(|| Path::new("."));
            run_fix_loop(root).await
        }
        CoreAgentCommand::Plan { goal, workspace } => {
            let root = workspace.as_ref().map(Path::new).unwrap_or_else(|| Path::new("."));
            run_autonomous_plan(root, goal).await
        }
        CoreAgentCommand::Refactor { file, old_name, new_name, workspace } => {
            let root = workspace.as_ref().map(Path::new).unwrap_or_else(|| Path::new("."));
            run_ast_refactor(root, file, old_name, new_name).await
        }
    }
}

/// 错误修复闭环
async fn run_fix_loop(root: &Path) -> Result<(), String> {
    println!("🔧 错误修复闭环启动...");
    let engine = crate::compilation_engine::AutoFixLoop::new(root, Default::default());
    let result = engine.run_cycle(&[]).await?;

    if result.success {
        println!("✅ 编译通过! ({}轮)", result.iterations);
    } else {
        println!("❌ 仍有 {} 个错误未修复 (已修复 {})", result.remaining_errors, result.errors_fixed);
        if !result.fix_applied.is_empty() {
            println!("   已修改文件: {:?}", result.fix_applied);
        }
    }
    println!("{}", engine.stats().await);
    Ok(())
}

/// 自主规划闭环
async fn run_autonomous_plan(root: &Path, goal: &str) -> Result<(), String> {
    println!("📋 自主规划闭环启动...");
    println!("目标: {}", goal);

    let agent = crate::agent_runtime::AutonomousAgent::new(root);
    let result = agent.execute_task(goal).await?;
    println!("{}", result);
    println!("{}", agent.stats().await);
    Ok(())
}

/// 语义重构闭环 (AST 精确重命名)
async fn run_ast_refactor(root: &Path, file: &str, old_name: &str, new_name: &str) -> Result<(), String> {
    println!("🔍 语义重构闭环启动...");
    println!("文件: {}", file);
    println!("重命名: '{}' → '{}'", old_name, new_name);

    let renamer = crate::refactor::semantic::AstRenamer::new(root);
    let result = renamer.rename_ast(file, old_name, new_name).await?;
    println!("✅ {}", result);
    Ok(())
}
