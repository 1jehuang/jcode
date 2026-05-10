//! # jcode-skills
//! Skills 模式 & 链式调用 — 将复杂开发流程封装成标准技能。
//!
//! ## 架构
//! ```text
//! TAPD需求 → IDE编码 → 单元测试 → 部署
//!    ↓          ↓          ↓         ↓
//! Skill 1    Skill 2    Skill 3   Skill 4
//! ```
//!
//! ## 使用示例
//! ```rust
//! let engine = SkillsEngine::new();
//! let result = engine.run_chain(&["code_review", "ci_pipeline"], context).await?;
//! ```
//!
//! ## TAPD → 部署流水线
//! ```rust
//! let engine = SkillsEngine::new();
//! let result = engine.tapd_to_deploy(context).await?;
//! // 自动执行: tapd_fetch → fullstack_scaffold → ci_pipeline → deploy
//! ```

mod skill;
mod chain;
mod builtin;
mod deploy;
mod tapd_skill;

pub use skill::{Skill, SkillDef, SkillInput, SkillOutput, SkillStatus};
pub use chain::{ChainCaller, ChainContext, ChainResult};
pub use builtin::{CodeReviewSkill, CiPipelineSkill, FullstackScaffoldSkill};
pub use deploy::DeploySkill;
pub use tapd_skill::TapdSkill;

use std::collections::HashMap;
use std::sync::Arc;

/// 预定义流水线名称常量
pub mod pipelines {
    /// TAPD 需求 → 代码生成 → CI → 部署
    pub const TAPD_TO_DEPLOY: &[&str] = &["tapd_fetch", "fullstack_scaffold", "ci_pipeline", "deploy"];
    /// 仅 CI → 部署
    pub const CI_TO_DEPLOY: &[&str] = &["ci_pipeline", "deploy"];
    /// TAPD → 代码审查
    pub const TAPD_TO_REVIEW: &[&str] = &["tapd_fetch", "code_review"];
}

/// Skills 引擎 — 注册 + 发现 + 链式执行
pub struct SkillsEngine {
    skills: HashMap<String, Arc<dyn Skill>>,
}

impl SkillsEngine {
    pub fn new() -> Self {
        let mut engine = Self { skills: HashMap::new() };
        engine.register_builtins();
        engine
    }

    fn register_builtins(&mut self) {
        self.register(Arc::new(CodeReviewSkill));
        self.register(Arc::new(CiPipelineSkill));
        self.register(Arc::new(FullstackScaffoldSkill));
        self.register(Arc::new(DeploySkill));
        self.register(Arc::new(TapdSkill));
    }

    pub fn register(&mut self, skill: Arc<dyn Skill>) {
        self.skills.insert(skill.name().to_string(), skill);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Skill>> {
        self.skills.get(name).cloned()
    }

    pub fn list(&self) -> Vec<(&str, &str)> {
        self.skills.iter().map(|(n, s)| (n.as_str(), s.description())).collect()
    }

    /// 链式调用多个技能
    pub async fn run_chain(&self, skill_names: &[&str], context: ChainContext) -> anyhow::Result<ChainResult> {
        let caller = ChainCaller::new(self);
        caller.execute(skill_names, context).await
    }

    /// TAPD → 部署 全自动流水线
    ///
    /// 从 TAPD 获取需求 → 生成项目代码 → CI 构建测试 → 部署到目标环境
    pub async fn tapd_to_deploy(&self, context: ChainContext) -> anyhow::Result<ChainResult> {
        self.run_chain(pipelines::TAPD_TO_DEPLOY, context).await
    }

    /// CI → 部署 流水线
    ///
    /// 对已有代码运行 CI 检查后直接部署
    pub async fn ci_to_deploy(&self, context: ChainContext) -> anyhow::Result<ChainResult> {
        self.run_chain(pipelines::CI_TO_DEPLOY, context).await
    }

    /// TAPD → 审查 流水线
    ///
    /// 从 TAPD 获取需求后进行代码审查
    pub async fn tapd_to_review(&self, context: ChainContext) -> anyhow::Result<ChainResult> {
        self.run_chain(pipelines::TAPD_TO_REVIEW, context).await
    }
}
