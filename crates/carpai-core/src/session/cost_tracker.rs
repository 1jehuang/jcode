//! Enhanced Cost Tracker - Detailed cost tracking and analysis

use crate::session::core_types::{CostSummary, SessionCostTracker};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Detailed cost breakdown per API call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostRecord {
    pub timestamp: DateTime<Utc>,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub duration_ms: u64,
}

/// Enhanced cost tracker with detailed records
pub struct EnhancedCostTracker {
    base_tracker: SessionCostTracker,
    records: Vec<CostRecord>,
    model_costs: HashMap<String, f64>,
}

impl EnhancedCostTracker {
    pub fn new(session_id: &str) -> Self {
        Self {
            base_tracker: SessionCostTracker::new(session_id),
            records: Vec::new(),
            model_costs: HashMap::new(),
        }
    }

    /// Record an API call with details
    pub fn record_call(
        &mut self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
        duration_ms: u64,
    ) {
        self.base_tracker.record_api_call(input_tokens, output_tokens, cost_usd);
        
        let record = CostRecord {
            timestamp: Utc::now(),
            model: model.to_string(),
            input_tokens,
            output_tokens,
            cost_usd,
            duration_ms,
        };
        
        self.records.push(record);
        
        // Track per-model costs
        *self.model_costs.entry(model.to_string()).or_insert(0.0) += cost_usd;
    }

    /// Get cost breakdown by model
    pub fn get_model_breakdown(&self) -> HashMap<String, f64> {
        self.model_costs.clone()
    }

    /// Get recent records
    pub fn get_recent_records(&self, count: usize) -> Vec<&CostRecord> {
        self.records.iter().rev().take(count).collect()
    }

    /// Get average cost per call
    pub fn get_average_cost(&self) -> f64 {
        if self.base_tracker.api_calls == 0 {
            return 0.0;
        }
        self.base_tracker.total_cost_usd / self.base_tracker.api_calls as f64
    }

    /// Get cost summary
    pub fn get_summary(&self) -> CostSummary {
        self.base_tracker.get_summary()
    }

    /// Get total records count
    pub fn record_count(&self) -> usize {
        self.records.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enhanced_tracking() {
        let mut tracker = EnhancedCostTracker::new("test-session");
        
        tracker.record_call("gpt-4", 100, 200, 0.01, 500);
        tracker.record_call("gpt-3.5", 50, 100, 0.005, 300);
        
        assert_eq!(tracker.record_count(), 2);
        
        let breakdown = tracker.get_model_breakdown();
        assert!(breakdown.contains_key("gpt-4"));
        assert!(breakdown.contains_key("gpt-3.5"));
        
        let avg_cost = tracker.get_average_cost();
        assert!((avg_cost - 0.0075).abs() < 0.0001);
    }
}
