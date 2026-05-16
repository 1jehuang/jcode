//! # TAPD Skill — 从 TAPD 获取需求并创建代码任务
//!
//! 支持两种模式:
//! 1. API 模式: 通过 TAPD API 获取需求 (需设置 TAPD_API_URL, TAPD_API_TOKEN 环境变量)
//! 2. 本地文件模式: 从 `.tapd/requirements.json` 读取

use crate::skill::{Skill, SkillDef, SkillInput, SkillOutput, SkillStatus};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// TAPD 需求项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TapdRequirement {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub status: Option<String>,
    pub owner: Option<String>,
}

/// TAPD 需求列表响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TapdResponse {
    pub count: usize,
    pub items: Vec<TapdRequirement>,
}

/// TAPD 技能 — 从 TAPD 获取需求并生成代码任务
pub struct TapdSkill;

impl TapdSkill {
    /// 从 TAPD API 获取需求
    async fn fetch_from_api() -> anyhow::Result<Vec<TapdRequirement>> {
        let api_url = std::env::var("TAPD_API_URL")
            .map_err(|_| anyhow::anyhow!("TAPD_API_URL not set"))?;
        let api_token = std::env::var("TAPD_API_TOKEN")
            .map_err(|_| anyhow::anyhow!("TAPD_API_TOKEN not set"))?;

        let client = reqwest::Client::new();
        let resp = client
            .get(&api_url)
            .header("Authorization", format!("Bearer {}", api_token))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("TAPD API request failed: {}", e))?;

        if !resp.status().is_success() {
            anyhow::bail!("TAPD API returned status: {}", resp.status());
        }

        let data: TapdResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse TAPD response: {}", e))?;

        Ok(data.items)
    }

    /// 从本地文件获取需求
    fn fetch_from_file() -> anyhow::Result<Vec<TapdRequirement>> {
        let path = std::path::Path::new(".tapd").join("requirements.json");
        if !path.exists() {
            anyhow::bail!(".tapd/requirements.json not found. Create this file or set TAPD_API_URL");
        }
        let content = std::fs::read_to_string(&path)?;
        let data: TapdResponse = serde_json::from_str(&content)?;
        Ok(data.items)
    }

    /// 生成任务计划
    fn generate_task_plan(requirements: &[TapdRequirement]) -> String {
        let mut plan = String::from("## TAPD 需求 -> 代码任务计划\n\n");
        for (i, req) in requirements.iter().enumerate() {
            plan.push_str(&format!(
                "### {}. {} [{}]\n",
                i + 1,
                req.title,
                req.priority.as_deref().unwrap_or("normal")
            ));
            if let Some(desc) = &req.description {
                plan.push_str(&format!("   {}\n", desc));
            }
            plan.push_str(&format!(
                "   任务: 实现 `{}`\n\n",
                req.title
            ));
        }
        plan
    }
}

#[async_trait]
impl Skill for TapdSkill {
    fn name(&self) -> &'static str {
        "tapd_fetch"
    }

    fn description(&self) -> &'static str {
        "从 TAPD 获取需求 -> 生成代码任务计划"
    }

    fn definition(&self) -> SkillDef {
        SkillDef {
            name: "tapd_fetch",
            description: self.description(),
            version: "1.0",
            required_params: &[],
        }
    }

    async fn execute(&self, _input: SkillInput) -> anyhow::Result<SkillOutput> {
        tracing::info!("[TAPD] Fetching requirements...");

        // Try API first, fallback to local file
        let requirements = Self::fetch_from_api()
            .await
            .or_else(|_| Self::fetch_from_file())
            .map_err(|e| anyhow::anyhow!("Failed to fetch TAPD requirements: {}", e))?;

        let plan = Self::generate_task_plan(&requirements);

        let metrics: HashMap<String, f64> = [
            ("requirements_count".into(), requirements.len() as f64),
        ]
        .into();

        tracing::info!(
            "[TAPD] Fetched {} requirements, generated task plan ({} chars)",
            requirements.len(),
            plan.len()
        );

        Ok(SkillOutput {
            status: SkillStatus::Success,
            message: format!("获取 {} 条需求，生成任务计划", requirements.len()),
            artifacts: vec!["tapd_plan.md".into()],
            metrics,
        })
    }
}
