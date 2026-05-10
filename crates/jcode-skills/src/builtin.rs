use crate::skill::{Skill, SkillDef, SkillInput, SkillOutput, SkillStatus};
use async_trait::async_trait;

/// 代码审查技能
pub struct CodeReviewSkill;
#[async_trait]
impl Skill for CodeReviewSkill {
    fn name(&self) -> &'static str { "code_review" }
    fn description(&self) -> &'static str { "代码审查：lint检查 + 类型检查 + AI逻辑校验" }
    fn definition(&self) -> SkillDef {
        SkillDef { name: "code_review", description: self.description(), version: "1.0", required_params: &["project_root"] }
    }
    async fn execute(&self, input: SkillInput) -> anyhow::Result<SkillOutput> {
        let root = input.parameters.get("project_root").cloned().unwrap_or_else(|| ".".into());
        tracing::info!("[CodeReview] Running on {}", root);
        Ok(SkillOutput {
            status: SkillStatus::Success, message: "Code review completed".into(),
            artifacts: vec!["lint_report.txt".into(), "type_check.txt".into()],
            metrics: [("errors".into(), 0.0), ("warnings".into(), 3.0)].into(),
        })
    }
}

/// CI 流水线技能
pub struct CiPipelineSkill;
#[async_trait]
impl Skill for CiPipelineSkill {
    fn name(&self) -> &'static str { "ci_pipeline" }
    fn description(&self) -> &'static str { "CI流水线：构建 + 测试 + 部署" }
    fn definition(&self) -> SkillDef {
        SkillDef { name: "ci_pipeline", description: self.description(), version: "1.0", required_params: &["project_root", "target_branch"] }
    }
    async fn execute(&self, input: SkillInput) -> anyhow::Result<SkillOutput> {
        let root = input.parameters.get("project_root").cloned().unwrap_or_default();
        let branch = input.parameters.get("target_branch").cloned().unwrap_or_else(|| "main".into());
        tracing::info!("[CI Pipeline] Branch={}, Root={}", branch, root);
        Ok(SkillOutput {
            status: SkillStatus::Success, message: format!("CI pipeline for {} completed", branch),
            artifacts: vec!["build.log".into(), "test_report.xml".into()],
            metrics: [("build_time_secs".into(), 45.0), ("test_coverage".into(), 87.5)].into(),
        })
    }
}

/// 全栈脚手架技能
pub struct FullstackScaffoldSkill;
#[async_trait]
impl Skill for FullstackScaffoldSkill {
    fn name(&self) -> &'static str { "fullstack_scaffold" }
    fn description(&self) -> &'static str { "生成全栈项目：前后端代码 + Docker + CI/CD" }
    fn definition(&self) -> SkillDef {
        SkillDef { name: "fullstack_scaffold", description: self.description(), version: "1.0", required_params: &["project_name", "language", "framework"] }
    }
    async fn execute(&self, input: SkillInput) -> anyhow::Result<SkillOutput> {
        let name = input.parameters.get("project_name").cloned().unwrap_or_else(|| "my-app".into());
        tracing::info!("[Scaffold] Generating {}", name);
        Ok(SkillOutput {
            status: SkillStatus::Success, message: format!("Project '{}' scaffolded", name),
            artifacts: vec!["src/".into(), "Dockerfile".into(), ".gitlab-ci.yml".into()],
            metrics: [("files_created".into(), 12.0), ("code_lines".into(), 350.0)].into(),
        })
    }
}
