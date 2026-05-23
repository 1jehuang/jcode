//! Multi-runtime architecture for service isolation
//!
//! Splits the single Tokio runtime into per-service runtimes to improve:
//! - Fault isolation: One service crash doesn't affect others
//! - Resource control: Each runtime can have dedicated thread pool size
//! - Performance: Reduced contention between different workload types
//!
//! Architecture:
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │              Main Thread                     │
//! └──────────────────┬──────────────────────────┘
//!                    │
//!     ┌──────────────┼──────────────┐
//!     ▼              ▼              ▼
//! ┌─────────┐  ┌──────────┐  ┌──────────┐
//! │ API     │  │ Agent    │  │ Infra    │
//! │ Runtime │  │ Runtime  │  │ Runtime  │
//! │(REST/   │  │(Swarm/   │  │(DB/      │
//! │ gRPC/WS)│  │ GOAP)    │  │ Redis)   │
//! └─────────┘  └──────────┘  └──────────┘
//!     4 threads     8 threads    2 threads
//! ```

use std::sync::Arc;
use tokio::runtime::{Runtime, Builder};
use tracing::{info, error};

/// Runtime configuration for a specific service
#[derive(Debug, Clone)]
pub struct ServiceRuntimeConfig {
    /// Name of the service runtime
    pub name: &'static str,
    /// Number of worker threads
    pub worker_threads: usize,
    /// Maximum number of blocking threads
    pub max_blocking_threads: usize,
    /// Stack size for spawned threads
    pub thread_stack_size: usize,
}

impl ServiceRuntimeConfig {
    /// Default config for API-facing services (high concurrency, low latency)
    pub fn api_runtime() -> Self {
        let cpu_count = num_cpus::get();
        Self {
            name: "api",
            worker_threads: (cpu_count / 2).max(2).min(8),
            max_blocking_threads: 64,
            thread_stack_size: 2 * 1024 * 1024, // 2MB
        }
    }

    /// Default config for agent/compute services (CPU-intensive tasks)
    pub fn agent_runtime() -> Self {
        let cpu_count = num_cpus::get();
        Self {
            name: "agent",
            worker_threads: cpu_count.max(4).min(16),
            max_blocking_threads: 128,
            thread_stack_size: 4 * 1024 * 1024, // 4MB for deep recursion
        }
    }

    /// Default config for infrastructure services (I/O bound)
    pub fn infra_runtime() -> Self {
        Self {
            name: "infra",
            worker_threads: 2.min(num_cpus::get()),
            max_blocking_threads: 32,
            thread_stack_size: 1 * 1024 * 1024, // 1MB
        }
    }

    /// Default config for background/GC tasks (low priority)
    pub fn background_runtime() -> Self {
        Self {
            name: "background",
            worker_threads: 1.min(num_cpus::get()),
            max_blocking_threads: 16,
            thread_stack_size: 1 * 1024 * 1024,
        }
    }
}

/// Manages multiple Tokio runtimes for service isolation
pub struct RuntimeManager {
    /// API runtime for handling incoming requests (REST/gRPC/WebSocket)
    api_runtime: Option<Runtime>,
    /// Agent runtime for AI agent execution and computation
    agent_runtime: Option<Runtime>,
    /// Infrastructure runtime for database/Redis operations
    infra_runtime: Option<Runtime>,
    /// Background runtime for GC/metrics/cleanup tasks
    background_runtime: Option<Runtime>,
}

impl RuntimeManager {
    /// Create a new runtime manager with default configurations
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        info!("Initializing multi-runtime architecture...");

        let api_config = ServiceRuntimeConfig::api_runtime();
        let agent_config = ServiceRuntimeConfig::agent_runtime();
        let infra_config = ServiceRuntimeConfig::infra_runtime();
        let bg_config = ServiceRuntimeConfig::background_runtime();

        let api_runtime = Self::build_runtime(&api_config)?;
        let agent_runtime = Self::build_runtime(&agent_config)?;
        let infra_runtime = Self::build_runtime(&infra_config)?;
        let background_runtime = Self::build_runtime(&bg_config)?;

        info!(
            "Multi-runtime initialized: API={} threads, Agent={} threads, Infra={} threads, BG={} threads",
            api_config.worker_threads,
            agent_config.worker_threads,
            infra_config.worker_threads,
            bg_config.worker_threads
        );

        Ok(Self {
            api_runtime: Some(api_runtime),
            agent_runtime: Some(agent_runtime),
            infra_runtime: Some(infra_runtime),
            background_runtime: Some(background_runtime),
        })
    }

    /// Build a Tokio runtime from configuration
    fn build_runtime(config: &ServiceRuntimeConfig) -> Result<Runtime, Box<dyn std::error::Error + Send + Sync>> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(config.worker_threads)
            .max_blocking_threads(config.max_blocking_threads)
            .thread_name(format!("carpai-{}-worker", config.name))
            .thread_stack_size(config.thread_stack_size)
            .enable_all()
            .build()?;

        info!(
            "Runtime '{}' built: {} workers, {} blocking threads",
            config.name, config.worker_threads, config.max_blocking_threads
        );

        Ok(runtime)
    }

    /// Get a reference to the API runtime
    pub fn api_runtime(&self) -> &Runtime {
        self.api_runtime.as_ref().expect("API runtime not initialized")
    }

    /// Get a reference to the Agent runtime
    pub fn agent_runtime(&self) -> &Runtime {
        self.agent_runtime.as_ref().expect("Agent runtime not initialized")
    }

    /// Get a reference to the Infrastructure runtime
    pub fn infra_runtime(&self) -> &Runtime {
        self.infra_runtime.as_ref().expect("Infra runtime not initialized")
    }

    /// Get a reference to the Background runtime
    pub fn background_runtime(&self) -> &Runtime {
        self.background_runtime.as_ref().expect("Background runtime not initialized")
    }

    /// Spawn a task on the appropriate runtime based on service type
    pub fn spawn_on<F>(&self, service: ServiceType, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        match service {
            ServiceType::Api => self.api_runtime().spawn(future),
            ServiceType::Agent => self.agent_runtime().spawn(future),
            ServiceType::Infra => self.infra_runtime().spawn(future),
            ServiceType::Background => self.background_runtime().spawn(future),
        }
    }

    /// Block on a future using the specified runtime
    pub fn block_on<F>(&self, service: ServiceType, future: F) -> F::Output
    where
        F: std::future::Future,
    {
        match service {
            ServiceType::Api => self.api_runtime().block_on(future),
            ServiceType::Agent => self.agent_runtime().block_on(future),
            ServiceType::Infra => self.infra_runtime().block_on(future),
            ServiceType::Background => self.background_runtime().block_on(future),
        }
    }

    /// Shutdown all runtimes gracefully
    pub async fn shutdown(&mut self) {
        info!("Shutting down all runtimes...");

        // Shutdown in reverse dependency order
        if let Some(rt) = self.background_runtime.take() {
            info!("Background runtime shutdown");
            drop(rt);
        }

        if let Some(rt) = self.infra_runtime.take() {
            info!("Infrastructure runtime shutdown");
            drop(rt);
        }

        if let Some(rt) = self.agent_runtime.take() {
            info!("Agent runtime shutdown");
            drop(rt);
        }

        if let Some(rt) = self.api_runtime.take() {
            info!("API runtime shutdown");
            drop(rt);
        }

        info!("All runtimes shut down successfully");
    }

    /// Get runtime statistics
    pub fn get_stats(&self) -> RuntimeStats {
        RuntimeStats {
            api_workers: self.api_runtime.as_ref().map(|rt| rt.metrics().num_workers()).unwrap_or(0),
            agent_workers: self.agent_runtime.as_ref().map(|rt| rt.metrics().num_workers()).unwrap_or(0),
            infra_workers: self.infra_runtime.as_ref().map(|rt| rt.metrics().num_workers()).unwrap_or(0),
            bg_workers: self.background_runtime.as_ref().map(|rt| rt.metrics().num_workers()).unwrap_or(0),
        }
    }
}

impl Default for RuntimeManager {
    fn default() -> Self {
        // This will panic if called before proper initialization
        // Use RuntimeManager::new() instead
        Self {
            api_runtime: None,
            agent_runtime: None,
            infra_runtime: None,
            background_runtime: None,
        }
    }
}

/// Service type for routing tasks to appropriate runtimes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceType {
    /// API-facing services (REST, gRPC, WebSocket handlers)
    Api,
    /// Agent execution (AI inference, swarm coordination, GOAP planning)
    Agent,
    /// Infrastructure (database queries, Redis operations, file I/O)
    Infra,
    /// Background tasks (GC, metrics collection, cleanup)
    Background,
}

/// Runtime statistics snapshot
#[derive(Debug, Clone)]
pub struct RuntimeStats {
    pub api_workers: usize,
    pub agent_workers: usize,
    pub infra_workers: usize,
    pub bg_workers: usize,
}

impl std::fmt::Display for RuntimeStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Runtime Stats: API={}w, Agent={}w, Infra={}w, BG={}w",
            self.api_workers, self.agent_workers, self.infra_workers, self.bg_workers
        )
    }
}

/// Global runtime manager instance (lazy-initialized)
static RUNTIME_MANAGER: std::sync::OnceLock<std::sync::Mutex<Option<RuntimeManager>>> = std::sync::OnceLock::new();

/// Initialize the global runtime manager
pub fn init_global_runtime_manager() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let manager = RuntimeManager::new()?;
    RUNTIME_MANAGER.get_or_init(|| std::sync::Mutex::new(Some(manager)));
    Ok(())
}

/// Get a reference to the global runtime manager
pub fn global_runtime_manager() -> Option<std::sync::MutexGuard<'static, Option<RuntimeManager>>> {
    RUNTIME_MANAGER.get()?.lock().ok()
}

/// Convenience macro to spawn tasks on specific runtimes
#[macro_export]
macro_rules! spawn_on {
    ($service:expr, $future:expr) => {
        if let Some(manager_guard) = $crate::runtime_manager::global_runtime_manager() {
            if let Some(manager) = manager_guard.as_ref() {
                manager.spawn_on($service, $future)
            } else {
                tokio::spawn($future)
            }
        } else {
            tokio::spawn($future)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_config_defaults() {
        let api = ServiceRuntimeConfig::api_runtime();
        assert!(api.worker_threads >= 2);
        assert!(api.worker_threads <= 8);

        let agent = ServiceRuntimeConfig::agent_runtime();
        assert!(agent.worker_threads >= 4);

        let infra = ServiceRuntimeConfig::infra_runtime();
        assert_eq!(infra.worker_threads, 1.min(num_cpus::get()));
    }

    #[tokio::test]
    async fn test_runtime_manager_creation() {
        let manager = RuntimeManager::new().expect("Failed to create runtime manager");
        assert!(manager.api_runtime.is_some());
        assert!(manager.agent_runtime.is_some());
        assert!(manager.infra_runtime.is_some());
        assert!(manager.background_runtime.is_some());

        let stats = manager.get_stats();
        assert!(stats.api_workers > 0);
        assert!(stats.agent_workers > 0);
    }
}
