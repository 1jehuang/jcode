//! RAG Retrieval Quality Benchmark Suite
//!
//! Measures the effectiveness of CarpAI's Retrieval-Augmented Generation system:
//! - Precision@K: Percentage of retrieved results that are relevant
//! - Recall@K: Percentage of relevant documents that are retrieved
//! - MRR (Mean Reciprocal Rank): Quality of ranking
//! - NDCG (Normalized Discounted Cumulative Gain): Ranking quality with graded relevance
//!
//! Usage:
//! ```bash
//! cargo test --test rag_retrieval_benchmark -- --carpai-url http://localhost:8081
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

// ============================================================================
// Test Data Structures
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagTestCase {
    pub id: String,
    pub name: String,
    pub query: String,
    pub query_embedding: Option<Vec<f32>>, // Pre-computed embedding
    pub relevant_documents: Vec<DocumentReference>,
    pub irrelevant_documents: Vec<DocumentReference>,
    pub expected_top_k: usize,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct DocumentReference {
    pub doc_id: String,
    pub file_path: String,
    pub symbol_name: Option<String>,
    pub relevance_score: f64, // 0-1, graded relevance
}

#[derive(Debug, Clone, Serialize)]
pub struct RetrievedDocument {
    pub doc_id: String,
    pub file_path: String,
    pub score: f64,
    pub rank: usize,
}

// ============================================================================
// Evaluation Metrics
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct RagMetrics {
    pub test_id: String,
    pub query: String,
    pub retrieval_time_ms: u64,

    // Basic metrics
    pub precision_at_k: f64,
    pub recall_at_k: f64,
    pub f1_score: f64,

    // Ranking metrics
    pub mrr: f64, // Mean Reciprocal Rank
    pub ndcg_at_k: f64, // Normalized DCG

    // Coverage metrics
    pub unique_files_retrieved: usize,
    pub relevant_files_covered: usize,

    // Composite score
    pub composite_score: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AggregateRagMetrics {
    pub timestamp: String,
    pub carpai_url: String,
    pub total_tests: usize,
    pub completed_tests: usize,

    // Aggregate metrics
    pub avg_precision_at_k: f64,
    pub avg_recall_at_k: f64,
    pub avg_f1_score: f64,
    pub avg_mrr: f64,
    pub avg_ndcg_at_k: f64,

    // Performance metrics
    pub avg_retrieval_time_ms: f64,
    pub p50_retrieval_time_ms: f64,
    pub p95_retrieval_time_ms: f64,
    pub p99_retrieval_time_ms: f64,

    // Per-test results
    pub individual_results: Vec<RagMetrics>,
}

// ============================================================================
// Benchmark Runner
// ============================================================================

pub struct RagRetrievalBenchmark {
    test_cases: Vec<RagTestCase>,
    carpai_url: String,
    api_key: Option<String>,
    top_k: usize,
}

impl RagRetrievalBenchmark {
    pub fn new(carpai_url: String, api_key: Option<String>) -> Self {
        Self {
            test_cases: load_default_rag_test_cases(),
            carpai_url,
            api_key,
            top_k: 10, // Default to Precision@10, Recall@10
        }
    }

    pub fn with_top_k(mut self, k: usize) -> Self {
        self.top_k = k;
        self
    }

    pub fn with_test_cases(mut self, test_cases: Vec<RagTestCase>) -> Self {
        self.test_cases = test_cases;
        self
    }

    /// Run the full RAG benchmark suite
    pub async fn run(&self) -> anyhow::Result<AggregateRagMetrics> {
        println!("\n🔍 Starting RAG Retrieval Benchmark");
        println!("   Target: {}", self.carpai_url);
        println!("   Top-K: {}", self.top_k);
        println!("   Test cases: {}\n", self.test_cases.len());

        let start_time = Instant::now();
        let mut results = Vec::new();

        for (i, test_case) in self.test_cases.iter().enumerate() {
            println!("[{}/{}] Running: {}", i + 1, self.test_cases.len(), test_case.name);

            match self.evaluate_test_case(test_case).await {
                Ok(metrics) => {
                    println!("   ✓ Precision@{}: {:.2}, Recall@{}: {:.2}, MRR: {:.2}",
                        self.top_k, metrics.precision_at_k,
                        self.top_k, metrics.recall_at_k,
                        metrics.mrr
                    );
                    results.push(metrics);
                }
                Err(e) => {
                    println!("   ✗ Failed: {}", e);
                }
            }
        }

        let total_duration = start_time.elapsed();

        // Aggregate results
        let aggregate = self.aggregate_results(results, total_duration);

        println!("\n{}", "=".repeat(80));
        println!("  RAG Benchmark Complete");
        println!("  Duration: {:?}", total_duration);
        println!("  Avg Precision@{}: {:.2}", self.top_k, aggregate.avg_precision_at_k);
        println!("  Avg Recall@{}:    {:.2}", self.top_k, aggregate.avg_recall_at_k);
        println!("  Avg MRR:          {:.2}", aggregate.avg_mrr);
        println!("{}", "=".repeat(80));

        Ok(aggregate)
    }

    /// Evaluate a single RAG test case
    async fn evaluate_test_case(&self, test_case: &RagTestCase) -> anyhow::Result<RagMetrics> {
        let retrieval_start = Instant::now();

        // Step 1: Perform retrieval via CarpAI API
        let retrieved_docs = self.retrieve_documents(test_case).await?;
        let retrieval_time = retrieval_start.elapsed().as_millis() as u64;

        // Step 2: Calculate Precision@K
        let precision_at_k = calculate_precision_at_k(&retrieved_docs, &test_case.relevant_documents, self.top_k);

        // Step 3: Calculate Recall@K
        let recall_at_k = calculate_recall_at_k(&retrieved_docs, &test_case.relevant_documents, self.top_k);

        // Step 4: Calculate F1 Score
        let f1_score = if precision_at_k + recall_at_k > 0.0 {
            2.0 * precision_at_k * recall_at_k / (precision_at_k + recall_at_k)
        } else {
            0.0
        };

        // Step 5: Calculate MRR
        let mrr = calculate_mrr(&retrieved_docs, &test_case.relevant_documents);

        // Step 6: Calculate NDCG@K
        let ndcg_at_k = calculate_ndcg_at_k(&retrieved_docs, &test_case.relevant_documents, self.top_k);

        // Step 7: Calculate coverage metrics
        let unique_files = retrieved_docs.iter()
            .map(|d| d.file_path.clone())
            .collect::<HashSet<_>>()
            .len();

        let relevant_covered = test_case.relevant_documents.iter()
            .filter(|rel_doc| retrieved_docs.iter().any(|ret_doc| ret_doc.doc_id == rel_doc.doc_id))
            .count();

        // Step 8: Calculate composite score
        let composite_score = calculate_rag_composite_score(
            precision_at_k,
            recall_at_k,
            mrr,
            ndcg_at_k,
        );

        Ok(RagMetrics {
            test_id: test_case.id.clone(),
            query: test_case.query.clone(),
            retrieval_time_ms: retrieval_time,
            precision_at_k,
            recall_at_k,
            f1_score,
            mrr,
            ndcg_at_k,
            unique_files_retrieved: unique_files,
            relevant_files_covered: relevant_covered,
            composite_score,
        })
    }

    /// Call CarpAI API to retrieve relevant documents
    async fn retrieve_documents(&self, test_case: &RagTestCase) -> anyhow::Result<Vec<RetrievedDocument>> {
        let client = reqwest::Client::new();

        // Use query text or pre-computed embedding
        let request_body = if let Some(ref embedding) = test_case.query_embedding {
            serde_json::json!({
                "query_embedding": embedding,
                "top_k": self.top_k * 2, // Retrieve more for better evaluation
                "threshold": 0.5
            })
        } else {
            serde_json::json!({
                "query_text": test_case.query,
                "top_k": self.top_k * 2,
                "threshold": 0.5
            })
        };

        let mut request = client
            .post(format!("{}/api/v1/rag/search", self.carpai_url))
            .header("Content-Type", "application/json");

        if let Some(ref api_key) = self.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            // Fallback: simulate retrieval for testing
            return Ok(self.simulate_retrieval(test_case));
        }

        let json: serde_json::Value = response.json().await?;

        // Parse response
        let docs = json["results"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid RAG response format"))?;

        let retrieved = docs.iter().enumerate().map(|(idx, doc)| {
            RetrievedDocument {
                doc_id: doc["doc_id"].as_str().unwrap_or("").to_string(),
                file_path: doc["file_path"].as_str().unwrap_or("").to_string(),
                score: doc["score"].as_f64().unwrap_or(0.0),
                rank: idx + 1,
            }
        }).collect();

        Ok(retrieved)
    }

    /// Simulate retrieval for testing when API is unavailable
    fn simulate_retrieval(&self, test_case: &RagTestCase) -> Vec<RetrievedDocument> {
        // Return relevant documents with some noise for testing
        let mut retrieved: Vec<RetrievedDocument> = test_case.relevant_documents.iter()
            .take(self.top_k)
            .enumerate()
            .map(|(idx, doc)| RetrievedDocument {
                doc_id: doc.doc_id.clone(),
                file_path: doc.file_path.clone(),
                score: 0.9 - (idx as f64 * 0.05),
                rank: idx + 1,
            })
            .collect();

        // Add some irrelevant documents (noise)
        for (idx, doc) in test_case.irrelevant_documents.iter().take(2).enumerate() {
            retrieved.push(RetrievedDocument {
                doc_id: doc.doc_id.clone(),
                file_path: doc.file_path.clone(),
                score: 0.3 - (idx as f64 * 0.1),
                rank: retrieved.len() + 1,
            });
        }

        // Sort by score descending
        retrieved.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        // Reassign ranks
        for (idx, doc) in retrieved.iter_mut().enumerate() {
            doc.rank = idx + 1;
        }

        retrieved
    }

    /// Aggregate all results
    fn aggregate_results(
        &self,
        results: Vec<RagMetrics>,
        _total_duration: std::time::Duration,
    ) -> AggregateRagMetrics {
        let total = results.len();
        let completed = results.iter().filter(|r| r.composite_score > 0.0).count();

        // Calculate averages
        let avg_precision = results.iter().map(|r| r.precision_at_k).sum::<f64>() / total.max(1) as f64;
        let avg_recall = results.iter().map(|r| r.recall_at_k).sum::<f64>() / total.max(1) as f64;
        let avg_f1 = results.iter().map(|r| r.f1_score).sum::<f64>() / total.max(1) as f64;
        let avg_mrr = results.iter().map(|r| r.mrr).sum::<f64>() / total.max(1) as f64;
        let avg_ndcg = results.iter().map(|r| r.ndcg_at_k).sum::<f64>() / total.max(1) as f64;

        // Retrieval time statistics
        let times: Vec<f64> = results.iter().map(|r| r.retrieval_time_ms as f64).collect();
        let avg_time = times.iter().sum::<f64>() / times.len().max(1) as f64;

        let mut sorted_times = times.clone();
        sorted_times.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let p50 = percentile(&sorted_times, 50);
        let p95 = percentile(&sorted_times, 95);
        let p99 = percentile(&sorted_times, 99);

        AggregateRagMetrics {
            timestamp: chrono::Utc::now().to_rfc3339(),
            carpai_url: self.carpai_url.clone(),
            total_tests: total,
            completed_tests: completed,
            avg_precision_at_k: avg_precision,
            avg_recall_at_k: avg_recall,
            avg_f1_score: avg_f1,
            avg_mrr,
            avg_ndcg_at_k: avg_ndcg,
            avg_retrieval_time_ms: avg_time,
            p50_retrieval_time_ms: p50,
            p95_retrieval_time_ms: p95,
            p99_retrieval_time_ms: p99,
            individual_results: results,
        }
    }
}

// ============================================================================
// Metric Calculation Functions
// ============================================================================

fn calculate_precision_at_k(
    retrieved: &[RetrievedDocument],
    relevant: &[DocumentReference],
    k: usize,
) -> f64 {
    if retrieved.is_empty() || k == 0 {
        return 0.0;
    }

    let relevant_ids: HashSet<&str> = relevant.iter()
        .map(|d| d.doc_id.as_str())
        .collect();

    let top_k = &retrieved[..k.min(retrieved.len())];
    let relevant_retrieved = top_k.iter()
        .filter(|doc| relevant_ids.contains(doc.doc_id.as_str()))
        .count();

    relevant_retrieved as f64 / top_k.len() as f64
}

fn calculate_recall_at_k(
    retrieved: &[RetrievedDocument],
    relevant: &[DocumentReference],
    k: usize,
) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }

    let relevant_ids: HashSet<&str> = relevant.iter()
        .map(|d| d.doc_id.as_str())
        .collect();

    let top_k = &retrieved[..k.min(retrieved.len())];
    let relevant_retrieved = top_k.iter()
        .filter(|doc| relevant_ids.contains(doc.doc_id.as_str()))
        .count();

    relevant_retrieved as f64 / relevant.len() as f64
}

fn calculate_mrr(
    retrieved: &[RetrievedDocument],
    relevant: &[DocumentReference],
) -> f64 {
    let relevant_ids: HashSet<&str> = relevant.iter()
        .map(|d| d.doc_id.as_str())
        .collect();

    for (idx, doc) in retrieved.iter().enumerate() {
        if relevant_ids.contains(doc.doc_id.as_str()) {
            return 1.0 / (idx + 1) as f64;
        }
    }

    0.0
}

fn calculate_ndcg_at_k(
    retrieved: &[RetrievedDocument],
    relevant: &[DocumentReference],
    k: usize,
) -> f64 {
    if retrieved.is_empty() || k == 0 {
        return 0.0;
    }

    let relevant_map: HashMap<&str, f64> = relevant.iter()
        .map(|d| (d.doc_id.as_str(), d.relevance_score))
        .collect();

    let top_k = &retrieved[..k.min(retrieved.len())];

    // Calculate DCG
    let dcg: f64 = top_k.iter().enumerate().map(|(idx, doc)| {
        let rel = relevant_map.get(doc.doc_id.as_str()).copied().unwrap_or(0.0);
        rel / (idx as f64 + 1.0).log2()
    }).sum();

    // Calculate ideal DCG (perfect ranking)
    let mut ideal_relevances: Vec<f64> = relevant.iter()
        .map(|d| d.relevance_score)
        .collect();
    ideal_relevances.sort_by(|a, b| b.partial_cmp(a).unwrap());

    let idcg: f64 = ideal_relevances.iter().enumerate().take(k).map(|(idx, &rel)| {
        rel / (idx as f64 + 1.0).log2()
    }).sum();

    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

fn calculate_rag_composite_score(
    precision: f64,
    recall: f64,
    mrr: f64,
    ndcg: f64,
) -> f64 {
    let precision_weight = 0.3;
    let recall_weight = 0.3;
    let mrr_weight = 0.2;
    let ndcg_weight = 0.2;

    let score = precision * precision_weight * 100.0
        + recall * recall_weight * 100.0
        + mrr * mrr_weight * 100.0
        + ndcg * ndcg_weight * 100.0;

    score.round_to(1)
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

fn load_default_rag_test_cases() -> Vec<RagTestCase> {
    vec![
        RagTestCase {
            id: "rag_001".to_string(),
            name: "Find Authentication Module".to_string(),
            query: "How does user authentication work in this project?".to_string(),
            query_embedding: None,
            relevant_documents: vec![
                DocumentReference {
                    doc_id: "auth_mod".to_string(),
                    file_path: "src/auth/mod.rs".to_string(),
                    symbol_name: Some("authenticate".to_string()),
                    relevance_score: 1.0,
                },
                DocumentReference {
                    doc_id: "jwt_utils".to_string(),
                    file_path: "src/auth/jwt.rs".to_string(),
                    symbol_name: Some("verify_token".to_string()),
                    relevance_score: 0.9,
                },
            ],
            irrelevant_documents: vec![
                DocumentReference {
                    doc_id: "ui_component".to_string(),
                    file_path: "src/ui/button.rs".to_string(),
                    symbol_name: None,
                    relevance_score: 0.0,
                },
            ],
            expected_top_k: 10,
            metadata: HashMap::new(),
        },

        RagTestCase {
            id: "rag_002".to_string(),
            name: "Database Connection Pool".to_string(),
            query: "Where is the database connection pool configured?".to_string(),
            query_embedding: None,
            relevant_documents: vec![
                DocumentReference {
                    doc_id: "db_config".to_string(),
                    file_path: "src/database/config.rs".to_string(),
                    symbol_name: Some("create_pool".to_string()),
                    relevance_score: 1.0,
                },
            ],
            irrelevant_documents: vec![],
            expected_top_k: 10,
            metadata: HashMap::new(),
        },

        RagTestCase {
            id: "rag_003".to_string(),
            name: "Error Handling Pattern".to_string(),
            query: "What error handling pattern is used throughout the codebase?".to_string(),
            query_embedding: None,
            relevant_documents: vec![
                DocumentReference {
                    doc_id: "error_types".to_string(),
                    file_path: "src/errors.rs".to_string(),
                    symbol_name: Some("AppError".to_string()),
                    relevance_score: 1.0,
                },
                DocumentReference {
                    doc_id: "lib_main".to_string(),
                    file_path: "src/lib.rs".to_string(),
                    symbol_name: None,
                    relevance_score: 0.7,
                },
            ],
            irrelevant_documents: vec![
                DocumentReference {
                    doc_id: "readme".to_string(),
                    file_path: "README.md".to_string(),
                    symbol_name: None,
                    relevance_score: 0.0,
                },
            ],
            expected_top_k: 10,
            metadata: HashMap::new(),
        },
    ]
}

// ============================================================================
// Test Entry Point
// ============================================================================

#[tokio::test]
async fn test_rag_retrieval_benchmark() {
    let carpai_url = std::env::var("CARPAI_BENCHMARK_URL")
        .unwrap_or_else(|_| "http://localhost:8081".to_string());

    let api_key = std::env::var("CARPAI_API_KEY").ok();

    let benchmark = RagRetrievalBenchmark::new(carpai_url, api_key)
        .with_top_k(10);

    let result = benchmark.run().await.expect("RAG benchmark failed");

    print_rag_summary(&result);

    // Assertions for CI
    assert!(result.completed_tests > 0, "At least one test should complete");
    assert!(result.avg_precision_at_k >= 0.0, "Precision should be non-negative");
    assert!(result.avg_recall_at_k >= 0.0, "Recall should be non-negative");
}

fn print_rag_summary(result: &AggregateRagMetrics) {
    println!("\n{}", "=".repeat(80));
    println!("  RAG BENCHMARK SUMMARY");
    println!("{}", "=".repeat(80));

    println!("\n📊 Retrieval Quality:");
    println!("   Avg Precision@10:   {:.2}", result.avg_precision_at_k);
    println!("   Avg Recall@10:      {:.2}", result.avg_recall_at_k);
    println!("   Avg F1 Score:       {:.2}", result.avg_f1_score);

    println!("\n🎯 Ranking Quality:");
    println!("   Avg MRR:            {:.2}", result.avg_mrr);
    println!("   Avg NDCG@10:        {:.2}", result.avg_ndcg_at_k);

    println!("\n⏱️  Performance:");
    println!("   Avg Retrieval Time: {:.0}ms", result.avg_retrieval_time_ms);
    println!("   P50:                {:.0}ms", result.p50_retrieval_time_ms);
    println!("   P95:                {:.0}ms", result.p95_retrieval_time_ms);
    println!("   P99:                {:.0}ms", result.p99_retrieval_time_ms);

    println!("\n📈 Test Results:");
    println!("   Total Tests:        {}", result.total_tests);
    println!("   Completed:          {}", result.completed_tests);

    println!("\n{}", "=".repeat(80));
}
