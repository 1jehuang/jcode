use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct OptimizationParameter {
    pub name: String,
    pub current_value: f64,
    pub min_value: f64,
    pub max_value: f64,
    pub optimal_value: Option<f64>,
    pub optimization_history: Vec<OptimizationStep>,
}

#[derive(Debug, Clone)]
pub struct OptimizationStep {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub old_value: f64,
    pub new_value: f64,
    pub improvement: f64,
}

pub struct AutoOptimizer {
    parameters: HashMap<String, OptimizationParameter>,
    learning_rate: f64,
    enabled: bool,
}

impl AutoOptimizer {
    pub fn new() -> Self {
        AutoOptimizer {
            parameters: HashMap::new(),
            learning_rate: 0.1,
            enabled: true,
        }
    }

    pub fn register_parameter(&mut self, name: &str, initial: f64, min: f64, max: f64) {
        self.parameters.insert(name.to_string(), OptimizationParameter {
            name: name.to_string(),
            current_value: initial,
            min_value: min,
            max_value: max,
            optimal_value: None,
            optimization_history: vec![],
        });
    }

    pub fn optimize(&mut self, metric_name: &str, current_performance: f64) -> Option<f64> {
        if !self.enabled { return None; }

        let param = self.parameters.get_mut(metric_name)?;
        use std::time::{SystemTime, UNIX_EPOCH};
        let random_val = SystemTime::now().duration_since(UNIX_EPOCH)
            .unwrap_or(std::time::Duration::from_secs(0))
            .as_secs_f64().fract() * 2.0 - 1.0;
        let adjustment = (random_val - 0.5) * self.learning_rate;
        let new_value = (param.current_value + adjustment).clamp(param.min_value, param.max_value);

        if new_value != param.current_value {
            param.optimization_history.push(OptimizationStep {
                timestamp: chrono::Utc::now(),
                old_value: param.current_value,
                new_value,
                improvement: current_performance,
            });
            param.current_value = new_value;
            Some(new_value)
        } else {
            None
        }
    }

    pub fn get_optimal_value(&self, name: &str) -> Option<f64> { self.parameters.get(name).map(|p| p.current_value) }

    pub fn get_optimization_report(&self) -> Vec<(String, f64)> {
        self.parameters.iter()
            .map(|(name, param)| (name.clone(), param.current_value))
            .collect()
    }

    pub fn set_learning_rate(&mut self, rate: f64) { self.learning_rate = rate.clamp(0.01, 1.0); }
}
