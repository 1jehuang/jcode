use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct ProjectInitOptions {
    pub force: bool,
    pub yes: bool,
    pub include_memory_wiki: bool,
}

impl Default for ProjectInitOptions {
    fn default() -> Self {
        Self {
            force: false,
            yes: false,
            include_memory_wiki: true,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ProjectInitReport {
    pub root: PathBuf,
    pub files_written: Vec<PathBuf>,
    pub files_skipped: Vec<PathBuf>,
    pub detected_stack: Vec<String>,
    pub next_steps: Vec<String>,
}

pub fn run_project_init(root: &Path, options: ProjectInitOptions) -> Result<ProjectInitReport> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let analysis = analyze_project(&root);
    let mut report = ProjectInitReport {
        root: root.clone(),
        files_written: Vec::new(),
        files_skipped: Vec::new(),
        detected_stack: analysis.detected_stack.clone(),
        next_steps: vec![
            "Review AGENTS.md and .jcode/INIT_QUESTIONS.md".to_string(),
            "Let `/init` finish its swarm analysis before treating generated plans as complete"
                .to_string(),
            "Run `jcode-harness skills doctor`".to_string(),
            "Run `jcode memory wiki init` if using Living Memory".to_string(),
            "Review .jcode/mcp.json before enabling any MCP server".to_string(),
        ],
    };

    std::fs::create_dir_all(root.join(".jcode"))?;
    std::fs::create_dir_all(root.join(".jcode/init"))?;
    std::fs::create_dir_all(root.join(".jcode/side_panel"))?;

    write_project_file(
        &root.join("AGENTS.md"),
        &agents_md(&analysis),
        options.force,
        &mut report,
    )?;
    write_project_file(
        &root.join(".jcode/INIT_REPORT.md"),
        &init_report_md(&analysis),
        options.force,
        &mut report,
    )?;
    write_project_file(
        &root.join(".jcode/INIT_QUESTIONS.md"),
        &init_questions_md(),
        options.force,
        &mut report,
    )?;
    write_project_file(
        &root.join(".jcode/init/SWARM_ANALYSIS_PLAN.md"),
        &swarm_analysis_plan_md(&analysis),
        options.force,
        &mut report,
    )?;
    write_project_file(
        &root.join(".jcode/init/SWARM_ANALYSIS_REPORT.md"),
        &swarm_analysis_report_stub_md(),
        options.force,
        &mut report,
    )?;
    write_project_file(
        &root.join(".jcode/SKILLS_PLAN.md"),
        &skills_plan_md(&analysis),
        options.force,
        &mut report,
    )?;
    write_project_file(
        &root.join(".jcode/MCP_PLAN.md"),
        &mcp_plan_md(&analysis),
        options.force,
        &mut report,
    )?;
    write_project_file(
        &root.join(".jcode/mcp.json"),
        &mcp_json_stub(),
        options.force,
        &mut report,
    )?;
    write_project_file(
        &root.join(".jcode/side_panel/status.md"),
        &side_panel_status_md(&analysis),
        options.force,
        &mut report,
    )?;
    write_project_file(
        &root.join(".jcode/side_panel/questions.md"),
        &side_panel_questions_md(),
        options.force,
        &mut report,
    )?;

    if options.include_memory_wiki {
        crate::memory_wiki::ensure_layout_at(&root.join(".jcode/memory_wiki"))?;
        report.files_written.push(root.join(".jcode/memory_wiki"));
    }

    if options.yes {
        report.next_steps.push(
            "Non-interactive mode used. Fill unanswered project questions manually.".to_string(),
        );
    }

    Ok(report)
}

#[derive(Debug)]
struct ProjectAnalysis {
    detected_stack: Vec<String>,
    important_files: Vec<String>,
    test_commands: Vec<String>,
    package_managers: Vec<String>,
}

fn analyze_project(root: &Path) -> ProjectAnalysis {
    let exists = |p: &str| root.join(p).exists();
    let mut stack = Vec::new();
    let mut tests = Vec::new();
    let mut managers = Vec::new();
    let mut important = Vec::new();

    for file in [
        "Cargo.toml",
        "package.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lockb",
        "pyproject.toml",
        "requirements.txt",
        "go.mod",
        "composer.json",
        "Gemfile",
        "deno.json",
        "Makefile",
        "docker-compose.yml",
        "Dockerfile",
        "README.md",
    ] {
        if exists(file) {
            important.push(file.to_string());
        }
    }

    if exists("Cargo.toml") {
        stack.push("Rust".into());
        tests.push("cargo test".into());
        tests.push("cargo check".into());
    }
    if exists("package.json") {
        stack.push("Node/JavaScript/TypeScript".into());
        if exists("pnpm-lock.yaml") {
            managers.push("pnpm".into());
            tests.push("pnpm test".into());
        } else if exists("yarn.lock") {
            managers.push("yarn".into());
            tests.push("yarn test".into());
        } else if exists("bun.lockb") {
            managers.push("bun".into());
            tests.push("bun test".into());
        } else {
            managers.push("npm".into());
            tests.push("npm test".into());
        }
    }
    if exists("pyproject.toml") || exists("requirements.txt") {
        stack.push("Python".into());
        tests.push("pytest".into());
    }
    if exists("go.mod") {
        stack.push("Go".into());
        tests.push("go test ./...".into());
    }
    if exists("docker-compose.yml") || exists("Dockerfile") {
        stack.push("Docker".into());
    }

    ProjectAnalysis {
        detected_stack: stack,
        important_files: important,
        test_commands: tests,
        package_managers: managers,
    }
}

fn write_project_file(
    path: &Path,
    content: &str,
    force: bool,
    report: &mut ProjectInitReport,
) -> Result<()> {
    if path.exists() && !force {
        report.files_skipped.push(path.to_path_buf());
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    report.files_written.push(path.to_path_buf());
    Ok(())
}

fn agents_md(analysis: &ProjectAnalysis) -> String {
    format!(
        "# Agent Guidelines\n\nGenerated by `jcode-harness init` on {}.\n\n## Project bootstrap\n\n- Read `.jcode/INIT_REPORT.md` before large work.\n- Answer or update `.jcode/INIT_QUESTIONS.md` when project goals/status-panel preferences change.\n- Use `.jcode/SKILLS_PLAN.md` for recommended skills and `.jcode/MCP_PLAN.md` for MCP setup decisions.\n- Do not store secrets, tokens, private keys, `.env` contents, or credentials in memory/wiki/docs.\n- Prefer small, reversible changes and validate with project commands.\n\n## Detected stack\n\n{}\n\n## Suggested validation\n\n{}\n",
        Utc::now().to_rfc3339(),
        bullet_list(&analysis.detected_stack),
        bullet_list(&analysis.test_commands),
    )
}

fn init_report_md(analysis: &ProjectAnalysis) -> String {
    format!(
        "# Jcode Harness Init Report\n\nGenerated: {}\n\n## Detected stack\n\n{}\n\n## Important files\n\n{}\n\n## Package managers\n\n{}\n\n## Validation candidates\n\n{}\n\n## Research-informed defaults\n\n- Keep `AGENTS.md` short, human-readable, and project-specific.\n- Make MCP setup explicit and reviewable. Do not auto-install remote servers without consent.\n- Keep durable memory in Markdown with provenance.\n- Prefer a side panel that shows project status, open questions, test commands, and active risks.\n",
        Utc::now().to_rfc3339(),
        bullet_list(&analysis.detected_stack),
        bullet_list(&analysis.important_files),
        bullet_list(&analysis.package_managers),
        bullet_list(&analysis.test_commands),
    )
}

fn init_questions_md() -> String {
    "# Jcode Harness Init Questions\n\nAnswer these to customize the harness for this project.\n\n## Project\n\n1. What is the main goal of this project?\n2. What commands must pass before work is considered done?\n3. What files/directories are forbidden to edit?\n4. What data is sensitive and must never enter memory?\n\n## Side panel/status\n\n1. What should always appear in the side panel?\n   - Current goal?\n   - Todo status?\n   - Test commands?\n   - Open risks?\n   - Architecture notes?\n2. Should side panel pages be linked files under `.jcode/side_panel/`?\n3. Which page should be focused by default?\n\n## MCP\n\n1. Which external systems should MCP access?\n2. Which MCP servers require credentials?\n3. Which MCP servers are allowed in CI?\n4. Should network MCP servers be disabled by default?\n\n## Skills\n\n1. Which built-in skills should be active by default?\n2. Which project-specific skills should be added?\n".to_string()
}

fn swarm_analysis_plan_md(analysis: &ProjectAnalysis) -> String {
    format!(
        "# Swarm Init Analysis Plan\n\nGenerated: {}\n\nThis file is the deterministic plan that `/init` gives to the LLM-driven swarm analysis turn. The static init pass creates scaffolding first; the queued LLM turn must then run parallel agents and synthesize their findings.\n\n## Detected stack seed\n\n{}\n\n## Blocking phase order\n\n1. **Discovery fan-out**: spawn parallel agents for architecture, testing/quality, documentation/onboarding, and tooling/MCP/security.\n2. **Barrier**: wait for all discovery agents to report before writing final recommendations.\n3. **Synthesis**: merge findings into `.jcode/init/SWARM_ANALYSIS_REPORT.md`, `.jcode/SKILLS_PLAN.md`, `.jcode/MCP_PLAN.md`, and side-panel status pages.\n4. **Verification plan**: record commands that should be run before real work begins.\n\n## Required swarm roles\n\n- `architect`: map project structure, boundaries, risks, and core workflows.\n- `qa`: identify tests, validation commands, CI gaps, and high-risk untested areas.\n- `documenter`: inspect README/docs/onboarding gaps and propose project-specific AGENTS.md improvements.\n- `tooling-security`: inspect package managers, MCP candidates, secrets boundaries, and automation risks.\n\n## Output requirements\n\n- Keep generated content project-specific.\n- Do not invent commands that are not supported by repository files.\n- Do not store secrets or environment values.\n- Clearly mark unknowns and questions for humans.\n",
        Utc::now().to_rfc3339(),
        bullet_list(&analysis.detected_stack),
    )
}

fn swarm_analysis_report_stub_md() -> String {
    format!(
        "# Swarm Init Analysis Report\n\nGenerated: {}\n\nStatus: pending LLM/swarm analysis.\n\n`/init` should replace this stub after its parallel discovery agents finish and the synthesis phase completes.\n",
        Utc::now().to_rfc3339()
    )
}

fn skills_plan_md(analysis: &ProjectAnalysis) -> String {
    let mut recommended = vec!["karpathy-guidelines".to_string()];
    if analysis.detected_stack.iter().any(|s| s == "Rust") {
        recommended.push("rust".into());
    }
    recommended.push("optimization".into());
    format!(
        "# Skills Plan\n\n## Recommended initial skills\n\n{}\n\n## Notes\n\n- Built-in skills are available offline.\n- Project-local skills can override built-ins under `.jcode/skills/<name>/SKILL.md`.\n- Do not inject every full skill by default. Route skills by task.\n",
        bullet_list(&recommended)
    )
}

fn mcp_plan_md(_analysis: &ProjectAnalysis) -> String {
    "# MCP Plan\n\nMCP setup is intentionally review-first. This init command does not download or install MCP servers automatically.\n\n## Recommended review steps\n\n1. Identify required systems: filesystem, GitHub, browser, database, issue tracker, docs, deployment.\n2. Prefer local/offline MCP servers when possible.\n3. Document credential requirements and never commit secrets.\n4. Add reviewed server definitions to `.jcode/mcp.json`.\n5. Validate with `jcode` after reviewing permissions.\n\n## Candidate server categories\n\n- Filesystem/code search: usually already covered by native jcode tools.\n- Browser/Playwright: useful for UI QA.\n- GitHub/GitLab: useful for issues/PRs, requires tokens.\n- Database: useful for diagnostics, requires strict read/write boundaries.\n- Docs/search: useful, may require network.\n".to_string()
}

fn mcp_json_stub() -> String {
    "{\n  \"mcpServers\": {}\n}\n".to_string()
}

fn side_panel_status_md(analysis: &ProjectAnalysis) -> String {
    format!(
        "# Project Status Panel\n\n## Current goal\n\nNot set yet.\n\n## Detected stack\n\n{}\n\n## Validation\n\n{}\n\n## Open questions\n\nSee `.jcode/INIT_QUESTIONS.md`.\n\n## Risks\n\n- Secrets must not be stored in memory or docs.\n- MCP servers must be reviewed before enabling.\n",
        bullet_list(&analysis.detected_stack),
        bullet_list(&analysis.test_commands),
    )
}

fn side_panel_questions_md() -> String {
    "# Side Panel Preferences\n\nDecide what the right-side status panel should show for this project.\n\n- [ ] Current goal\n- [ ] Active todos\n- [ ] Test commands\n- [ ] Failing checks\n- [ ] Open risks\n- [ ] Architecture notes\n- [ ] MCP status\n- [ ] Memory/wiki status\n\nDefault recommendation: focus `.jcode/side_panel/status.md` at startup.\n".to_string()
}

fn bullet_list(items: &[String]) -> String {
    if items.is_empty() {
        return "- Not detected yet".to_string();
    }
    items
        .iter()
        .map(|item| format!("- {}", item))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_creates_core_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.1.0'\n",
        )
        .unwrap();
        let report = run_project_init(temp.path(), ProjectInitOptions::default()).expect("init");
        assert!(temp.path().join("AGENTS.md").exists());
        assert!(temp.path().join(".jcode/INIT_REPORT.md").exists());
        assert!(
            temp.path()
                .join(".jcode/init/SWARM_ANALYSIS_PLAN.md")
                .exists()
        );
        assert!(
            temp.path()
                .join(".jcode/init/SWARM_ANALYSIS_REPORT.md")
                .exists()
        );
        assert!(temp.path().join(".jcode/mcp.json").exists());
        assert!(temp.path().join(".jcode/memory_wiki/schema.md").exists());
        assert!(report.detected_stack.contains(&"Rust".to_string()));
    }
}
