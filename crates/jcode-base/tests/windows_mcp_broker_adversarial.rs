#![cfg(all(windows, feature = "test-support"))]
use jcode_base::mcp::{McpClient, McpServerConfig};
use std::{
    collections::HashMap,
    ffi::c_void,
    future::Future,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
    time::Duration,
};
type Handle = *mut c_void;
#[repr(C)]
struct Startup {
    cb: u32,
    reserved: *mut u16,
    desktop: *mut u16,
    title: *mut u16,
    x: u32,
    y: u32,
    xs: u32,
    ys: u32,
    xc: u32,
    yc: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved2: u16,
    reservedptr: *mut u8,
    stdin: Handle,
    stdout: Handle,
    stderr: Handle,
}
#[repr(C)]
struct Process {
    process: Handle,
    thread: Handle,
    pid: u32,
    tid: u32,
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
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn DuplicateHandle(
        source: Handle,
        handle: Handle,
        target: Handle,
        duplicate: *mut Handle,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn GetHandleInformation(handle: Handle, flags: *mut u32) -> i32;
    fn GetProcessId(process: Handle) -> u32;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
    fn QueryInformationJobObject(
        job: Handle,
        class: i32,
        info: Handle,
        size: u32,
        returned: *mut u32,
    ) -> i32;
    fn CreateEventW(attrs: Handle, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn SetEvent(event: Handle) -> i32;
    fn CreateProcessW(
        app: *const u16,
        line: *mut u16,
        processattrs: Handle,
        threadattrs: Handle,
        inherit: i32,
        flags: u32,
        env: Handle,
        cwd: *const u16,
        startup: *mut Startup,
        process: *mut Process,
    ) -> i32;
    fn IsProcessInJob(process: Handle, job: Handle, result: *mut i32) -> i32;
}
struct Custody {
    job: OwnedHandle,
    root: OwnedHandle,
    broker: OwnedHandle,
    original_job: usize,
    root_id: u32,
    broker_id: u32,
}
static CUSTODY: OnceLock<Mutex<Option<Custody>>> = OnceLock::new();
fn capture() -> &'static Mutex<Option<Custody>> {
    CUSTODY.get_or_init(|| Mutex::new(None))
}
fn duplicate(raw: usize) -> OwnedHandle {
    let mut target = std::ptr::null_mut();
    assert_ne!(
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                raw as _,
                GetCurrentProcess(),
                &mut target,
                0,
                0,
                2,
            )
        },
        0
    );
    unsafe { OwnedHandle::from_raw_handle(target) }
}
fn observe(job: usize, root: usize, broker: usize, root_id: u32, broker_id: u32) {
    let job_handle = duplicate(job);
    let mut limits = [0u8; 144];
    assert_ne!(
        unsafe {
            QueryInformationJobObject(
                job_handle.as_raw_handle(),
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
        0
    );
    let custody = Custody {
        job: job_handle,
        root: duplicate(root),
        broker: duplicate(broker),
        original_job: job,
        root_id,
        broker_id,
    };
    assert_eq!(
        unsafe { GetProcessId(custody.root.as_raw_handle()) },
        root_id
    );
    assert_eq!(
        unsafe { GetProcessId(custody.broker.as_raw_handle()) },
        broker_id
    );
    assert!(capture().lock().unwrap().replace(custody).is_none());
}
async fn drain(custody: &Custody) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let mut info = Accounting::default();
            assert_ne!(
                unsafe {
                    QueryInformationJobObject(
                        custody.job.as_raw_handle(),
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
    .expect("natural Job drain");
    for process in [&custody.root, &custody.broker] {
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 0) },
            0
        );
        let mut code = 0;
        assert_ne!(
            unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) },
            0
        );
        assert_eq!(code, 0);
    }
    println!(
        "CUSTODY root={} broker={} final_job_active=0 natural_exit_codes=0",
        custody.root_id, custody.broker_id
    );
}
struct Hooks;
impl Hooks {
    fn start() -> Self {
        assert!(capture().lock().unwrap().is_none());
        McpClient::test_set_native_observer(Some(observe));
        Self
    }
}
impl Drop for Hooks {
    fn drop(&mut self) {
        McpClient::test_set_native_observer(None);
        McpClient::test_set_spawn_window_observer(None);
        McpClient::test_set_shutdown_observer(None);
    }
}
fn fixture() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/natural_mcp_fixture.py").into()
}
fn config(record: &std::path::Path, mode: &str) -> McpServerConfig {
    McpServerConfig {
        command: "C:\\Python314\\python.exe".into(),
        args: vec![fixture(), record.display().to_string(), mode.into()],
        env: HashMap::new(),
        shared: false,
        transport: None,
        url: None,
        headers: HashMap::new(),
        enabled: None,
        disabled: None,
        timeout_secs: Some(5),
    }
}
fn rows(record: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(record)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
struct Broad {
    event: OwnedHandle,
    process: Option<OwnedHandle>,
    pid: u32,
    name: String,
    record: std::path::PathBuf,
}
static BROAD: OnceLock<Mutex<Option<Broad>>> = OnceLock::new();
fn broad() -> &'static Mutex<Option<Broad>> {
    BROAD.get_or_init(|| Mutex::new(None))
}
fn spawn_broad() {
    let mut state = broad().lock().unwrap();
    let state = state.as_mut().unwrap();
    assert!(state.process.is_none());
    let app = wide("C:\\Python314\\python.exe");
    let mut line = wide(&format!(
        "C:\\Python314\\python.exe {} {} broad {}",
        fixture(),
        state.record.display(),
        state.name
    ));
    let mut startup: Startup = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<Startup>() as u32;
    let mut info: Process = unsafe { std::mem::zeroed() };
    assert_ne!(
        unsafe {
            CreateProcessW(
                app.as_ptr(),
                line.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                0x08000000,
                std::ptr::null_mut(),
                std::ptr::null(),
                &mut startup,
                &mut info,
            )
        },
        0
    );
    drop(unsafe { OwnedHandle::from_raw_handle(info.thread) });
    state.process = Some(unsafe { OwnedHandle::from_raw_handle(info.process) });
    state.pid = info.pid;
}
struct BroadRelease;
impl Drop for BroadRelease {
    fn drop(&mut self) {
        if let Some(state) = broad().lock().unwrap().take() {
            unsafe {
                SetEvent(state.event.as_raw_handle());
            }
            if let Some(process) = state.process {
                unsafe {
                    WaitForSingleObject(process.as_raw_handle(), 10000);
                }
            }
        }
    }
}
#[tokio::test]
async fn pending_eof_fails_closed_while_unrelated_inheritance_process_is_alive() {
    assert_eq!(
        std::env::var("JCODE_SYNTHETIC_API_KEY").unwrap(),
        "parent-synthetic"
    );
    let _hooks = Hooks::start();
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("pending.jsonl");
    let name = format!("Local\\jcode-actual-broad-{}", std::process::id());
    let encoded = wide(&name);
    let event = unsafe { CreateEventW(std::ptr::null_mut(), 1, 0, encoded.as_ptr()) };
    assert!(!event.is_null());
    *broad().lock().unwrap() = Some(Broad {
        event: unsafe { OwnedHandle::from_raw_handle(event) },
        process: None,
        pid: 0,
        name,
        record: temp.path().join("broad.jsonl"),
    });
    let release = BroadRelease;
    McpClient::test_set_spawn_window_observer(Some(spawn_broad));
    let mut client =
        McpClient::connect_natural_test("adversarial-eof".into(), &config(&record, "normal"))
            .await
            .unwrap();
    McpClient::test_set_spawn_window_observer(None);
    let custody = capture().lock().unwrap().take().unwrap();
    let handle = client.handle();
    assert!(!handle.is_closed());
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        handle.request("fixture/pending", None),
    )
    .await
    .expect("EOF must beat 5s request deadline")
    .unwrap_err();
    assert!(format!("{error:#}").contains("connection closed"));
    assert!(handle.is_closed());
    assert_eq!(handle.test_pending_count().await, 0);
    let late = handle.request("fixture/echo", None).await.unwrap_err();
    assert!(format!("{late:#}").contains("connection closed"));
    drain(&custody).await;
    {
        let state = broad().lock().unwrap();
        let state = state.as_ref().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(state.process.as_ref().unwrap().as_raw_handle(), 0) },
            258
        );
        let mut member = 0;
        assert_ne!(
            unsafe {
                IsProcessInJob(
                    state.process.as_ref().unwrap().as_raw_handle(),
                    custody.job.as_raw_handle(),
                    &mut member,
                )
            },
            0
        );
        assert_eq!(member, 0);
        println!(
            "UNRELATED broad_pid={} remained_alive_after_actual_EOF=true",
            state.pid
        );
    }
    let data = rows(&record);
    assert!(
        data.iter()
            .any(|row| row["stage"] == "pending_request_received")
    );
    assert!(data[0]["synthetic"].is_null());
    println!(
        "PENDING_EOF records={} error={error:#}",
        serde_json::to_string(&data).unwrap()
    );
    client.shutdown().await.unwrap();
    drop(release);
}
#[tokio::test]
async fn initialization_failure_drains_exact_root_broker_and_job() {
    let _hooks = Hooks::start();
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("init-error.jsonl");
    let error = match McpClient::connect_natural_test(
        "adversarial-init".into(),
        &config(&record, "initialize_error"),
    )
    .await
    {
        Ok(_) => panic!("must fail"),
        Err(error) => error,
    };
    assert!(format!("{error:#}").contains("synthetic initialization failure"));
    let custody = capture().lock().unwrap().take().unwrap();
    drain(&custody).await;
    assert!(
        rows(&record)
            .iter()
            .any(|row| row["stage"] == "natural_exit")
    );
    println!(
        "INIT_FAILURE records={}",
        std::fs::read_to_string(record).unwrap()
    );
}
static GRACEFUL: AtomicBool = AtomicBool::new(false);
fn mark_graceful() {
    GRACEFUL.store(true, Ordering::SeqCst);
}
#[tokio::test]
async fn shutdown_cancellation_is_injected_at_verified_graceful_wait() {
    let _hooks = Hooks::start();
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("cancel.jsonl");
    let mut client =
        McpClient::connect_natural_test("adversarial-cancel".into(), &config(&record, "normal"))
            .await
            .unwrap();
    let custody = capture().lock().unwrap().take().unwrap();
    GRACEFUL.store(false, Ordering::SeqCst);
    McpClient::test_set_shutdown_observer(Some(mark_graceful));
    let mut future = Box::pin(client.shutdown());
    std::future::poll_fn(|context| match future.as_mut().poll(context) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("shutdown completed before cancellation point"),
    })
    .await;
    assert!(GRACEFUL.load(Ordering::SeqCst));
    drop(future);
    assert_eq!(client.test_owned_job_handle(), Some(custody.original_job));
    assert!(client.cleanup_pending());
    let mut flags = 0;
    assert_eq!(
        unsafe { GetHandleInformation(custody.original_job as _, &mut flags) },
        1
    );
    drain(&custody).await;
    let data = rows(&record);
    assert!(data.iter().any(|row| row["stage"] == "shutdown_received"));
    assert!(data.iter().any(|row| row["stage"] == "natural_exit"));
    client.shutdown().await.unwrap();
    assert!(!client.cleanup_pending());
    println!("CANCEL records={}", serde_json::to_string(&data).unwrap());
}
#[tokio::test]
async fn explicit_synthetic_environment_is_opted_in() {
    assert_eq!(
        std::env::var("JCODE_SYNTHETIC_API_KEY").unwrap(),
        "parent-synthetic"
    );
    let _hooks = Hooks::start();
    let temp = tempfile::tempdir().unwrap();
    let record = temp.path().join("env.jsonl");
    let mut configured = config(&record, "normal");
    configured.env.insert(
        "JCODE_SYNTHETIC_API_KEY".into(),
        "explicit-synthetic".into(),
    );
    let mut client = McpClient::connect_natural_test("adversarial-env".into(), &configured)
        .await
        .unwrap();
    let custody = capture().lock().unwrap().take().unwrap();
    assert_eq!(rows(&record)[0]["synthetic"], "explicit-synthetic");
    client.handle().request("fixture/exit", None).await.unwrap();
    drain(&custody).await;
    client.shutdown().await.unwrap();
    println!("ENV records={}", std::fs::read_to_string(record).unwrap());
}
