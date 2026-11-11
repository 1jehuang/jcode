use jcode_base::mcp::{McpClient, McpConfig, McpServerConfig, SharedMcpPool};
use std::sync::Arc;
#[path = "pool_drain_observer.rs"]
mod drain_observer;
use std::collections::HashMap;
use std::ffi::c_void;
use std::future::{Future, poll_fn};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;
use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

#[repr(C)]
struct FileTime {
    low: u32,
    high: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetProcessTimes(
        process: *mut c_void,
        created: *mut FileTime,
        exited: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    fn QueryFullProcessImageNameW(
        process: *mut c_void,
        flags: u32,
        image: *mut u16,
        size: *mut u32,
    ) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn IsProcessInJob(process: *mut c_void, job: *mut c_void, result: *mut i32) -> i32;
}

struct Generation {
    wrapper: OwnedHandle,
    descendant: OwnedHandle,
}

fn retain(row: &serde_json::Value, key: &str) -> OwnedHandle {
    let raw = unsafe { OpenProcess(0x0010_1000, 0, row[key].as_u64().unwrap() as u32) };
    assert!(!raw.is_null(), "retain fixture handle before cleanup");
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut created = FileTime { low: 0, high: 0 };
    let mut exited = FileTime { low: 0, high: 0 };
    let mut kernel = FileTime { low: 0, high: 0 };
    let mut user = FileTime { low: 0, high: 0 };
    assert_ne!(
        unsafe { GetProcessTimes(raw, &mut created, &mut exited, &mut kernel, &mut user) },
        0
    );
    let identity = &row[format!("{key}_identity")];
    assert_eq!(
        Some(((created.high as u64) << 32) | created.low as u64),
        identity["creation_time"].as_u64(),
    );
    let mut image = vec![0u16; 32768];
    let mut size = image.len() as u32;
    assert_ne!(
        unsafe { QueryFullProcessImageNameW(raw, 0, image.as_mut_ptr(), &mut size) },
        0
    );
    assert!(
        String::from_utf16(&image[..size as usize])
            .unwrap()
            .eq_ignore_ascii_case(identity["image"].as_str().unwrap())
    );
    handle
}

impl Generation {
    fn retain(row: &serde_json::Value) -> Self {
        Self {
            wrapper: retain(row, "wrapper"),
            descendant: retain(row, "grandchild"),
        }
    }

    fn assert_owned_job(&self, job: usize) {
        for (process, expected) in [
            (self.wrapper.as_raw_handle(), 1),
            (self.descendant.as_raw_handle(), 1),
            (unsafe { GetCurrentProcess() }, 0),
        ] {
            let mut inside = -1;
            assert_ne!(
                unsafe { IsProcessInJob(process, job as *mut c_void, &mut inside) },
                0
            );
            assert_eq!(
                inside, expected,
                "exact MCP job membership with test-process negative control"
            );
        }
    }

    fn snapshot(&self, stage: &'static str) -> [u32; 2] {
        let states = [&self.wrapper, &self.descendant]
            .map(|handle| unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) });
        assert!(states.iter().all(|state| matches!(state, 0 | 258)));
        drain_observer::record_snapshot(stage, states);
        states
    }

    fn assert_state(&self, stage: &str, expected: u32, timeout: u32) {
        for (role, handle) in [("wrapper", &self.wrapper), ("descendant", &self.descendant)] {
            let started = std::time::Instant::now();
            let observed_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis();
            let initial = unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) };
            let observed = if initial == 258 && expected == 0 && timeout != 0 {
                unsafe { WaitForSingleObject(handle.as_raw_handle(), timeout) }
            } else {
                initial
            };
            println!(
                "OVERLAP_STATE stage={stage} role={role} unix_ms={observed_at} initial={initial} observed={observed} elapsed_ms={}",
                started.elapsed().as_millis()
            );
            assert_eq!(observed, expected, "{stage}: {role} process state");
        }
    }
}

async fn pending_once<F: Future>(mut future: Pin<&mut F>) {
    poll_fn(|context| match future.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("operation escaped its held reference barrier"),
    })
    .await;
}

fn generations(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

pub async fn run(observations: bool) {
    assert_eq!(
        std::env::var("JCODE_MCP_TEST_CONTROLLER").as_deref(),
        Ok("1")
    );
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("generations.jsonl");
    let config = McpServerConfig {
        command: std::env::var("JCODE_MCP_TEST_PYTHON").unwrap(),
        args: vec![
            std::env::var("JCODE_MCP_TEST_FIXTURE").unwrap(),
            "serve-in-job".into(),
            record.display().to_string(),
        ],
        env: HashMap::new(),
        shared: true,
        transport: None,
        url: None,
        headers: HashMap::new(),
        enabled: None,
        disabled: None,
        timeout_secs: Some(10),
    };
    let pool = SharedMcpPool::new(McpConfig::default());
    pool.connect_server("fixture", &config).await.unwrap();
    let old_handles = pool.acquire_handles("old-session").await;
    let old_handle = old_handles.get("fixture").unwrap();
    assert_eq!(pool.ref_counts().await.get("fixture"), Some(&1));
    let initial = generations(&record);
    assert_eq!(initial.len(), 1);
    let old = Arc::new(Generation::retain(&initial[0]));
    let _drain_observer = observations.then(drain_observer::install);
    if observations {
        drain_observer::set_generation("old", Arc::clone(&old));
    }
    old.assert_state("initial", 258, 0);
    pool.test_with_owned_job("fixture", |job| old.assert_owned_job(job))
        .await;
    println!("OVERLAP_INITIAL {}", initial[0]);

    let baseline = pool.test_wait_points();
    let at_barrier = |release, commit, publication, acquire| {
        [
            baseline[0] + release,
            baseline[1] + commit,
            baseline[2] + publication,
            baseline[3] + acquire,
        ]
    };
    let names = vec!["fixture".into()];
    let reference_barrier = pool.test_lock_ref_counts().await;
    let mut old_release = Box::pin(pool.release_handles("old-session", &names));
    pending_once(old_release.as_mut()).await;
    assert_eq!(pool.test_wait_points(), at_barrier(1, 0, 0, 0));
    let mut disconnect = Box::pin(pool.disconnect_server("fixture"));
    let mut visibility = Box::pin(pool.get_handle("fixture"));
    tokio::time::timeout(
        Duration::from_secs(10),
        poll_fn(|context| {
            if let Poll::Ready(result) = disconnect.as_mut().poll(context) {
                panic!("disconnect committed through held reference barrier: {result:?}");
            }
            match visibility.as_mut().poll(context) {
                Poll::Pending => Poll::Ready(()),
                Poll::Ready(_) => {
                    visibility = Box::pin(pool.get_handle("fixture"));
                    Poll::Pending
                }
            }
        }),
    )
    .await
    .expect("disconnect must reach its registry commit barrier");
    drop(visibility);
    assert_eq!(pool.test_wait_points(), at_barrier(1, 1, 0, 0));
    if observations {
        old.snapshot("commit_before_refs_no_wait");
    }
    if observations {
        drain_observer::assert_observed(1);
    }
    assert!(old_handle.is_closed());
    assert_eq!(reference_barrier.get("fixture"), Some(&1));
    assert!(!pool.cleanup_pending());
    let mut reconnect = Box::pin(pool.connect_server("fixture", &config));
    pending_once(reconnect.as_mut()).await;
    assert_eq!(pool.test_wait_points(), at_barrier(1, 1, 1, 0));
    assert_eq!(
        generations(&record).len(),
        1,
        "successor must not publish early"
    );

    drop(reference_barrier);
    old_release.await;
    disconnect.await.unwrap();
    assert!(pool.ref_counts().await.is_empty());
    tokio::time::timeout(Duration::from_secs(15), reconnect)
        .await
        .expect("successor reconnect deadline")
        .unwrap();
    if observations {
        old.snapshot("successor_connect_returned_no_wait");
    }
    println!(
        "OVERLAP_BARRIER witnessed_before_release=true old_release=pending disconnect_commit=pending reconnect_visibility_read=pending"
    );
    let rows = generations(&record);
    assert_eq!(rows.len(), 2, "exactly one successor generation");
    if observations {
        drain_observer::assert_successor_identity(&rows[1]);
    }
    assert_ne!(rows[0]["wrapper_identity"], rows[1]["wrapper_identity"]);
    let successor = Arc::new(Generation::retain(&rows[1]));
    if observations {
        drain_observer::set_generation("successor", Arc::clone(&successor));
    }
    successor.assert_state("successor_live", 258, 0);
    pool.test_with_owned_job("fixture", |job| successor.assert_owned_job(job))
        .await;
    if observations {
        old.snapshot("after_publication_no_wait");
    }
    println!("OVERLAP_SUCCESSOR {}", rows[1]);

    let reference_barrier = pool.test_lock_ref_counts().await;
    let mut acquire = Box::pin(pool.acquire_handles("successor-session"));
    pending_once(acquire.as_mut()).await;
    let mut stale_release = Box::pin(pool.release_handles("old-session", &names));
    pending_once(stale_release.as_mut()).await;
    assert_eq!(pool.test_wait_points(), at_barrier(2, 1, 1, 1));
    drop(reference_barrier);
    let current = acquire.await;
    stale_release.await;
    assert_eq!(pool.ref_counts().await.get("fixture"), Some(&1));
    let handle = current.get("fixture").unwrap();
    assert!(!handle.is_closed());
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            old_handle.request("tools/list", None)
        )
        .await
        .expect("retired handle must fail promptly")
        .is_err()
    );
    let response = tokio::time::timeout(Duration::from_secs(2), handle.request("tools/list", None))
        .await
        .expect("successor RPC deadline")
        .unwrap();
    assert!(response.result.as_ref().unwrap()["tools"].is_array());
    successor.assert_state("successor_live", 258, 0);
    if observations {
        old.snapshot("after_successor_rpc_no_wait");
    }
    old.assert_state("retired_eventual_after_rpc", 0, 5000);
    assert_eq!(generations(&record).len(), 2);
    println!("OVERLAP_RETAINED refs=1 stale_release=no_effect old_handle=closed successor_rpc=ok");

    pool.disconnect_server("fixture").await.unwrap();
    if observations {
        drain_observer::report(2);
    }
    successor.assert_state("successor_cleanup", 0, 5000);
    assert!(handle.is_closed());
    assert!(pool.ref_counts().await.is_empty());
    if observations {
        println!("OVERLAP_OBSERVATIONAL_PASS eventual_signals=true strict_no_overlap=unproven");
    } else {
        println!(
            "OVERLAP_REGRESSION_PASS stale_release_preserves_successor=true eventual_signals=true"
        );
    }
}
