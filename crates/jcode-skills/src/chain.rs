use crate::skill::{SkillInput, SkillOutput, SkillStatus};
use crate::SkillsEngine;
use std::collections::HashMap;

/// 链式调用上下文 — 在技能之间传递数据
#[derive(Debug, Clone, Default)]
pub struct ChainContext {
    pub project_root: String,
    pub target_branch: String,
    pub commit_message: String,
    pub extra: HashMap<String, String>,
}

/// 链式调用结果
#[derive(Debug, Clone)]
pub struct ChainResult {
    pub results: Vec<SkillOutput>,
    pub all_success: bool,
    pub summary: String,
    /// 所有向后传递的产物路径（累积）
    pub accumulated_artifacts: Vec<String>,
}

/// 链式调用器
pub struct ChainCaller<'a> {
    engine: &'a SkillsEngine,
}

impl<'a> ChainCaller<'a> {
    pub fn new(engine: &'a SkillsEngine) -> Self { Self { engine } }

    /// 按顺序执行技能链，自动将前一个技能的产物传递给下一个技能
    pub async fn execute(&self, skill_names: &[&str], context: ChainContext) -> anyhow::Result<ChainResult> {
        let mut results = Vec::new();
        let mut all_success = true;
        let mut summary = String::new();
        let mut accumulated_artifacts: Vec<String> = Vec::new();

        for (i, name) in skill_names.iter().enumerate() {
            let skill = match self.engine.get(name) {
                Some(s) => s,
                None => {
                    summary.push_str(&format!("❌ Step {}: Skill '{}' not found\n", i + 1, name));
                    all_success = false;
                    continue;
                }
            };

            tracing::info!("[Chain] Step {}/{}: {} ({})", i + 1, skill_names.len(), name, skill.description());

            // 将累积产物和上下文注入参数
            let mut parameters = context.extra.clone();
            if !accumulated_artifacts.is_empty() {
                parameters.insert("prev_artifacts".to_string(), accumulated_artifacts.join(","));
            }
            parameters.insert("project_root".to_string(), context.project_root.clone());
            parameters.insert("target_branch".to_string(), context.target_branch.clone());
            parameters.insert("commit_message".to_string(), context.commit_message.clone());
            parameters.insert("_chain_step".to_string(), (i + 1).to_string());
            parameters.insert("_chain_total".to_string(), skill_names.len().to_string());

            let input = SkillInput { parameters };
            let output = skill.execute(input).await?;

            // 累积产物
            for art in &output.artifacts {
                if !accumulated_artifacts.contains(art) {
                    accumulated_artifacts.push(art.clone());
                }
            }

            let status = if output.status == SkillStatus::Success { "✅" } else { "❌" };
            summary.push_str(&format!("{} Step {}: {} — {}\n", status, i + 1, name, output.message));

            if output.status != SkillStatus::Success {
                all_success = false;
            }

            results.push(output);
        }

        Ok(ChainResult { results, all_success, summary, accumulated_artifacts })
    }
}
