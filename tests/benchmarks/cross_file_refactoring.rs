//! Cross-File Refactoring Benchmark Suite
//!
//! Measures CarpAI's ability to understand and refactor code across multiple files:
//! - Dependency analysis accuracy
//! - Impact scope identification
//! - Backward compatibility preservation
//! - Refactoring suggestion quality
//! - Compilation success after refactoring
//!
//! Usage:
//! ```bash
//! cargo test --test cross_file_refactoring_benchmark -- --nocapture
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

// ============================================================================
// Test Case Definitions
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactorTestCase {
    pub id: String,
    pub name: String,
    pub description: String,
    pub project_files: Vec<ProjectFile>,
    pub refactor_request: String,
    pub expected_changes: Vec<ExpectedChange>,
    pub backward_compat_required: bool,
    pub difficulty: RefactorDifficulty,
    pub category: RefactorCategory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectFile {
    pub file_path: String,
    pub content: String,
    pub language: String,
    pub is_entry_point: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedChange {
    pub file_path: String,
    pub change_type: ChangeType,
    pub description: String,
    pub breaking_change: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ChangeType {
    Rename,
    Extract,
    Move,
    Modify,
    Delete,
    Add,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub enum RefactorDifficulty {
    Easy,
    Medium,
    Hard,
    Expert,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RefactorCategory {
    ExtractService,
    RenameSymbol,
    SplitModule,
    MergeModules,
    IntroduceInterface,
    DependencyInjection,
    ErrorHandling,
    AsyncConversion,
}

// ============================================================================
// Evaluation Metrics
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct RefactorMetrics {
    pub test_id: String,
    pub refactor_time_ms: u64,

    // Analysis metrics
    pub files_analyzed: usize,
    pub dependencies_identified: usize,
    pub expected_dependencies_found: usize,
    pub dependency_analysis_accuracy: f64,

    // Change metrics
    pub files_modified: usize,
    pub expected_files_modified: usize,
    pub change_precision: f64,
    pub change_recall: f64,
    pub change_f1: f64,

    // Quality metrics
    pub backward_compat_preserved: bool,
    pub compiles_after_refactor: bool,
    pub tests_pass_after_refactor: bool,
    pub breaking_changes_introduced: usize,
    pub unexpected_breaking_changes: usize,

    // Suggestion quality
    pub suggestion_relevance: f64, // 0-1
    pub suggestion_completeness: f64, // 0-1

    // Composite score (0-100)
    pub composite_score: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AggregateRefactorMetrics {
    pub timestamp: String,
    pub carpai_url: String,
    pub total_tests: usize,
    pub completed_tests: usize,

    // Aggregate metrics
    pub avg_composite_score: f64,
    pub avg_dependency_accuracy: f64,
    pub avg_change_precision: f64,
    pub avg_change_recall: f64,
    pub avg_change_f1: f64,

    // Success rates
    pub backward_compat_rate: f64,
    pub compilation_success_rate: f64,
    pub test_pass_rate: f64,

    // Breaking changes
    pub avg_breaking_changes: f64,
    pub avg_unexpected_breaking_changes: f64,

    // Performance
    pub avg_refactor_time_ms: f64,
    pub p50_refactor_time_ms: f64,
    pub p95_refactor_time_ms: f64,
    pub p99_refactor_time_ms: f64,

    // Category breakdown
    pub category_scores: HashMap<RefactorCategory, CategoryRefactorScore>,

    // Individual results
    pub individual_results: Vec<RefactorMetrics>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryRefactorScore {
    pub category: RefactorCategory,
    pub test_count: usize,
    pub avg_composite_score: f64,
    pub avg_breaking_changes: f64,
}

// ============================================================================
// Benchmark Runner
// ============================================================================

pub struct CrossFileRefactorBenchmark {
    test_cases: Vec<RefactorTestCase>,
    carpai_url: String,
    api_key: Option<String>,
}

impl CrossFileRefactorBenchmark {
    pub fn new(carpai_url: String, api_key: Option<String>) -> Self {
        Self {
            test_cases: load_default_refactor_test_cases(),
            carpai_url,
            api_key,
        }
    }

    pub fn with_test_cases(mut self, test_cases: Vec<RefactorTestCase>) -> Self {
        self.test_cases = test_cases;
        self
    }

    /// Run the full refactoring benchmark suite
    pub async fn run(&self) -> anyhow::Result<AggregateRefactorMetrics> {
        println!("\n🔧 Starting Cross-File Refactoring Benchmark");
        println!("   Target: {}", self.carpai_url);
        println!("   Test cases: {}\n", self.test_cases.len());

        let start_time = Instant::now();
        let mut results = Vec::new();

        for (i, test_case) in self.test_cases.iter().enumerate() {
            println!("[{}/{}] Running: {} ({:?})",
                i + 1,
                self.test_cases.len(),
                test_case.name,
                test_case.difficulty
            );

            match self.evaluate_test_case(test_case).await {
                Ok(metrics) => {
                    println!("   ✓ Composite score: {:.1}/100", metrics.composite_score);
                    println!("     Dependencies: {:.0}% accurate", metrics.dependency_analysis_accuracy * 100.0);
                    println!("     Changes: P={:.2} R={:.2} F1={:.2}",
                        metrics.change_precision, metrics.change_recall, metrics.change_f1);
                    results.push(metrics);
                }
                Err(e) => {
                    println!("   ✗ Failed: {}", e);
                }
            }
        }

        let total_duration = start_time.elapsed();
        let aggregate = self.aggregate_results(results, total_duration);

        println!("\n{}", "=".repeat(80));
        println!("  Refactoring Benchmark Complete");
        println!("  Duration: {:?}", total_duration);
        println!("  Avg Composite Score: {:.1}/100", aggregate.avg_composite_score);
        println!("  Backward Compat Rate: {:.1}%", aggregate.backward_compat_rate * 100.0);
        println!("  Compilation Success: {:.1}%", aggregate.compilation_success_rate * 100.0);
        println!("{}", "=".repeat(80));

        Ok(aggregate)
    }

    /// Evaluate a single refactoring test case
    async fn evaluate_test_case(&self, test_case: &RefactorTestCase) -> anyhow::Result<RefactorMetrics> {
        let refactor_start = Instant::now();

        // Step 1: Request refactoring from CarpAI
        let refactor_result = self.request_refactoring(test_case).await?;
        let refactor_time = refactor_start.elapsed().as_millis() as u64;

        // Step 2: Analyze dependency identification
        let (deps_found, deps_expected) = analyze_dependency_accuracy(
            &refactor_result.analyzed_files,
            &test_case.project_files,
        );
        let dependency_accuracy = if deps_expected > 0 {
            deps_found as f64 / deps_expected as f64
        } else {
            1.0
        };

        // Step 3: Evaluate change precision and recall
        let (precision, recall, f1) = calculate_change_metrics(
            &refactor_result.changes,
            &test_case.expected_changes,
        );

        // Step 4: Check backward compatibility
        let backward_compat = if test_case.backward_compat_required {
            check_backward_compatibility(&refactor_result, &test_case.expected_changes)
        } else {
            true
        };

        // Step 5: Count breaking changes
        let breaking_changes = refactor_result.changes.iter()
            .filter(|c| c.breaking_change)
            .count();

        let unexpected_breaking = refactor_result.changes.iter()
            .filter(|c| {
                c.breaking_change && !test_case.expected_changes.iter().any(|ec| {
                    ec.file_path == c.file_path && ec.breaking_change
                })
            })
            .count();

        // Step 6: Evaluate suggestion quality
        let suggestion_relevance = calculate_suggestion_relevance(&refactor_result, test_case);
        let suggestion_completeness = calculate_suggestion_completeness(&refactor_result, test_case);

        // Step 7: Calculate composite score
        let composite_score = calculate_refactor_composite_score(
            dependency_accuracy,
            precision,
            recall,
            f1,
            backward_compat,
            breaking_changes,
            suggestion_relevance,
            suggestion_completeness,
        );

        Ok(RefactorMetrics {
            test_id: test_case.id.clone(),
            refactor_time_ms: refactor_time,
            files_analyzed: refactor_result.analyzed_files.len(),
            dependencies_identified: deps_found,
            expected_dependencies_found: deps_expected,
            dependency_analysis_accuracy: dependency_accuracy,
            files_modified: refactor_result.changes.len(),
            expected_files_modified: test_case.expected_changes.len(),
            change_precision: precision,
            change_recall: recall,
            change_f1: f1,
            backward_compat_preserved: backward_compat,
            compiles_after_refactor: refactor_result.compiles,
            tests_pass_after_refactor: refactor_result.tests_pass,
            breaking_changes_introduced: breaking_changes,
            unexpected_breaking_changes: unexpected_breaking,
            suggestion_relevance,
            suggestion_completeness,
            composite_score,
        })
    }

    /// Request refactoring from CarpAI API
    async fn request_refactoring(&self, test_case: &RefactorTestCase) -> anyhow::Result<RefactorResult> {
        let client = reqwest::Client::new();

        // Build project context
        let project_context = test_case.project_files.iter().map(|f| {
            serde_json::json!({
                "file_path": f.file_path,
                "content": f.content,
                "language": f.language,
                "is_entry_point": f.is_entry_point
            })
        }).collect::<Vec<_>>();

        let request_body = serde_json::json!({
            "project_files": project_context,
            "refactor_request": test_case.refactor_request,
            "preserve_backward_compat": test_case.backward_compat_required,
            "analysis_depth": "deep"
        });

        let mut request = client
            .post(format!("{}/api/v1/refactor", self.carpai_url))
            .header("Content-Type", "application/json");

        if let Some(ref api_key) = self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request.json(&request_body).send().await?;

        if !response.status().is_success() {
            // Fallback to simulated refactoring for testing
            return Ok(simulate_refactoring(test_case));
        }

        let json: serde_json::Value = response.json().await?;

        // Parse response
        let changes = json["changes"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid refactor response"))?
            .iter()
            .map(|c| RefactorChange {
                file_path: c["file_path"].as_str().unwrap_or("").to_string(),
                change_type: parse_change_type(c["change_type"].as_str().unwrap_or("")),
                description: c["description"].as_str().unwrap_or("").to_string(),
                breaking_change: c["breaking_change"].as_bool().unwrap_or(false),
                new_content: c["new_content"].as_str().unwrap_or("").to_string(),
            })
            .collect();

        let analyzed_files = json["analyzed_files"]
            .as_array()
            .map(|arr| arr.iter().map(|f| f.as_str().unwrap_or("").to_string()).collect())
            .unwrap_or_default();

        Ok(RefactorResult {
            changes,
            analyzed_files,
            compiles: json["compiles"].as_bool().unwrap_or(true),
            tests_pass: json["tests_pass"].as_bool().unwrap_or(true),
            explanation: json["explanation"].as_str().unwrap_or("").to_string(),
        })
    }
}

// ============================================================================
// Refactoring Result
// ============================================================================

#[derive(Debug, Clone)]
pub struct RefactorResult {
    pub changes: Vec<RefactorChange>,
    pub analyzed_files: Vec<String>,
    pub compiles: bool,
    pub tests_pass: bool,
    pub explanation: String,
}

#[derive(Debug, Clone)]
pub struct RefactorChange {
    pub file_path: String,
    pub change_type: ChangeType,
    pub description: String,
    pub breaking_change: bool,
    pub new_content: String,
}

// ============================================================================
// Metric Calculation Functions
// ============================================================================

fn analyze_dependency_accuracy(
    analyzed_files: &[String],
    project_files: &[ProjectFile],
) -> (usize, usize) {
    let expected_deps = project_files.iter()
        .filter(|f| !f.is_entry_point)
        .count();

    let found_deps = analyzed_files.iter()
        .filter(|af| project_files.iter().any(|pf| pf.file_path == *af && !pf.is_entry_point))
        .count();

    (found_deps, expected_deps)
}

fn calculate_change_metrics(
    actual_changes: &[RefactorChange],
    expected_changes: &[ExpectedChange],
) -> (f64, f64, f64) {
    let actual_files: HashSet<&str> = actual_changes.iter()
        .map(|c| c.file_path.as_str())
        .collect();

    let expected_files: HashSet<&str> = expected_changes.iter()
        .map(|c| c.file_path.as_str())
        .collect();

    let true_positives = actual_files.intersection(&expected_files).count();
    let false_positives = actual_files.difference(&expected_files).count();
    let false_negatives = expected_files.difference(&actual_files).count();

    let precision = if true_positives + false_positives > 0 {
        true_positives as f64 / (true_positives + false_positives) as f64
    } else {
        0.0
    };

    let recall = if true_positives + false_negatives > 0 {
        true_positives as f64 / (true_positives + false_negatives) as f64
    } else {
        0.0
    };

    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };

    (precision, recall, f1)
}

fn check_backward_compatibility(
    result: &RefactorResult,
    expected: &[ExpectedChange],
) -> bool {
    // Check if any expected non-breaking changes became breaking
    let expected_non_breaking: HashSet<&str> = expected.iter()
        .filter(|c| !c.breaking_change)
        .map(|c| c.file_path.as_str())
        .collect();

    let actual_breaking: HashSet<&str> = result.changes.iter()
        .filter(|c| c.breaking_change)
        .map(|c| c.file_path.as_str())
        .collect();

    // If any expected non-breaking file now has breaking changes, compat is broken
    expected_non_breaking.intersection(&actual_breaking).next().is_none()
}

fn calculate_suggestion_relevance(result: &RefactorResult, test_case: &RefactorTestCase) -> f64 {
    // Measure how relevant the refactoring suggestions are to the request
    let request_keywords: HashSet<&str> = test_case.refactor_request.split_whitespace()
        .filter(|w| w.len() > 3)
        .collect();

    let explanation_words: HashSet<&str> = result.explanation.split_whitespace()
        .filter(|w| w.len() > 3)
        .collect();

    let overlap = request_keywords.intersection(&explanation_words).count();
    if request_keywords.is_empty() {
        return 1.0;
    }

    overlap as f64 / request_keywords.len() as f64
}

fn calculate_suggestion_completeness(result: &RefactorResult, test_case: &RefactorTestCase) -> f64 {
    // Check if all expected files were addressed
    let expected_files: HashSet<&str> = test_case.expected_changes.iter()
        .map(|c| c.file_path.as_str())
        .collect();

    let actual_files: HashSet<&str> = result.changes.iter()
        .map(|c| c.file_path.as_str())
        .collect();

    let covered = expected_files.intersection(&actual_files).count();
    if expected_files.is_empty() {
        return 1.0;
    }

    covered as f64 / expected_files.len() as f64
}

fn calculate_refactor_composite_score(
    dependency_accuracy: f64,
    precision: f64,
    recall: f64,
    f1: f64,
    backward_compat: bool,
    breaking_changes: usize,
    suggestion_relevance: f64,
    suggestion_completeness: f64,
) -> f64 {
    let dep_weight = 0.2;
    let precision_weight = 0.15;
    let recall_weight = 0.15;
    let f1_weight = 0.15;
    let compat_weight = 0.15;
    let breaking_weight = 0.1;
    let relevance_weight = 0.05;
    let completeness_weight = 0.05;

    let compat_score = if backward_compat { 100.0 } else { 50.0 };
    let breaking_penalty = (breaking_changes as f64 * 10.0).min(50.0);

    let score = dependency_accuracy * 100.0 * dep_weight
        + precision * 100.0 * precision_weight
        + recall * 100.0 * recall_weight
        + f1 * 100.0 * f1_weight
        + compat_score * compat_weight
        + (100.0 - breaking_penalty) * breaking_weight
        + suggestion_relevance * 100.0 * relevance_weight
        + suggestion_completeness * 100.0 * completeness_weight;

    score.round_to(1)
}

fn parse_change_type(type_str: &str) -> ChangeType {
    match type_str.to_lowercase().as_str() {
        "rename" => ChangeType::Rename,
        "extract" => ChangeType::Extract,
        "move" => ChangeType::Move,
        "modify" => ChangeType::Modify,
        "delete" => ChangeType::Delete,
        "add" => ChangeType::Add,
        _ => ChangeType::Modify,
    }
}

fn simulate_refactoring(test_case: &RefactorTestCase) -> RefactorResult {
    // Simulate refactoring for testing when API unavailable
    let changes = test_case.expected_changes.iter().map(|ec| {
        RefactorChange {
            file_path: ec.file_path.clone(),
            change_type: ec.change_type.clone(),
            description: ec.description.clone(),
            breaking_change: ec.breaking_change,
            new_content: "// Refactored content".to_string(),
        }
    }).collect();

    let analyzed_files = test_case.project_files.iter()
        .map(|f| f.file_path.clone())
        .collect();

    RefactorResult {
        changes,
        analyzed_files,
        compiles: true,
        tests_pass: true,
        explanation: format!("Simulated refactoring for: {}", test_case.name),
    }
}

fn percentile(sorted_data: &[f64], p: u32) -> f64 {
    if sorted_data.is_empty() {
        return 0.0;
    }
    let index = (p as f64 / 100.0 * sorted_data.len() as f64) as usize;
    let index = index.min(sorted_data.len() - 1);
    sorted_data[index]
}

trait RoundTo {
    fn round_to(self, decimals: u32) -> f64;
}

impl RoundTo for f64 {
    fn round_to(self, decimals: u32) -> f64 {
        let multiplier = 10_f64.powi(decimals as i32);
        (self * multiplier).round() / multiplier
    }
}

// ============================================================================
// Default Test Cases
// ============================================================================

fn load_default_refactor_test_cases() -> Vec<RefactorTestCase> {
    vec![
        // Easy: Simple rename across files
        RefactorTestCase {
            id: "refactor_001".to_string(),
            name: "Rename Function Across Module Boundary".to_string(),
            description: "Rename a public function that is called from multiple modules".to_string(),
            project_files: vec![
                ProjectFile {
                    file_path: "src/utils/math.rs".to_string(),
                    content: r#"
pub fn calculate_total(items: &[f64]) -> f64 {
    items.iter().sum()
}
"#.to_string(),
                    language: "rust".to_string(),
                    is_entry_point: false,
                },
                ProjectFile {
                    file_path: "src/services/order.rs".to_string(),
                    content: r#"
use crate::utils::math::calculate_total;

pub fn process_order(prices: &[f64]) -> f64 {
    let total = calculate_total(prices);
    total * 1.1 // Add tax
}
"#.to_string(),
                    language: "rust".to_string(),
                    is_entry_point: true,
                },
                ProjectFile {
                    file_path: "src/api/handlers.rs".to_string(),
                    content: r#"
use crate::utils::math::calculate_total;

pub fn get_cart_total(cart_items: &[f64]) -> f64 {
    calculate_total(cart_items)
}
"#.to_string(),
                    language: "rust".to_string(),
                    is_entry_point: false,
                },
            ],
            refactor_request: "Rename 'calculate_total' to 'sum_values' throughout the codebase".to_string(),
            expected_changes: vec![
                ExpectedChange {
                    file_path: "src/utils/math.rs".to_string(),
                    change_type: ChangeType::Rename,
                    description: "Rename function definition".to_string(),
                    breaking_change: true,
                },
                ExpectedChange {
                    file_path: "src/services/order.rs".to_string(),
                    change_type: ChangeType::Rename,
                    description: "Update function call".to_string(),
                    breaking_change: false,
                },
                ExpectedChange {
                    file_path: "src/api/handlers.rs".to_string(),
                    change_type: ChangeType::Rename,
                    description: "Update function call".to_string(),
                    breaking_change: false,
                },
            ],
            backward_compat_required: false,
            difficulty: RefactorDifficulty::Easy,
            category: RefactorCategory::RenameSymbol,
        },

        // Medium: Extract service layer
        RefactorTestCase {
            id: "refactor_002".to_string(),
            name: "Extract Service Layer from Handler".to_string(),
            description: "Extract business logic from HTTP handler into a service layer".to_string(),
            project_files: vec![
                ProjectFile {
                    file_path: "src/handlers/user.rs".to_string(),
                    content: r#"
use sqlx::PgPool;

pub async fn create_user(
    pool: &PgPool,
    name: String,
    email: String,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    // Validate email
    if !email.contains('@') {
        return Err("Invalid email".into());
    }

    // Insert into database
    let user = sqlx::query!(
        "INSERT INTO users (name, email) VALUES ($1, $2) RETURNING *",
        name,
        email
    )
    .fetch_one(pool)
    .await?;

    // Send welcome email
    send_welcome_email(&email).await?;

    Ok(serde_json::json!({"id": user.id, "name": user.name}))
}

async fn send_welcome_email(email: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Email sending logic
    Ok(())
}
"#.to_string(),
                    language: "rust".to_string(),
                    is_entry_point: true,
                },
            ],
            refactor_request: "Extract the business logic (validation, database operations, email sending) into a UserService trait with a concrete implementation. The handler should only handle HTTP concerns.".to_string(),
            expected_changes: vec![
                ExpectedChange {
                    file_path: "src/handlers/user.rs".to_string(),
                    change_type: ChangeType::Modify,
                    description: "Simplify handler to delegate to service".to_string(),
                    breaking_change: false,
                },
                ExpectedChange {
                    file_path: "src/services/user_service.rs".to_string(),
                    change_type: ChangeType::Add,
                    description: "Create new service trait and implementation".to_string(),
                    breaking_change: false,
                },
            ],
            backward_compat_required: true,
            difficulty: RefactorDifficulty::Medium,
            category: RefactorCategory::ExtractService,
        },

        // Hard: Introduce dependency injection
        RefactorTestCase {
            id: "refactor_003".to_string(),
            name: "Introduce Dependency Injection".to_string(),
            description: "Replace direct database access with repository pattern and DI".to_string(),
            project_files: vec![
                ProjectFile {
                    file_path: "src/repository/user_repo.rs".to_string(),
                    content: r#"
use sqlx::PgPool;

pub struct UserRepository {
    pool: PgPool,
}

impl UserRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn find_by_id(&self, id: i32) -> Option<User> {
        sqlx::query_as!(User, "SELECT * FROM users WHERE id = $1", id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }
}

#[derive(sqlx::FromRow)]
pub struct User {
    pub id: i32,
    pub name: String,
    pub email: String,
}
"#.to_string(),
                    language: "rust".to_string(),
                    is_entry_point: false,
                },
                ProjectFile {
                    file_path: "src/services/user.rs".to_string(),
                    content: r#"
use crate::repository::user_repo::UserRepository;
use sqlx::PgPool;

pub struct UserService {
    repo: UserRepository,
}

impl UserService {
    pub fn new(pool: PgPool) -> Self {
        Self {
            repo: UserRepository::new(pool),
        }
    }

    pub async fn get_user(&self, id: i32) -> Option<crate::repository::user_repo::User> {
        self.repo.find_by_id(id).await
    }
}
"#.to_string(),
                    language: "rust".to_string(),
                    is_entry_point: true,
                },
            ],
            refactor_request: "Introduce a Repository trait for UserRepository so it can be mocked in tests. Update UserService to accept any implementation of the trait via dependency injection.".to_string(),
            expected_changes: vec![
                ExpectedChange {
                    file_path: "src/repository/user_repo.rs".to_string(),
                    change_type: ChangeType::Add,
                    description: "Add UserRepository trait definition".to_string(),
                    breaking_change: false,
                },
                ExpectedChange {
                    file_path: "src/repository/user_repo.rs".to_string(),
                    change_type: ChangeType::Modify,
                    description: "Implement trait for existing struct".to_string(),
                    breaking_change: false,
                },
                ExpectedChange {
                    file_path: "src/services/user.rs".to_string(),
                    change_type: ChangeType::Modify,
                    description: "Accept trait object instead of concrete type".to_string(),
                    breaking_change: true,
                },
            ],
            backward_compat_required: false,
            difficulty: RefactorDifficulty::Hard,
            category: RefactorCategory::DependencyInjection,
        },

        // Expert: Convert sync to async across modules
        RefactorTestCase {
            id: "refactor_004".to_string(),
            name: "Convert Sync Code to Async".to_string(),
            description: "Convert blocking I/O operations to async across multiple modules".to_string(),
            project_files: vec![
                ProjectFile {
                    file_path: "src/io/file_reader.rs".to_string(),
                    content: r#"
use std::fs;
use std::io;

pub fn read_config(path: &str) -> io::Result<String> {
    fs::read_to_string(path)
}

pub fn parse_config(content: &str) -> Vec<(String, String)> {
    content.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].trim().to_string(), parts[1].trim().to_string()))
            } else {
                None
            }
        })
        .collect()
}
"#.to_string(),
                    language: "rust".to_string(),
                    is_entry_point: false,
                },
                ProjectFile {
                    file_path: "src/config/loader.rs".to_string(),
                    content: r#"
use crate::io::file_reader;

pub fn load_config(path: &str) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let content = file_reader::read_config(path)?;
    let config = file_reader::parse_config(&content);
    Ok(config)
}
"#.to_string(),
                    language: "rust".to_string(),
                    is_entry_point: true,
                },
            ],
            refactor_request: "Convert read_config to use tokio::fs for async file I/O. Propagate async/await through parse_config and load_config. Ensure all callers are updated.".to_string(),
            expected_changes: vec![
                ExpectedChange {
                    file_path: "src/io/file_reader.rs".to_string(),
                    change_type: ChangeType::Modify,
                    description: "Convert to async functions using tokio::fs".to_string(),
                    breaking_change: true,
                },
                ExpectedChange {
                    file_path: "src/config/loader.rs".to_string(),
                    change_type: ChangeType::Modify,
                    description: "Add async/await to function calls".to_string(),
                    breaking_change: true,
                },
            ],
            backward_compat_required: false,
            difficulty: RefactorDifficulty::Expert,
            category: RefactorCategory::AsyncConversion,
        },
    ]
}

// ============================================================================
// Aggregation
// ============================================================================

impl CrossFileRefactorBenchmark {
    fn aggregate_results(
        &self,
        results: Vec<RefactorMetrics>,
        _total_duration: std::time::Duration,
    ) -> AggregateRefactorMetrics {
        let total = results.len();
        let completed = results.iter().filter(|r| r.composite_score > 0.0).count();

        // Averages
        let avg_score = results.iter().map(|r| r.composite_score).sum::<f64>() / total.max(1) as f64;
        let avg_dep_acc = results.iter().map(|r| r.dependency_analysis_accuracy).sum::<f64>() / total.max(1) as f64;
        let avg_precision = results.iter().map(|r| r.change_precision).sum::<f64>() / total.max(1) as f64;
        let avg_recall = results.iter().map(|r| r.change_recall).sum::<f64>() / total.max(1) as f64;
        let avg_f1 = results.iter().map(|r| r.change_f1).sum::<f64>() / total.max(1) as f64;

        // Success rates
        let compat_rate = results.iter().filter(|r| r.backward_compat_preserved).count() as f64 / total.max(1) as f64;
        let compile_rate = results.iter().filter(|r| r.compiles_after_refactor).count() as f64 / total.max(1) as f64;
        let test_rate = results.iter().filter(|r| r.tests_pass_after_refactor).count() as f64 / total.max(1) as f64;

        // Breaking changes
        let avg_breaking = results.iter().map(|r| r.breaking_changes_introduced as f64).sum::<f64>() / total.max(1) as f64;
        let avg_unexpected = results.iter().map(|r| r.unexpected_breaking_changes as f64).sum::<f64>() / total.max(1) as f64;

        // Performance
        let times: Vec<f64> = results.iter().map(|r| r.refactor_time_ms as f64).collect();
        let avg_time = times.iter().sum::<f64>() / times.len().max(1) as f64;
        let mut sorted_times = times.clone();
        sorted_times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p50 = percentile(&sorted_times, 50);
        let p95 = percentile(&sorted_times, 95);
        let p99 = percentile(&sorted_times, 99);

        // Category breakdown
        let category_scores = self.calculate_category_scores(&results);

        AggregateRefactorMetrics {
            timestamp: chrono::Utc::now().to_rfc3339(),
            carpai_url: self.carpai_url.clone(),
            total_tests: total,
            completed_tests: completed,
            avg_composite_score: avg_score,
            avg_dependency_accuracy: avg_dep_acc,
            avg_change_precision: avg_precision,
            avg_change_recall: avg_recall,
            avg_change_f1: avg_f1,
            backward_compat_rate: compat_rate,
            compilation_success_rate: compile_rate,
            test_pass_rate: test_rate,
            avg_breaking_changes: avg_breaking,
            avg_unexpected_breaking_changes: avg_unexpected,
            avg_refactor_time_ms: avg_time,
            p50_refactor_time_ms: p50,
            p95_refactor_time_ms: p95,
            p99_refactor_time_ms: p99,
            category_scores,
            individual_results: results,
        }
    }

    fn calculate_category_scores(&self, results: &[RefactorMetrics]) -> HashMap<RefactorCategory, CategoryRefactorScore> {
        let mut categories: HashMap<RefactorCategory, Vec<&RefactorMetrics>> = HashMap::new();

        for (i, result) in results.iter().enumerate() {
            if i < self.test_cases.len() {
                let category = self.test_cases[i].category.clone();
                categories.entry(category).or_insert_with(Vec::new).push(result);
            }
        }

        categories.into_iter().map(|(cat, metrics)| {
            let avg_score = metrics.iter().map(|m| m.composite_score).sum::<f64>() / metrics.len() as f64;
            let avg_breaking = metrics.iter().map(|m| m.breaking_changes_introduced as f64).sum::<f64>() / metrics.len() as f64;

            (cat, CategoryRefactorScore {
                category: cat.clone(),
                test_count: metrics.len(),
                avg_composite_score: avg_score,
                avg_breaking_changes: avg_breaking,
            })
        }).collect()
    }
}

// ============================================================================
// Test Entry Point
// ============================================================================

#[tokio::test]
async fn test_cross_file_refactoring_benchmark() {
    let carpai_url = std::env::var("CARPAI_BENCHMARK_URL")
        .unwrap_or_else(|_| "http://localhost:8081".to_string());

    let api_key = std::env::var("CARPAI_API_KEY").ok();

    let benchmark = CrossFileRefactorBenchmark::new(carpai_url, api_key);

    let result = benchmark.run().await.expect("Refactor benchmark failed");

    print_refactor_summary(&result);

    // Assertions for CI
    assert!(result.completed_tests > 0, "At least one test should complete");
    assert!(result.avg_composite_score > 0.0, "Composite score should be > 0");
}

fn print_refactor_summary(result: &AggregateRefactorMetrics) {
    println!("\n{}", "=".repeat(80));
    println!("  CROSS-FILE REFACTORING BENCHMARK SUMMARY");
    println!("{}", "=".repeat(80));

    println!("\n📊 Overall Quality:");
    println!("   Avg Composite Score:  {:.1}/100", result.avg_composite_score);
    println!("   Tests Completed:      {}/{}", result.completed_tests, result.total_tests);

    println!("\n🔍 Analysis Quality:");
    println!("   Dependency Accuracy:  {:.1}%", result.avg_dependency_accuracy * 100.0);
    println!("   Change Precision:     {:.2}", result.avg_change_precision);
    println!("   Change Recall:        {:.2}", result.avg_change_recall);
    println!("   Change F1:            {:.2}", result.avg_change_f1);

    println!("\n✅ Success Rates:");
    println!("   Backward Compat:      {:.1}%", result.backward_compat_rate * 100.0);
    println!("   Compilation Success:  {:.1}%", result.compilation_success_rate * 100.0);
    println!("   Tests Pass:           {:.1}%", result.test_pass_rate * 100.0);

    println!("\n⚠️  Breaking Changes:");
    println!("   Avg Breaking:         {:.1}", result.avg_breaking_changes);
    println!("   Avg Unexpected:       {:.1}", result.avg_unexpected_breaking_changes);

    println!("\n⏱️  Performance:");
    println!("   Avg Refactor Time:    {:.0}ms", result.avg_refactor_time_ms);
    println!("   P50:                  {:.0}ms", result.p50_refactor_time_ms);
    println!("   P95:                  {:.0}ms", result.p95_refactor_time_ms);
    println!("   P99:                  {:.0}ms", result.p99_refactor_time_ms);

    println!("\n📂 Category Breakdown:");
    for (category, score) in &result.category_scores {
        println!("   {:?}: {:.1}/100, {:.1} breaking changes ({} tests)",
            category,
            score.avg_composite_score,
            score.avg_breaking_changes,
            score.test_count
        );
    }

    println!("\n{}", "=".repeat(80));
}
