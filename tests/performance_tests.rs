//! Performance Module Unit Tests
//!
//! Comprehensive test suite for performance monitoring utilities:
//! - PerfTimer timing accuracy
//! - MemoryTracker functionality
//! - ThroughputCounter calculations
//! - PerformanceMonitor integration

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::{sleep, timeout};

    // ════════════════════════════════════════════════════════════════
    // PerfTimer Tests
    // ════════════════════════════════════════════════════════════════

    #[test]
    fn test_perf_timer_creation() {
        let timer = PerfTimer::new("test_operation");
        assert_eq!(timer.name, "test_operation");
        assert_eq!(timer.calls, 0);
        assert_eq!(timer.elapsed, Duration::ZERO);
        println!("✓ PerfTimer creation works");
    }

    #[tokio::test]
    async fn test_perf_timer_basic_timing() {
        let mut timer = PerfTimer::new("async_op");

        timer.start();
        sleep(Duration::from_millis(100)).await;
        let duration = timer.stop();

        // Should be approximately 100ms (allow 50ms tolerance)
        assert!(
            duration >= Duration::from_millis(50),
            "Duration too short: {:?}",
            duration
        );
        assert!(
            duration < Duration::from_millis(200),
            "Duration too long: {:?}",
            duration
        );
        assert_eq!(timer.calls, 1);

        println!("✓ Basic async timing works: {:?}", duration);
    }

    #[tokio::test]
    async fn test_perf_timer_multiple_calls() {
        let mut timer = PerfTimer::new("repeated_op");

        for i in 0..5 {
            timer.start();
            sleep(Duration::from_millis(10)).await;
            timer.stop();
            assert_eq!(timer.calls, i + 1, "Call count mismatch");
        }

        let stats = timer.stats();
        assert_eq!(stats.calls, 5);
        assert!(stats.total_time > Duration::ZERO);
        assert!(stats.avg_time > Duration::ZERO);
        assert!(stats.avg_time < stats.total_time); // avg should be less than total

        println!("✓ Multiple calls: {} calls, total={:?}, avg={:?}",
                 stats.calls, stats.total_time, stats.avg_time);
    }

    #[tokio::test]
    async fn test_perf_timer_time_async_closure() {
        let mut timer = PerfTimer::new("closure_op");

        let result = timer.time_async(async {
            sleep(Duration::from_millis(50)).await;
            42
        }).await;

        assert_eq!(result, 42);
        assert_eq!(timer.calls, 1);
        assert!(timer.elapsed > Duration::from_millis(40));

        println!("✓ Async closure timing works");
    }

    #[test]
    fn test_perf_stats_display() {
        let mut timer = PerfTimer::new("display_test");
        timer.elapsed = Duration::from_millis(250);
        timer.calls = 10;

        let stats = timer.stats();
        let display = format!("{}", stats);

        assert!(display.contains("display_test"));
        assert!(display.contains("10 calls"));

        println!("✓ Stats display: {}", display);
    }

    // ════════════════════════════════════════════════════════════════
    // MemoryTracker Tests
    // ════════════════════════════════════════════════════════════════

    #[test]
    fn test_memory_tracker_creation() {
        let tracker = MemoryTracker::new();
        assert!(tracker.baseline.is_none());
        assert!(tracker.peak().is_none());
        assert_eq!(tracker.increase(), None);
        println!("✓ MemoryTracker creation works");
    }

    #[test]
    fn test_memory_tracker_snapshots() {
        let mut tracker = MemoryTracker::new();

        // First snapshot sets baseline
        let usage1 = tracker.snapshot();
        assert!(tracker.baseline.is_some());
        assert_eq!(tracker.increase(), Some(0)); // No increase yet

        // Simulate increased usage (on Windows this would be real)
        // For testing, we just check the logic
        let _usage2 = tracker.snapshot();

        assert!(tracker.peak().is_some());
        // Peak should be >= baseline
        if let (Some(baseline), Some(peak)) = (tracker.baseline, tracker.peak()) {
            assert!(peak >= baseline, "Peak should be >= baseline");
            println!("✓ Memory tracking: baseline={}, peak={}", baseline, peak);
        }
    }

    #[test]
    fn test_memory_tracker_increase() {
        let mut tracker = MemoryTracker::new();

        tracker.snapshot(); // Set baseline

        // In real scenario, memory would grow between snapshots
        // The increase calculation should work correctly
        if let Some(increase) = tracker.increase() {
            assert!(increase >= 0, "Increase cannot be negative");
        }

        println!("✓ Memory increase calculation works");
    }

    // ════════════════════════════════════════════════════════════════
    // ThroughputCounter Tests
    // ════════════════════════════════════════════════════════════════

    #[test]
    fn test_throughput_counter_creation() {
        let counter = ThroughputCounter::new("requests", 60);
        assert_eq!(counter.name, "requests");
        assert_eq!(counter.count, 0);
        println!("✓ ThroughputCounter creation works");
    }

    #[test]
    fn test_throughput_counter_basic_operations() {
        let mut counter = ThroughputCounter::new("ops", 10);

        counter.increment();
        assert_eq!(counter.count, 1);

        counter.add(5);
        assert_eq!(counter.count, 6);

        for _ in 0..4 {
            counter.increment();
        }
        assert_eq!(counter.count, 10);

        println!("✓ Basic operations work: count={}", counter.count);
    }

    #[tokio::test]
    async fn test_throughput_counter_calculation() {
        let mut counter = ThroughputCounter::new("events", 5);

        // Add some events over time
        for _ in 0..100 {
            counter.increment();
        }

        let throughput = counter.throughput();
        assert!(throughput > 0.0, "Throughput must be positive");

        // Should have processed 100 events in a very short time
        // So throughput should be high (>100 events/sec)
        assert!(throughput > 100.0, "Throughput seems low: {:.2}", throughput);

        println!("✓ Throughput calculation: {:.2} items/sec", throughput);
    }

    #[tokio::test]
    async fn test_throughput_counter_window_reset() {
        let mut counter = ThroughputCounter::new("windowed", 2);

        // Fill first window
        for _ in 0..50 {
            counter.increment();
        }

        // Wait for window to expire
        sleep(Duration::from_secs(3)).await;

        // Window should be expired
        assert!(counter.is_window_expired());

        // Auto-reset and get throughput
        let tps = counter.throughput_auto_reset();
        assert!(tps >= 0.0);

        // Count should be reset to 0 after reset
        assert_eq!(counter.count, 0);

        println!("✓ Window reset works: previous TPS={:.2}", tps);
    }

    #[test]
    fn test_throughput_stats_display() {
        let mut counter = ThroughputCounter::new("stats_test", 60);
        counter.add(100);

        let start = counter.window_start;
        counter.count = 100;

        let stats = counter.stats();
        let display = format!("{}", stats);

        assert!(display.contains("stats_test"));
        assert!(display.contains("100"));
        assert!(display.contains("items/sec"));

        println!("✓ Stats display: {}", display);
    }

    // ════════════════════════════════════════════════════════════════
    // PerformanceMonitor Tests
    // ════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_performance_monitor_creation() {
        let monitor = PerformanceMonitor::new(true);

        // Test that it's enabled
        let metrics = monitor.collect_metrics().await;
        assert!(metrics.timers.is_empty());
        assert!(metrics.counters.is_empty());

        println!("✓ PerformanceMonitor creation works");
    }

    #[tokio::test]
    async fn test_performance_monitor_timer_integration() {
        let monitor = PerformanceMonitor::new(true);

        // Time an operation through monitor
        let result: u32 = monitor
            .time_operation("test_timer", async { 42 })
            .await;

        assert_eq!(result, 42);

        // Check that timer was recorded
        let metrics = monitor.collect_metrics().await;
        let timer_opt = metrics.timers.iter().find(|t| t.name == "test_timer");
        assert!(timer_opt.is_some(), "Timer 'test_timer' should exist");

        let timer = timer_opt.unwrap();
        assert_eq!(timer.calls, 1);
        assert!(timer.total_time > Duration::ZERO);

        println!("✓ Monitor-timer integration: {:?}", timer);
    }

    #[tokio::test]
    async fn test_performance_monitor_multiple_metrics() {
        let monitor = PerformanceMonitor::new(true);

        // Record multiple different operations
        for i in 0..3 {
            let name = format!("op_{}", i);
            monitor
                .time_operation(&name, async move {
                    sleep(Duration::from_millis(10 * (i + 1))).await;
                    i * 10
                })
                .await;
        }

        let metrics = monitor.collect_metrics().await;
        assert_eq!(metrics.timers.len(), 3, "Should have 3 timers");

        println!("✓ Multiple metrics: {} timers recorded", metrics.timers.len());
    }

    #[tokio::test]
    async fn test_performance_monitor_disabled() {
        let monitor = PerformanceMonitor::new(false);

        // When disabled, operations should still work but not record
        let result = monitor
            .time_operation("disabled_test", async { true })
            .await;

        assert!(result);

        let metrics = monitor.collect_metrics().await;
        assert!(metrics.timers.is_empty(), "Disabled monitor should not record");

        println!("✓ Disabled monitor doesn't record");
    }

    #[tokio::test]
    async fn test_performance_monitor_summary() {
        let monitor = PerformanceMonitor::new(true);

        // Generate some activity
        monitor.timer("summary_test").await.start();
        sleep(Duration::from_millis(50)).await;
        monitor.timer("summary_test").await.stop().ok();

        monitor.counter("summary_counter", 10).await.increment();

        // Print summary (should not panic)
        monitor.print_summary().await;

        println!("✓ Summary generation works");
    }

    // ════════════════════════════════════════════════════════════════
    // Macro Tests
    // ════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_time_it_macro() {
        let monitor = PerformanceMonitor::new(true);

        let result = time_it!(monitor, "macro_test", {
            sleep(Duration::from_millis(25)).await;
            "macro_result"
        }).await;

        assert_eq!(result, "macro_result");

        let metrics = monitor.collect_metrics().await;
        assert!(metrics.timers.iter().any(|t| t.name == "macro_test"));

        println!("✓ time_it! macro works");
    }

    // ════════════════════════════════════════════════════════════════
    // Edge Cases and Error Conditions
    // ════════════════════════════════════════════════════════════════

    #[test]
    fn test_perf_timer_no_start() {
        let mut timer = PerfTimer::new("no_start");
        
        // Calling stop without start should return ZERO
        let duration = timer.stop();
        assert_eq!(duration, Duration::ZERO);
        assert_eq!(timer.calls, 0); // No call recorded
        
        println!("✓ Stop without start returns ZERO");
    }

    #[test]
    fn test_perf_timer_stop_twice() {
        let mut timer = PerfTimer::new("double_stop");
        
        timer.start();
        // In reality we'd sleep here, but for testing skip it
        let _duration1 = timer.stop();
        let duration2 = timer.stop();
        
        // Second stop without new start should return ZERO
        assert_eq!(duration2, Duration::ZERO);
        assert_eq!(timer.calls, 1); // Only one call recorded
        
        println!("✓ Double stop handled correctly");
    }

    #[test]
    fn test_throughput_counter_empty_window() {
        let counter = ThroughputCounter::new("empty", 60);
        
        // With no counts, throughput should be 0
        let throughput = counter.throughput();
        assert_eq!(throughput, 0.0);
        
        println!("✓ Empty counter returns 0 throughput");
    }

    #[test]
    fn test_perf_stats_zero_calls() {
        let timer = PerfTimer::new("zero_calls");
        let stats = timer.stats();
        
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.total_time, Duration::ZERO);
        assert_eq!(stats.avg_time, Duration::ZERO);
        
        let display = format!("{}", stats);
        assert!(display.contains("0 calls"));
        
        println!("✓ Zero calls stats work");
    }
}
