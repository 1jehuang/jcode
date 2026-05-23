//! Plan CLI — 计划文件读/写/恢复
//!
//! 对标 Claude Code 的 /plan 命令 + plans.ts 持久化
//!
//! 命令:
//!   jcode plan new <goal>            — 创建新计划
//!   jcode plan list                  — 列出所有计划
//!   jcode plan show <slug>           — 显示计划内容
//!   jcode plan edit <slug>           — 编辑计划
//!   jcode plan recover <slug>        — 恢复计划 (三级)
//!   jcode plan fork <src> <dst>      — 分支计划
//!   jcode plan delete <slug>         — 删除计划

use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::claude_agent_port::PlanManager;

/// Plan CLI 处理器
pub struct PlanCli {
    manager: Arc<PlanManager>,
}

impl PlanCli {
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            manager: Arc::new(PlanManager::new(workspace_root)),
        }
    }

    /// 创建新计划
    pub async fn create(&self, goal: &str) -> Result<String, String> {
        let slug = self.manager.generate_slug();
        let content = self.build_plan_content(slug.clone(), goal);
        let path = self.manager.save_plan(&slug, &content, None).await?;
        Ok(format!("✅ Created plan '{}'\n   File: {}\n   Run: jcode plan show {}", slug, path.display(), slug))
    }

    /// 列出所有计划
    pub async fn list(&self) -> Result<String, String> {
        let plans = self.manager.list_plans().await?;
        if plans.is_empty() {
            return Ok("📋 No plans found. Create one with: jcode plan new \"<goal>\"".to_string());
        }
        let mut out = format!("📋 Plans ({}):\n", plans.len());
        for plan in &plans {
            let slug = plan.trim_end_matches(".md");
            out.push_str(&format!("  · {}  (jcode plan show {})\n", slug, slug));
        }
        Ok(out)
    }

    /// 显示计划内容
    pub async fn show(&self, slug: &str) -> Result<String, String> {
        let content = self.manager.load_plan(slug).await?;
        Ok(format!("━━━ Plan: {} ━━━\n\n{}", slug, content))
    }

    /// 恢复计划 (三级: 文件→快照→新建)
    pub async fn recover(&self, slug: &str, fallback: Option<&str>) -> Result<String, String> {
        match self.manager.recover_plan(slug, fallback).await {
            Some(content) => {
                // 如果是从 fallback 恢复的, 重新保存到文件
                if fallback.is_some() {
                    self.manager.save_plan(slug, &content, None).await?;
                }
                Ok(format!("🔄 Recovered plan '{}'\n\n{}", slug, content))
            }
            None => {
                // 三级恢复都失败 → 新建空计划
                let content = format!("# Plan: {}\n\n> Recovered empty - edit to add steps\n", slug);
                self.manager.save_plan(slug, &content, None).await?;
                Ok(format!("🆕 Created empty plan '{}'. Edit with: jcode plan edit {}", slug, slug))
            }
        }
    }

    /// 分支计划 (用于 fork 的 session)
    pub async fn fork(&self, src_slug: &str, dst_slug: &str) -> Result<String, String> {
        self.manager.fork_plan(src_slug, dst_slug).await?;
        Ok(format!("🔀 Forked '{}' → '{}'", src_slug, dst_slug))
    }

    /// 删除计划
    pub async fn delete(&self, slug: &str) -> Result<String, String> {
        let path = self.manager.plans_dir().join(format!("{}.md", slug));
        if path.exists() {
            tokio::fs::remove_file(&path).await
                .map_err(|e| format!("Delete failed: {}", e))?;
            Ok(format!("🗑️  Deleted plan '{}'", slug))
        } else {
            Err(format!("Plan '{}' not found", slug))
        }
    }

    /// 构建计划内容
    fn build_plan_content(&self, slug: String, goal: &str) -> String {
        format!(
            "# Plan: {}\n\n\
             ## Goal\n\
             {}\n\n\
             ## Steps\n\
             1. Analyze codebase structure\n\
             2. Plan the changes\n\
             3. Implement changes\n\
             4. Run cargo check\n\
             5. Fix compilation errors\n\
             6. Verify the result\n\n\
             ## Status\n\
             - Created: {:?}\n\
             - Slug: {}\n",
            slug, goal, std::time::SystemTime::now(), slug
        )
    }
}
