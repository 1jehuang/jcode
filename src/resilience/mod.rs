//! Error Recovery and Resilience System
//!
//! Advanced error handling with:
//! - Automatic reconnection strategies
//! - Data consistency checks
//! - Graceful degradation
//! - Circuit breaker pattern
//! - Retry with exponential backoff

use anyhow::Result;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

/// Reconnection strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectConfig {
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
    pub jitter: bool,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            initial_delay_ms: 100,
            max_delay_ms: 30000,
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Connected,
    Connecting,
    Disconnected,
    Reconnecting {
        attempt: u32,
        next_retry: Instant,
    },
    Failed {
        last_error: String,
    },
}

impl std::fmt::Display for ConnectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connected => write!(f, "Connected"),
            Self::Connecting => write!(f, "Connecting"),
            Self::Disconnected => write!(f, "Disconnected"),
            Self::Reconnecting { attempt, .. } => {
                write!(f, "Reconnecting (attempt {})", attempt)
            }
            Self::Failed { last_error } => {
                write!(f, "Failed: {}", last_error)
            }
        }
    }
}

/// Reconnection manager with exponential backoff
pub struct ReconnectionManager {
    config: ReconnectConfig,
    state: Arc<RwLock<ConnectionState>>,
    attempt: Mutex<u32>,
    current_delay: Mutex<Duration>,
    history: Mutex<VecDeque<ReconnectAttempt>>,
    on_reconnect: Option<Arc<dyn Fn() -> Result<()> + Send + Sync>>,
    on_failure: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

impl ReconnectionManager {
    pub fn new(config: ReconnectConfig) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            attempt: Mutex::new(0),
            current_delay: Mutex::new(Duration::from_millis(
                config.initial_delay_ms,
            )),
            history: Mutex::new(VecDeque::with_capacity(100)),
            on_reconnect: None,
            on_failure: None,
        }
    }

    /// Set reconnection callback
    pub fn on_reconnect<F>(&mut self, callback: F)
    where
        F: Fn() -> Result<()> + Send + Sync + 'static,
    {
        self.on_reconnect = Some(Arc::new(callback));
    }

    /// Set failure callback
    pub fn on_failure<F>(&mut self, callback: F)
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_failure = Some(Arc::new(callback));
    }

    /// Get current state
    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    /// Attempt to reconnect
    pub async fn try_reconnect(&self) -> Result<bool> {
        let mut attempt = self.attempt.lock().await;
        *attempt += 1;
        let current_attempt = *attempt;

        if current_attempt > self.config.max_attempts {
            // All attempts exhausted
            *self.state.write().await = ConnectionState::Failed {
                last_error: format!(
                    "Max attempts ({}) exceeded",
                    self.config.max_attempts
                ),
            };

            if let Some(ref cb) = self.on_failure {
                cb("Max reconnection attempts exceeded");
            }

            return Ok(false);
        }

        // Calculate delay with jitter
        let mut delay = {
            let mut current_delay = self.current_delay.lock().await;
            if self.config.jitter {
                use rand::RngCore;
                let mut rng = rand::rngs::OsRng;
                let jitter_range = (*current_delay).as_millis() as f64 * 0.1;
                let jitter_val = (rng.next_u64() as f64 % jitter_range) as u64;
                Duration::from_millis((*current_delay).as_millis() + jitter_val)
            } else {
                *current_delay
            }
        };

        // Update state to reconnecting
        *self.state.write().await = ConnectionState::Reconnecting {
            attempt: current_attempt,
            next_retry: Instant::now() + delay,
        };

        info!(
            "Reconnection attempt {}/{}, waiting {:?}",
            current_attempt, self.config.max_attempts, delay
        );

        // Wait before attempting
        tokio::time::sleep(delay).await;

        // Attempt reconnection
        let result = if let Some(ref cb) = self.on_reconnect {
            cb()
        } else {
            Err(anyhow::anyhow!("No reconnection handler configured"))
        };

        // Record attempt
        let record = ReconnectAttempt {
            timestamp: chrono::Utc::now(),
            attempt: current_attempt,
            success: result.is_ok(),
            duration: delay,
            error: result.as_ref().err().map(|e| e.to_string()),
        };
        {
            let mut history = self.history.lock().await;
            history.push_back(record);
            while history.len() > 100 {
                history.pop_front();
            }
        }

        match result {
            Ok(_) => {
                // Success! Reset counters
                *attempt = 0;
                *self.current_delay.lock().await =
                    Duration::from_millis(self.config.initial_delay_ms);
                *self.state.write().await = ConnectionState::Connected;

                info!("Successfully reconnected after {} attempts", current_attempt);
                Ok(true)
            }
            Err(e) => {
                // Increase delay for next attempt
                {
                    let mut current_delay = self.current_delay.lock().await;
                    let new_delay = Duration::from_millis(
                        ((*current_delay).as_millis() as f64 * self.config.backoff_multiplier)
                            .min(self.config.max_delay_ms as f64) as u64,
                    );
                    *current_delay = new_delay;
                }

                warn!(
                    "Reconnection attempt {} failed: {}",
                    current_attempt, e
                );

                Err(e)
            }
        }
    }

    /// Mark connection as disconnected
    pub async fn mark_disconnected(&self) {
        *self.state.write().await = ConnectionState::Disconnected;
    }

    /// Check if should attempt reconnection
    pub async fn should_reconnect(&self) -> bool {
        match *self.state.read().await {
            ConnectionState::Disconnected | ConnectionState::Failed { .. } => true,
            _ => false,
        }
    }

    /// Get reconnection history
    pub async fn history(&self) -> Vec<ReconnectAttempt> {
        let history = self.history.lock().await;
        history.iter().cloned().collect()
    }
}

/// Reconnection attempt record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectAttempt {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub attempt: u32,
    pub success: bool,
    pub duration: Duration,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Circuit breaker states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open {
        opened_at: Instant,
    },
    HalfOpen,
}

/// Circuit breaker for preventing cascade failures
pub struct CircuitBreaker {
    name: String,
    state: Arc<RwLock<CircuitState>>,
    failures: Mutex<u32>,
    successes: Mutex<u32>,
    threshold: u32,
    timeout: Duration,
    last_failure: Mutex<Option<Instant>>,
}

impl CircuitBreaker {
    /// Create new circuit breaker
    ///
    /// # Arguments
    /// * `name` - Circuit breaker identifier
    /// * `threshold` - Number of failures before opening circuit
    /// * `timeout` - How long to stay open before trying again
    pub fn new(name: impl Into<String>, threshold: u32, timeout: Duration) -> Self {
        Self {
            name: name.into(),
            state: Arc::new(RwLock::new(CircuitState::Closed)),
            failures: Mutex::new(0),
            successes: Mutex::new(0),
            threshold,
            timeout,
            last_failure: Mutex::new(None),
        }
    }

    /// Execute operation with circuit breaker protection
    pub async fn execute<F, T>(&self, operation: F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        // Check if circuit is open
        {
            let state = self.state.read().await;
            if let CircuitState::Open { opened_at } = *state {
                if opened_at.elapsed() < self.timeout {
                    return Err(anyhow::anyhow!(
                        "Circuit '{}' is open. Rejecting request.",
                        self.name
                    ));
                }
                // Timeout elapsed, transition to half-open
            }
        }

        // Execute operation
        match operation.await {
            Ok(result) => {
                self.record_success().await;
                Ok(result)
            }
            Err(e) => {
                self.record_failure().await;
                Err(e)
            }
        }
    }

    /// Record a successful operation
    async fn record_success(&self) {
        {
            let mut successes = self.successes.lock().await;
            *successes += 1;
        }

        // Reset failure count on success in half-open state
        {
            let mut failures = self.failures.lock().await;
            if *failures > 0 {
                *failures -= 1;
            }
        }

        // If in half-open, close the circuit on success
        {
            let mut state = self.state.write().await;
            if matches!(*state, CircuitState::HalfOpen) {
                *state = CircuitState::Closed;
                info!("Circuit '{}' closed after successful operation", self.name);
            }
        }
    }

    /// Record a failed operation
    async fn record_failure(&self) {
        {
            let mut failures = self.failures.lock().await;
            *failures += 1;
            let current_failures = *failures;

            // Update last failure time
            *self.last_failure.lock().await = Some(Instant::now());

            if current_failures >= self.threshold {
                // Open the circuit
                let mut state = self.state.write().await;
                *state = CircuitState::Open {
                    opened_at: Instant::now(),
                };
                warn!(
                    "Circuit '{}' opened after {} failures",
                    self.name, current_failures
                );
            }
        }

        // Reset success count on failure
        {
            let mut successes = self.successes.lock().await;
            *successes = 0;
        }
    }

    /// Get current circuit state
    pub async fn state(&self) -> CircuitState {
        *self.state.read().await
    }

    /// Get statistics
    pub async fn stats(&self) -> CircuitStats {
        let state = self.state.read().await;
        let failures = *self.failures.lock().await;
        let successes = *self.successes.lock().await;

        CircuitStats {
            name: self.name.clone(),
            state: *state,
            failures,
            successes,
            threshold: self.threshold,
            is_available: !matches!(*state, CircuitState::Open { .. }),
        }
    }

    /// Reset circuit breaker to closed state
    pub async fn reset(&self) {
        *self.state.write().await = CircuitState::Closed;
        *self.failures.lock().await = 0;
        *self.successes.lock().await = 0;
        info!("Circuit '{}' manually reset", self.name);
    }
}

/// Circuit breaker statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitStats {
    pub name: String,
    pub state: CircuitState,
    pub failures: u32,
    pub successes: u32,
    pub threshold: u32,
    pub is_available: bool,
}

impl std::fmt::Display for CircuitStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} [{}]: {} successes, {} failures/{} threshold",
            self.name,
            if self.is_available { "✅ Available" } else { "❌ Open" },
            self.successes,
            self.failures,
            self.threshold
        )
    }
}

/// Data consistency checker
pub struct ConsistencyChecker {
    checks: Vec<Box<dyn ConsistencyCheck + Send + Sync>>,
}

impl ConsistencyChecker {
    pub fn new() -> Self {
        Self { checks: Vec::new() }
    }

    /// Add a consistency check
    pub fn add_check<C>(&mut self, check: C)
    where
        C: ConsistencyCheck + Send + Sync + 'static,
    {
        self.checks.push(Box::new(check));
    }

    /// Run all consistency checks
    pub async fn check_all(&self) -> ConsistencyReport {
        let mut results = Vec::with_capacity(self.checks.len());

        for check in &self.checks {
            let result = check.check().await;
            results.push(result);
        }

        let all_passed = results.iter().all(|r| r.passed);

        ConsistencyReport {
            timestamp: chrono::Utc::now(),
            checks: results,
            passed: all_passed,
        }
    }
}

/// Trait for consistency checks
#[async_trait]
pub trait ConsistencyCheck: Send + Sync {
    fn name(&self) -> &str;
    async fn check(&self) -> CheckResult;
}

/// Result of a single consistency check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
    pub duration: Duration,
}

/// Full consistency report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsistencyReport {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub checks: Vec<CheckResult>,
    pub passed: bool,
}

/// Graceful degradation manager
pub struct DegradationManager {
    levels: Vec<DegradationLevel>,
    current_level: Arc<RwLock<usize>>,
    on_degrade: Option<Arc<dyn Fn(usize, usize) + Send + Sync>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DegradationLevel {
    pub name: String,
    pub priority: usize,
    pub description: String,
    pub features_disabled: Vec<String>,
}

impl DegradationManager {
    pub fn new(levels: Vec<DegradationLevel>) -> Self {
        Self {
            levels,
            current_level: Arc::new(RwLock::new(0)),
            on_degrade: None,
        }
    }

    /// Set degradation callback
    pub fn on_degrade<F>(&mut self, callback: F)
    where
        F: Fn(usize, usize) + Send + Sync + 'static,
    {
        self.on_degrade = Some(Arc::new(callback));
    }

    /// Trigger degradation to specified level
    pub async fn degrade_to(&self, level: usize) -> Result<usize> {
        if level >= self.levels.len() {
            anyhow::bail!("Invalid degradation level {}", level);
        }

        let old_level = *self.current_level.read().await;
        *self.current_level.write().await = level;

        if let Some(ref cb) = self.on_degrade {
            cb(old_level, level);
        }

        info!(
            "Degraded from '{}' to '{}'",
            self.levels[old_level].name,
            self.levels[level].name
        );

        Ok(level)
    }

    /// Auto-degrade based on system health
    pub async fn auto_degrade(&self, health_score: f64) -> Option<usize> {
        // health_score: 0.0 (worst) to 1.0 (best)
        let target_level = if health_score < 0.2 {
            self.levels.len() - 1 // Maximum degradation
        } else if health_score < 0.4 {
            (self.levels.len() as f64 * 0.75).floor() as usize
        } else if health_score < 0.6 {
            (self.levels.len() as f64 * 0.5).floor() as usize
        } else if health_score < 0.8 {
            (self.levels.len() as f64 * 0.25).floor() as usize
        } else {
            0 // No degradation
        };

        let current = *self.current_level.read().await;
        if target_level > current {
            Some(self.degrade_to(target_level).await.ok()?)
        } else {
            None
        }
    }

    /// Get current degradation level
    pub async fn current_level(&self) -> &DegradationLevel {
        let idx = *self.current_level.read().await;
        &self.levels[idx]
    }

    /// Check if feature is available at current level
    pub async fn is_feature_available(&self, feature: &str) -> bool {
        let idx = *self.current_level.read().await;
        for i in 0..=idx {
            if self.levels[i].features_disabled.contains(&feature.to_string()) {
                return false;
            }
        }
        true
    }
}
