use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ABTest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub variants: Vec<Variant>,
    pub status: TestStatus,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub traffic_percentage: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variant {
    pub id: String,
    pub name: String,
    pub config: HashMap<String, String>,
    pub metrics: VariantMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantMetrics {
    pub participants: u64,
    pub conversions: u64,
    pub conversion_rate: f64,
    pub average_duration_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TestStatus {
    Draft,
    Running,
    Paused,
    Completed,
    WinnerDeclared(String),
}

pub struct ABTestFramework {
    tests: HashMap<String, ABTest>,
    user_assignments: HashMap<String, String>,
}

impl ABTestFramework {
    pub fn new() -> Self {
        ABTestFramework {
            tests: HashMap::new(),
            user_assignments: HashMap::new(),
        }
    }

    pub fn create_test(
        &mut self,
        name: &str,
        description: &str,
        variant_names: &[&str],
        traffic_pct: f64,
    ) -> Result<ABTest, String> {
        let test_id = Uuid::new_v4().to_string();
        let variants: Vec<Variant> = variant_names.iter().enumerate().map(|(i, name)| Variant {
            id: format!("{}-v{}", test_id, i),
            name: name.to_string(),
            config: HashMap::new(),
            metrics: VariantMetrics {
                participants: 0,
                conversions: 0,
                conversion_rate: 0.0,
                average_duration_ms: 0.0,
            },
        }).collect();

        let test = ABTest {
            id: test_id.clone(),
            name: name.to_string(),
            description: description.to_string(),
            variants,
            status: TestStatus::Draft,
            created_at: chrono::Utc::now(),
            traffic_percentage: traffic_pct,
        };

        self.tests.insert(test_id.clone(), test.clone());
        Ok(test)
    }

    pub fn start_test(&mut self, test_id: &str) -> Result<(), String> {
        let test = self.tests.get_mut(test_id).ok_or("Test not found")?;
        if test.status != TestStatus::Draft { return Err("Test is not in draft status".to_string()); }
        test.status = TestStatus::Running;
        Ok(())
    }

    pub fn assign_variant(&mut self, test_id: &str, user_id: &str) -> Result<&Variant, String> {
        let test = self.tests.get(test_id).ok_or("Test not found")?;
        if test.status != TestStatus::Running { return Err("Test is not running".to_string()); }

        let variant_idx = self.user_assignments
            .entry(user_id.to_string())
            .or_insert_with(|| {
                use std::time::{SystemTime, UNIX_EPOCH};
                let seed = SystemTime::now().duration_since(UNIX_EPOCH)
                    .unwrap_or(std::time::Duration::from_secs(0))
                    .as_nanos() as usize;
                (seed % test.variants.len()).to_string()
            })
            .parse::<usize>()
            .unwrap_or(0);

        Ok(&test.variants[variant_idx])
    }

    pub fn record_conversion(&mut self, test_id: &str, user_id: &str) -> Result<(), String> {
        let test = self.tests.get_mut(test_id).ok_or("Test not found")?;
        let variant_idx = self.user_assignments.get(user_id)
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);

        if let Some(variant) = test.variants.get_mut(variant_idx) {
            variant.metrics.conversions += 1;
            variant.metrics.conversion_rate =
                variant.metrics.conversions as f64 / variant.metrics.participants.max(1) as f64;
        }
        Ok(())
    }

    pub fn get_results(&self, test_id: &str) -> Option<&ABTest> { self.tests.get(test_id) }

    pub fn declare_winner(&mut self, test_id: &str, variant_id: &str) -> Result<(), String> {
        let test = self.tests.get_mut(test_id).ok_or("Test not found")?;
        if !test.variants.iter().any(|v| v.id == variant_id) { return Err("Invalid variant ID".to_string()); }
        test.status = TestStatus::WinnerDeclared(variant_id.to_string());
        Ok(())
    }
}
