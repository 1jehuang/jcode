use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use std::thread;
use super::session::{SshSession, SshConfig};

/// Connection Pool Configuration
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_connections: usize,
    pub min_idle_connections: usize,
    pub max_idle_time: Duration,
    pub connection_timeout: Duration,
    pub health_check_interval: Duration,
    pub enable_auto_reconnect: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        PoolConfig {
            max_connections: 10,
            min_idle_connections: 2,
            max_idle_time: Duration::from_secs(300), // 5 minutes
            connection_timeout: Duration::from_secs(30),
            health_check_interval: Duration::from_secs(60),
            enable_auto_reconnect: true,
        }
    }
}

/// Pooled Session with metadata
struct PooledSession {
    session: SshSession,
    created_at: Instant,
    last_used_at: Instant,
    in_use: bool,
    use_count: u64,
}

impl PooledSession {
    /// Seconds since session was created
    fn age_secs(&self) -> f64 {
        self.created_at.elapsed().as_secs_f64()
    }
}

/// Enhanced SSH Connection Pool
pub struct SshConnectionPool {
    config: PoolConfig,
    sessions: Arc<RwLock<HashMap<String, PooledSession>>>,
    default_config: Option<SshConfig>,
    stats: Arc<Mutex<PoolStats>>,
    shutdown_flag: Arc<Mutex<bool>>,
}

#[derive(Debug, Default, Clone)]
pub struct PoolStats {
    pub total_created: u64,
    pub total_destroyed: u64,
    pub active_count: usize,
    pub idle_count: usize,
    pub wait_count: u64,
    pub checkout_failures: u64,
    pub oldest_session_age_secs: f64,
    pub youngest_session_age_secs: f64,
}

impl SshConnectionPool {
    /// Create new connection pool with default configuration
    pub fn new(max_connections: usize) -> Self {
        Self::with_config(PoolConfig {
            max_connections,
            ..Default::default()
        })
    }

    /// Create connection pool with custom configuration
    pub fn with_config(config: PoolConfig) -> Self {
        let pool = SshConnectionPool {
            config: config.clone(),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            default_config: None,
            stats: Arc::new(Mutex::new(PoolStats::default())),
            shutdown_flag: Arc::new(Mutex::new(false)),
        };

        // Start background maintenance task if auto-reconnect is enabled
        if config.enable_auto_reconnect {
            let sessions = pool.sessions.clone();
            let stats = pool.stats.clone();
            let shutdown = pool.shutdown_flag.clone();
            
            thread::spawn(move || {
                Self::_maintenance_loop(sessions, stats, shutdown, config);
            });
        }

        pool
    }

    /// Set default SSH configuration for new connections
    pub fn with_default_config(mut self, config: SshConfig) -> Self {
        self.default_config = Some(config);
        self
    }

    /// Get or create a session for the given host (key)
    pub fn get_session(&self, host_key: &str) -> Result<String, String> {
        let key = host_key.to_lowercase();

        // Try to find an existing idle session
        {
            let mut sessions = self.sessions.write().map_err(|e| e.to_string())?;
            if let Some(pooled) = sessions.get_mut(&key) {
                if !pooled.in_use && pooled.session.is_alive() {
                    drop(sessions);
                    return self._checkout_session(&key);
                }
            }
        }

        // Need to create new session
        self._create_and_checkout(&key)
    }

    /// Return session to pool after use
    pub fn return_session(&self, session_id: &str) -> Result<(), String> {
        let mut sessions = self.sessions.write().map_err(|e| e.to_string())?;

        // Find session by ID and mark as not in use
        for (_, pooled) in sessions.iter_mut() {
            if pooled.session.id() == session_id {
                pooled.in_use = false;
                pooled.last_used_at = Instant::now();
                
                // Update stats
                if let Ok(mut stats) = self.stats.lock() {
                    stats.active_count = stats.active_count.saturating_sub(1);
                    stats.idle_count += 1;
                }

                return Ok(());
            }
        }

        Err(format!("Session '{}' not found in pool", session_id))
    }

    /// Execute command on a specific host (auto-manages session lifecycle)
    pub fn execute_on<F, T>(&self, host: &str, operation: F) -> Result<T, String>
    where
        F: FnOnce(&mut SshSession) -> Result<T, String>,
    {
        let session_id = self.get_session(host)?;
        
        // Get mutable reference to session
        let result = {
            let mut sessions = self.sessions.write().map_err(|e| e.to_string())?;
            
            // Find session by ID
            let pooled = sessions.values_mut()
                .find(|p| p.session.id() == session_id)
                .ok_or("Session not found")?;

            operation(&mut pooled.session)
        };

        // Always return session to pool
        let _ = self.return_session(&session_id);

        result
    }

    /// Parallel execution across multiple hosts
    pub fn parallel_execute<F, T>(&self, hosts: Vec<&str>, operation: F) -> Vec<(String, Result<T, String>)>
    where
        F: Fn(&mut SshSession) -> Result<T, String> + Send + Sync + Clone,
        F: 'static,
        T: Send + 'static,
    {
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel();
        let pool_sessions = self.sessions.clone();
        let pool_stats = self.stats.clone();
        let pool_config = self.config.clone();
        let default_cfg = self.default_config.clone();

        // Spawn threads for parallel execution
        for host in hosts {
            let tx = tx.clone();
            let op = operation.clone();
            let sessions = pool_sessions.clone();
            let stats = pool_stats.clone();
            let cfg = pool_config.clone();
            let default = default_cfg.clone();
            let host_str = host.to_string();

            thread::spawn(move || {
                let key = host_str.to_lowercase();
                
                // Get or create session
                let result = {
                    let mut sess_map = match sessions.write() {
                        Ok(s) => s,
                        Err(e) => {
                            let _ = tx.send((host_str, Err(e.to_string())));
                            return;
                        }
                    };

                    // Find or create session
                    let pooled = if let Some(p) = sess_map.get_mut(&key) {
                        if p.session.is_alive() && !p.in_use {
                            p
                        } else {
                            // Need to create new session
                            if sess_map.len() >= cfg.max_connections {
                                let _ = tx.send((host_str, Err("Pool exhausted".to_string())));
                                return;
                            }

                            let ssh_config = default.unwrap_or_else(|| SshConfig::with_host(&key));
                            let mut session = SshSession::new(ssh_config);
                            
                            match session.connect() {
                                Ok(_) => {}
                                Err(e) => {
                                    let _ = tx.send((host_str, Err(format!("Connection failed: {}", e))));
                                    return;
                                }
                            }

                            sess_map.entry(key.clone()).or_insert_with(|| PooledSession {
                                session,
                                created_at: Instant::now(),
                                last_used_at: Instant::now(),
                                in_use: false,
                                use_count: 0,
                            })
                        }
                    } else {
                        // Need to create new session
                        if sess_map.len() >= cfg.max_connections {
                            let _ = tx.send((host_str, Err("Pool exhausted".to_string())));
                            return;
                        }

                        let ssh_config = default.unwrap_or_else(|| SshConfig::with_host(&key));
                        let mut session = SshSession::new(ssh_config);
                        
                        match session.connect() {
                            Ok(_) => {}
                            Err(e) => {
                                let _ = tx.send((host_str, Err(format!("Connection failed: {}", e))));
                                return;
                            }
                        }

                        sess_map.entry(key.clone()).or_insert_with(|| PooledSession {
                            session,
                            created_at: Instant::now(),
                            last_used_at: Instant::now(),
                            in_use: false,
                            use_count: 0,
                        })
                    };

                    pooled.in_use = true;
                    pooled.use_count += 1;
                    pooled.last_used_at = Instant::now();

                    op(&mut pooled.session)
                };

                // Update stats and return session
                if let Ok(mut s) = stats.lock() {
                    s.active_count = s.active_count.saturating_sub(1);
                    s.idle_count += 1;
                }

                // Mark as available again
                let key_for_cleanup = key.clone();
                if let Ok(mut sess_map) = sessions.write() {
                    if let Some(pooled) = sess_map.get_mut(&key_for_cleanup) {
                        pooled.in_use = false;
                    }
                }

                let _ = tx.send((host_str, result));
            });
        }

        // Collect results
        drop(tx); // Drop sender so receiver knows when all threads are done
        
        rx.into_iter().collect()
    }

    /// Close all connections and shut down pool
    pub fn shutdown(&self) -> Result<usize, String> {
        *self.shutdown_flag.lock().unwrap_or_else(|e| e.into_inner()) = true;

        let mut sessions = self.sessions.write().map_err(|e| e.to_string())?;
        let count = sessions.len();

        for (_, mut pooled) in sessions.drain() {
            let _ = pooled.session.disconnect();
        }

        if let Ok(mut stats) = self.stats.lock() {
            stats.active_count = 0;
            stats.idle_count = 0;
        }

        Ok(count)
    }

    /// Get pool statistics
    pub fn stats(&self) -> PoolStats {
        let mut base = self.stats.lock().map(|s| s.clone()).unwrap_or_default();
        // Populate session age info from actual sessions
        if let Ok(sessions) = self.sessions.read() {
            let mut oldest = 0.0f64;
            let mut youngest = f64::MAX;
            for pooled in sessions.values() {
                let age = pooled.age_secs();
                if age > oldest { oldest = age; }
                if age < youngest { youngest = age; }
            }
            base.oldest_session_age_secs = oldest;
            base.youngest_session_age_secs = if youngest == f64::MAX { 0.0 } else { youngest };
        }
        base
    }

    /// Get number of active connections
    pub fn active_count(&self) -> usize {
        self.sessions.read()
            .map(|s| s.values().filter(|p| p.in_use).count())
            .unwrap_or(0)
    }

    /// Get number of idle connections
    pub fn idle_count(&self) -> usize {
        self.sessions.read()
            .map(|s| s.values().filter(|p| !p.in_use).count())
            .unwrap_or(0)
    }

    /// Evict idle sessions that have exceeded max idle time
    pub fn evict_idle(&self) -> Result<usize, String> {
        let now = Instant::now();
        let max_session_age = self.config.max_idle_time * 2; // Sessions older than 2x idle time are evicted regardless
        let mut to_remove = vec![];

        {
            let sessions = self.sessions.read().map_err(|e| e.to_string())?;
            for (key, pooled) in sessions.iter() {
                let idle_timeout = !pooled.in_use && (now - pooled.last_used_at) > self.config.max_idle_time;
                let age_timeout = !pooled.in_use && (now - pooled.created_at) > max_session_age;
                if idle_timeout || age_timeout {
                    to_remove.push(key.clone());
                }
            }
        }

        if to_remove.is_empty() {
            return Ok(0);
        }

        let mut sessions = self.sessions.write().map_err(|e| e.to_string())?;
        let count = to_remove.len();

        for key in to_remove {
            if let Some(mut pooled) = sessions.remove(&key) {
                let _ = pooled.session.disconnect();
                
                if let Ok(mut stats) = self.stats.lock() {
                    stats.total_destroyed += 1;
                    stats.idle_count = stats.idle_count.saturating_sub(1);
                }
            }
        }

        Ok(count)
    }

    // Private helper methods

    fn _checkout_session(&self, key: &str) -> Result<String, String> {
        let mut sessions = self.sessions.write().map_err(|e| e.to_string())?;

        if let Some(pooled) = sessions.get_mut(key) {
            pooled.in_use = true;
            pooled.use_count += 1;
            pooled.last_used_at = Instant::now();

            let id = pooled.session.id().to_string();

            if let Ok(mut stats) = self.stats.lock() {
                stats.active_count += 1;
                stats.idle_count = stats.idle_count.saturating_sub(1);
            }

            return Ok(id);
        }

        Err("Failed to checkout session".to_string())
    }

    fn _create_and_checkout(&self, key: &str) -> Result<String, String> {
        // Check pool capacity
        {
            let sessions = self.sessions.read().map_err(|e| e.to_string())?;
            if sessions.len() >= self.config.max_connections {
                if let Ok(mut stats) = self.stats.lock() {
                    stats.checkout_failures += 1;
                }
                return Err("Connection pool exhausted. Max connections reached.".to_string());
            }
        }

        // Create new session
        let config = self.default_config.clone()
            .unwrap_or_else(|| SshConfig::with_host(key));

        let mut session = SshSession::new(config);
        session.connect()?;

        let id = session.id().to_string();

        // Add to pool
        {
            let mut sessions = self.sessions.write().map_err(|e| e.to_string())?;
            sessions.insert(key.to_string(), PooledSession {
                session,
                created_at: Instant::now(),
                last_used_at: Instant::now(),
                in_use: true,
                use_count: 1,
            });

            if let Ok(mut stats) = self.stats.lock() {
                stats.total_created += 1;
                stats.active_count += 1;
            }
        }

        Ok(id)
    }

    fn _maintenance_loop(
        sessions: Arc<RwLock<HashMap<String, PooledSession>>>,
        stats: Arc<Mutex<PoolStats>>,
        shutdown: Arc<Mutex<bool>>,
        config: PoolConfig,
    ) {
        loop {
            // Check shutdown flag
            if *shutdown.lock().unwrap_or_else(|e| e.into_inner()) {
                break;
            }

            thread::sleep(config.health_check_interval);

            // Health check and cleanup
            let now = Instant::now();
            let mut dead_sessions = vec![];
            let mut idle_to_evict = vec![];

            {
                if let Ok(mut sess_map) = sessions.write() {
                    for (key, pooled) in sess_map.iter_mut() {
                        // Check if session is still alive
                        if !pooled.session.is_alive() {
                            dead_sessions.push(key.clone());
                            continue;
                        }

                        // Check idle timeout
                        if !pooled.in_use && (now - pooled.last_used_at) > config.max_idle_time {
                            idle_to_evict.push(key.clone());
                        }
                    }
                }
            }

            // Remove dead sessions
            if !dead_sessions.is_empty() || !idle_to_evict.is_empty() {
                if let Ok(mut sess_map) = sessions.write() {
                    for key in dead_sessions.iter().chain(idle_to_evict.iter()) {
                        if let Some(mut pooled) = sess_map.remove(key) {
                            let _ = pooled.session.disconnect();
                            
                            if let Ok(mut s) = stats.lock() {
                                s.total_destroyed += 1;
                                if pooled.in_use {
                                    s.active_count = s.active_count.saturating_sub(1);
                                } else {
                                    s.idle_count = s.idle_count.saturating_sub(1);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
