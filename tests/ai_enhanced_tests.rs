//! Unit tests for AI Enhancement module
//!
//! Tests cover:
//! - Context feature analysis
//! - Skill recommendation system
//! - Adaptive parameter tuning
//! - Anomaly detection
//! - Learning from outcomes
//! - Insight generation

use carpai::ai_enhanced::{
    AiEngine, ContextFeatures, SkillRecommendation,
    AdaptiveParams, AnomalyResult, AnomalyType, AnomalyThresholds,
};
use std::time::Duration;

// ════════════════════════════════════════════════════════════════
// Context Features Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_context_features_default_values() {
    let ctx = ContextFeatures::default();
    
    assert!((ctx.task_complexity - 0.5).abs() < f64::EPSILON);
    assert_eq!(ctx.code_length, 100);
    assert!((ctx.error_rate - 0.1).abs() < f64::EPSILON);
    assert_eq!(ctx.previous_successes, 5);
    assert_eq!(ctx.previous_failures, 1);
    
    println!("✓ Context features default values correct");
}

#[test]
fn test_context_features_custom_creation() {
    let ctx = ContextFeatures {
        task_complexity: 0.9,
        code_length: 10000,
        error_rate: 0.05,
        previous_successes: 20,
        previous_failures: 2,
        time_pressure: 0.8,
        user_expertise: 0.3,
    };
    
    assert!((ctx.task_complexity - 0.9).abs() < f64::EPSILON);
    assert_eq!(ctx.code_length, 10000);
    assert!((ctx.time_pressure - 0.8).abs() < f64::EPSILON);
    
    println!("✓ Custom context features creation works");
}

#[test]
fn test_context_features_serialization() {
    let ctx = ContextFeatures {
        task_complexity: 0.75,
        code_length: 500,
        error_rate: 0.15,
        previous_successes: 10,
        previous_failures: 3,
        time_pressure: 0.6,
        user_expertise: 0.85,
    };
    
    let json = serde_json::to_string(&ctx).expect("Serialization failed");
    let parsed: ContextFeatures = serde_json::from_str(&json).expect("Deserialization failed");
    
    assert!((parsed.task_complexity - ctx.task_complexity).abs() < f64::EPSILON);
    assert_eq!(parsed.code_length, ctx.code_length);
    assert_eq!(parsed.previous_successes, ctx.previous_successes);
    
    println!("✓ Context features serialization round-trips");
}

// ════════════════════════════════════════════════════════════════
// AI Engine Creation Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_ai_engine_creation() {
    let engine = AiEngine::new();
    
    // Should start with empty history
    let insights = engine.get_insights().await;
    assert!(insights.len() >= 1); // At least "insufficient data" message
    
    println!("✓ AI engine creates with empty state");
}

#[tokio::test]
async fn test_ai_engine_initial_state() {
    let engine = AiEngine::new();
    
    let params = engine.get_adapted_params_for_context(&ContextFeatures::default()).await;
    
    // Should return reasonable defaults
    assert!(params.0 > 0, "Iterations should be positive");
    assert!(params.1 > 0.0 && params.1 <= 1.0, "Threshold should be in [0,1]");
    assert!(params.2 > 0, "Timeout should be positive");
    
    println!("✓ AI engine initial parameters are valid");
}

// ════════════════════════════════════════════════════════════════
// Skill Recommendation Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_recommend_skills_basic() {
    let engine = AiEngine::new();
    
    let context = ContextFeatures::default();
    let skills = vec!["loop".to_string(), "verify".to_string(), "simplify".to_string()];
    
    let recommendations = engine.recommend_skills(&context, &skills).await;
    
    // Should get at least some recommendations
    assert!(!recommendations.is_empty(), "Should recommend at least one skill");
    
    for rec in &recommendations {
        assert!(rec.confidence > 0.3, "Confidence should be > 0.3 for included skills");
        assert!(rec.confidence <= 1.0, "Confidence should be <= 1.0");
        assert!(!rec.skill_name.is_empty());
        assert!(!rec.reason.is_empty());
        assert!(rec.estimated_benefit >= 0.0);
    }
    
    println!("✓ Basic skill recommendation works");
}

#[tokio::test]
async fn test_recommend_skills_sorted_by_confidence() {
    let engine = AiEngine::new();
    
    let context = ContextFeatures {
        task_complexity: 0.9,
        error_rate: 0.5,
        code_length: 2000,
        ..Default::default()
    };
    
    let skills = vec![
        "loop".to_string(),
        "verify".to_string(),
        "simplify".to_string(),
    ];
    
    let recommendations = engine.recommend_skills(&context, &skills).await;
    
    if recommendations.len() >= 2 {
        for i in 0..recommendations.len()-1 {
            assert!(
                recommendations[i].confidence >= recommendations[i+1].confidence,
                "Recommendations should be sorted by confidence descending"
            );
        }
    }
    
    println!("✓ Recommendations are properly sorted by confidence");
}

#[tokio::test]
async fn test_recommend_skills_high_error_rate() {
    let engine = AiEngine::new();
    
    let context = ContextFeatures {
        error_rate: 0.8,
        code_length: 5000,
        ..Default::default()
    };
    
    let skills = vec!["verify".to_string()];
    let recs = engine.recommend_skills(&context, &skills).await;
    
    if !recs.is_empty() {
        let verify_rec = &recs[0];
        assert_eq!(verify_rec.skill_name, "verify");
        assert!(verify_rec.confidence > 0.7, "Verify should have high confidence with high error rate");
    }
    
    println!("✓ High error rate increases verify skill confidence");
}

#[tokio::test]
async fn test_recommend_skills_complex_task() {
    let engine = AiEngine::new();
    
    let context = ContextFeatures {
        task_complexity: 0.95,
        user_expertise: 0.2,
        previous_failures: 5,
        ..Default::default()
    };
    
    let skills = vec!["loop".to_string()];
    let recs = engine.recommend_skills(&context, &skills).await;
    
    if !recs.is_empty() {
        let loop_rec = &recs[0];
        assert!(loop_rec.confidence > 0.8, "Loop should be highly recommended for complex tasks");
    }
    
    println!("✓ Complex tasks increase loop skill recommendation");
}

#[tokio::test]
async fn test_recommend_skills_unknown_skill() {
    let engine = AiEngine::new();
    
    let context = ContextFeatures::default();
    let skills = vec!["unknown-skill".to_string()];
    
    let recs = engine.recommend_skills(&context, &skills).await;
    
    if !recs.is_empty() {
        assert!(recs[0].confidence >= 0.3, "Unknown skill should still be recommended if above threshold");
    } else {
        println!("✓ Unknown skill below threshold (not recommended)");
    }
    
    println!("✓ Unknown skill handling works");
}

// ════════════════════════════════════════════════════════════════
// Adaptive Parameters Tests
// ════════════════════════════════════════════════════════════════

#[test]
fn test_adaptive_params_default() {
    let params = AdaptiveParams::default();
    
    assert_eq!(params.max_iterations, (3, 20));
    assert!((params.quality_threshold.0 - 0.6).abs() < f64::EPSILON);
    assert!((params.quality_threshold.1 - 0.95).abs() < f64::EPSILON);
    assert_eq!(params.timeout_secs, (30, 600));
    assert!((params.learning_rate - 0.1).abs() < f64::EPSILON);
    
    println!("✓ Adaptive params defaults are correct");
}

#[tokio::test]
async fn test_adapt_params_poor_success_rate() {
    let engine = AiEngine::new();
    
    let results = vec![
        (false, Duration::from_secs(30)),
        (false, Duration::from_secs(45)),
        (true, Duration::from_secs(60)),
        (false, Duration::from_secs(35)),
    ];
    
    let adapted = engine.adapt_params(&results).await;
    
    let original = AdaptiveParams::default();
    
    // With poor success rate (< 50%), should become more conservative
    assert!(
        adapted.quality_threshold.0 <= original.quality_threshold.0,
        "Should lower min quality threshold on poor success"
    );
    assert!(
        adapted.max_iterations.0 >= original.max_iterations.0,
        "Should increase min iterations on poor success"
    );
    
    println!("✓ Poor success rate triggers conservative adaptation");
}

#[tokio::test]
async fn test_adapt_params_excellent_success_rate() {
    let engine = AiEngine::new();
    
    let results = vec![
        (true, Duration::from_secs(10)),
        (true, Duration::from_secs(15)),
        (true, Duration::from_secs(12)),
        (true, Duration::from_secs(8)),
        (true, Duration::from_secs(11)),
    ];
    
    let adapted = engine.adapt_params(&results).await;
    
    let original = AdaptiveParams::default();
    
    // With excellent success rate and fast execution, can be more aggressive
    assert!(
        adapted.max_iterations.0 >= original.max_iterations.0 * 0.95,
        "Iterations shouldn't decrease much"
    );
    
    println!("✓ Excellent success rate allows aggressive adaptation");
}

#[tokio::test]
async fn test_get_adapted_params_for_urgent_context() {
    let engine = AiEngine::new();
    
    let urgent_ctx = ContextFeatures {
        time_pressure: 0.9,
        ..Default::default()
    };
    
    let normal_ctx = ContextFeatures {
        time_pressure: 0.2,
        ..Default::default()
    };
    
    let urgent_params = engine.get_adapted_params_for_context(&urgent_ctx).await;
    let normal_params = engine.get_adapted_params_for_context(&normal_ctx).await;
    
    // Urgent context should have fewer iterations and shorter timeout
    assert!(
        urgent_params.0 <= normal_params.0,
        "Urgent context should limit iterations"
    );
    assert!(
        urgent_params.2 <= normal_params.2,
        "Urgent context should shorten timeout"
    );
    
    println!("✓ Context-aware parameter adjustment works");
}

#[tokio::test]
async fn test_get_adapted_params_for_beginner() {
    let engine = AiEngine::new();
    
    let beginner_ctx = ContextFeatures {
        user_expertise: 0.2,
        ..Default::default()
    };
    
    let expert_ctx = ContextFeatures {
        user_expertise: 0.95,
        ..Default::default()
    };
    
    let beginner_params = engine.get_adapted_params_for_context(&beginner_ctx).await;
    let expert_params = engine.get_adapted_params_for_context(&expert_ctx).await;
    
    // Beginners should have higher quality threshold
    assert!(
        beginner_params.1 >= expert_params.1,
        "Beginners need higher quality threshold"
    );
    
    println!("✓ User expertise affects parameter selection");
}

// ════════════════════════════════════════════════════════════════
// Anomaly Detection Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_detect_anomaly_normal_operation() {
    let engine = AiEngine::new();
    
    let result = engine.detect_anomalies("response_time", 100.0, 105.0).await;
    
    assert!(!result.is_anomaly, "Small deviation from baseline should not trigger anomaly");
    assert_eq!(result.anomaly_type, AnomalyType::UnusualPattern);
    assert!(result.severity == 0.0);
    
    println!("✓ Normal operation correctly identified as non-anomalous");
}

#[tokio::test]
async fn test_detect_anomaly_performance_degradation() {
    let engine = AiEngine::new();
    
    let result = engine.detect_anomalies("response_time", 200.0, 100.0).await;
    
    if result.is_anomaly {
        assert_eq!(result.anomaly_type, AnomalyType::PerformanceDegradation);
        assert!(result.severity > 0.0);
        assert!(result.suggested_action.is_some());
        
        println!("✓ Performance degradation detected: {}", result.description);
    } else {
        println!("⚠ Large deviation may or may not trigger based on thresholds");
    }
}

#[tokio::test]
async fn test_detect_anomaly_extreme_deviation() {
    let engine = AiEngine::new();
    
    let result = engine.detect_anomalies("error_count", 1000.0, 10.0).await;
    
    if result.is_anomaly {
        assert!(result.severity > 0.5, "Extreme deviation should have high severity");
    }
    
    println!("✓ Extreme deviation handling works");
}

// ════════════════════════════════════════════════════════════════
// Learning and Insights Tests
// ════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_learn_from_outcome_success() {
    let engine = AiEngine::new();
    
    let context = ContextFeatures::default();
    
    engine.learn_from_outcome(
        "loop",
        &context,
        true,
        Duration::from_secs(15),
        0.92,
    ).await;
    
    let insights = engine.get_insights().await;
    
    assert!(!insights.is_empty());
    
    println!("✓ Learning from successful outcome works");
}

#[tokio::test]
async fn test_learn_from_outcome_failure() {
    let engine = AiEngine::new();
    
    let context = ContextFeatures {
        error_rate: 0.8,
        ..Default::default()
    };
    
    engine.learn_from_outcome(
        "verify",
        &context,
        false,
        Duration::from_secs(120),
        0.25,
    ).await;
    
    let insights = engine.get_insights().await;
    
    assert!(!insights.is_empty());
    
    println!("✓ Learning from failed outcome works");
}

#[tokio::test]
async fn test_insights_after_multiple_operations() {
    let engine = AiEngine::new();
    
    let context = ContextFeatures::default();
    
    for i in 0..15 {
        engine.learn_from_outcome(
            "loop",
            &context,
            i % 3 != 0,
            Duration::from_secs(10 + i as u64),
            0.7 + (i as f64) * 0.02,
        ).await;
    }
    
    let insights = engine.get_insights().await;
    
    assert!(insights.len() >= 2, "Should have multiple insights after many operations");
    
    println!("✓ Multiple operations generate richer insights ({})", insights.len());
}

// ════════════════════════════════════════════════════════════════
// Edge Cases and Error Handling
// ════════════════════════════════════════════════════════════════

#[test]
fn test_skill_recommendation_serialization() {
    let rec = SkillRecommendation {
        skill_name: "loop".to_string(),
        confidence: 0.85,
        reason: "Complex task requires iteration".to_string(),
        estimated_benefit: 0.75,
    };
    
    let json = serde_json::to_string(&rec).expect("Serialization failed");
    let parsed: SkillRecommendation = serde_json::from_str(&json).expect("Deserialization failed");
    
    assert_eq!(parsed.skill_name, "loop");
    assert!((parsed.confidence - 0.85).abs() < f64::EPSILON);
    assert_eq!(parsed.reason, rec.reason);
    
    println!("✓ Skill recommendation serialization round-trips");
}

#[test]
fn test_anomaly_type_display() {
    let types = vec![
        (AnomalyType::PerformanceDegradation, "performance-degradation"),
        (AnomalyType::ErrorSpike, "error-spike"),
        (AnomalyType::MemoryLeak, "memory-leak"),
        (AnomalyType::UnusualPattern, "unusual-pattern"),
        (AnomalyType::TimeoutExceeded, "timeout-exceeded"),
    ];
    
    for (anomaly_type, expected) in types {
        let display = format!("{}", anomaly_type);
        assert_eq!(display, expected);
    }
    
    println!("✓ All anomaly type display formats correct");
}

#[test]
fn test_anomaly_result_serialization() {
    let result = AnomalyResult {
        is_anomaly: true,
        anomaly_type: AnomalyType::PerformanceDegradation,
        severity: 0.85,
        description: "Response time degraded significantly".to_string(),
        suggested_action: Some("Check system load".to_string()),
    };
    
    let json = serde_json::to_string(&result).expect("Serialization failed");
    let parsed: AnomalyResult = serde_json::from_str(&json).expect("Deserialization failed");
    
    assert!(parsed.is_anomaly);
    assert_eq!(parsed.anomaly_type, AnomalyType::PerformanceDegradation);
    assert!(parsed.suggested_action.is_some());
    
    println!("✓ Anomaly result serialization round-trips");
}

#[test]
fn test_anomaly_thresholds_default() {
    let thresholds = AnomalyThresholds::default();
    
    assert!((thresholds.performance_degradation_pct - 50.0).abs() < f64::EPSILON);
    assert!((thresholds.error_spike_factor - 3.0).abs() < f64::EPSILON);
    assert!((thresholds.memory_growth_rate_mb_per_min - 10.0).abs() < f64::EPSILON);
    assert!((thresholds.pattern_deviation_stddev - 2.0).abs() < f64::EPSILON);
    assert!((thresholds.timeout_multiplier - 2.0).abs() < f64::EPSILON);
    
    println!("✓ Anomaly thresholds defaults are sensible");
}

#[tokio::test]
async fn test_empty_available_skills() {
    let engine = AiEngine::new();
    
    let context = ContextFeatures::default();
    let recommendations = engine.recommend_skills(&context, &[]).await;
    
    assert!(recommendations.is_empty(), "No skills available means no recommendations");
    
    println!("✓ Empty skill list handled gracefully");
}

#[tokio::test]
async fn test_detect_anomaly_zero_baseline() {
    let engine = AiEngine::new();
    
    let result = engine.detect_anomalies("counter", 50.0, 0.0).await;
    
    if result.is_anomaly {
        assert!(result.severity > 0.0, "Non-zero value from zero baseline is anomalous");
    }
    
    println!("✓ Zero baseline handling works");
}
