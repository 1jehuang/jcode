// Health check implementation

use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

/// Service health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub service: String,
    pub status: HealthStatus,
    pub details: Option<String>,
}

/// Overall health status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Health checker for all server components
pub struct HealthChecker {
    components: Arc<RwLock<Vec<ServiceHealth>>>,
}

impl HealthChecker {
    pub fn new() -> Self {
        Self {
            components: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn register_component(&self, service: String) {
        let mut components = self.components.write().await;
        components.push(ServiceHealth {
            service,
            status: HealthStatus::Healthy,
            details: None,
        });
    }

    pub async fn update_status(&self, service: &str, status: HealthStatus, details: Option<String>) {
        let mut components = self.components.write().await;
        if let Some(component) = components.iter_mut().find(|c| c.service == service) {
            component.status = status;
            component.details = details;
        }
    }

    pub async fn get_overall_status(&self) -> HealthStatus {
        let components = self.components.read().await;
        if components.is_empty() {
            return HealthStatus::Healthy;
        }

        if components.iter().any(|c| c.status == HealthStatus::Unhealthy) {
            HealthStatus::Unhealthy
        } else if components.iter().any(|c| c.status == HealthStatus::Degraded) {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        }
    }

    pub async fn get_all_components(&self) -> Vec<ServiceHealth> {
        self.components.read().await.clone()
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}
