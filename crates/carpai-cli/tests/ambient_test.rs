//! Ambient 模块集成测试
//!
//! 测试 BackgroundRunner 和 TaskScheduler 的基本功能

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use carpai_cli::{BackgroundRunner, TaskScheduler};
use carpai_cli::ambient::runner::BackgroundTask;
use carpai_cli::ambient::scheduler::ScheduledTask;

// ===== Test helpers =====

struct TestTask {
    name: &'static str,
    ran: Arc<AtomicBool>,
}

impl BackgroundTask for TestTask {
    fn name(&self) -> &'static str { self.name }

    async fn run(self: Box<Self>, cancel: tokio_util::sync::CancellationToken) {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    self.ran.store(true, Ordering::SeqCst);
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(10)) => {
                    // Simulate work
                }
            }
        }
    }
}

struct TestScheduledTask {
    name: &'static str,
    counter: Arc<AtomicU32>,
}

impl ScheduledTask for TestScheduledTask {
    fn name(&self) -> &'static str { self.name }
    fn interval(&self) -> Duration { Duration::from_millis(50) }

    async fn execute(&self) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

// ===== Tests =====

#[tokio::test]
async fn test_background_runner_spawn_and_shutdown() {
    let runner = BackgroundRunner::new();
    let ran = Arc::new(AtomicBool::new(false));

    runner.spawn(TestTask { name: "test-task", ran: ran.clone() }).await;

    // Give it time to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Shutdown should cancel the task
    runner.shutdown().await;

    assert!(ran.load(Ordering::SeqCst), "Task should have been cancelled");
}

#[tokio::test]
async fn test_background_runner_is_cancelled() {
    let runner = BackgroundRunner::new();
    assert!(!runner.is_cancelled());

    let ran = Arc::new(AtomicBool::new(false));
    runner.spawn(TestTask { name: "cancel-test", ran }).await;

    tokio::time::sleep(Duration::from_millis(20)).await;
    runner.shutdown().await;

    assert!(runner.is_cancelled());
}

#[tokio::test]
async fn test_task_scheduler_register_and_shutdown() {
    let scheduler = TaskScheduler::new();
    let counter = Arc::new(AtomicU32::new(0));

    scheduler.register(TestScheduledTask {
        name: "counter-task",
        counter: counter.clone(),
    });

    // Give it time to run a few times
    tokio::time::sleep(Duration::from_millis(120)).await;

    scheduler.shutdown();

    let count = counter.load(Ordering::SeqCst);
    assert!(count >= 1, "Scheduled task should have run at least once, got {}", count);
}

#[tokio::test]
async fn test_task_scheduler_shutdown_stops_execution() {
    let scheduler = TaskScheduler::new();
    let counter = Arc::new(AtomicU32::new(0));

    scheduler.register(TestScheduledTask {
        name: "stop-task",
        counter: counter.clone(),
    });

    // Let it run once
    tokio::time::sleep(Duration::from_millis(60)).await;
    scheduler.shutdown();

    let count_after_shutdown = counter.load(Ordering::SeqCst);

    // Wait a bit more — counter should not increase
    tokio::time::sleep(Duration::from_millis(100)).await;
    let count_final = counter.load(Ordering::SeqCst);

    assert_eq!(
        count_after_shutdown, count_final,
        "Counter should not increase after shutdown"
    );
}

#[tokio::test]
async fn test_multiple_scheduled_tasks() {
    let scheduler = TaskScheduler::new();
    let c1 = Arc::new(AtomicU32::new(0));
    let c2 = Arc::new(AtomicU32::new(0));

    scheduler.register(TestScheduledTask { name: "task-1", counter: c1.clone() });
    scheduler.register(TestScheduledTask { name: "task-2", counter: c2.clone() });

    tokio::time::sleep(Duration::from_millis(100)).await;

    scheduler.shutdown();

    assert!(c1.load(Ordering::SeqCst) >= 1, "Task 1 should have run");
    assert!(c2.load(Ordering::SeqCst) >= 1, "Task 2 should have run");
}
