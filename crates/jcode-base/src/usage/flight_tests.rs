use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn with_home(f: impl FnOnce(tokio::runtime::Runtime)) {
    let _guard = crate::storage::lock_test_env();
    let dir = tempfile::tempdir().unwrap();
    struct Home(Option<std::ffi::OsString>);
    impl Drop for Home {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }
    let _home = Home(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", dir.path());
    f(tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap());
}

fn sample() -> UsageData {
    UsageData {
        five_hour: 0.4,
        fetched_at: Some(Instant::now()),
        ..Default::default()
    }
}

#[test]
fn response_diagnostics_capture_only_bounded_printable_retry_hints() {
    let mut log = RequestLog::new("label:diagnostics-test");
    for value in ["120", "Sun, 18 Oct 2026 00:15:00 GMT"] {
        log.received(429, Some(value));
        assert_eq!(log.status, Some(429));
        assert_eq!(log.retry_after.as_deref(), Some(value));
    }
    for value in ["", "10\nforged-log-line", "\u{7f}"] {
        log.received(429, Some(value));
        assert!(log.retry_after.is_none());
    }
    log.received(429, Some(&"9".repeat(129)));
    assert!(log.retry_after.is_none());
    log.received(200, None);
    assert_eq!(log.status, Some(200));
    assert!(log.retry_after.is_none());
}

#[test]
fn concurrent_callers_make_one_probe() {
    with_home(|runtime| {
        runtime.block_on(async {
            let probes = AtomicUsize::new(0);
            let key = "label:flight-concurrent";
            let results = futures::future::join_all((0..16).map(|_| {
                fetch(key.into(), || async {
                    probes.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let data = disk_cache::store_success(key, &sample());
                    store_anthropic_usage(key.into(), data.clone());
                    Ok(data)
                })
            }))
            .await;
            assert!(results.into_iter().all(|r| r.unwrap().five_hour == 0.4));
            assert_eq!(probes.load(Ordering::SeqCst), 1);
        })
    });
}

#[test]
fn different_accounts_can_probe_concurrently() {
    with_home(|runtime| {
        runtime.block_on(async {
            let barrier = tokio::sync::Barrier::new(2);
            let probes = (0..2).map(|index| {
                let barrier = &barrier;
                let key = format!("label:flight-parallel-{index}");
                fetch(key.clone(), move || async move {
                    barrier.wait().await;
                    let data = disk_cache::store_success(&key, &sample());
                    store_anthropic_usage(key, data.clone());
                    Ok(data)
                })
            });
            let results =
                tokio::time::timeout(Duration::from_secs(2), futures::future::join_all(probes))
                    .await
                    .unwrap();
            assert!(results.into_iter().all(|r| r.is_ok()));
        })
    });
}

#[test]
fn cancelled_probe_persists_backoff_and_releases_locks() {
    with_home(|runtime| {
        runtime.block_on(async {
            let key = "label:flight-cancel";
            let started = Arc::new(tokio::sync::Notify::new());
            let notify = started.clone();
            let task = tokio::spawn(fetch(key.into(), move || async move {
                notify.notify_one();
                std::future::pending().await
            }));
            started.notified().await;
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            let data = disk_cache::fresh(key).unwrap();
            assert!(data.last_error.unwrap().contains("cancelled"));
            let _lock = tokio::time::timeout(Duration::from_secs(1), disk_cache::lock_probe(key))
                .await
                .unwrap()
                .unwrap();
            let cached = fetch(key.into(), || async {
                panic!("backoff must prevent a new probe")
            })
            .await
            .unwrap();
            assert!(cached.last_error.is_some());
        })
    });
}

#[test]
fn deadline_is_cached_before_the_outer_account_timeout() {
    with_home(|runtime| {
        runtime.block_on(async {
            tokio::time::pause();
            let started = tokio::time::Instant::now();
            let key = "label:flight-timeout";
            let error = fetch(key.into(), std::future::pending).await.unwrap_err();
            assert!(error.to_string().contains("timed out"));
            let elapsed = tokio::time::Instant::now() - started;
            assert!(elapsed >= Duration::from_secs(15) && elapsed < Duration::from_secs(16));
            assert!(
                disk_cache::fresh(key)
                    .unwrap()
                    .last_error
                    .unwrap()
                    .contains("timed out")
            );
            assert!(
                fetch(key.into(), || async { panic!("timeout must back off") })
                    .await
                    .unwrap()
                    .last_error
                    .is_some()
            );
        })
    });
}

#[test]
fn cancelled_lock_waiter_does_not_poison_the_account() {
    with_home(|runtime| {
        runtime.block_on(async {
            let key = "label:flight-waiter";
            let held = disk_cache::lock_probe(key).await.unwrap();
            assert!(
                tokio::time::timeout(
                    Duration::from_millis(60),
                    fetch(key.into(), || async { panic!("lock held") })
                )
                .await
                .is_err()
            );
            assert!(disk_cache::fresh(key).is_none());
            drop(held);
            let data = fetch(key.into(), || async { Ok(sample()) }).await.unwrap();
            assert_eq!(data.five_hour, 0.4);
        })
    });
}

#[test]
fn received_429_is_cached_without_waiting_for_error_body() {
    with_home(|runtime| {
        runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/usage", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            assert!(stream.read(&mut bytes).await.unwrap() > 0, "request connection closed before any bytes arrived");
            stream.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 62\r\nContent-Length: 10000\r\nConnection: close\r\n\r\n").await.unwrap();
            std::future::pending::<()>().await;
        });
        let key = "label:flight-http-429";
        let result = tokio::time::timeout(Duration::from_secs(2), fetch(key.into(), || super::super::fetch_anthropic_usage_data_uncached("fixture-only".into(), key.into(), &url))).await.unwrap();
        assert!(result.unwrap_err().to_string().contains("429"));
        let cached = disk_cache::fresh(key).unwrap();
        assert!(cached.last_error.unwrap().contains("429"));
        assert!(cached.refresh_after.unwrap().duration_since(Instant::now()) > Duration::from_secs(1798));
        fetch(key.into(), || async { panic!("429 must prevent a second HTTP request") }).await.unwrap();
        server.abort();
    })
    });
}

#[test]
fn aborted_http_body_read_persists_backoff() {
    with_home(|runtime| {
        runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/usage", listener.local_addr().unwrap());
        let (sent, received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            assert!(stream.read(&mut bytes).await.unwrap() > 0, "request connection closed before any bytes arrived");
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10000\r\n\r\n").await.unwrap();
            let _ = sent.send(());
            std::future::pending::<()>().await;
        });
        let key = "label:flight-http-cancel";
        let probe = tokio::spawn(async move {
            fetch(key.into(), || super::super::fetch_anthropic_usage_data_uncached("fixture-only".into(), key.into(), &url)).await
        });
        tokio::time::timeout(Duration::from_secs(2), received).await.unwrap().unwrap();
        tokio::task::yield_now().await;
        probe.abort();
        assert!(probe.await.unwrap_err().is_cancelled());
        assert!(disk_cache::fresh(key).unwrap().last_error.unwrap().contains("cancelled"));
        fetch(key.into(), || async { panic!("cancelled HTTP probe must back off") }).await.unwrap();
        server.abort();
    })
    });
}
