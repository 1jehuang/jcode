use std::time::{Duration, Instant};

use std::collections::VecDeque;
use super::session::{SshSession, SshConfig};

/// Reconnection Strategy Types
#[derive(Debug, Clone)]
pub enum ReconnectStrategy {
    /// Fixed interval between retries
    FixedInterval(Duration),
    /// Exponential backoff with optional jitter
    ExponentialBackoff {
        initial_delay: Duration,
        max_delay: Duration,
        multiplier: f64,
        jitter: bool,
    },
    /// Linear increase in delay
    LinearBackoff {
        initial_delay: Duration,
        increment: Duration,
        max_delay: Duration,
    },
    /// Custom strategy with callback
    Custom(Box<dyn Fn(u32) -> Duration + Send + Sync>),
}

impl Default for ReconnectStrategy {
    fn default() -> Self {
        ReconnectStrategy::ExponentialBackoff {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
            jitter: true,
        }
    }
}

/// Connection Error Classification
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorClassification {
    /// Transient error - retry might succeed
    Transient(String),
    /// Permanent error - don't retry
    Permanent(String),
    /// Timeout - may retry
    Timeout(Duration),
    /// Authentication failure - don't retry without new credentials
    Authentication(String),
    /// Network unreachable - retry after delay
    NetworkUnreachable(String),
    /// Resource exhausted - retry after cooldown
    ResourceExhausted(String),
}

/// Circuit Breaker States
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitState {
    Closed,      // Normal operation
    Open,        // Failing fast
    HalfOpen,    // Testing if recovered
}

/// Circuit Breaker Configuration
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,      // Failures before opening
    pub success_threshold: u32,      // Successes to close again
    pub timeout: Duration,           // Time in open state before trying
    pub half_open_max_calls: u32,    // Max calls in half-open state
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        CircuitBreakerConfig {
            failure_threshold: 5,
            success_threshold: 3,
            timeout: Duration::from_secs(30),
            half_open_max_calls: 1,
        }
    }
}

/// Circuit Breaker for preventing cascade failures
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: CircuitState,
    failure_count: u32,
    success_count: u32,
    last_failure_time: Option<Instant>,
    total_failures: u32,
    total_successes: u32,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        CircuitBreaker {
            config,
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            last_failure_time: None,
            total_failures: 0,
            total_successes: 0,
        }
    }

    /// Check if operation should be allowed
    pub fn allow_request(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(last_failure) = self.last_failure_time {
                    if last_failure.elapsed() > self.config.timeout {
                        self.state = CircuitState::HalfOpen;
                        self.success_count = 0;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => {
                self.success_count < self.config.half_open_max_calls
            }
        }
    }

    /// Record successful operation
    pub fn record_success(&mut self) {
        self.total_successes += 1;

        match self.state {
            CircuitState::Closed => {
                self.failure_count = 0;
            }
            CircuitState::HalfOpen => {
                self.success_count += 1;
                if self.success_count >= self.config.success_threshold {
                    self.state = CircuitState::Closed;
                    self.failure_count = 0;
                }
            }
            _ => {}
        }
    }

    /// Record failed operation
    pub fn record_failure(&mut self) {
        self.total_failures += 1;
        self.failure_count += 1;
        self.last_failure_time = Some(Instant::now());

        match self.state {
            CircuitState::Closed => {
                if self.failure_count >= self.config.failure_threshold {
                    self.state = CircuitState::Open;
                }
            }
            CircuitState::HalfOpen => {
                self.state = CircuitState::Open; // Immediately back to open
            }
            _ => {}
        }
    }

    /// Get current state
    pub fn state(&self) -> &CircuitState { &self.state }

    /// Get statistics
    pub fn stats(&self) -> CircuitBreakerStats {
        CircuitBreakerStats {
            state: self.state.clone(),
            failure_count: self.failure_count,
            success_count: self.success_count,
            total_failures: self.total_failures,
            total_successes: self.total_successes,
        }
    }

    /// Reset circuit breaker to initial state
    pub fn reset(&mut self) {
        self.state = CircuitState::Closed;
        self.failure_count = 0;
        self.success_count = 0;
        self.last_failure_time = None;
    }
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerStats {
    pub state: CircuitState,
    pub failure_count: u32,
    pub success_count: u32,
    pub total_failures: u32,
    pub total_successes: u32,
}

/// Retry Policy Configuration
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub strategy: ReconnectStrategy,
    pub retryable_errors: Vec<ErrorClassification>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_attempts: 3,
            strategy: ReconnectStrategy::default(),
            retryable_errors: vec![
                ErrorClassification::Transient("Connection reset".to_string()),
                ErrorClassification::Timeout(Duration::from_secs(30)),
                ErrorClassification::NetworkUnreachable("Host unreachable".to_string()),
            ],
        }
    }
}

/// Smart Retry Handler with intelligent backoff and classification
pub struct SmartRetryHandler {
    policy: RetryPolicy,
    attempt_history: VecDeque<RetryAttempt>,
    circuit_breaker: CircuitBreaker,
}

#[derive(Debug, Clone)]
pub struct RetryAttempt {
    pub attempt_number: u32,
    pub timestamp: Instant,
    pub error: String,
    pub delay_before: Duration,
    pub classification: ErrorClassification,
}

impl SmartRetryHandler {
    pub fn new(policy: RetryPolicy) -> Self {
        SmartRetryHandler {
            policy,
            attempt_history: VecDeque::new(),
            circuit_breaker: CircuitBreaker::new(CircuitBreakerConfig::default()),
        }
    }

    /// Calculate next retry delay based on strategy
    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        match &self.policy.strategy {
            ReconnectStrategy::FixedInterval(delay) => *delay,
            ReconnectStrategy::ExponentialBackoff { initial_delay, max_delay, multiplier, jitter } => {
                let mut delay = initial_delay.as_secs_f64() * multiplier.powi(attempt as i32);
                
                if *jitter {
                    use std::time::SystemTime;
                    let nanos = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos();
                    let random_factor = (nanos % 1000) as f64 / 1000.0; // 0.0-1.0
                    delay *= (0.5 + random_factor); // ±50% jitter
                }

                let delay_secs = delay.min(max_delay.as_secs_f64());
                Duration::from_secs_f64(delay_secs)
            }
            ReconnectStrategy::LinearBackoff { initial_delay, increment, max_delay } => {
                let delay = *initial_delay + *increment * attempt;
                delay.min(*max_delay)
            }
            ReconnectStrategy::Custom(callback) => callback(attempt),
        }
    }

    /// Classify error type for retry decision
    pub fn classify_error(&self, error: &str) -> ErrorClassification {
        let error_lower = error.to_lowercase();

        if error_lower.contains("connection refused") || 
           error_lower.contains("connection reset") ||
           error_lower.contains("broken pipe") {
            ErrorClassification::Transient(error.to_string())
        } else if error_lower.contains("timeout") || 
                  error_lower.contains("timed out") {
            ErrorClassification::Timeout(Duration::from_secs(30))
        } else if error_lower.contains("permission denied") ||
                  error_lower.contains("authentication failed") ||
                  error_lower.contains("publickey") {
            ErrorClassification::Authentication(error.to_string())
        } else if error_lower.contains("no route to host") ||
                  error_lower.contains("network is unreachable") ||
                  error_lower.contains("host unreachable") {
            ErrorClassification::NetworkUnreachable(error.to_string())
        } else if error_lower.contains("too many connections") ||
                  error_lower.contains("resource temporarily unavailable") {
            ErrorClassification::ResourceExhausted(error.to_string())
        } else {
            ErrorClassification::Permanent(error.to_string())
        }
    }

    /// Check if error is retryable
    pub fn is_retryable(&self, classification: &ErrorClassification) -> bool {
        match classification {
            ErrorClassification::Transient(_) => true,
            ErrorClassification::Timeout(_) => true,
            ErrorClassification::NetworkUnreachable(_) => true,
            ErrorClassification::ResourceExhausted(_) => true,
            ErrorClassification::Permanent(_) => false,
            ErrorClassification::Authentication(_) => false,
        }
    }

    /// Record a retry attempt
    pub fn record_attempt(&mut self, attempt_num: u32, error: &str) {
        let classification = self.classify_error(error);
        let delay = self.calculate_delay(attempt_num);

        self.attempt_history.push_back(RetryAttempt {
            attempt_number: attempt_num,
            timestamp: Instant::now(),
            error: error.to_string(),
            delay_before: delay,
            classification: classification.clone(),
        });

        // Keep only recent history
        while self.attempt_history.len() > 100 {
            self.attempt_history.pop_front();
        }

        if !self.is_retryable(&classification) {
            self.circuit_breaker.record_failure();
        }
    }

    /// Should we continue retrying?
    pub fn should_retry(&mut self, attempt: u32) -> Result<Duration, String> {
        if attempt >= self.policy.max_attempts {
            return Err(format!("Max attempts ({}) reached", self.policy.max_attempts));
        }

        if !self.circuit_breaker.allow_request() {
            return Err("Circuit breaker is open".to_string());
        }

        let delay = self.calculate_delay(attempt);
        Ok(delay)
    }

    /// Get retry statistics
    pub fn stats(&self) -> RetryStats {
        let total_attempts = self.attempt_history.len() as u32;
        let transient_errors = self.attempt_history.iter()
            .filter(|a| matches!(a.classification, ErrorClassification::Transient(_)))
            .count() as u32;
        let permanent_errors = self.attempt_history.iter()
            .filter(|a| matches!(a.classification, ErrorClassification::Permanent(_)))
            .count() as u32;

        RetryStats {
            total_attempts,
            transient_errors,
            permanent_errors,
            circuit_state: self.circuit_breaker.state().clone(),
        }
    }

    /// Clear history
    pub fn clear_history(&mut self) {
        self.attempt_history.clear();
        self.circuit_breaker.reset();
    }
}

#[derive(Debug, Clone)]
pub struct RetryStats {
    pub total_attempts: u32,
    pub transient_errors: u32,
    pub permanent_errors: u32,
    pub circuit_state: CircuitState,
}

/// Enhanced SSH Session with automatic reconnection
pub struct ResilientSshSession {
    session: SshSession,
    retry_handler: SmartRetryHandler,
    auto_reconnect: bool,
    max_reconnect_attempts: u32,
    health_check_interval: Duration,
    last_health_check: Option<Instant>,
    consecutive_failures: u32,
}

impl ResilientSshSession {
    pub fn new(config: SshConfig, retry_policy: Option<RetryPolicy>) -> Self {
        ResilientSshSession {
            session: SshSession::new(config),
            retry_handler: SmartRetryHandler::new(retry_policy.unwrap_or_default()),
            auto_reconnect: true,
            max_reconnect_attempts: 10,
            health_check_interval: Duration::from_secs(60),
            last_health_check: None,
            consecutive_failures: 0,
        }
    }

    /// Connect with automatic retry
    pub fn connect_with_retry(&mut self) -> Result<String, String> {
        let mut attempt = 0;
        
        loop {
            match self.session.connect() {
                Ok(msg) => {
                    self.retry_handler.circuit_breaker.record_success();
                    self.consecutive_failures = 0;
                    return Ok(msg);
                }
                Err(e) => {
                    attempt += 1;
                    self.consecutive_failures += 1;
                    
                    let classification = self.retry_handler.classify_error(&e);
                    self.retry_handler.record_attempt(attempt, &e);

                    if !self.retry_handler.is_retryable(&classification) {
                        return Err(format!("Non-retryable error: {} ({:?})", e, classification));
                    }

                    if attempt > self.max_reconnect_attempts {
                        return Err(format!("Max reconnect attempts ({}) exceeded. Last error: {}", 
                            self.max_reconnect_attempts, e));
                    }

                    let delay = self.retry_handler.calculate_delay(attempt);
                    eprintln!("[RETRY] Attempt {}/{} failed: {}. Retrying in {:?}...", 
                        attempt, self.max_reconnect_attempts, e, delay);

                    std::thread::sleep(delay);
                }
            }
        }
    }

    /// Execute command with automatic reconnection on failure
    pub fn execute_resilient(&mut self, command: &str) -> Result<super::session::SshOutput, String> {
        if self.auto_reconnect && !self.session.is_alive() {
            eprintln!("[RESILIENT] Session lost, attempting reconnection...");
            self.connect_with_retry()?;
        }

        match self.session.execute(command) {
            Ok(output) => {
                self.retry_handler.circuit_breaker.record_success();
                self.consecutive_failures = 0;
                Ok(output)
            }
            Err(e) => {
                self.consecutive_failures += 1;
                self.retry_handler.circuit_breaker.record_failure();

                if self.auto_reconnect && self.should_attempt_reconnect() {
                    eprintln!("[RESILIENT] Command failed, reconnecting...");
                    self.connect_with_retry()?;
                    return self.session.execute(command);
                }

                Err(e)
            }
        }
    }

    /// Health check
    pub fn health_check(&mut self) -> HealthStatus {
        let now = Instant::now();

        if let Some(last) = self.last_health_check {
            if now.duration_since(last) < self.health_check_interval {
                return HealthStatus::Skipped;
            }
        }

        self.last_health_check = Some(now);

        if self.session.is_alive() {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unhealthy {
                uptime: self.session.uptime(),
                consecutive_failures: self.consecutive_failures,
            }
        }
    }

    fn should_attempt_reconnect(&self) -> bool {
        self.consecutive_failures < 3 && self.auto_reconnect
    }

    /// Get session reference
    pub fn session(&self) -> &SshSession { &self.session }

    /// Get mutable session reference
    pub fn session_mut(&mut self) -> &mut SshSession { &mut self.session }

    /// Get retry stats
    pub fn retry_stats(&self) -> RetryStats { self.retry_handler.stats() }

    /// Get resilience metrics
    pub fn resilience_metrics(&self) -> ResilienceMetrics {
        ResilienceMetrics {
            consecutive_failures: self.consecutive_failures,
            auto_reconnect_enabled: self.auto_reconnect,
            health_check_interval: self.health_check_interval,
            last_health_check: self.last_health_check,
            session_stats: self.session.stats().clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum HealthStatus {
    Healthy,
    Unhealthy {
        uptime: Option<Duration>,
        consecutive_failures: u32,
    },
    Skipped,
}

#[derive(Debug, Clone)]
pub struct ResilienceMetrics {
    pub consecutive_failures: u32,
    pub auto_reconnect_enabled: bool,
    pub health_check_interval: Duration,
    pub last_health_check: Option<Instant>,
    pub session_stats: super::session::SessionStats,
}

/// Connection Pool with built-in resilience
pub struct ResilientConnectionPool {
    sessions: std::collections::HashMap<String, ResilientSshSession>,
    default_policy: RetryPolicy,
    max_sessions: usize,
}

impl ResilientConnectionPool {
    pub fn new(max_sessions: usize, default_policy: RetryPolicy) -> Self {
        ResilientConnectionPool {
            sessions: std::collections::HashMap::new(),
            default_policy,
            max_sessions,
        }
    }

    /// Get or create resilient session
    pub fn get_or_create_session(&mut self, key: &str, config: SshConfig) -> Result<&mut ResilientSshSession, String> {
        if !self.sessions.contains_key(key) {
            if self.sessions.len() >= self.max_sessions {
                return Err("Pool capacity reached".to_string());
            }

            let mut session = ResilientSshSession::new(config, Some(self.default_policy.clone()));
            session.connect_with_retry()?;
            self.sessions.insert(key.to_string(), session);
        }

        self.sessions.get_mut(key).ok_or("Session not found".to_string())
    }

    /// Execute command on resilient session
    pub fn execute_on(&mut self, key: &str, command: &str) -> Result<super::session::SshOutput, String> {
        let session = self.sessions.get_mut(key).ok_or("Session not found".to_string())?;
        session.execute_resilient(command)
    }

    /// Health check all sessions
    pub fn health_check_all(&mut self) -> Vec<(String, HealthStatus)> {
        self.sessions.iter_mut()
            .map(|(k, s)| (k.clone(), s.health_check()))
            .collect()
    }

    /// Remove dead sessions
    pub fn evict_dead_sessions(&mut self) -> Vec<String> {
        let keys: Vec<String> = self.sessions.keys().cloned().collect();
        let dead_keys: Vec<String> = keys.into_iter()
            .filter(|k| {
                if let Some(s) = self.sessions.get_mut(k) {
                    !s.session_mut().is_alive()
                } else {
                    false
                }
            })
            .collect();

        for key in &dead_keys {
            self.sessions.remove(key);
        }

        dead_keys
    }

    /// Get pool statistics
    pub fn stats(&self) -> ResilientPoolStats {
        let healthy = self.sessions.values().filter(|s| s.session().is_connected()).count();
        let unhealthy = self.sessions.len() - healthy;

        ResilientPoolStats {
            total_sessions: self.sessions.len(),
            healthy_sessions: healthy,
            unhealthy_sessions: unhealthy,
            max_capacity: self.max_sessions,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResilientPoolStats {
    pub total_sessions: usize,
    pub healthy_sessions: usize,
    pub unhealthy_sessions: usize,
    pub max_capacity: usize,
}
