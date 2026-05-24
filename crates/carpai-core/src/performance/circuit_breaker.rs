use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: usize,
    pub success_threshold: usize,
    pub timeout_duration: Duration,
    pub sliding_window_size: usize,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            success_threshold: 3,
            timeout_duration: Duration::from_secs(30),
            sliding_window_size: 10,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    state: Arc<RwLock<CircuitState>>,
    config: CircuitBreakerConfig,
    failure_count: Arc<RwLock<usize>>,
    success_count: Arc<RwLock<usize>>,
    last_failure_time: Arc<RwLock<Option<Instant>>>,
    last_state_change: Arc<RwLock<Instant>>,
    recent_results: Arc<RwLock<VecDeque<bool>>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        let window_size = config.sliding_window_size;
        Self {
            state: Arc::new(RwLock::new(CircuitState::Closed)),
            config,
            failure_count: Arc::new(RwLock::new(0)),
            success_count: Arc::new(RwLock::new(0)),
            last_failure_time: Arc::new(RwLock::new(None)),
            last_state_change: Arc::new(RwLock::new(Instant::now())),
            recent_results: Arc::new(RwLock::new(VecDeque::with_capacity(window_size))),
        }
    }

    pub fn default() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }

    pub async fn call<T, F>(&self, operation: F) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
    {
        let current_state = self.get_state().await;

        match current_state {
            CircuitState::Open => {
                if self.should_allow_test().await {
                    self.set_state(CircuitState::HalfOpen).await;
                    return self.attempt_operation(operation).await;
                }
                Err(anyhow!("Circuit is open - refusing to execute"))
            }
            CircuitState::HalfOpen => self.attempt_operation(operation).await,
            CircuitState::Closed => self.attempt_operation(operation).await,
        }
    }

    async fn attempt_operation<T, F>(&self, operation: F) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
    {
        let result = operation();

        match &result {
            Ok(_) => self.on_success().await,
            Err(_) => self.on_failure().await,
        }

        result
    }

    async fn on_success(&self) {
        let mut state = self.state.write().await;
        let mut success_count = self.success_count.write().await;
        let mut recent_results = self.recent_results.write().await;
        let mut last_state_change = self.last_state_change.write().await;
        let mut failure_count = self.failure_count.write().await;

        recent_results.push_back(true);
        if recent_results.len() > self.config.sliding_window_size {
            recent_results.pop_front();
        }

        match *state {
            CircuitState::Open => {
                *success_count += 1;
                if *success_count >= self.config.success_threshold {
                    *state = CircuitState::Closed;
                    *success_count = 0;
                    *last_state_change = Instant::now();
                }
            }
            CircuitState::HalfOpen => {
                *success_count += 1;
                if *success_count >= self.config.success_threshold {
                    *state = CircuitState::Closed;
                    *success_count = 0;
                    *last_state_change = Instant::now();
                }
            }
            CircuitState::Closed => {
                *success_count = 0;
                *failure_count = 0;
            }
        }
    }

    async fn on_failure(&self) {
        let mut state = self.state.write().await;
        let mut failure_count = self.failure_count.write().await;
        let mut recent_results = self.recent_results.write().await;
        let mut last_failure_time = self.last_failure_time.write().await;
        let mut last_state_change = self.last_state_change.write().await;

        recent_results.push_back(false);
        if recent_results.len() > self.config.sliding_window_size {
            recent_results.pop_front();
        }

        *last_failure_time = Some(Instant::now());

        match *state {
            CircuitState::Closed => {
                *failure_count += 1;
                
                let failure_rate = self.calculate_failure_rate().await;
                if *failure_count >= self.config.failure_threshold || failure_rate > 0.5 {
                    *state = CircuitState::Open;
                    *failure_count = 0;
                    *last_state_change = Instant::now();
                }
            }
            CircuitState::HalfOpen => {
                *state = CircuitState::Open;
                *failure_count = 0;
                *last_state_change = Instant::now();
            }
            CircuitState::Open => {}
        }
    }

    async fn calculate_failure_rate(&self) -> f64 {
        let recent_results = self.recent_results.read().await;
        if recent_results.is_empty() {
            return 0.0;
        }

        let failures = recent_results.iter().filter(|&&r| !r).count() as f64;
        failures / recent_results.len() as f64
    }

    async fn should_allow_test(&self) -> bool {
        let last_change = *self.last_state_change.read().await;
        Instant::now() - last_change >= self.config.timeout_duration
    }

    async fn get_state(&self) -> CircuitState {
        let mut state = self.state.write().await;
        let mut last_state_change = self.last_state_change.write().await;

        if *state == CircuitState::Open
            && self.should_allow_test().await {
            *state = CircuitState::HalfOpen;
            *last_state_change = Instant::now();
        }

        state.clone()
    }

    pub async fn set_state(&self, new_state: CircuitState) {
        let mut state = self.state.write().await;
        let mut last_state_change = self.last_state_change.write().await;
        *state = new_state;
        *last_state_change = Instant::now();
    }

    pub async fn reset(&self) {
        *self.state.write().await = CircuitState::Closed;
        *self.failure_count.write().await = 0;
        *self.success_count.write().await = 0;
        *self.last_failure_time.write().await = None;
        *self.last_state_change.write().await = Instant::now();
        self.recent_results.write().await.clear();
    }

    pub async fn get_metrics(&self) -> CircuitBreakerMetrics {
        CircuitBreakerMetrics {
            state: self.get_state().await,
            failure_count: *self.failure_count.read().await,
            success_count: *self.success_count.read().await,
            last_failure_time: *self.last_failure_time.read().await,
            last_state_change: *self.last_state_change.read().await,
            recent_results: self.recent_results.read().await.clone(),
            config: self.config.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerMetrics {
    pub state: CircuitState,
    pub failure_count: usize,
    pub success_count: usize,
    pub last_failure_time: Option<Instant>,
    pub last_state_change: Instant,
    pub recent_results: VecDeque<bool>,
    pub config: CircuitBreakerConfig,
}

#[derive(Debug, Clone)]
pub struct CircuitBreakerManager {
    breakers: Arc<RwLock<HashMap<String, Arc<CircuitBreaker>>>>,
    default_config: CircuitBreakerConfig,
}

impl CircuitBreakerManager {
    pub fn new(default_config: CircuitBreakerConfig) -> Self {
        Self {
            breakers: Arc::new(RwLock::new(HashMap::new())),
            default_config,
        }
    }

    pub fn default() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }

    pub async fn get_or_create(&self, name: &str) -> Arc<CircuitBreaker> {
        let mut breakers = self.breakers.write().await;

        if let Some(breaker) = breakers.get(name) {
            return breaker.clone();
        }

        let breaker = Arc::new(CircuitBreaker::new(self.default_config.clone()));
        breakers.insert(name.to_string(), breaker.clone());
        breaker
    }

    pub async fn get(&self, name: &str) -> Option<Arc<CircuitBreaker>> {
        self.breakers.read().await.get(name).cloned()
    }

    pub async fn remove(&self, name: &str) -> Option<Arc<CircuitBreaker>> {
        self.breakers.write().await.remove(name)
    }

    pub async fn reset_all(&self) {
        let breakers = self.breakers.read().await;
        for breaker in breakers.values() {
            breaker.reset().await;
        }
    }

    pub async fn get_all_metrics(&self) -> HashMap<String, CircuitBreakerMetrics> {
        let breakers = self.breakers.read().await;
        let mut metrics = HashMap::new();

        for (name, breaker) in breakers.iter() {
            metrics.insert(name.clone(), breaker.get_metrics().await);
        }

        metrics
    }
}
