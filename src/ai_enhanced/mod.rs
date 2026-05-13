//! AI Enhancement System
//!
//! Intelligent features including:
//! - Smart skill recommendation
//! - Adaptive parameter tuning
//! - Anomaly detection
//! - Predictive analytics

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Skill recommendation based on context analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecommendation {
    pub skill_name: String,
    pub confidence: f64,
    pub reason: String,
    pub estimated_benefit: f64,
}

/// Context features for AI analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextFeatures {
    pub task_complexity: f64,
    pub code_length: usize,
    pub error_rate: f64,
    pub previous_successes: u32,
    pub previous_failures: u32,
    pub time_pressure: f64, // 0.0 (relaxed) to 1.0 (urgent)
    pub user_expertise: f64, // 0.0 (beginner) to 1.0 (expert)
}

impl Default for ContextFeatures {
    fn default() -> Self {
        Self {
            task_complexity: 0.5,
            code_length: 100,
            error_rate: 0.1,
            previous_successes: 5,
            previous_failures: 1,
            time_pressure: 0.5,
            user_expertise: 0.7,
        }
    }
}

/// Adaptive parameter configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveParams {
    pub max_iterations: (u32, u32),       // (min, max)
    pub quality_threshold: (f64, f64),     // (min, max)
    pub timeout_secs: (u64, u64),          // (min, max)
    pub retry_count: (u32, u32),           // (min, max)
    pub learning_rate: f64,                // How fast to adapt (0.0-1.0)
}

impl Default for AdaptiveParams {
    fn default() -> Self {
        Self {
            max_iterations: (3, 20),
            quality_threshold: (0.6, 0.95),
            timeout_secs: (30, 600),
            retry_count: (1, 10),
            learning_rate: 0.1,
        }
    }
}

/// Anomaly detection result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyResult {
    pub is_anomaly: bool,
    pub anomaly_type: AnomalyType,
    pub severity: f64,
    pub description: String,
    pub suggested_action: Option<String>,
}

/// Types of anomalies that can be detected
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnomalyType {
    PerformanceDegradation,
    ErrorSpike,
    MemoryLeak,
    UnusualPattern,
    TimeoutExceeded,
}

impl std::fmt::Display for AnomalyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PerformanceDegradation => write!(f, "performance-degradation"),
            Self::ErrorSpike => write!(f, "error-spike"),
            Self::MemoryLeak => write!(f, "memory-leak"),
            Self::UnusualPattern => write!(f, "unusual-pattern"),
            Self::TimeoutExceeded => write!(f, "timeout-exceeded"),
        }
    }
}

/// Historical data point for trend analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DataPoint {
    timestamp: Instant,
    value: f64,
    metadata: HashMap<String, String>,
}

/// AI Enhancement Engine
pub struct AiEngine {
    context_history: RwLock<VecDeque<ContextFeatures>>,
    performance_history: RwLock<VecDeque<DataPoint>>,
    recommendations_cache: RwLock<HashMap<String, SkillRecommendation>>,
    adaptive_params: RwLock<AdaptiveParams>,
    anomaly_thresholds: RwLock<AnomalyThresholds>,
}

/// Thresholds for anomaly detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnomalyThresholds {
    pub performance_degradation_pct: f64,
    pub error_spike_factor: f64,
    pub memory_growth_rate_mb_per_min: f64,
    pub pattern_deviation_stddev: f64,
    pub timeout_multiplier: f64,
}

impl Default for AnomalyThresholds {
    fn default() -> Self {
        Self {
            performance_degradation_pct: 50.0, // 50% slower than normal
            error_spike_factor: 3.0,          // 3x normal error rate
            memory_growth_rate_mb_per_min: 10.0, // 10 MB/min growth
            pattern_deviation_stddev: 2.0,     // 2 standard deviations
            timeout_multiplier: 2.0,         // 2x expected duration
        }
    }
}

impl AiEngine {
    pub fn new() -> Self {
        Self {
            context_history: RwLock::new(VecDeque::with_capacity(100)),
            performance_history: RwLock::new(VecDeque::with_capacity(1000)),
            recommendations_cache: RwLock::new(HashMap::new()),
            adaptive_params: RwLock::new(AdaptiveParams::default()),
            anomaly_thresholds: RwLock::new(AnomalyThresholds::default()),
        }
    }

    /// Analyze context and recommend skills
    pub async fn recommend_skills(
        &self,
        context: &ContextFeatures,
        available_skills: &[String],
    ) -> Vec<SkillRecommendation> {
        let mut recommendations = Vec::new();

        // Store context in history
        {
            let mut history = self.context_history.write().await;
            history.push_back(context.clone());
            while history.len() > 100 {
                history.pop_front();
            }
        }

        for skill_name in available_skills {
            let confidence = self.calculate_skill_confidence(context, skill_name).await;

            if confidence > 0.3 { // Only include reasonable matches
                let reason = self.generate_recommendation_reason(context, skill_name);
                let benefit = self.estimate_benefit(context, skill_name).await;

                recommendations.push(SkillRecommendation {
                    skill_name: skill_name.clone(),
                    confidence,
                    reason,
                    estimated_benefit: benefit,
                });
            }
        }

        // Sort by confidence
        recommendations.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());

        // Cache results
        let mut cache = self.recommendations_cache.write().await;
        cache.insert(
            format!("{:?}", chrono::Utc::now()),
            recommendations.first().cloned().unwrap_or(SkillRecommendation {
                skill_name: "none".to_string(),
                confidence: 0.0,
                reason: "No suitable skill found".to_string(),
                estimated_benefit: 0.0,
            }),
        );

        recommendations
    }

    /// Calculate confidence score for a skill given context
    async fn calculate_skill_confidence(
        &self,
        context: &ContextFeatures,
        skill_name: &str,
    ) -> f64 {
        let mut score = 0.5; // Base score

        match skill_name {
            "loop" => {
                // Loop skill is good for complex tasks with room for improvement
                score += context.task_complexity * 0.3;
                score += (1.0 - context.user_expertise) * 0.2;
                score += if context.previous_failures > 0 { 0.2 } else { 0.0 };
                score -= context.time_pressure * 0.1; // Less good under pressure
            }

            "verify" => {
                // Verify skill is important when errors are likely
                score += context.error_rate * 0.4;
                score += (context.code_length as f64 / 1000.0).min(1.0) * 0.2;
                score += context.task_complexity * 0.1;
            }

            "simplify" => {
                // Simplify is good for large codebases or beginners
                score += (context.code_length as f64 / 500.0).min(1.0) * 0.4;
                score += (1.0 - context.user_expertise) * 0.3;
                score -= context.time_pressure * 0.1;
            }

            _ => {
                // Unknown skill - use heuristics
                score = 0.3 + context.task_complexity * 0.2;
            }
        }

        score.max(0.0).min(1.0)
    }

    /// Generate human-readable reason for recommendation
    fn generate_recommendation_reason(
        &self,
        context: &ContextFeatures,
        skill_name: &str,
    ) -> String {
        let reasons = Vec::new();

        if context.error_rate > 0.2 {
            reasons.push("high error rate detected");
        }

        if context.task_complexity > 0.7 {
            reasons.push("complex task requires iteration");
        }

        if context.code_length > 500 {
            reasons.push("large codebase may need simplification");
        }

        if context.time_pressure > 0.8 {
            reasons.push("urgent timeline - consider fast mode");
        }

        if context.user_expertise < 0.4 {
            reasons.push("may benefit from verification");
        }

        if reasons.is_empty() {
            format!("{} recommended based on general heuristics", skill_name)
        } else {
            format!(
                "{} recommended because: {}",
                skill_name,
                reasons.join("; ")
            )
        }
    }

    /// Estimate potential benefit of using a skill
    async fn estimate_benefit(&self, context: &ContextFeatures, _skill_name: &str) -> f64 {
        // Simplified benefit estimation
        // In production, would use ML model trained on historical data

        let base_benefit = context.task_complexity * 0.5;
        let error_reduction = context.error_rate * 0.3;
        let efficiency_gain = (1.0 - context.time_pressure) * 0.2;

        base_benefit + error_reduction + efficiency_gain
    }

    /// Adapt parameters based on historical performance
    pub async fn adapt_params(&self, recent_results: &[(bool, Duration)]) -> AdaptiveParams {
        let params = self.adaptive_params.read().await.clone();
        let mut new_params = params.clone();

        if !recent_results.is_empty() {
            let success_rate =
                recent_results.iter().filter(|(success, _)| *success).count() as f64
                    / recent_results.len() as f64;
            let avg_duration: Duration =
                recent_results.iter().map(|(_, d)| *d).sum::<Duration>() / recent_results.len() as u32;

            // Adjust parameters based on success rate
            if success_rate < 0.5 {
                // Poor success rate - be more conservative
                new_params.quality_threshold.0 = (params.quality_threshold.0 * 0.9).max(0.4);
                new_params.max_iterations.0 = (params.max_iterations.0 * 1.2).min(30);
            } else if success_rate > 0.9 && avg_duration < Duration::from_secs(60) {
                // High success and fast - can be more aggressive
                new_params.quality_threshold.1 = (params.quality_threshold.1 * 1.05).min(0.99);
                new_params.timeout_secs.1 = (params.timeout_secs.1 * 0.9).max(30);
            }

            // Apply learning rate
            let lr = params.learning_rate;
            new_params.max_iterations.0 = Self::lerp(
                params.max_iterations.0,
                new_params.max_iterations.0,
                lr,
            );
            new_params.max_iterations.1 = Self::lerp(
                params.max_iterations.1,
                new_params.max_iterations.1,
                lr,
            );
        }

        // Update stored params
        *self.adaptive_params.write().await = new_params.clone();

        new_params
    }

    /// Linear interpolation helper
    fn lerp(a: u32, b: u32, t: f64) -> u32 {
        ((a as f64) + ((b as f64) - (a as f64)) * t) as u32
    }

    /// Detect anomalies in metrics
    pub async fn detect_anomalies(
        &self,
        metric_name: &str,
        current_value: f64,
        baseline_value: f64,
    ) -> AnomalyResult {
        let thresholds = self.anomaly_thresholds.read().await.clone();

        // Record data point
        {
            let mut history = self.performance_history.write().await;
            history.push_back(DataPoint {
                timestamp: Instant::now(),
                value: current_value,
                metadata: HashMap::from([(
                    "metric".to_string(),
                    metric_name.to_string(),
                )]),
            });
            while history.len() > 1000 {
                history.pop_front();
            }
        }

        // Check for different anomaly types
        let anomalies = vec![
            // Performance degradation
            (
                AnomalyType::PerformanceDegradation,
                if baseline_value > 0.0 {
                    ((current_value - baseline_value) / baseline_value * 100.0).abs()
                        > thresholds.performance_degradation_pct
                } else {
                    false
                },
                format!(
                    "{} is {:.1}% from baseline of {}",
                    metric_name,
                    ((current_value - baseline_value) / baseline_value.abs().max(1.0) * 100.0),
                    baseline_value
                ),
                Some("Consider optimizing the operation or reducing load"),
            ),
        ];

        // Return first detected anomaly (if any)
        for (anomaly_type, is_anomalous, description, action) in anomalies {
            if is_anomalous {
                return AnomalyResult {
                    is_anomaly: true,
                    anomaly_type,
                    severity: self.calculate_severity(current_value, baseline_value),
                    description,
                    suggested_action: action,
                };
            }
        }

        // No anomaly detected
        AnomalyResult {
            is_anomaly: false,
            anomaly_type: AnomalyType::UnusualPattern,
            severity: 0.0,
            description: "Normal operation".to_string(),
            suggested_action: None,
        }
    }

    /// Calculate severity score (0.0-1.0)
    fn calculate_severity(&self, current: f64, baseline: f64) -> f64 {
        if baseline == 0.0 {
            return if current > 0.0 { 1.0 } else { 0.0 };
        }

        let deviation = ((current - baseline) / baseline.abs()).abs();
        (deviation / 2.0).min(1.0) // Normalize to 0-1 range
    }

    /// Get adaptive parameters for a specific context
    pub async fn get_adapted_params_for_context(
        &self,
        context: &ContextFeatures,
    ) -> (u32, f64, u64) {
        let params = self.adaptive_params.read().await.clone();

        // Adjust based on context
        let iterations = if context.time_pressure > 0.8 {
            // Under pressure, limit iterations
            params.max_iterations.0
        } else if context.task_complexity > 0.7 {
            // Complex tasks get more iterations
            params.max_iterations.1
        } else {
            // Normal case - middle ground
            (params.max_iterations.0 + params.max_iterations.1) / 2
        };

        let threshold = if context.user_expertise < 0.4 {
            // Beginners need higher quality
            params.quality_threshold.1
        } else if context.time_pressure > 0.6 {
            // Under pressure, lower threshold
            params.quality_threshold.0
        } else {
            (params.quality_threshold.0 + params.quality_threshold.1) / 2.0
        };

        let timeout = if context.time_pressure > 0.7 {
            // Urgent - shorter timeout
            params.timeout_secs.0
        } else if context.task_complexity > 0.7 {
            // Complex - longer timeout
            params.timeout_secs.1
        } else {
            (params.timeout_secs.0 + params.timeout_secs.1) / 2
        };

        (iterations, threshold, timeout)
    }

    /// Learn from execution outcome
    pub async fn learn_from_outcome(
        &self,
        _skill_name: &str,
        context: &ContextFeatures,
        success: bool,
        duration: Duration,
        quality_score: f64,
    ) {
        // Record outcome for future learning
        info!(
            "AI Learning: skill executed - success={}, duration={:.2}s, quality={:.2}",
            success,
            duration.as_secs_f64(),
            quality_score
        );

        // In production, would update internal ML model here
        // For now, just log and adjust simple heuristics
    }

    /// Get insights about system behavior
    pub async fn get_insights(&self) -> Vec<String> {
        let mut insights = Vec::new();

        let context_count = self.context_history.read().await.len();
        if context_count > 10 {
            insights.push(format!(
                "Analyzed {} task contexts",
                context_count
            ));
        }

        let perf_count = self.performance_history.read().await.len();
        if perf_count > 50 {
            insights.push(format!(
                "Collected {} performance data points",
                perf_count
            ));

            // Simple trend analysis
            let history = self.performance_history.read().await;
            if history.len() >= 10 {
                let recent_avg: f64 = history
                    .iter()
                    .rev()
                    .take(10)
                    .map(|p| p.value)
                    .sum::<f64>()
                    / 10.0;

                let older_avg: f64 = history
                    .iter()
                    .rev()
                    .skip(10)
                    .take(10)
                    .map(|p| p.value)
                    .sum::<f64>()
                    / 10.0.min(history.len().saturating_sub(10) as f64);

                if older_avg > 0.0 {
                    let change_percent =
                        ((recent_avg - older_avg) / older_avg * 100.0).round();
                    if change_percent > 10.0 {
                        insights.push(format!(
                            "⚠️ Metrics increasing by {}% recently",
                            change_percent
                        ));
                    } else if change_percent < -10.0 {
                        insights.push(format!(
                            "✓ Metrics improving by {}% recently",
                            change_percent.abs()
                        ));
                    }
                }
            }
        }

        let cache_size = self.recommendations_cache.read().await.len();
        if cache_size > 0 {
            insights.push(format!(
                "Generated {} skill recommendations",
                cache_size
            ));
        }

        if insights.is_empty() {
            insights.push("Insufficient data for insights. Continue using the system.".to_string());
        }

        insights
    }
}
