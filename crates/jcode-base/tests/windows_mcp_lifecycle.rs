#![cfg(windows)]

use jcode_base::mcp::{McpClient, McpConfig, McpManager, McpServerConfig};
use std::collections::HashMap;
use std::ffi::c_void;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

#[tokio::test]
#[ignore = "run only under the dedicated fixture controller job"]
async fn owned_mcp_disconnect_stops_wrapper_and_descendant() {
    let fixture = std::env::var("JCODE_MCP_TEST_FIXTURE").expect("fixture path");
    let python = std::env::var("JCODE_MCP_TEST_PYTHON").expect("python executable");
    assert_eq!(
        std::env::var("JCODE_MCP_TEST_CONTROLLER").as_deref(),
        Ok("1")
    );
    let temp = tempfile::tempdir().expect("private fixture directory");
    let record = temp.path().join("starts.jsonl");
    let config = McpServerConfig {
        command: python,
        args: vec![fixture, "serve-in-job".into(), record.display().to_string()],
        env: HashMap::new(),
        shared: false,
        transport: None,
        url: None,
        headers: HashMap::new(),
        enabled: None,
        disabled: None,
        timeout_secs: Some(10),
    };
    let manager = McpManager::with_config(McpConfig::default());
    manager
        .connect("fixture", &config)
        .await
        .expect("connect fixture");
    let text = std::fs::read_to_string(&record).expect("fixture identity record");
    let rows: Vec<_> = text.lines().collect();
    assert_eq!(rows.len(), 1, "exactly one fixture wrapper");
    let row: serde_json::Value = serde_json::from_str(rows[0]).expect("fixture record");
    let wrapper_pid = row["wrapper"].as_u64().expect("wrapper pid") as u32;
    let child_pid = row["grandchild"].as_u64().expect("grandchild pid") as u32;
    unsafe {
        let wrapper = OpenProcess(0x0010_0000, 0, wrapper_pid);
        let child = OpenProcess(0x0010_0000, 0, child_pid);
        assert!(
            !wrapper.is_null() && !child.is_null(),
            "retain fixture handles"
        );
        assert_eq!(
            WaitForSingleObject(child, 0),
            258,
            "negative control: descendant is alive"
        );
        manager
            .disconnect("fixture")
            .await
            .expect("disconnect fixture");
        let wrapper_exit = WaitForSingleObject(wrapper, 5000);
        let child_exit = WaitForSingleObject(child, 5000);
        CloseHandle(wrapper as *mut c_void);
        CloseHandle(child as *mut c_void);
        assert_eq!(wrapper_exit, 0, "wrapper must exit");
        assert_eq!(
            child_exit, 0,
            "descendant must exit before controller job teardown"
        );
    }
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "run only under the dedicated fixture controller job"]
async fn shutdown_completes_when_fixture_stops_reading() {
    assert_eq!(
        std::env::var("JCODE_MCP_TEST_CONTROLLER").ok().as_deref(),
        Some("1")
    );
    let temp = tempfile::tempdir().unwrap();
    let config = McpServerConfig {
        command: std::env::var("JCODE_MCP_TEST_PYTHON").unwrap(),
        args: vec![
            std::env::var("JCODE_MCP_TEST_FIXTURE").unwrap(),
            "serve-in-job".into(),
            temp.path().join("starts.jsonl").display().to_string(),
            "block-after-init".into(),
        ],
        env: HashMap::new(),
        shared: false,
        transport: None,
        url: None,
        headers: HashMap::new(),
        enabled: None,
        disabled: None,
        timeout_secs: Some(20),
    };
    let mut client = McpClient::connect("blocked-fixture".into(), &config)
        .await
        .unwrap();
    let row: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(temp.path().join("starts.jsonl"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    let wrapper = unsafe { OpenProcess(0x0010_0000, 0, row["wrapper"].as_u64().unwrap() as u32) };
    let child = unsafe { OpenProcess(0x0010_0000, 0, row["grandchild"].as_u64().unwrap() as u32) };
    assert!(
        !wrapper.is_null() && !child.is_null(),
        "retain fixture handles"
    );
    assert_eq!(
        unsafe { WaitForSingleObject(child, 0) },
        258,
        "descendant negative control"
    );
    let handle = client.handle();
    let mut requests = Vec::new();
    for _ in 0..40 {
        let handle = handle.clone();
        requests.push(tokio::spawn(async move {
            handle
                .request(
                    "fixture/blocked",
                    Some(serde_json::json!({"payload": "x".repeat(262144)})),
                )
                .await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while handle.test_writer_remaining_capacity() != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("writer queue must fill");
    assert!(
        requests.iter().all(|request| !request.is_finished()),
        "requests must remain blocked"
    );
    let result = tokio::time::timeout(std::time::Duration::from_secs(4), client.shutdown()).await;
    let wrapper_exit = unsafe { WaitForSingleObject(wrapper, 5000) };
    let child_exit = unsafe { WaitForSingleObject(child, 5000) };
    unsafe {
        CloseHandle(wrapper);
        CloseHandle(child);
    }
    drop(client);
    for request in requests {
        request.abort();
    }
    assert!(result.is_ok(), "shutdown blocked behind full writer queue");
    assert_eq!(
        wrapper_exit, 0,
        "wrapper must exit before outer job teardown"
    );
    assert_eq!(
        child_exit, 0,
        "descendant must exit before outer job teardown"
    );
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[ignore = "run only under the dedicated fixture controller job"]
async fn cancelled_shutdown_closes_owned_job() {
    assert_eq!(
        std::env::var("JCODE_MCP_TEST_CONTROLLER").ok().as_deref(),
        Some("1")
    );
    let temp = tempfile::tempdir().unwrap();
    let config = McpServerConfig {
        command: std::env::var("JCODE_MCP_TEST_PYTHON").unwrap(),
        args: vec![
            std::env::var("JCODE_MCP_TEST_FIXTURE").unwrap(),
            "serve-in-job".into(),
            temp.path().join("starts.jsonl").display().to_string(),
            "block-after-init".into(),
        ],
        env: HashMap::new(),
        shared: false,
        transport: None,
        url: None,
        headers: HashMap::new(),
        enabled: None,
        disabled: None,
        timeout_secs: Some(20),
    };
    let mut client = McpClient::connect("cancel-fixture".into(), &config)
        .await
        .unwrap();
    let row: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(temp.path().join("starts.jsonl"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    let wrapper = unsafe { OpenProcess(0x0010_0000, 0, row["wrapper"].as_u64().unwrap() as u32) };
    let child = unsafe { OpenProcess(0x0010_0000, 0, row["grandchild"].as_u64().unwrap() as u32) };
    assert!(!wrapper.is_null() && !child.is_null());
    assert_eq!(
        unsafe { WaitForSingleObject(child, 0) },
        258,
        "descendant negative control"
    );
    let result =
        tokio::time::timeout(std::time::Duration::from_millis(10), client.shutdown()).await;
    assert!(
        result.is_err(),
        "cancellation must occur during graceful wait"
    );
    let wrapper_exit = unsafe { WaitForSingleObject(wrapper, 5000) };
    let child_exit = unsafe { WaitForSingleObject(child, 5000) };
    unsafe {
        CloseHandle(wrapper);
        CloseHandle(child);
    }
    assert_eq!(wrapper_exit, 0, "cancelled shutdown must close wrapper job");
    assert_eq!(
        child_exit, 0,
        "cancelled shutdown must close descendant job"
    );
    drop(client);
}
#[tokio::test]
#[ignore = "run only under the dedicated fixture controller job"]
async fn shared_pool_release_and_reconnect_preserve_process_ownership() {
    use jcode_base::mcp::SharedMcpPool;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    assert_eq!(
        std::env::var("JCODE_MCP_TEST_CONTROLLER").as_deref(),
        Ok("1")
    );
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("starts.jsonl");
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
    let first = pool.acquire_handles("session-a").await;
    let second = pool.acquire_handles("session-b").await;
    assert!(first.contains_key("fixture") && second.contains_key("fixture"));
    assert_eq!(pool.ref_counts().await.get("fixture"), Some(&2));
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(&record)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(rows.len(), 1, "shared sessions must reuse one fixture");
    let retained = |row: &serde_json::Value, key: &str| {
        let raw = unsafe { OpenProcess(0x0010_0000, 0, row[key].as_u64().unwrap() as u32) };
        assert!(!raw.is_null());
        unsafe { OwnedHandle::from_raw_handle(raw) }
    };
    let wrapper1 = retained(&rows[0], "wrapper");
    let child1 = retained(&rows[0], "grandchild");
    assert_eq!(
        unsafe { WaitForSingleObject(child1.as_raw_handle(), 0) },
        258
    );
    pool.release_handles("session-a", &["fixture".into()]).await;
    assert_eq!(pool.ref_counts().await.get("fixture"), Some(&1));
    assert_eq!(
        unsafe { WaitForSingleObject(child1.as_raw_handle(), 0) },
        258
    );
    pool.release_handles("session-b", &["fixture".into()]).await;
    assert_eq!(pool.ref_counts().await.get("fixture"), Some(&0));
    assert_eq!(
        unsafe { WaitForSingleObject(child1.as_raw_handle(), 0) },
        258,
        "pool retains owner after leases release"
    );
    pool.disconnect_server("fixture").await.unwrap();
    assert_eq!(
        unsafe { WaitForSingleObject(wrapper1.as_raw_handle(), 5000) },
        0
    );
    assert_eq!(
        unsafe { WaitForSingleObject(child1.as_raw_handle(), 5000) },
        0
    );
    pool.connect_server("fixture", &config).await.unwrap();
    let rows: Vec<serde_json::Value> = std::fs::read_to_string(&record)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(rows.len(), 2, "reconnect must create a new fixture");
    let wrapper2 = retained(&rows[1], "wrapper");
    let child2 = retained(&rows[1], "grandchild");
    assert_eq!(
        unsafe { WaitForSingleObject(wrapper2.as_raw_handle(), 0) },
        258
    );
    assert_eq!(
        unsafe { WaitForSingleObject(child2.as_raw_handle(), 0) },
        258
    );
    assert_eq!(
        unsafe { WaitForSingleObject(wrapper1.as_raw_handle(), 0) },
        0
    );
    assert_eq!(unsafe { WaitForSingleObject(child1.as_raw_handle(), 0) }, 0);
    pool.disconnect_server("fixture").await.unwrap();
    assert_eq!(
        unsafe { WaitForSingleObject(wrapper2.as_raw_handle(), 5000) },
        0
    );
    assert_eq!(
        unsafe { WaitForSingleObject(child2.as_raw_handle(), 5000) },
        0
    );
}
#[tokio::test]
#[ignore = "run only under the dedicated crash witness controller"]
async fn controlled_mcp_manager_owner_exit_or_reconnect() {
    use std::io::Write;
    assert_eq!(
        std::env::var("JCODE_MCP_TEST_CONTROLLER").as_deref(),
        Ok("1")
    );
    let stage = std::env::var("JCODE_MCP_CRASH_STAGE").unwrap();
    let record = std::env::var("JCODE_MCP_CRASH_RECORD").unwrap();
    let config = McpServerConfig {
        command: std::env::var("JCODE_MCP_TEST_PYTHON").unwrap(),
        args: vec![
            std::env::var("JCODE_MCP_TEST_FIXTURE").unwrap(),
            "serve-in-job".into(),
            record.clone(),
        ],
        env: HashMap::new(),
        shared: false,
        transport: None,
        url: None,
        headers: HashMap::new(),
        enabled: None,
        disabled: None,
        timeout_secs: Some(10),
    };
    let manager = McpManager::with_config(McpConfig::default());
    manager.connect("fixture", &config).await.unwrap();
    let text = std::fs::read_to_string(&record).unwrap();
    let rows: Vec<_> = text.lines().collect();
    let expected = if stage == "crash" { 1 } else { 2 };
    assert_eq!(rows.len(), expected);
    let row: serde_json::Value = serde_json::from_str(rows[expected - 1]).unwrap();
    println!(
        "READY {} {}",
        row["wrapper"].as_u64().unwrap(),
        row["grandchild"].as_u64().unwrap()
    );
    std::io::stdout().flush().unwrap();
    let mut signal = String::new();
    std::io::stdin().read_line(&mut signal).unwrap();
    assert_eq!(signal.trim(), "GO");
    if stage == "crash" {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn ExitProcess(code: u32) -> !;
        }
        unsafe { ExitProcess(42) }
    }
    assert_eq!(stage, "reconnect");
    manager.disconnect("fixture").await.unwrap();
}
#[tokio::test]
#[ignore = "run only under the dedicated fixture controller job"]
async fn synthetic_sensitive_environment_requires_explicit_opt_in() {
    assert_eq!(
        std::env::var("JCODE_MCP_TEST_CONTROLLER").as_deref(),
        Ok("1")
    );
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("starts.jsonl");
    unsafe { std::env::set_var("JCODE_SYNTHETIC_API_KEY", "parent-synthetic-value") };
    let fixture = std::path::PathBuf::from(std::env::var("JCODE_MCP_TEST_FIXTURE").unwrap())
        .with_file_name("windows_mcp_sensitive_fixture.py");
    let mut config = McpServerConfig {
        command: std::env::var("JCODE_MCP_TEST_PYTHON").unwrap(),
        args: vec![
            fixture.display().to_string(),
            "serve-in-job".into(),
            record.display().to_string(),
        ],
        env: HashMap::new(),
        shared: false,
        transport: None,
        url: None,
        headers: HashMap::new(),
        enabled: None,
        disabled: None,
        timeout_secs: Some(10),
    };
    let manager = McpManager::with_config(McpConfig::default());
    manager.connect("fixture", &config).await.unwrap();
    let row: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&record)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert!(
        row["synthetic"].is_null(),
        "inherited sensitive synthetic value must be absent"
    );
    manager.disconnect("fixture").await.unwrap();
    config.env.insert(
        "JCODE_SYNTHETIC_API_KEY".into(),
        "explicit-synthetic-value".into(),
    );
    manager.connect("fixture", &config).await.unwrap();
    let rows: Vec<_> = std::fs::read_to_string(&record)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["synthetic"], "explicit-synthetic-value");
    manager.disconnect("fixture").await.unwrap();
    unsafe { std::env::remove_var("JCODE_SYNTHETIC_API_KEY") };
}
