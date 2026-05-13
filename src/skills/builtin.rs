use super::skill::{SkillDefinition, SkillCategory};
use super::registry::SkillRegistry;

/// Load all built-in skills into the registry
pub async fn load_builtin_skills(registry: &SkillRegistry) {
    let skills = vec![
        builtin_fix_build(),
        builtin_run_tests(),
        builtin_code_review(),
        builtin_security_audit(),
        builtin_refactor(),
        builtin_optimize(),
        builtin_lint_fix(),
        builtin_document(),
        builtin_deploy(),
        builtin_git_cleanup(),
        builtin_dependency_update(),
        builtin_benchmark(),
    ];

    for skill in skills {
        registry.register(&skill.name.clone(), skill, None).await;
    }
}

fn builtin_fix_build() -> SkillDefinition {
    SkillDefinition::new("fix-build", "Analyze and fix build errors automatically", SkillCategory::Build)
        .with_param("target", "Build target (debug/release)", true)
        .with_tag("build")
        .with_tag("fix")
}

fn builtin_run_tests() -> SkillDefinition {
    SkillDefinition::new("run-tests", "Run project tests with coverage reporting", SkillCategory::Testing)
        .with_param("scope", "Test scope (unit/integration/all)", false)
        .with_param("coverage", "Enable coverage report", false)
        .with_tag("test")
        .with_tag("coverage")
}

fn builtin_code_review() -> SkillDefinition {
    SkillDefinition::new("code-review", "Perform comprehensive code review on changes", SkillCategory::Review)
        .with_param("files", "Files to review (comma-separated)", true)
        .with_param("depth", "Review depth (basic/detailed)", false)
        .with_tag("review")
        .with_tag("quality")
}

fn builtin_security_audit() -> SkillDefinition {
    SkillDefinition::new("security-audit", "Run security audit on project dependencies and code", SkillCategory::Security)
        .with_param("level", "Audit level (quick/full)", false)
        .with_tag("security")
        .with_tag("audit")
}

fn builtin_refactor() -> SkillDefinition {
    SkillDefinition::new("refactor", "Refactor code with suggested improvements", SkillCategory::Development)
        .with_param("target", "Component or module to refactor", true)
        .with_tag("refactor")
        .with_tag("improvement")
}

fn builtin_optimize() -> SkillDefinition {
    SkillDefinition::new("optimize", "Optimize code performance", SkillCategory::Development)
        .with_param("target", "Code to optimize", true)
        .with_tag("performance")
}

fn builtin_lint_fix() -> SkillDefinition {
    SkillDefinition::new("lint-fix", "Auto-fix linting issues in the codebase", SkillCategory::Development)
        .with_param("path", "Directory or file to fix", false)
        .with_tag("lint")
}

fn builtin_document() -> SkillDefinition {
    SkillDefinition::new("document", "Generate documentation for code", SkillCategory::Documentation)
        .with_param("target", "Code to document", true)
        .with_param("format", "Output format (markdown/rustdoc)", false)
        .with_tag("docs")
}

fn builtin_deploy() -> SkillDefinition {
    SkillDefinition::new("deploy", "Deploy project to target environment", SkillCategory::Deploy)
        .with_param("environment", "Target environment", true)
        .with_param("version", "Version to deploy", true)
        .with_tag("deploy")
        .with_tag("release")
}

fn builtin_git_cleanup() -> SkillDefinition {
    SkillDefinition::new("git-cleanup", "Clean up merged git branches", SkillCategory::Git)
        .with_param("dry_run", "Preview without deleting", false)
        .with_tag("git")
}

fn builtin_dependency_update() -> SkillDefinition {
    SkillDefinition::new("update-deps", "Update project dependencies safely", SkillCategory::Development)
        .with_param("check_only", "Only check for updates, don't apply", false)
        .with_tag("dependencies")
}

fn builtin_benchmark() -> SkillDefinition {
    SkillDefinition::new("benchmark", "Run performance benchmarks", SkillCategory::Testing)
        .with_param("target", "Benchmark target", false)
        .with_tag("performance")
        .with_tag("benchmark")
}