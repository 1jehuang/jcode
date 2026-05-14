use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExperimentStatus {
    Draft,
    Running,
    Paused,
    Completed,
    StoppedEarly(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MetricType {
    ConversionRate,
    AverageDuration,
    ErrorRate,
    UserSatisfaction,
    TaskCompletionRate,
    Throughput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variant {
    pub id: String,
    pub name: String,
    pub config: serde_json::Value,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experiment {
    pub id: String,
    pub name: String,
    pub description: String,
    pub variants: Vec<Variant>,
    pub target_metric: MetricType,
    pub traffic_split: Vec<f64>,
    pub status: ExperimentStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub min_sample_size: u64,
    pub significance_level: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAssignment {
    pub experiment_id: String,
    pub variant_id: String,
    pub assigned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VariantMetrics {
    pub participant_count: u64,
    pub conversions: u64,
    pub total_value: f64,
    pub sum_of_squares: f64,
    pub errors: u64,
}

impl VariantMetrics {
    pub fn conversion_rate(&self) -> f64 {
        if self.participant_count == 0 { 0.0 }
        else { self.conversions as f64 / self.participant_count as f64 }
    }

    pub fn average_value(&self) -> f64 {
        if self.participant_count == 0 { 0.0 }
        else { self.total_value / self.participant_count as f64 }
    }

    pub fn variance(&self) -> f64 {
        if self.participant_count <= 1 { return 0.0; }
        let mean = self.average_value();
        let n = self.participant_count as f64;
        (self.sum_of_squares - n * mean * mean) / (n - 1.0)
    }

    pub fn error_rate(&self) -> f64 {
        if self.participant_count == 0 { 0.0 }
        else { self.errors as f64 / self.participant_count as f64 }
    }

    pub fn record_event(&mut self, value: f64, converted: bool, is_error: bool) {
        self.participant_count += 1;
        self.total_value += value;
        self.sum_of_squares += value * value;
        if converted { self.conversions += 1; }
        if is_error { self.errors += 1; }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AbTestResults {
    pub variant_metrics: HashMap<String, VariantMetrics>,
    pub winner: Option<String>,
    pub confidence: Option<f64>,
    pub effect_size: Option<f64>,
    pub stopped_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbTestConfig {
    pub default_significance_level: f64,
    pub min_sample_per_variant: u64,
    pub early_stop_enabled: bool,
    pub early_stop_min_observations: u64,
    pub early_stop_threshold: f64,
    pub max_experiment_duration_hours: u64,
}

impl Default for AbTestConfig {
    fn default() -> Self {
        AbTestConfig {
            default_significance_level: 0.05,
            min_sample_per_variant: 100,
            early_stop_enabled: true,
            early_stop_min_observations: 50,
            early_stop_threshold: 0.99,
            max_experiment_duration_hours: 168,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatisticalResult {
    pub is_significant: bool,
    pub p_value: f64,
    pub test_statistic: f64,
    pub confidence_interval: (f64, f64),
    pub effect_size: f64,
    pub power: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualizationData {
    pub experiment_name: String,
    pub variant_names: Vec<String>,
    pub conversion_rates: Vec<f64>,
    pub confidence_intervals: Vec<(f64, f64)>,
    pub sample_sizes: Vec<u64>,
    pub cumulative_data: Vec<CumulativePoint>,
    pub winner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CumulativePoint {
    pub timestamp: i64,
    pub variant_metrics: HashMap<String, (f64, u64)>,
}

pub struct AbTestManager {
    experiments: HashMap<String, Experiment>,
    active_sessions: HashMap<String, SessionAssignment>,
    results_store: AbTestResults,
    config: AbTestConfig,
}

impl AbTestManager {
    pub fn new(config: Option<AbTestConfig>) -> Self {
        AbTestManager {
            experiments: HashMap::new(),
            active_sessions: HashMap::new(),
            results_store: AbTestResults::default(),
            config: config.unwrap_or_default(),
        }
    }

    pub fn create_experiment(
        &mut self,
        name: &str,
        description: &str,
        variants: Vec<Variant>,
        target_metric: MetricType,
        traffic_split: Option<Vec<f64>>,
    ) -> Result<Experiment, String> {
        if variants.is_empty() { return Err("At least one variant required".to_string()); }
        if variants.len() > 10 { return Err("Maximum 10 variants allowed".to_string()); }

        let split = traffic_split.unwrap_or_else(|| {
            let w = 1.0 / variants.len() as f64;
            variants.iter().map(|_| w).collect()
        });

        if (split.iter().sum::<f64>() - 1.0).abs() > 1e-6 {
            return Err("Traffic split must sum to 1.0".to_string());
        }
        if split.len() != variants.len() {
            return Err("Traffic split length must match variants".to_string());
        }

        let id = uuid::Uuid::new_v4().to_string();
        let experiment = Experiment {
            id: id.clone(),
            name: name.to_string(),
            description: description.to_string(),
            target_metric,
            status: ExperimentStatus::Draft,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            min_sample_size: self.config.min_sample_per_variant * variants.len() as u64,
            significance_level: self.config.default_significance_level,
            variants,
            traffic_split: split,
        };

        self.results_store.variant_metrics = experiment.variants
            .iter().map(|v| (v.id.clone(), VariantMetrics::default()))
            .collect();

        self.experiments.insert(id.clone(), experiment.clone());
        Ok(experiment)
    }

    pub fn start_experiment(&mut self, experiment_id: &str) -> Result<(), String> {
        let exp = self.experiments.get_mut(experiment_id).ok_or("Experiment not found")?;
        match exp.status {
            ExperimentStatus::Draft => {
                exp.status = ExperimentStatus::Running;
                exp.started_at = Some(Utc::now());
                Ok(())
            }
            ExperimentStatus::Paused => {
                exp.status = ExperimentStatus::Running;
                Ok(())
            }
            _ => Err(format!("Cannot start experiment in {:?} state", exp.status)),
        }
    }

    pub fn pause_experiment(&mut self, experiment_id: &str) -> Result<(), String> {
        let exp = self.experiments.get_mut(experiment_id).ok_or("Experiment not found")?;
        if exp.status != ExperimentStatus::Running {
            return Err("Only running experiments can be paused".to_string());
        }
        exp.status = ExperimentStatus::Paused;
        Ok(())
    }

    pub fn complete_experiment(&mut self, experiment_id: &str) -> Result<AbTestResults, String> {
        let exp = self.experiments.get_mut(experiment_id).ok_or("Experiment not found")?;
        if !matches!(exp.status, ExperimentStatus::Running | ExperimentStatus::Paused) {
            return Err("Only running or paused experiments can be completed".to_string());
        }
        exp.status = ExperimentStatus::Completed;
        exp.completed_at = Some(Utc::now());

        let result = self.analyze_results(experiment_id)?;
        self.results_store.winner = result.winner.clone();
        self.results_store.confidence = result.confidence;
        self.results_store.effect_size = result.effect_size;
        Ok(self.results_store.clone())
    }

    pub fn consistent_hash_assign(
        &self,
        experiment_id: &str,
        user_id: &str,
    ) -> Result<&Variant, String> {
        let exp = self.experiments.get(experiment_id).ok_or("Experiment not found")?;
        if exp.status != ExperimentStatus::Running {
            return Err("Experiment is not running".to_string());
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        format!("{}:{}{}", experiment_id, user_id, "ab_test_salt").hash(&mut hasher);
        let hash_val = hasher.finish();

        let total_weight: f64 = exp.traffic_split.iter().sum();
        let mut cumulative = 0.0;
        let normalized_hash = (hash_val % 10_000_000) as f64 / 10_000_000.0;

        for (i, weight) in exp.traffic_split.iter().enumerate() {
            cumulative += weight / total_weight;
            if normalized_hash < cumulative {
                return Ok(&exp.variants[i]);
            }
        }
        Ok(exp.variants.last().unwrap())
    }

    pub fn assign_user(&mut self, experiment_id: &str, user_id: &str) -> Result<SessionAssignment, String> {
        let variant = self.consistent_hash_assign(experiment_id, user_id)?;
        let assignment = SessionAssignment {
            experiment_id: experiment_id.to_string(),
            variant_id: variant.id.clone(),
            assigned_at: Utc::now(),
        };
        self.active_sessions.insert(user_id.to_string(), assignment.clone());
        Ok(assignment)
    }

    pub fn record_metric(
        &mut self,
        experiment_id: &str,
        variant_id: &str,
        value: f64,
        converted: bool,
        is_error: bool,
    ) -> Result<(), String> {
        let _exp = self.experiments.get(experiment_id).ok_or("Experiment not found")?;
        let metrics = self.results_store.variant_metrics
            .get_mut(variant_id)
            .ok_or("Variant not found")?;
        metrics.record_event(value, converted, is_error);
        Ok(())
    }

    pub fn t_test_independent(
        &self,
        sample1: &[f64],
        sample2: &[f64],
        alpha: f64,
    ) -> StatisticalResult {
        let n1 = sample1.len().max(1) as f64;
        let n2 = sample2.len().max(1) as f64;
        let mean1 = sample1.iter().sum::<f64>() / n1;
        let mean2 = sample2.iter().sum::<f64>() / n2;
        let var1 = sample1.iter().map(|x| (x - mean1).powi(2)).sum::<f64>() / (n1 - 1.0).max(1.0);
        let var2 = sample2.iter().map(|x| (x - mean2).powi(2)).sum::<f64>() / (n2 - 1.0).max(1.0);

        let pooled_se = ((var1 / n1) + (var2 / n2)).sqrt();
        let t_stat = if pooled_se > 0.0 { (mean1 - mean2) / pooled_se } else { 0.0 };
        let df = (n1 + n2 - 2.0).max(1.0);

        let p_value = self.approx_t_pvalue(t_stat.abs(), df);
        let se = pooled_se * 1.96;
        let ci_lower = (mean1 - mean2) - se;
        let ci_upper = (mean1 - mean2) + se;

        let effect_size = if pooled_se > 0.0 { (mean1 - mean2) / (var1.sqrt().max(var2.sqrt()).max(1e-10)) } else { 0.0 };

        StatisticalResult {
            is_significant: p_value < alpha,
            p_value,
            test_statistic: t_stat,
            confidence_interval: (ci_lower, ci_upper),
            effect_size,
            power: Some(self.approx_power(t_stat.abs(), df, alpha)),
        }
    }

    fn approx_t_pvalue(&self, t: f64, df: f64) -> f64 {
        let x = df / (df + t * t);
        let regularized_beta = self.regularized_incomplete_beta(df / 2.0, 0.5, x);
        1.0 - regularized_beta
    }

    fn regularized_incomplete_beta(&self, a: f64, b: f64, x: f64) -> f64 {
        if x <= 0.0 { return 0.0; }
        if x >= 1.0 { return 1.0; }
        let max_iter = 200;
        let eps = 1e-10;
        let mut result = 0.0;
        let mut term = 1.0 / a;
        for n in 0..max_iter {
            result += term;
            let m = n as f64;
            term *= x * (a + b + m) / (a + m + 1.0);
            if term.abs() < eps * result.abs() { break; }
        }
        result * x.powf(a) * (1.0 - x).powf(b) / (a * self.beta_func(a, b))
    }

    fn beta_func(&self, a: f64, b: f64) -> f64 {
        self.ln_gamma(a) + self.ln_gamma(b) - self.ln_gamma(a + b)
    }

    fn ln_gamma(&self, x: f64) -> f64 {
        let cof: [f64; 6] = [
            76.18009172947146, -86.50532032941677,
            24.01409824083091, -1.231739572450155,
            0.1208650973866179e-2, -0.5395239384953e-5,
        ];
        let y = x;
        let tmp = y + 5.5;
        let ser = 1.000000000190015 +
            cof[0]/(y+1.0) + cof[1]/(y+2.0) + cof[2]/(y+3.0) +
            cof[3]/(y+4.0) + cof[4]/(y+5.0) + cof[5]/(y+6.0);
        (y + 0.5) * tmp.ln() - tmp + ser.ln()
    }

    fn approx_power(&self, t_observed: f64, df: f64, alpha: f64) -> f64 {
        let t_critical = self.inverse_t_cdf(1.0 - alpha / 2.0, df);
        let ncp = t_observed;
        let noncentral_p = 1.0 - self.approx_noncentral_t_cdf(t_critical, df, ncp);
        noncentral_p.max(0.0).min(1.0)
    }

    fn inverse_t_cdf(&self, p: f64, df: f64) -> f64 {
        let mut lo = -20.0;
        let mut hi = 20.0;
        for _ in 0..100 {
            let mid = (lo + hi) / 2.0;
            let cdf = 1.0 - self.approx_t_pvalue(mid, df);
            if (cdf - p).abs() < 1e-10 { return mid; }
            if cdf < p { lo = mid; } else { hi = mid; }
        }
        (lo + hi) / 2.0
    }

    fn approx_noncentral_t_cdf(&self, t: f64, df: f64, ncp: f64) -> f64 {
        let z = (t - ncp) / (1.0 + t.abs() / (2.0 * df).sqrt()).sqrt().max(1e-10);
        self.approx_normal_cdf(z)
    }

    fn approx_normal_cdf(&self, x: f64) -> f64 {
        let a1 = 0.254829592;
        let a2 = -0.284496736;
        let a3 = 1.421413741;
        let a4 = -1.453152027;
        let a5 = 1.061405429;
        let p = 0.3275911;
        let sign = if x >= 0.0 { 1.0 } else { -1.0 };
        let x = x.abs() / (2.0_f64).sqrt();
        let t = 1.0 / (1.0 + p * x);
        let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x * x / 2.0).exp();
        0.5 * (1.0 + sign * y)
    }

    pub fn chi_squared_test(
        &self,
        observed: &[u64],
        expected: &[f64],
        alpha: f64,
    ) -> StatisticalResult {
        let chi_stat: f64 = observed.iter().zip(expected.iter())
            .map(|(o, e)| {
                if *e > 0.0 { (*o as f64 - e).powi(2) / e }
                else { 0.0 }
            })
            .sum();

        let df = (observed.len().saturating_sub(1)) as f64;
        let p_value = self.approx_chi_squared_pvalue(chi_stat, df);

        let total_o: f64 = observed.iter().map(|&x| x as f64).sum();
        let prop_vec: Vec<f64> = observed.iter().map(|&o| o as f64 / total_o.max(1.0)).collect();
        let max_prop = prop_vec.iter().cloned().fold(0.0_f64, f64::max);
        let min_prop = prop_vec.iter().cloned().fold(f64::MAX, f64::min);
        let effect_size = max_prop - min_prop;

        StatisticalResult {
            is_significant: p_value < alpha,
            p_value,
            test_statistic: chi_stat,
            confidence_interval: (0.0, chi_stat),
            effect_size,
            power: None,
        }
    }

    fn approx_chi_squared_pvalue(&self, chi_sq: f64, df: f64) -> f64 {
        if chi_sq <= 0.0 || df <= 0.0 { return 1.0; }
        self.regularized_incomplete_beta(df / 2.0, 0.5, df / (df + chi_sq))
    }

    pub fn analyze_results(&self, experiment_id: &str) -> Result<StatisticalResult, String> {
        let exp = self.experiments.get(experiment_id).ok_or("Experiment not found")?;

        if exp.variants.len() < 2 {
            return Err("Need at least 2 variants to analyze".to_string());
        }

        let control_metrics = self.results_store.variant_metrics
            .get(&exp.variants[0].id)
            .ok_or("Control variant metrics not found")?;
        let treatment_metrics = self.results_store.variant_metrics
            .get(&exp.variants[1].id)
            .ok_or("Treatment variant metrics not found")?;

        if control_metrics.participant_count < self.config.min_sample_per_variant ||
           treatment_metrics.participant_count < self.config.min_sample_per_variant {
            return Err(format!(
                "Insufficient samples. Need at least {} per variant, got control={} treatment={}",
                self.config.min_sample_per_variant,
                control_metrics.participant_count,
                treatment_metrics.participant_count
            ));
        }

        match exp.target_metric {
            MetricType::ConversionRate | MetricType::TaskCompletionRate => {
                let obs = vec![control_metrics.conversions, treatment_metrics.conversions];
                let total_control = control_metrics.participant_count as f64;
                let total_treatment = treatment_metrics.participant_count as f64;
                let overall_rate =
                    (control_metrics.conversions + treatment_metrics.conversions) as f64 /
                    (total_control + total_treatment);
                let expected = vec![total_control * overall_rate, total_treatment * overall_rate];
                self.chi_squared_test(&obs, &expected, exp.significance_level)
            }
            _ => {
                let control_vals = vec![control_metrics.average_value(); control_metrics.participant_count.min(100) as usize];
                let treatment_vals = vec![treatment_metrics.average_value(); treatment_metrics.participant_count.min(100) as usize];
                self.t_test_independent(&control_vals, &treatment_vals, exp.significance_level)
            }
        }
    }

    pub fn check_early_stopping(&mut self, experiment_id: &str) -> Result<Option<EarlyStopDecision>, String> {
        if !self.config.early_stop_enabled {
            return Ok(None);
        }

        let exp = self.experiments.get(experiment_id).ok_or("Experiment not found")?;
        if exp.status != ExperimentStatus::Running {
            return Ok(None);
        }

        let total_samples: u64 = self.results_store.variant_metrics.values()
            .map(|m| m.participant_count).sum();

        if total_samples < self.config.early_stop_min_observations {
            return Ok(None);
        }

        let analysis = self.analyze_results(experiment_id)?;

        if analysis.is_significant && analysis.p_value < (1.0 - self.config.early_stop_threshold) {
            let winner_id = self.determine_winner(experiment_id)?;
            let decision = EarlyStopDecision {
                should_stop: true,
                reason: format!(
                    "Statistical significance reached (p={:.6}, threshold={}). Winner: {}",
                    analysis.p_value, self.config.early_stop_threshold, winner_id
                ),
                winner_id: Some(winner_id),
                confidence: Some(1.0 - analysis.p_value),
                at_sample_size: total_samples,
            };

            self.stop_experiment_early(experiment_id, &decision.reason)?;
            return Ok(Some(decision));
        }

        if let Some(started) = exp.started_at {
            let elapsed = Utc::now().signed_duration_since(started).num_hours() as u64;
            if elapsed >= self.config.max_experiment_duration_hours {
                let reason = format!("Max duration ({}) hours exceeded", self.config.max_experiment_duration_hours);
                self.stop_experiment_early(experiment_id, &reason)?;
                return Ok(Some(EarlyStopDecision {
                    should_stop: true,
                    reason,
                    winner_id: None,
                    confidence: None,
                    at_sample_size: total_samples,
                }));
            }
        }

        Ok(None)
    }

    fn determine_winner(&self, experiment_id: &str) -> Result<String, String> {
        let exp = self.experiments.get(experiment_id).ok_or("Experiment not found")?;
        let mut best_id = String::new();
        let mut best_rate = -1.0_f64;

        for v in &exp.variants {
            if let Some(m) = self.results_store.variant_metrics.get(&v.id) {
                let rate = m.conversion_rate();
                if rate > best_rate {
                    best_rate = rate;
                    best_id = v.id.clone();
                }
            }
        }

        if best_id.is_empty() { Err("No valid metrics to determine winner".to_string()) }
        else { Ok(best_id) }
    }

    fn stop_experiment_early(&mut self, experiment_id: &str, reason: &str) -> Result<(), String> {
        let exp = self.experiments.get_mut(experiment_id).ok_or("Experiment not found")?;
        exp.status = ExperimentStatus::StoppedEarly(reason.to_string());
        exp.completed_at = Some(Utc::now());
        self.results_store.stopped_reason = Some(reason.to_string());
        Ok(())
    }

    pub fn generate_visualization_data(&self, experiment_id: &str) -> Result<VisualizationData, String> {
        let exp = self.experiments.get(experiment_id).ok_or("Experiment not found")?;

        let variant_names: Vec<String> = exp.variants.iter().map(|v| v.name.clone()).collect();
        let conversion_rates: Vec<f64> = exp.variants.iter()
            .map(|v| {
                self.results_store.variant_metrics.get(&v.id)
                    .map(|m| m.conversion_rate())
                    .unwrap_or(0.0)
            })
            .collect();
        let sample_sizes: Vec<u64> = exp.variants.iter()
            .map(|v| {
                self.results_store.variant_metrics.get(&v.id)
                    .map(|m| m.participant_count)
                    .unwrap_or(0)
            })
            .collect();

        let confidence_intervals: Vec<(f64, f64)> = conversion_rates.iter().zip(sample_sizes.iter())
            .map(|(&rate, &n)| {
                if n == 0 { (0.0, 0.0) }
                else {
                    let se = (rate * (1.0 - rate) / n as f64).sqrt();
                    let margin = 1.96 * se;
                    ((rate - margin).max(0.0), (rate + margin).min(1.0))
                }
            })
            .collect();

        let winner = self.results_store.winner.clone().or_else(|| {
            if conversion_rates.len() >= 2 {
                let best_idx = conversion_rates.iter()
                    .enumerate().max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
                    .map(|(i, _)| i);
                best_idx.map(|i| exp.variants[i].id.clone())
            } else { None }
        });

        Ok(VisualizationData {
            experiment_name: exp.name.clone(),
            variant_names,
            conversion_rates,
            confidence_intervals,
            sample_sizes,
            cumulative_data: vec![],
            winner,
        })
    }

    pub fn get_experiment(&self, experiment_id: &str) -> Option<&Experiment> {
        self.experiments.get(experiment_id)
    }

    pub fn get_all_experiments(&self) -> Vec<&Experiment> {
        self.experiments.values().collect()
    }

    pub fn get_active_sessions(&self) -> &HashMap<String, SessionAssignment> {
        &self.active_sessions
    }

    pub fn get_results(&self) -> &AbTestResults {
        &self.results_store
    }

    pub fn get_config(&self) -> &AbTestConfig {
        &self.config
    }

    pub fn reset(&mut self) {
        self.experiments.clear();
        self.active_sessions.clear();
        self.results_store = AbTestResults::default();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EarlyStopDecision {
    pub should_stop: bool,
    pub reason: String,
    pub winner_id: Option<String>,
    pub confidence: Option<f64>,
    pub at_sample_size: u64,
}

pub fn create_auto_mode_confidence_experiment(manager: &mut AbTestManager) -> Result<Experiment, String> {
    let variants = vec![
        Variant {
            id: "conf-low".to_string(),
            name: "Low Confidence (0.60)".to_string(),
            config: serde_json::json!({"threshold": 0.60, "auto_accept": false}),
            weight: 0.33,
        },
        Variant {
            id: "conf-mid".to_string(),
            name: "Medium Confidence (0.75)".to_string(),
            config: serde_json::json!({"threshold": 0.75, "auto_accept": true}),
            weight: 0.34,
        },
        Variant {
            id: "conf-high".to_string(),
            name: "High Confidence (0.90)".to_string(),
            config: serde_json::json!({"threshold": 0.90, "auto_accept": true}),
            weight: 0.33,
        },
    ];
    manager.create_experiment(
        "Auto Mode Confidence Threshold",
        "Compare different confidence thresholds for auto-mode acceptance",
        variants,
        MetricType::TaskCompletionRate,
        None,
    )
}

pub fn create_safety_rail_experiment(manager: &mut AbTestManager) -> Result<Experiment, String> {
    let variants = vec![
        Variant {
            id: "safety-lenient".to_string(),
            name: "Lenient Safety".to_string(),
            config: serde_json::json!({"strictness": "low", "allow_risky_commands": true}),
            weight: 0.5,
        },
        Variant {
            id: "safety-strict".to_string(),
            name: "Strict Safety".to_string(),
            config: serde_json::json!({"strictness": "high", "allow_risky_commands": false}),
            weight: 0.5,
        },
    ];
    manager.create_experiment(
        "Safety Rail Strictness",
        "Compare lenient vs strict safety rail policies",
        variants,
        MetricType::ErrorRate,
        None,
    )
}

pub fn create_completion_sorting_experiment(manager: &mut AbTestManager) -> Result<Experiment, String> {
    let variants = vec![
        Variant {
            id: "sort-freq".to_string(),
            name: "Frequency Sort".to_string(),
            config: serde_json::json!({"algorithm": "frequency", "boost_recent": false}),
            weight: 0.5,
        },
        Variant {
            id: "sort-recency".to_string(),
            name: "Recency Sort".to_string(),
            config: serde_json::json!({"algorithm": "recency", "boost_recent": true}),
            weight: 0.5,
        },
    ];
    manager.create_experiment(
        "Completion Sorting Strategy",
        "Compare frequency-based vs recency-based completion sorting",
        variants,
        MetricType::ConversionRate,
        None,
    )
}

pub fn create_memory_extraction_experiment(manager: &mut AbTestManager) -> Result<Experiment, String> {
    let variants = vec![
        Variant {
            id: "mem-frequent".to_string(),
            name: "Frequent Extraction".to_string(),
            config: serde_json::json!({"extract_every_n_turns": 3, "max_context_items": 20}),
            weight: 0.5,
        },
        Variant {
            id: "mem-sparse".to_string(),
            name: "Sparse Extraction".to_string(),
            config: serde_json::json!({"extract_every_n_turns": 10, "max_context_items": 10}),
            weight: 0.5,
        },
    ];
    manager.create_experiment(
        "Memory Extraction Frequency",
        "Compare frequent vs sparse memory extraction strategies",
        variants,
        MetricType::UserSatisfaction,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_manager() -> AbTestManager {
        AbTestManager::new(Some(AbTestConfig {
            default_significance_level: 0.05,
            min_sample_per_variant: 10,
            early_stop_enabled: true,
            early_stop_min_observations: 5,
            early_stop_threshold: 0.95,
            max_experiment_duration_hours: 720,
        }))
    }

    fn create_two_variant_experiment(manager: &mut AbTestManager) -> String {
        let variants = vec![
            Variant { id: "ctrl".into(), name: "Control".into(), config: serde_json::json!({}), weight: 0.5 },
            Variant { id: "treat".into(), name: "Treatment".into(), config: serde_json::json!({}), weight: 0.5 },
        ];
        let exp = manager.create_experiment("Test Exp", "Desc", variants, MetricType::ConversionRate, None).unwrap();
        exp.id
    }

    #[test]
    fn test_create_experiment_success() {
        let mut mgr = setup_manager();
        let exp_id = create_two_variant_experiment(&mut mgr);
        let exp = mgr.get_experiment(&exp_id).unwrap();
        assert_eq!(exp.name, "Test Exp");
        assert_eq!(exp.status, ExperimentStatus::Draft);
        assert_eq!(exp.variants.len(), 2);
    }

    #[test]
    fn test_create_experiment_empty_variants_fails() {
        let mut mgr = setup_manager();
        let result = mgr.create_experiment("Bad", "No variants", vec![], MetricType::ConversionRate, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_start_and_pause_experiment() {
        let mut mgr = setup_manager();
        let exp_id = create_two_variant_experiment(&mut mgr);
        assert!(mgr.start_experiment(&exp_id).is_ok());
        assert_eq!(mgr.get_experiment(&exp_id).unwrap().status, ExperimentStatus::Running);
        assert!(mgr.pause_experiment(&exp_id).is_ok());
        assert_eq!(mgr.get_experiment(&exp_id).unwrap().status, ExperimentStatus::Paused);
        assert!(mgr.start_experiment(&exp_id).is_ok());
        assert_eq!(mgr.get_experiment(&exp_id).unwrap().status, ExperimentStatus::Running);
    }

    #[test]
    fn test_consistent_hash_deterministic_assignment() {
        let mgr = setup_manager();
        let exp_id = {
            let mut m = setup_manager();
            create_two_variant_experiment(&mut m)
        };
        let mut m = setup_manager();
        create_two_variant_experiment(&mut m);
        let real_exp_id = m.get_all_experiments()[0].id.clone();

        let v1 = m.consistent_hash_assign(&real_exp_id, "user_123").unwrap();
        let v2 = m.consistent_hash_assign(&real_exp_id, "user_123").unwrap();
        assert_eq!(v1.id, v2.id);
    }

    #[test]
    fn test_user_assignment_creates_session() {
        let mut mgr = setup_manager();
        let exp_id = create_two_variant_experiment(&mut mgr);
        mgr.start_experiment(&exp_id).unwrap();
        let assignment = mgr.assign_user(&exp_id, "user_alpha").unwrap();
        assert_eq!(assignment.experiment_id, exp_id);
        assert!(!assignment.variant_id.is_empty());
        assert!(mgr.get_active_sessions().contains_key("user_alpha"));
    }

    #[test]
    fn test_record_metrics_and_aggregation() {
        let mut mgr = setup_manager();
        let exp_id = create_two_variant_experiment(&mut mgr);
        mgr.record_metric(&exp_id, "ctrl", 1.0, true, false).unwrap();
        mgr.record_metric(&exp_id, "ctrl", 0.0, false, false).unwrap();
        mgr.record_metric(&exp_id, "treat", 1.0, true, false).unwrap();

        let ctrl = mgr.get_results().variant_metrics.get("ctrl").unwrap();
        assert_eq!(ctrl.participant_count, 2);
        assert_eq!(ctrl.conversions, 1);
        assert!((ctrl.conversion_rate() - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_t_test_significant_difference() {
        let mgr = setup_manager();
        let group_a: Vec<f64> = (0..50).map(|i| 5.0 + (i as f64 % 10.0) * 0.1).collect();
        let group_b: Vec<f64> = (0..50).map(|i| 8.0 + (i as f64 % 10.0) * 0.1).collect();
        let result = mgr.t_test_independent(&group_a, &group_b, 0.05);
        assert!(result.is_significant);
        assert!(result.p_value < 0.05);
        assert!(result.effect_size.abs() > 0.01);
    }

    #[test]
    fn test_t_test_no_significant_difference() {
        let mgr = setup_manager();
        let base: Vec<f64> = (0..50).map(|i| 5.0 + (i as f64 % 10.0) * 0.1).collect();
        let noise: Vec<f64> = base.iter().map(|x| x + rand::rng().random_range(-0.1..0.1)).collect();
        let result = mgr.t_test_independent(&base, &noise, 0.05);
        assert!(!result.is_significant || result.p_value > 0.01);
    }

    #[test]
    fn test_chi_squared_test_distribution() {
        let mgr = setup_manager();
        let observed = vec![45u64, 55];
        let expected = vec![50.0, 50.0];
        let result = mgr.chi_squared_test(&observed, &expected, 0.05);
        assert!(!result.is_significant);
        assert!(result.test_statistic >= 0.0);
    }

    #[test]
    fn test_complete_experiment_produces_results() {
        let mut mgr = setup_manager();
        let exp_id = create_two_variant_experiment(&mut mgr);
        mgr.start_experiment(&exp_id).unwrap();
        for _ in 0..15 {
            mgr.record_metric(&exp_id, "ctrl", 1.0, true, false).unwrap();
            mgr.record_metric(&exp_id, "treat", 1.0, true, false).unwrap();
        }
        let results = mgr.complete_experiment(&exp_id).unwrap();
        assert!(results.winner.is_some());
    }

    #[test]
    fn test_visualization_data_generation() {
        let mut mgr = setup_manager();
        let exp_id = create_two_variant_experiment(&mut mgr);
        mgr.record_metric(&exp_id, "ctrl", 1.0, true, false).unwrap();
        mgr.record_metric(&exp_id, "treat", 1.0, false, false).unwrap();
        let viz = mgr.generate_visualization_data(&exp_id).unwrap();
        assert_eq!(viz.variant_names.len(), 2);
        assert_eq!(viz.conversion_rates.len(), 2);
        assert_eq!(viz.sample_sizes.len(), 2);
        assert_eq!(viz.experiment_name, "Test Exp");
    }

    #[test]
    fn test_early_stopping_with_strong_signal() {
        let mut mgr = setup_manager();
        let exp_id = create_two_variant_experiment(&mut mgr);
        mgr.start_experiment(&exp_id).unwrap();
        for _ in 0..20 {
            mgr.record_metric(&exp_id, "ctrl", 1.0, false, false).unwrap();
            mgr.record_metric(&exp_id, "treat", 1.0, true, false).unwrap();
        }
        let decision = mgr.check_early_stopping(&exp_id).unwrap();
        assert!(decision.is_some());
        assert!(decision.unwrap().should_stop);
    }

    #[test]
    fn test_scenario_factory_functions() {
        let mut mgr = setup_manager();
        let conf_exp = create_auto_mode_confidence_experiment(&mut mgr).unwrap();
        assert_eq!(conf_exp.variants.len(), 3);
        let safety_exp = create_safety_rail_experiment(&mut mgr).unwrap();
        assert_eq!(safety_exp.variants.len(), 2);
        let comp_exp = create_completion_sorting_experiment(&mut mgr).unwrap();
        assert_eq!(comp_exp.target_metric, MetricType::ConversionRate);
        let mem_exp = create_memory_extraction_experiment(&mut mgr).unwrap();
        assert_eq!(mem_exp.target_metric, MetricType::UserSatisfaction);
    }

    #[test]
    fn test_invalid_traffic_split_rejected() {
        let mut mgr = setup_manager();
        let variants = vec![
            Variant { id: "a".into(), name: "A".into(), config: serde_json::json!({}), weight: 0.5 },
            Variant { id: "b".into(), name: "B".into(), config: serde_json::json!({}), weight: 0.5 },
        ];
        let result = mgr.create_experiment("X", "Y", variants, MetricType::ConversionRate, Some(vec![0.3, 0.3]));
        assert!(result.is_err());
    }

    #[test]
    fn test_reset_clears_state() {
        let mut mgr = setup_manager();
        create_two_variant_experiment(&mut mgr);
        mgr.reset();
        assert!(mgr.get_all_experiments().is_empty());
        assert!(mgr.get_active_sessions().is_empty());
    }
}
