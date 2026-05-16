use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 技能输入
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInput {
    pub parameters: std::collections::HashMap<String, String>,
}

/// 技能输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillOutput {
    pub status: SkillStatus,
    pub message: String,
    pub artifacts: Vec<String>,
    pub metrics: std::collections::HashMap<String, f64>,
}

/// 技能状态
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SkillStatus { Pending, Running, Success, Failed, Warning }

/// 技能定义
#[derive(Debug, Clone)]
pub struct SkillDef {
    pub name: &'static str,
    pub description: &'static str,
    pub version: &'static str,
    pub required_params: &'static [&'static str],
}

/// 技能 trait
#[async_trait]
pub trait Skill: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn definition(&self) -> SkillDef;
    async fn execute(&self, input: SkillInput) -> anyhow::Result<SkillOutput>;
}
