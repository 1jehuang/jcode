//! Controlled scheduler tests. These use no live mobile API or network service.

use super::poll_commands;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::{Instant, advance};

// Keep the test future runnable so Tokio cannot auto-advance its paused clock.
async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

async fn tick(seconds: u64) {
    advance(Duration::from_secs(seconds)).await;
    settle().await;
}

async fn cancel(task: JoinHandle<()>) {
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn poll_commands_starts_immediately() {
    let started = Instant::now();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let task = tokio::spawn(poll_commands(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        async { Ok(false) }
    }));

    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        Instant::now(),
        started,
        "startup must not advance the clock"
    );
    cancel(task).await;
}

#[tokio::test(start_paused = true)]
async fn poll_commands_drains_completed_work_without_delay() {
    let started = Instant::now();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let task = tokio::spawn(poll_commands(move || {
        let call = observed.fetch_add(1, Ordering::SeqCst);
        async move { Ok(call < 4) }
    }));

    settle().await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        5,
        "four completed commands then one idle claim"
    );
    assert_eq!(
        Instant::now(),
        started,
        "queued commands must have no polling gap"
    );
    tick(4).await;
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    tick(1).await;
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    cancel(task).await;
}

#[tokio::test(start_paused = true)]
async fn poll_commands_idle_claims_once_per_five_seconds() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let task = tokio::spawn(poll_commands(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        async { Ok(false) }
    }));

    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    for expected in 2..=5 {
        tick(4).await;
        assert_eq!(calls.load(Ordering::SeqCst), expected - 1);
        tick(1).await;
        assert_eq!(calls.load(Ordering::SeqCst), expected);
    }
    cancel(task).await;
}

#[tokio::test(start_paused = true)]
async fn poll_commands_errors_retry_after_twenty_not_twenty_five_seconds() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let task = tokio::spawn(poll_commands(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        async { Err(anyhow::anyhow!("controlled claim failure")) }
    }));

    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tick(19).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tick(1).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "retry must occur at t=20, not t=25"
    );
    tick(5).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    tick(14).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    tick(1).await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    cancel(task).await;
}

#[tokio::test(start_paused = true)]
async fn poll_commands_cancellation_stops_future_claims() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let task = tokio::spawn(poll_commands(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        async { Ok(false) }
    }));

    settle().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    cancel(task).await;
    for _ in 0..8 {
        tick(5).await;
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "cancelled poller must never claim again"
    );
}

#[tokio::test(start_paused = true)]
async fn poll_commands_controlled_clock_baseline_comparison() {
    let started = Instant::now();
    let baseline_times = Arc::new(Mutex::new(Vec::new()));
    let recorded = baseline_times.clone();
    // Model the old sleep-before-every-claim scheduling, not a live API.
    let baseline = tokio::spawn(async move {
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_secs(5)).await;
            recorded.lock().unwrap().push(started.elapsed());
        }
    });
    let new_times = Arc::new(Mutex::new(Vec::new()));
    let recorded = new_times.clone();
    let new = tokio::spawn(poll_commands(move || {
        let mut times = recorded.lock().unwrap();
        let completed = times.len() < 3;
        if completed {
            times.push(started.elapsed());
        }
        async move { Ok(completed) }
    }));

    settle().await;
    assert!(baseline_times.lock().unwrap().is_empty());
    assert_eq!(*new_times.lock().unwrap(), vec![Duration::ZERO; 3]);
    for _ in 0..3 {
        tick(5).await;
    }
    baseline.await.unwrap();
    let old = baseline_times.lock().unwrap().clone();
    let new_observed = new_times.lock().unwrap().clone();
    assert_eq!(
        old,
        vec![
            Duration::from_secs(5),
            Duration::from_secs(10),
            Duration::from_secs(15)
        ]
    );
    assert_eq!(new_observed, vec![Duration::ZERO; 3]);
    assert!(
        old.windows(2)
            .all(|pair| pair[1] - pair[0] == Duration::from_secs(5))
    );
    assert!(new_observed.windows(2).all(|pair| pair[1] == pair[0]));
    println!(
        "Controlled paused-clock scheduling comparison, NOT a live API benchmark: baseline command times {old:?}, new {new_observed:?}. Removed 5s startup delay and 5s gap per queued command."
    );
    cancel(new).await;
}
