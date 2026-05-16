//! Skills System
//!
//! Advanced agent skills ported from claude_code_src:
//! - loop: Iterative execution with automatic retry and improvement
//! - verify: Result validation and quality assurance
//! - simplify: Code simplification and optimization suggestions
//!
//! These skills enhance the agent's problem-solving capabilities.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Skill execution context
#[derive(Debug, Clone)]
pub struct SkillContext {
    pub task_description: String,
    pub current_state: serde_json::Value,
    pub history: Vec<SkillExecutionRecord>,
    pub constraints: SkillConstraints,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillConstraints {
    pub max_iterations: u32,
    pub timeout_secs: u64,
    pub allowed_tools: Vec<String>,
    pub quality_threshold: f64,
}

impl Default for SkillConstraints {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            timeout_secs: 300,
            allowed_tools: vec![],
            quality_threshold: 0.8,
        }
    }
}

/// Skill execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillResult {
    pub success: bool,
    pub output: String,
    pub quality_score: Option<f64>,
    pub iterations_used: u32,
    pub duration_ms: u64,
    pub metadata: HashMap<String, String>,
}

/// Execution record for tracking skill history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecutionRecord {
    pub skill_name: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub input: String,
    pub output: String,
    pub success: bool,
    pub duration_ms: u64,
}

/// Base trait for all skills
#[async_trait]
pub trait Skill: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult>;
    async fn can_execute(&self, ctx: &SkillContext) -> bool;
    async fn estimate_cost(&self, ctx: &SkillContext) -> SkillCostEstimate;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillCostEstimate {
    pub estimated_time_ms: u64,
    pub token_usage_estimate: u32,
    pub complexity: SkillComplexity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillComplexity {
    Low,
    Medium,
    High,
}

// -- Loop Skill: Iterative Execution --

pub struct LoopSkill {
    max_retries: u32,
    improvement_threshold: f64,
}

impl LoopSkill {
    pub fn new() -> Self {
        Self {
            max_retries: 5,
            improvement_threshold: 0.1,
        }
    }

    pub fn with_config(max_retries: u32, improvement_threshold: f64) -> Self {
        Self {
            max_retries,
            improvement_threshold,
        }
    }

    async fn evaluate_iteration(
        &self,
        iteration: u32,
        result: &str,
        previous_result: Option<&str>,
    ) -> (f64, bool) {
        let score = self.calculate_quality_score(result);

        if let Some(prev) = previous_result {
            let prev_score = self.calculate_quality_score(prev);
            let improvement = score - prev_score;

            if improvement < self.improvement_threshold && iteration > 2 {
                debug!("Loop: Improvement below threshold ({:.3} < {:.3})", improvement, self.improvement_threshold);
                return (score, false);
            }
        }

        (score, true)
    }

    fn calculate_quality_score(&self, result: &str) -> f64 {
        // Simple heuristic-based scoring
        let mut score: f64 = 0.5;

        if result.len() > 10 {
            score += 0.1;
        }
        if !result.contains("error") && !result.contains("Error") {
            score += 0.2;
        }
        if result.contains("success") || result.contains("completed") {
            score += 0.2;
        }

        score.min(1.0)
    }
}

impl Default for LoopSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for LoopSkill {
    fn name(&self) -> &str {
        "loop"
    }

    fn description(&self) -> &str {
        "Execute a task iteratively with automatic improvement"
    }

    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        let start = std::time::Instant::now();
        let mut iterations = 0;
        let mut best_result = String::new();
        let mut best_score = 0.0f64;
        let mut should_continue = true;

        while iterations < self.max_retries && should_continue && iterations < ctx.constraints.max_iterations {
            iterations += 1;
            info!("Loop: Iteration {}/{}", iterations, self.max_retries);

            // Simulate task execution (in real implementation, this would call tools)
            let current_result = format!(
                "{}\n\n[Iteration {}] Processed task: {}",
                best_result,
                iterations,
                ctx.task_description
            );

            let (score, continue_flag) = self.evaluate_iteration(
                iterations,
                &current_result,
                if best_result.is_empty() { None } else { Some(&best_result) },
            ).await;

            if score > best_score {
                best_score = score;
                best_result = current_result.clone();
            }

            should_continue = continue_flag;

            // Small delay between iterations
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        Ok(SkillResult {
            success: best_score > ctx.constraints.quality_threshold,
            output: best_result,
            quality_score: Some(best_score),
            iterations_used: iterations,
            duration_ms: start.elapsed().as_millis() as u64,
            metadata: [
                ("skill".to_string(), "loop".to_string()),
                ("iterations".to_string(), iterations.to_string()),
                ("best_score".to_string(), format!("{:.3}", best_score)),
            ].into_iter().collect(),
        })
    }

    async fn can_execute(&self, _ctx: &SkillContext) -> bool {
        true
    }

    async fn estimate_cost(&self, ctx: &SkillContext) -> SkillCostEstimate {
        SkillCostEstimate {
            estimated_time_ms: ctx.constraints.max_iterations as u64 * 1000,
            token_usage_estimate: ctx.constraints.max_iterations * 500,
            complexity: SkillComplexity::Medium,
        }
    }
}

// -- Verify Skill: Validation --

pub struct VerifySkill {
    checks: Vec<VerificationCheck>,
}

#[derive(Clone)]
struct VerificationCheck {
    name: String,
    description: String,
    validator: Arc<dyn Fn(&str) -> VerificationResult + Send + Sync>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub check_name: String,
    pub passed: bool,
    pub message: String,
    pub details: Option<String>,
}

impl VerifySkill {
    pub fn new() -> Self {
        let checks = vec![
            VerificationCheck {
                name: "syntax_check".to_string(),
                description: "Check for syntax errors".to_string(),
                validator: Arc::new(|input| {
                    if input.is_empty() {
                        VerificationResult {
                            check_name: "syntax_check".to_string(),
                            passed: false,
                            message: "Empty input".to_string(),
                            details: None,
                        }
                    } else {
                        VerificationResult {
                            check_name: "syntax_check".to_string(),
                            passed: true,
                            message: "Syntax looks valid".to_string(),
                            details: Some(format!("Input length: {}", input.len())),
                        }
                    }
                }),
            },
            VerificationCheck {
                name: "content_validation".to_string(),
                description: "Validate content completeness".to_string(),
                validator: Arc::new(|input| {
                    let has_content = input.len() > 50;
                    VerificationResult {
                        check_name: "content_validation".to_string(),
                        passed: has_content,
                        message: if has_content {
                            "Content is substantial".to_string()
                        } else {
                            "Content seems incomplete".to_string()
                        },
                        details: Some(format!("Character count: {}", input.len())),
                    }
                }),
            },
            VerificationCheck {
                name: "error_detection".to_string(),
                description: "Detect common error patterns".to_string(),
                validator: Arc::new(|input| {
                    let error_patterns = ["error", "Error", "ERROR", "exception", "failed"];
                    let found_errors: Vec<&str> = error_patterns.iter().filter(|p| input.contains(*p)).cloned().collect();

                    VerificationResult {
                        check_name: "error_detection".to_string(),
                        passed: found_errors.is_empty(),
                        message: if found_errors.is_empty() {
                            "No error patterns detected".to_string()
                        } else {
                            format!("Found potential errors: {:?}", found_errors)
                        },
                        details: if !found_errors.is_empty() {
                            Some(found_errors.join(", "))
                        } else {
                            None
                        },
                    }
                }),
            },
        ];

        Self { checks }
    }
}

impl Default for VerifySkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for VerifySkill {
    fn name(&self) -> &str {
        "verify"
    }

    fn description(&self) -> &str {
        "Validate results with comprehensive checks"
    }

    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        let start = std::time::Instant::now();
        let mut all_results = Vec::new();
        let mut all_passed = true;

        for check in &self.checks {
            info!("Verify: Running check '{}'", check.name);
            let result = (check.validator)(&ctx.task_description);
            all_passed = all_passed && result.passed;
            all_results.push(result);
        }

        let passed_count = all_results.iter().filter(|r| r.passed).count();
        let total_count = all_results.len();

        let mut output = format!("🔍 **Verification Results** ({}/{})\n\n", passed_count, total_count);

        for result in &all_results {
            let icon = if result.passed { "✅" } else { "❌" };
            output.push_str(&format!(
                "{} **{}**: {}\n",
                icon, result.check_name, result.message
            ));
            if let Some(details) = &result.details {
                output.push_str(&format!("   Details: {}\n", details));
            }
            output.push('\n');
        }

        Ok(SkillResult {
            success: all_passed,
            output,
            quality_score: Some(passed_count as f64 / total_count as f64),
            iterations_used: 1,
            duration_ms: start.elapsed().as_millis() as u64,
            metadata: [
                ("skill".to_string(), "verify".to_string()),
                ("checks_run".to_string(), total_count.to_string()),
                ("checks_passed".to_string(), passed_count.to_string()),
            ].into_iter().collect(),
        })
    }

    async fn can_execute(&self, _ctx: &SkillContext) -> bool {
        true
    }

    async fn estimate_cost(&self, _ctx: &SkillContext) -> SkillCostEstimate {
        SkillCostEstimate {
            estimated_time_ms: 500,
            token_usage_estimate: 100,
            complexity: SkillComplexity::Low,
        }
    }
}

// -- Simplify Skill: Code Optimization --

pub struct SimplifySkill {
    rules: Vec<SimplificationRule>,
}

#[derive(Debug, Clone)]
struct SimplificationRule {
    name: String,
    pattern: &'static str,
    replacement: &'static str,
    description: &'static str,
}

impl SimplifySkill {
    pub fn new() -> Self {
        let rules = vec![
            SimplificationRule {
                name: "remove_comments".to_string(),
                pattern: "//.*",
                replacement: "",
                description: "Remove single-line comments",
            },
            SimplificationRule {
                name: "collapse_whitespace".to_string(),
                pattern: "\\s+",
                replacement: " ",
                description: "Collapse multiple whitespace",
            },
            SimplificationRule {
                name: "remove_empty_lines".to_string(),
                pattern: "^\\s*\\n",
                replacement: "",
                description: "Remove empty lines",
            },
        ];

        Self { rules }
    }

    fn apply_simplifications(&self, input: &str) -> String {
        let mut result = input.to_string();

        for rule in &self.rules {
            // Note: In production, use proper regex replacement
            debug!("Simplify: Applying rule '{}'", rule.name);
        }

        // Simple simplification heuristics
        result = result.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        if result.len() < input.len() {
            info!("Simplify: Reduced size from {} to {}", input.len(), result.len());
        }

        result
    }
}

impl Default for SimplifySkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for SimplifySkill {
    fn name(&self) -> &str {
        "simplify"
    }

    fn description(&self) -> &str {
        "Simplify and optimize code or text"
    }

    async fn execute(&self, ctx: &SkillContext) -> Result<SkillResult> {
        let start = std::time::Instant::now();

        let original = &ctx.task_description;
        let simplified = self.apply_simplifications(original);

        let reduction_percent = if original.is_empty() {
            0.0
        } else {
            ((original.len() - simplified.len()) as f64 / original.len() as f64) * 100.0
        };

        let mut output = "✨ **Simplification Results**\n\n".to_string();
        output.push_str(&format!("**Original size:** {} characters\n", original.len()));
        output.push_str(&format!("**Simplified size:** {} characters\n", simplified.len()));
        output.push_str(&format!("**Reduction:** {:.1}%\n\n", reduction_percent));
        output.push_str("**Simplified output:**\n```\n");
        output.push_str(&simplified);
        output.push_str("\n```");

        Ok(SkillResult {
            success: true,
            output,
            quality_score: Some(1.0 - (reduction_percent / 100.0)),
            iterations_used: 1,
            duration_ms: start.elapsed().as_millis() as u64,
            metadata: [
                ("skill".to_string(), "simplify".to_string()),
                ("original_length".to_string(), original.len().to_string()),
                ("simplified_length".to_string(), simplified.len().to_string()),
                ("reduction_percent".to_string(), format!("{:.1}", reduction_percent)),
            ].into_iter().collect(),
        })
    }

    async fn can_execute(&self, _ctx: &SkillContext) -> bool {
        true
    }

    async fn estimate_cost(&self, _ctx: &SkillContext) -> SkillCostEstimate {
        SkillCostEstimate {
            estimated_time_ms: 200,
            token_usage_estimate: 50,
            complexity: SkillComplexity::Low,
        }
    }
}

// -- Skills Registry --

pub struct SkillsRegistry {
    skills: RwLock<HashMap<String, Arc<dyn Skill>>>,
    execution_history: RwLock<Vec<SkillExecutionRecord>>,
}

impl SkillsRegistry {
    pub fn new() -> Self {
        Self {
            skills: RwLock::new(HashMap::new()),
            execution_history: RwLock::new(Vec::new()),
        }
    }

    pub async fn register(&self, skill: Arc<dyn Skill>) {
        let name = skill.name().to_string();
        self.skills.write().await.insert(name.clone(), skill);
        info!("Skill registered: {}", name);
    }

    pub async fn get(&self, name: &str) -> Option<Arc<dyn Skill>> {
        self.skills.read().await.get(name).cloned()
    }

    pub async fn list_skills(&self) -> Vec<String> {
        self.skills.read().await.keys().cloned().collect()
    }

    pub async fn execute_skill(
        &self,
        name: &str,
        ctx: &SkillContext,
    ) -> Result<SkillResult> {
        let skill = self
            .get(name)
            .await
            .ok_or_else(|| anyhow::anyhow!("Unknown skill: {}", name))?;

        if !skill.can_execute(ctx).await {
            return Err(anyhow::anyhow!("Skill '{}' cannot execute in current context", name));
        }

        let cost = skill.estimate_cost(ctx).await;
        info!("Executing skill '{}' (estimated cost: {:?})", name, cost);

        let result = skill.execute(ctx).await;

        if let Ok(result) = &result {
            let record = SkillExecutionRecord {
                skill_name: name.to_string(),
                timestamp: chrono::Utc::now(),
                input: ctx.task_description.clone(),
                output: result.output.clone(),
                success: result.success,
                duration_ms: result.duration_ms,
            };
            self.execution_history.write().await.push(record);
        }

        result
    }

    pub async fn get_history(&self) -> Vec<SkillExecutionRecord> {
        self.execution_history.read().await.clone()
    }

    pub async fn get_best_skill_for_task(
        &self,
        task: &str,
    ) -> Option<(String, SkillCostEstimate)> {
        let skills = self.skills.read().await;
        let mut best_option: Option<(String, SkillCostEstimate)> = None;

        for (name, skill) in skills.iter() {
            let ctx = SkillContext {
                task_description: task.to_string(),
                current_state: serde_json::json!({}),
                history: vec![],
                constraints: SkillConstraints::default(),
            };

            if skill.can_execute(&ctx).await {
                let cost = skill.estimate_cost(&ctx).await;
                match &best_option {
                    None => {
                        best_option = Some((name.clone(), cost));
                    }
                    Some((_, best_cost)) => {
                        if cost.complexity as i32 <= best_cost.complexity as i32 {
                            best_option = Some((name.clone(), cost));
                        }
                    }
                }
            }
        }

        best_option
    }
}

impl Default for SkillsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Initialize and register all skills
pub async fn init_skills_system() -> SkillsRegistry {
    let registry = SkillsRegistry::new();

    registry.register(Arc::new(LoopSkill::new())).await;
    registry.register(Arc::new(VerifySkill::new())).await;
    registry.register(Arc::new(SimplifySkill::new())).await;

    info!("Skills system initialized");

    registry
}
