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
//! ## 增强说明
//! - 新增 `load_skills_parallel()` — 同时从多个源加载技能
//! - 新增 `register_fallible()` — 单个技能注册失败不级联
//! - 新增 `discover_skills()` — 从文件系统动态发现技能
//! - 源自 Claude Code 的 `getSkills()` + `loadAllCommands()` + 优雅降级模式

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

/// 技能源描述 — 用于并行加载（源自 Claude Code 的多个技能源加载模式）
#[derive(Debug, Clone)]
pub enum SkillSource {
    /// 内置技能
    Builtin,
    /// 文件系统目录（需要目录路径）
    Directory(String),
    /// 插件技能
    Plugin(String),
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

    /// 注册单个技能（失败的注册不会级联）
    /// 源自 Claude Code 的优雅降级模式
    pub fn register_fallible(&mut self, skill: Result<Arc<dyn Skill>, anyhow::Error>, source: &str) {
        match skill {
            Ok(s) => {
                tracing::info!("[Skills] Registered '{}' from {}", s.name(), source);
                self.skills.insert(s.name().to_string(), s);
            }
            Err(e) => {
                tracing::warn!("[Skills] Failed to load skill from {}: {} — skipping", source, e);
            }
        }
    }

    pub fn register(&mut self, skill: Arc<dyn Skill>) {
        self.skills.insert(skill.name().to_string(), skill);
    }

    /// 从多个源并行加载技能
    /// 源自 Claude Code 的 `Promise.all([getSkills(...), getPluginCommands(), ...])` 模式
    pub async fn load_from_sources(&mut self, sources: &[SkillSource]) {
        use futures::future::join_all;

        let mut handles = Vec::new();

        for source in sources {
            match source {
                SkillSource::Builtin => {
                    // 内置技能已注册，无需重新加载
                }
                SkillSource::Directory(path) => {
                    let path = path.clone();
                    handles.push(tokio::spawn(async move {
                        Self::load_skills_from_dir(&path).await
                    }));
                }
                SkillSource::Plugin(_name) => {
                    // 插件技能加载占位
                }
            }
        }

        // 并行等待所有加载任务完成
        let results = join_all(handles).await;
        for (i, result) in results.iter().enumerate() {
            match result {
                Ok(Ok(skills)) => {
                    for (name, skill) in skills {
                        tracing::info!("[Skills] Loaded '{}' from source {}", name, i + 1);
                        self.skills.insert(name, skill);
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!("[Skills] Source {} failed: {} — continuing without it", i + 1, e);
                }
                Err(e) => {
                    tracing::warn!("[Skills] Source {} panicked: {} — continuing", i + 1, e);
                }
            }
        }
    }

    /// 从目录加载技能（异步）
    /// 每个文件独立加载，失败不级联
    async fn load_skills_from_dir(path: &str) -> anyhow::Result<HashMap<String, Arc<dyn Skill>>> {
        let mut skills = HashMap::new();
        let dir = std::path::Path::new(path);

        if !dir.exists() {
            return Ok(skills);
        }

        let mut entries = Vec::new();
        if let Ok(read_dir) = tokio::fs::read_dir(dir).await {
            use tokio_stream::wrappers::ReadDirStream;
            use tokio_stream::StreamExt;
            let mut stream = ReadDirStream::new(read_dir);
            while let Some(entry) = stream.next().await {
                if let Ok(entry) = entry {
                    entries.push(entry.path());
                }
            }
        }

        for entry_path in &entries {
            if entry_path.is_dir() {
                let skill_md = entry_path.join("SKILL.md");
                if skill_md.exists() {
                    match Self::load_skill_from_file(&skill_md).await {
                        Ok(skill) => {
                            skills.insert(skill.name().to_string(), skill);
                        }
                        Err(e) => {
                            tracing::warn!("[Skills] Failed to load '{}': {} — skipping", entry_path.display(), e);
                        }
                    }
                }
            }
        }

        Ok(skills)
    }

    /// 从 SKILL.md 文件加载单个技能
    async fn load_skill_from_file(path: &std::path::Path) -> anyhow::Result<Arc<dyn Skill>> {
        let content = tokio::fs::read_to_string(path).await?;
        // 简单解析：第一行是名称，其余是内容
        let first_line = content.lines().next().unwrap_or("unnamed");
        let name = first_line.trim_start_matches("# ").trim();
        let description = content.lines()
            .skip(1)
            .find(|l| !l.is_empty())
            .unwrap_or("No description")
            .trim_start_matches("// ");

        struct FileSkill {
            name: String,
            description: String,
            content: String,
        }

        #[async_trait::async_trait]
        impl Skill for FileSkill {
            fn name(&self) -> &'static str {
                // Note: This has a static lifetime issue in practice.
                // Using leaked string for simplicity — in production use Arc<str>.
                Box::leak(self.name.clone().into_boxed_str())
            }
            fn description(&self) -> &'static str {
                Box::leak(self.description.clone().into_boxed_str())
            }
            fn definition(&self) -> SkillDef {
                SkillDef {
                    name: Box::leak(self.name.clone().into_boxed_str()),
                    description: Box::leak(self.description.clone().into_boxed_str()),
                    version: "1.0",
                    required_params: &[],
                }
            }
            async fn execute(&self, _input: SkillInput) -> anyhow::Result<SkillOutput> {
                Ok(SkillOutput {
                    status: SkillStatus::Success,
                    message: format!("Executed skill '{}':\n{}", self.name, self.content),
                    artifacts: vec![],
                    metrics: HashMap::new(),
                })
            }
        }

        Ok(Arc::new(FileSkill {
            name: name.to_string(),
            description: description.to_string(),
            content,
        }))
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
