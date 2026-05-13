//! Performance Benchmark Suite for Enhanced Features
//!
//! Comprehensive benchmarking framework with:
//! - Timing utilities
//! - Memory profiling
//! - Throughput measurement
//! - Latency tracking
//! - Resource monitoring

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Performance timer for measuring execution time
#[derive(Debug, Clone)]
pub struct PerfTimer {
    name: String,
    start: Option<Instant>,
    elapsed: Duration,
    calls: u64,
}

impl PerfTimer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            start: None,
            elapsed: Duration::ZERO,
            calls: 0,
        }
    }

    /// Start timing
    pub fn start(&mut self) {
        self.start = Some(Instant::now());
    }

    /// Stop timing and record
    pub fn stop(&mut self) -> Duration {
        if let Some(start) = self.start.take() {
            let duration = start.elapsed();
            self.elapsed += duration;
            self.calls += 1;
            duration
        } else {
            Duration::ZERO
        }
    }

    /// Time a closure execution
    pub async fn time_async<F, T>(&mut self, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.start();
        let result = f.await;
        self.stop();
        result
    }

    /// Get statistics
    pub fn stats(&self) -> PerfStats {
        PerfStats {
            name: self.name.clone(),
            total_time: self.elapsed,
            avg_time: if self.calls > 0 {
                self.elapsed / self.calls
            } else {
                Duration::ZERO
            },
            calls: self.calls,
        }
    }
}

/// Performance statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfStats {
    pub name: String,
    pub total_time: Duration,
    pub avg_time: Duration,
    pub calls: u64,
}

impl std::fmt::Display for PerfStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {} calls, total={:.2?}, avg={:.2?}",
            self.name,
            self.calls,
            self.total_time.as_secs_f64() * 1000.0,
            self.avg_time.as_secs_f64() * 1000.0
        )
    }
}

/// Memory usage tracker
pub struct MemoryTracker {
    baseline: Option<usize>,
    peaks: Vec<usize>,
}

impl MemoryTracker {
    pub fn new() -> Self {
        Self {
            baseline: None,
            peaks: Vec::new(),
        }
    }

    /// Record current memory usage
    pub fn snapshot(&mut self) -> usize {
        let usage = self.get_current_usage();

        if self.baseline.is_none() {
            self.baseline = Some(usage);
        }

        if let Some(last) = self.peaks.last() {
            if usage > *last {
                self.peaks.push(usage);
            }
        } else {
            self.peaks.push(usage);
        }

        usage
    }

    /// Get current memory usage in bytes (platform-specific)
    #[cfg(target_os = "windows")]
    fn get_current_usage(&self) -> usize {
        // Windows implementation using GetProcessMemoryInfo
        use std::mem;
        use winapi::um::processthreadsapi::GetCurrentProcess;
        use winapi::um::psapi::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};

        unsafe {
            let mut pmc: PROCESS_MEMORY_COUNTERS = mem::zeroed();
            pmc.cbSize = mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;

            let handle = GetCurrentProcess();
            if GetProcessMemoryInfo(handle, &mut pmc, pmc.cbSize) != 0 {
                pmc.WorkingSetSize as usize
            } else {
                0
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn get_current_usage(&self) -> usize {
        // Unix implementation - read /proc/self/status or use libc
        // Simplified version that returns 0 on non-Windows platforms
        0
    }

    /// Get peak memory usage
    pub fn peak(&self) -> Option<usize> {
        self.peaks.last().copied()
    }

    /// Get memory increase from baseline
    pub fn increase(&self) -> Option<usize> {
        match (self.baseline, self.peak()) {
            (Some(baseline), Some(peak)) => Some(peak.saturating_sub(baseline)),
            _ => None,
        }
    }
}

/// Throughput counter
#[derive(Debug, Clone)]
pub struct ThroughputCounter {
    name: String,
    count: u64,
    window_start: Instant,
    window_duration: Duration,
    history: VecDeque<(Instant, u64)>,
}

use std::collections::VecDeque;

impl ThroughputCounter {
    pub fn new(name: impl Into<String>, window_secs: u64) -> Self {
        Self {
            name: name.into(),
            count: 0,
            window_start: Instant::now(),
            window_duration: Duration::from_secs(window_secs),
            history: VecDeque::with_capacity(100),
        }
    }

    /// Increment counter
    pub fn increment(&mut self) {
        self.count += 1;
    }

    /// Add multiple items
    pub fn add(&mut self, n: u64) {
        self.count += n;
    }

    /// Get current throughput (items/second)
    pub fn throughput(&self) -> f64 {
        let elapsed = self.window_start.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.count as f64 / elapsed
        } else {
            0.0
        }
    }

    /// Reset window
    pub fn reset_window(&mut self) {
        self.history.push_back((self.window_start, self.count));
        if self.history.len() > 100 {
            self.history.pop_front();
        }

        self.count = 0;
        self.window_start = Instant::now();
    }

    /// Check if window has expired
    pub fn is_window_expired(&self) -> bool {
        self.window_start.elapsed() >= self.window_duration
    }

    /// Auto-reset if expired and return throughput
    pub fn throughput_auto_reset(&mut self) -> f64 {
        if self.is_window_expired() {
            let t = self.throughput();
            self.reset_window();
            t
        } else {
            self.throughput()
        }
    }

    /// Get statistics
    pub fn stats(&self) -> ThroughputStats {
        ThroughputStats {
            name: self.name.clone(),
            current_count: self.count,
            throughput: self.throughput(),
            window_elapsed: self.window_start.elapsed(),
        }
    }
}

/// Throughput statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThroughputStats {
    pub name: String,
    pub current_count: u64,
    pub throughput: f64,
    pub window_elapsed: Duration,
}

impl std::fmt::Display for ThroughputStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {:.2} items/sec ({:.1}s window)",
            self.name,
            self.throughput,
            self.window_elapsed.as_secs_f64()
        )
    }
}

/// Comprehensive performance monitor
pub struct PerformanceMonitor {
    timers: RwLock<HashMap<String, PerfTimer>>,
    counters: RwLock<HashMap<String, ThroughputCounter>>,
    memory: RwLock<MemoryTracker>,
    enabled: bool,
}

impl PerformanceMonitor {
    pub fn new(enabled: bool) -> Self {
        Self {
            timers: RwLock::new(HashMap::new()),
            counters: RwLock::new(HashMap::new()),
            memory: RwLock::new(MemoryTracker::new()),
            enabled,
        }
    }

    /// Create or get a timer
    pub async fn timer(&self, name: &str) -> PerfTimer {
        if !self.enabled {
            return PerfTimer::new(name);
        }

        let mut timers = self.timers.write().await;
        timers
            .entry(name.to_string())
            .or_insert_with(|| PerfTimer::new(name))
            .clone()
    }

    /// Create or get a counter
    pub async fn counter(&self, name: &str, window_secs: u64) -> ThroughputCounter {
        if !self.enabled {
            return ThroughputCounter::new(name, window_secs);
        }

        let mut counters = self.counters.write().await;
        counters
            .entry(name.to_string())
            .or_insert_with(|| ThroughputCounter::new(name, window_secs))
            .clone()
    }

    /// Time an async operation
    pub async fn time_operation<F, T>(&self, name: &str, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        if !self.enabled {
            return f.await;
        }

        let mut timer = self.timer(name).await;
        timer.time_async(f).await
    }

    /// Record memory snapshot
    pub async fn snapshot_memory(&self) -> usize {
        if !self.enabled {
            return 0;
        }

        let mut mem = self.memory.write().await;
        mem.snapshot()
    }

    /// Get all performance metrics
    pub async fn collect_metrics(&self) -> PerformanceMetrics {
        let timers = self.timers.read().await;
        let counters = self.counters.read().await;
        let memory = self.memory.read().await;

        let timer_stats: Vec<PerfStats> = timers.values().map(|t| t.stats()).collect();
        let counter_stats: Vec<ThroughputStats> =
            counters.values().map(|c| c.stats()).collect();

        PerformanceMetrics {
            timestamp: chrono::Utc::now(),
            timers: timer_stats,
            counters: counter_stats,
            memory_peak: memory.peak(),
            memory_baseline: memory.baseline,
        }
    }

    /// Print summary to console
    pub async fn print_summary(&self) {
        if !self.enabled {
            return;
        }

        let metrics = self.collect_metrics().await;

        println!("\n📊 Performance Monitor Summary");
        println!("═" .repeat(50));

        println!("\n⏱️ Timers:");
        for timer in &metrics.timers {
            println!("  {}", timer);
        }

        println!("\n📈 Counters:");
        for counter in &metrics.counters {
            println!("  {}", counter);
        }

        if let (Some(baseline), Some(peak)) = (metrics.memory_baseline, metrics.memory_peak) {
            let increase = peak.saturating_sub(*baseline);
            println!(
                "\n💾 Memory: baseline={}KB, peak={}KB, increase={}KB",
                baseline / 1024,
                peak / 1024,
                increase / 1024
            );
        }

        println!("{}\n", "═" .repeat(50));
    }
}

/// Collected performance metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub timers: Vec<PerfStats>,
    pub counters: Vec<ThroughputStats>,
    pub memory_peak: Option<usize>,
    pub memory_baseline: Option<usize>,
}

/// Macro for easy timing
#[macro_export]
macro_rules! time_it {
    ($monitor:expr, $name:expr, $async_block:block) => {{
        $monitor.time_operation($name, async move { $async_block }).await
    }};
}
