#![cfg(all(windows, feature = "test-support"))]
use jcode_base::mcp::{McpClient, McpServerConfig};
use std::{
    collections::HashMap,
    ffi::c_void,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    time::Duration,
};
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn DuplicateHandle(
        source: *mut c_void,
        handle: *mut c_void,
        target: *mut c_void,
        duplicate: *mut *mut c_void,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn GetHandleInformation(handle: *mut c_void, flags: *mut u32) -> i32;
    fn QueryInformationJobObject(
        job: *mut c_void,
        class: i32,
        info: *mut c_void,
        size: u32,
        returned: *mut u32,
    ) -> i32;
}
#[repr(C)]
#[derive(Default)]
struct Accounting {
    user: i64,
    kernel: i64,
    perioduser: i64,
    periodkernel: i64,
    faults: u32,
    total: u32,
    active: u32,
    terminated: u32,
}
fn retain_job(raw: usize) -> OwnedHandle {
    let mut limits = [0u8; 144];
    assert_ne!(
        unsafe {
            QueryInformationJobObject(
                raw as _,
                9,
                limits.as_mut_ptr().cast(),
                144,
                std::ptr::null_mut(),
            )
        },
        0
    );
    assert_eq!(
        u32::from_ne_bytes(limits[16..20].try_into().unwrap()) & 0x2000,
        0,
        "natural fixtures must never enable kill-on-close"
    );
    let mut duplicate = std::ptr::null_mut();
    assert_ne!(
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                raw as _,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                2,
            )
        },
        0
    );
    unsafe { OwnedHandle::from_raw_handle(duplicate) }
}
async fn drained(job: &OwnedHandle) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut info = Accounting::default();
            assert_ne!(
                unsafe {
                    QueryInformationJobObject(
                        job.as_raw_handle(),
                        1,
                        (&mut info as *mut Accounting).cast(),
                        std::mem::size_of::<Accounting>() as u32,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            if info.active == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixtures must exit naturally");
}
fn config(record: &std::path::Path, mode: &str) -> McpServerConfig {
    McpServerConfig {
        command: "C:\\Python314\\python.exe".into(),
        args: vec![
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/natural_mcp_fixture.py").into(),
            record.display().to_string(),
            mode.into(),
        ],
        env: HashMap::new(),
        shared: false,
        transport: None,
        url: None,
        headers: HashMap::new(),
        enabled: None,
        disabled: None,
        timeout_secs: Some(3),
    }
}
#[tokio::test]
async fn actual_client_echo_eof_and_natural_job_drain() {
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("echo.jsonl");
    let mut client =
        McpClient::connect_natural_test("natural-echo".into(), &config(&record, "normal"))
            .await
            .unwrap();
    let (root, broker) = client.test_owned_process_ids().unwrap();
    let identity: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&record)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(identity["pid"].as_u64(), Some(root as u64));
    assert_eq!(identity["parent"].as_u64(), Some(broker as u64));
    let job = retain_job(client.test_owned_job_handle().unwrap());
    let handle = client.handle();
    let response = handle
        .request("fixture/echo", Some(serde_json::json!({"value":17})))
        .await
        .unwrap();
    assert_eq!(response.result.unwrap()["value"], 17);
    handle.request("fixture/exit", None).await.unwrap();
    drained(&job).await;
    assert!(
        handle.request("fixture/echo", None).await.is_err(),
        "EOF must close pending transport"
    );
    client.shutdown().await.unwrap();
    assert!(
        std::fs::read_to_string(record)
            .unwrap()
            .contains("natural_exit")
    );
}
#[tokio::test]
async fn cancelled_shutdown_closes_guard_without_terminating_fixture() {
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("cancel.jsonl");
    let mut client =
        McpClient::connect_natural_test("natural-cancel".into(), &config(&record, "normal"))
            .await
            .unwrap();
    let raw = client.test_owned_job_handle().unwrap();
    let job = retain_job(raw);
    assert!(
        tokio::time::timeout(Duration::from_millis(1), client.shutdown())
            .await
            .is_err()
    );
    assert_eq!(client.test_owned_job_handle(), Some(raw));
    assert!(client.cleanup_pending());
    assert!(client.handle().is_closed());
    let mut flags = 0;
    assert_eq!(
        unsafe { GetHandleInformation(raw as _, &mut flags) },
        1,
        "cancelled future must retain owner handle"
    );
    drained(&job).await;
    let text = std::fs::read_to_string(record).unwrap();
    assert!(text.contains("shutdown_received") && text.contains("natural_exit"));
    client.shutdown().await.unwrap();
    assert!(!client.cleanup_pending());
}
#[tokio::test]
async fn actual_initialization_error_releases_natural_fixture() {
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("failure.jsonl");
    assert!(
        McpClient::connect_natural_test(
            "natural-failure".into(),
            &config(&record, "initialize_error")
        )
        .await
        .is_err()
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(&record)
                .unwrap_or_default()
                .contains("natural_exit")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn unconfirmed_shutdown_retains_owner_until_retry_confirms_drain() {
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("slow.jsonl");
    let mut client =
        McpClient::connect_natural_test("natural-slow".into(), &config(&record, "slow_shutdown"))
            .await
            .unwrap();
    let raw = client.test_owned_job_handle().unwrap();
    let job = retain_job(raw);
    let error = client.shutdown().await.unwrap_err();
    assert!(format!("{error:#}").contains("shutdown unconfirmed"));
    assert!(client.cleanup_pending());
    assert!(client.handle().is_closed());
    assert_eq!(client.test_owned_job_handle(), Some(raw));
    drained(&job).await;
    client.shutdown().await.unwrap();
    assert!(!client.cleanup_pending());
    assert!(
        std::fs::read_to_string(record)
            .unwrap()
            .contains("natural_exit")
    );
}

#[tokio::test]
async fn manager_and_pool_retain_failed_owner_and_block_reconnect() {
    use jcode_base::mcp::{McpConfig, McpManager, SharedMcpPool};
    for shared in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let record = temp.path().join("slow-owner.jsonl");
        let configured = config(&record, "slow_shutdown");
        let client = McpClient::connect_natural_test("retained".into(), &configured)
            .await
            .unwrap();
        let job = retain_job(client.test_owned_job_handle().unwrap());
        if shared {
            let pool = SharedMcpPool::new(McpConfig::default());
            pool.test_adopt_natural_client("retained".into(), client)
                .await;
            assert!(pool.disconnect_server("retained").await.is_err());
            let error = pool
                .connect_server("retained", &configured)
                .await
                .unwrap_err();
            assert!(format!("{error:#}").contains("cleanup is unconfirmed"));
            drained(&job).await;
            pool.disconnect_server("retained").await.unwrap();
        } else {
            let manager = McpManager::new();
            manager
                .test_adopt_natural_client("retained".into(), client)
                .await;
            assert!(manager.disconnect("retained").await.is_err());
            let error = manager.connect("retained", &configured).await.unwrap_err();
            assert!(format!("{error:#}").contains("cleanup is unconfirmed"));
            drained(&job).await;
            manager.disconnect("retained").await.unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&record)
                .unwrap()
                .lines()
                .filter(|line| line.contains("started"))
                .count(),
            1
        );
    }
}
