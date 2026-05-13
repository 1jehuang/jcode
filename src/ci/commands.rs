use crate::ci::{
    pipeline::{PipelineConfig, PipelineId, PipelineRunner, PipelineTrigger},
    stage::{StageConfig, StageStep},
    runner::PipelineExecutor,
};
use std::path::PathBuf;
use std::sync::Arc;

/// Build a standard CI pipeline config for a Rust project
pub fn rust_build_pipeline(working_dir: &PathBuf, project_name: &str) -> PipelineConfig {
    let stages = vec![
        StageConfig::new("check")
            .with_step(StageStep::new("cargo check", "cargo check"))
            .with_step(StageStep::new("cargo clippy", "cargo clippy")),
        StageConfig::new("test")
            .with_dependency("check")
            .with_step(StageStep::new("unit tests", "cargo test --lib"))
            .with_step(StageStep::new("doc tests", "cargo test --doc")),
        StageConfig::new("build")
            .with_dependency("test")
            .with_step(StageStep::new("release build", "cargo build --release")),
    ];

    PipelineConfig {
        id: PipelineId(format!("rust-{}-{}", project_name, chrono::Utc::now().timestamp())),
        name: format!("Build {}", project_name),
        description: format!("Standard CI pipeline for Rust project: {}", project_name),
        stages,
        working_directory: working_dir.clone(),
        max_parallel_stages: 1,
        fail_fast: true,
        notify_on_failure: true,
        notify_on_success: false,
        timeout_secs: 1800,
        variables: std::collections::HashMap::new(),
        cache_dirs: vec![working_dir.join("target")],
        artifact_paths: vec![working_dir.join("target/release")],
        triggers: vec![
            PipelineTrigger::Manual,
            PipelineTrigger::GitPush { branch: "main".to_string() },
        ],
    }
}

/// Create a pipeline executor ready to run
pub fn create_pipeline_executor() -> (Arc<PipelineRunner>, PipelineExecutor) {
    let runner = Arc::new(PipelineRunner::new(4));
    let executor = PipelineExecutor::new(runner.clone());
    (runner, executor)
}

/// Display pipeline status in CLI
pub fn display_pipeline_status(config: &PipelineConfig, _runner: &PipelineRunner) -> String {
    use std::fmt::Write;

    let mut output = String::new();
    writeln!(output, "Pipeline: {} ({})", config.name, config.id.0).ok();
    writeln!(output, "  Description: {}", config.description).ok();
    writeln!(output, "  Stages: {}", config.stages.len()).ok();
    for stage in &config.stages {
        writeln!(output, "    - {} ({} steps)", stage.name, stage.steps.len()).ok();
        for dep in &stage.depends_on {
            writeln!(output, "      depends on: {}", dep).ok();
        }
    }
    writeln!(output, "  Working dir: {:?}", config.working_directory).ok();
    writeln!(output, "  Cache dirs: {:?}", config.cache_dirs).ok();
    writeln!(output, "  Triggers: {}", config.triggers.len()).ok();

    output
}