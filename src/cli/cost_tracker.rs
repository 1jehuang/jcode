//! Cost Tracking and Usage Statistics for CarpAI
//!
//! Provides comprehensive token usage and cost monitoring:
//! - Real-time token counting
//! - Cost estimation by model
//! - Budget management and alerts
//! - Session and daily aggregation

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

/// Token usage information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

impl TokenUsage {
    /// Create new token usage record
    pub fn new(model: impl Into<String>, input_tokens: u64, output_tokens: u64) -> Self {
        let total = input_tokens + output_tokens;
        Self {
            model: model.into(),
            input_tokens,
            output_tokens,
            total_tokens: total,
        }
    }

    /// Calculate cost based on pricing
    pub fn calculate_cost(&self, pricing: &ModelPricing) -> f64 {
        (self.input_tokens as f64 * pricing.input_per_1k / 1000.0)
            + (self.output_tokens as f64 * pricing.output_per_1k / 1000.0)
    }
}

/// Model pricing configuration (per 1K tokens)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub model: String,
    pub input_per_1k: f64,
    pub output_per_1k: f64,
    pub currency: String,
}

impl Default for ModelPricing {
    fn default() -> Self {
        Self {
            model: "gpt-4".to_string(),
            input_per_1k: 0.03,
            output_per_1k: 0.06,
            currency: "USD".to_string(),
        }
    }
}

/// Predefined pricing for common models
pub fn get_model_pricing(model: &str) -> Option<ModelPricing> {
    match model {
        "gpt-4" | "gpt-4-turbo" => Some(ModelPricing {
            model: model.to_string(),
            input_per_1k: 0.03,
            output_per_1k: 0.06,
            currency: "USD".to_string(),
        }),
        "gpt-3.5-turbo" => Some(ModelPricing {
            model: model.to_string(),
            input_per_1k: 0.0015,
            output_per_1k: 0.002,
            currency: "USD".to_string(),
        }),
        "claude-3-opus" => Some(ModelPricing {
            model: model.to_string(),
            input_per_1k: 0.015,
            output_per_1k: 0.075,
            currency: "USD".to_string(),
        }),
        "claude-3-sonnet" => Some(ModelPricing {
            model: model.to_string(),
            input_per_1k: 0.003,
            output_per_1k: 0.015,
            currency: "USD".to_string(),
        }),
        _ => None,
    }
}

/// Cost tracker with atomic operations for thread safety
pub struct CostTracker {
    total_input_tokens: AtomicU64,
    total_output_tokens: AtomicU64,
    total_cost: Mutex<f64>,
    by_model: RwLock<HashMap<String, ModelUsage>>,
    session_start: std::time::Instant,
}

/// Usage statistics per model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub request_count: u64,
    pub total_cost: f64,
}

impl Default for ModelUsage {
    fn default() -> Self {
        Self {
            model: String::new(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            request_count: 0,
            total_cost: 0.0,
        }
    }
}

impl CostTracker {
    /// Create new cost tracker
    pub fn new() -> Self {
        Self {
            total_input_tokens: AtomicU64::new(0),
            total_output_tokens: AtomicU64::new(0),
            total_cost: Mutex::new(0.0),
            by_model: RwLock::new(HashMap::new()),
            session_start: std::time::Instant::now(),
        }
    }

    /// Track token usage for a request
    pub fn track_usage(&self, usage: &TokenUsage) -> Result<f64> {
        // Update global counters
        self.total_input_tokens.fetch_add(usage.input_tokens, Ordering::SeqCst);
        self.total_output_tokens.fetch_add(usage.output_tokens, Ordering::SeqCst);

        // Calculate cost
        let cost = if let Some(pricing) = get_model_pricing(&usage.model) {
            usage.calculate_cost(&pricing)
        } else {
            0.0 // Unknown model, cannot calculate cost
        };
        
        *self.total_cost.lock().unwrap_or_else(|e| e.into_inner()) += cost;

        // Update per-model stats
        {
            let mut models = self.by_model.write().unwrap_or_else(|e| e.into_inner());
            let entry = models.entry(usage.model.clone()).or_insert_with(|| ModelUsage {
                model: usage.model.clone(),
                ..Default::default()
            });
            
            entry.total_input_tokens += usage.input_tokens;
            entry.total_output_tokens += usage.output_tokens;
            entry.request_count += 1;
            entry.total_cost += cost;
        }

        Ok(cost)
    }

    /// Get total session cost
    pub fn get_session_cost(&self) -> CostReport {
        let duration = self.session_start.elapsed();
        
        CostReport {
            total_input_tokens: self.total_input_tokens.load(Ordering::SeqCst),
            total_output_tokens: self.total_output_tokens.load(Ordering::SeqCst),
            total_tokens: self.total_input_tokens.load(Ordering::SeqCst)
                + self.total_output_tokens.load(Ordering::SeqCst),
            total_cost: *self.total_cost.lock().unwrap_or_else(|e| e.into_inner()),
            duration_secs: duration.as_secs_f64(),
            by_model: self.by_model.read().unwrap_or_else(|e| e.into_inner()).values().cloned().collect(),
            timestamp: chrono::Utc::now(),
        }
    }

    /// Get current budget status
    pub fn check_budget(&self, daily_limit: f64) -> BudgetStatus {
        let current = *self.total_cost.lock().unwrap_or_else(|e| e.into_inner());
        let remaining = daily_limit - current;
        let percentage = if daily_limit > 0.0 {
            (current / daily_limit) * 100.0
        } else {
            100.0
        };

        BudgetStatus {
            daily_limit,
            spent: current,
            remaining: remaining.max(0.0),
            percentage_used: percentage.min(100.0),
            is_over_budget: current > daily_limit,
            warning_level: if percentage > 90.0 {
                WarningLevel::Critical
            } else if percentage > 70.0 {
                WarningLevel::Warning
            } else {
                WarningLevel::Normal
            },
        }
    }

    /// Reset all counters
    pub fn reset(&self) {
        self.total_input_tokens.store(0, Ordering::SeqCst);
        self.total_output_tokens.store(0, Ordering::SeqCst);
        *self.total_cost.lock().unwrap_or_else(|e| e.into_inner()) = 0.0;
        
        let mut models = self.by_model.write().unwrap_or_else(|e| e.into_inner());
        models.clear();
    }
}

/// Comprehensive cost report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostReport {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
    pub duration_secs: f64,
    pub by_model: Vec<ModelUsage>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Budget status information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetStatus {
    pub daily_limit: f64,
    pub spent: f64,
    pub remaining: f64,
    pub percentage_used: f64,
    pub is_over_budget: bool,
    pub warning_level: WarningLevel,
}

/// Warning level for budget alerts
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WarningLevel {
    Normal,
    Warning,
    Critical,
}

/// CLI command handler for cost tracking
pub struct CostCommands {
    pub tracker: CostTracker,
}

impl CostCommands {
    /// Create new cost commands instance
    pub fn new(tracker: CostTracker) -> Self {
        Self { tracker }
    }

    /// Create with default tracker
    pub fn with_default_tracker() -> Self {
        Self::new(CostTracker::new())
    }

    /// Display session cost summary
    pub async fn show_session_cost(&self) -> Result<()> {
        let report = self.tracker.get_session_cost();
        
        println!("💰 Session Cost Summary");
        println!("{}", "═".repeat(50));
        println!("Total Tokens: {}", report.total_tokens);
        println!("  Input:  {}", report.total_input_tokens);
        println!("  Output: {}", report.total_output_tokens);
        println!("Total Cost: ${:.4} USD", report.total_cost);
        println!("Duration: {:.2} seconds", report.duration_secs);
        
        if !report.by_model.is_empty() {
            println!("\n📊 Breakdown by Model:");
            for model in &report.by_model {
                println!("  {}:", model.model);
                println!("    Requests: {}", model.request_count);
                println!("    Tokens:   {} in / {} out", 
                    model.total_input_tokens, model.total_output_tokens);
                println!("    Cost:     ${:.4}", model.total_cost);
            }
        }
        
        Ok(())
    }

    /// Display budget status
    pub async fn show_budget_status(&self, limit: f64) -> Result<()> {
        let status = self.tracker.check_budget(limit);
        
        match status.warning_level {
            WarningLevel::Normal => println!("✅ Budget Status: Normal"),
            WarningLevel::Warning => println!("⚠️  Budget Status: Warning"),
            WarningLevel::Critical => println!("🚨 Budget Status: CRITICAL!"),
        }
        
        println!("{}", "═".repeat(50));
        println!("Daily Limit:  ${:.2}", status.daily_limit);
        println!("Spent:        ${:.4}", status.spent);
        println!("Remaining:    ${:.4}", status.remaining);
        println!("Used:         {:.1}%", status.percentage_used);
        
        if status.is_over_budget {
            println!("\n❌ OVER BUDGET by ${:.4}!", status.spent - status.daily_limit);
        } else if status.percentage_used > 80.0 {
            println!("\n⚠️  Approaching budget limit!");
        }
        
        Ok(())
    }

    /// Reset counters
    pub async fn reset_counters(&self) -> Result<()> {
        self.tracker.reset();
        println!("✅ Cost counters reset successfully");
        Ok(())
    }
}

// Use parking_lot's RwLock for better performance
#[cfg(test)]
use parking_lot::RwLock;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_usage_creation() {
        let usage = TokenUsage::new("gpt-4", 100, 50);
        assert_eq!(usage.model, "gpt-4");
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn test_cost_calculation() {
        let usage = TokenUsage::new("gpt-4", 1000, 500);
        let pricing = get_model_pricing("gpt-4").unwrap();
        let cost = usage.calculate_cost(&pricing);
        
        // Expected: (1000 * 0.03 / 1000) + (500 * 0.06 / 1000)
        // = 0.03 + 0.03 = 0.06
        assert!((cost - 0.06).abs() < 0.001);
    }

    #[test]
    fn test_cost_tracker_basic() {
        let tracker = CostTracker::new();
        
        let usage1 = TokenUsage::new("gpt-4", 100, 50);
        let usage2 = TokenUsage::new("gpt-3.5-turbo", 200, 100);
        
        tracker.track_usage(&usage1).unwrap();
        tracker.track_usage(&usage2).unwrap();
        
        let report = tracker.get_session_cost();
        assert_eq!(report.total_tokens, 450); // 150 + 300
        assert!(report.total_cost > 0.0);
    }

    #[test]
    fn test_budget_check() {
        let tracker = CostTracker::new();
        
        let usage = TokenUsage::new("gpt-4", 10000, 5000);
        tracker.track_usage(&usage).unwrap();
        
        let status = tracker.check_budget(10.0);
        assert!(!status.is_over_budget);
        assert_eq!(status.warning_level, WarningLevel::Normal);
    }

    #[test]
    fn test_get_model_pricing() {
        assert!(get_model_pricing("gpt-4").is_some());
        assert!(get_model_pricing("unknown-model").is_none());
    }

    #[test]
    fn test_warning_level_display() {
        assert_eq!(format!("{:?}", WarningLevel::Normal), "Normal");
        assert_eq!(format!("{:?}", WarningLevel::Warning), "Warning");
        assert_eq!(format!("{:?}", WarningLevel::Critical), "Critical");
    }
}
