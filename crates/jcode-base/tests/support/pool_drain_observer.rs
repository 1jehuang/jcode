use super::{FileTime, Generation, GetProcessTimes, IsProcessInJob, McpClient};
use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::sync::{Arc, Mutex};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

#[repr(C)]
#[derive(Default)]
struct Accounting {
    times: [i64; 4],
    faults: u32,
    total: u32,
    active: u32,
    terminated: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryInformationJobObject(
        job: *mut c_void,
        class: i32,
        info: *mut c_void,
        size: u32,
        returned: *mut u32,
    ) -> i32;
}

#[repr(C)]
struct ProcessList {
    assigned: u32,
    listed: u32,
    ids: [usize; 64],
}

struct Sample {
    generation: &'static str,
    unix_ms: u128,
    active: u32,
    total: u32,
    assigned: Option<u32>,
    listed: Option<u32>,
    list_ok: bool,
    truncated: Option<bool>,
    list_error: Option<i32>,
    listed_ids: Option<Vec<usize>>,
    states: [u32; 2],
}

struct SpawnSample {
    root_id: u32,
    creation_time: u64,
    unix_ms: u128,
    old_states: [u32; 2],
    successor_state: u32,
}

static SPAWNS: Mutex<Vec<SpawnSample>> = Mutex::new(Vec::new());

fn observe_spawn(job: usize, root: usize, _broker: usize, root_id: u32, _broker_id: u32) {
    let (_, generation) = GENERATION.lock().unwrap().as_ref().unwrap().clone();
    let raw = root as *mut c_void;
    let mut inside = 0;
    assert_ne!(
        unsafe { IsProcessInJob(raw, job as *mut c_void, &mut inside) },
        0
    );
    assert_eq!(inside, 1);
    let mut created = FileTime { low: 0, high: 0 };
    let mut exited = FileTime { low: 0, high: 0 };
    let mut kernel = FileTime { low: 0, high: 0 };
    let mut user = FileTime { low: 0, high: 0 };
    assert_ne!(
        unsafe { GetProcessTimes(raw, &mut created, &mut exited, &mut kernel, &mut user) },
        0
    );
    let old_states = [&generation.wrapper, &generation.descendant]
        .map(|handle| unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) });
    let successor_state = unsafe { WaitForSingleObject(raw, 0) };
    assert!(old_states.iter().all(|state| matches!(state, 0 | 258)));
    assert!(matches!(successor_state, 0 | 258));
    SPAWNS.lock().unwrap().push(SpawnSample {
        root_id,
        creation_time: ((created.high as u64) << 32) | created.low as u64,
        unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
        old_states,
        successor_state,
    });
}

static GENERATION: Mutex<Option<(&'static str, Arc<Generation>)>> = Mutex::new(None);
static SAMPLES: Mutex<Vec<Sample>> = Mutex::new(Vec::new());
static SNAPSHOTS: Mutex<Vec<(&'static str, u128, [u32; 2])>> = Mutex::new(Vec::new());

pub(super) fn record_snapshot(stage: &'static str, states: [u32; 2]) {
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    SNAPSHOTS.lock().unwrap().push((stage, unix_ms, states));
}

pub(super) fn assert_observed(expected: usize) {
    let samples = SAMPLES.lock().unwrap();
    assert_eq!(samples.len(), expected);
    assert!(samples.iter().all(|sample| sample.active == 0));
}

fn observe(job: usize) {
    let (name, generation) = GENERATION.lock().unwrap().as_ref().unwrap().clone();
    let mut accounting = Accounting::default();
    assert_ne!(
        unsafe {
            QueryInformationJobObject(
                job as *mut c_void,
                1,
                (&mut accounting as *mut Accounting).cast(),
                std::mem::size_of::<Accounting>() as u32,
                std::ptr::null_mut(),
            )
        },
        0
    );
    let mut process_list = ProcessList {
        assigned: 0,
        listed: 0,
        ids: [0; 64],
    };
    let list_ok = unsafe {
        QueryInformationJobObject(
            job as *mut c_void,
            3,
            (&mut process_list as *mut ProcessList).cast(),
            std::mem::size_of::<ProcessList>() as u32,
            std::ptr::null_mut(),
        )
    };
    let list_error = if list_ok == 0 {
        std::io::Error::last_os_error().raw_os_error()
    } else {
        None
    };
    let states = [&generation.wrapper, &generation.descendant]
        .map(|handle| unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) });
    assert!(states.iter().all(|state| matches!(state, 0 | 258)));
    SAMPLES.lock().unwrap().push(Sample {
        generation: name,
        unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
        active: accounting.active,
        total: accounting.total,
        assigned: (list_ok != 0).then_some(process_list.assigned),
        listed: (list_ok != 0).then_some(process_list.listed),
        list_ok: list_ok != 0,
        truncated: (list_ok != 0)
            .then_some(process_list.listed < process_list.assigned || process_list.listed > 64),
        list_error,
        listed_ids: (list_ok != 0)
            .then(|| process_list.ids[..(process_list.listed as usize).min(64)].to_vec()),
        states,
    });
}

pub(super) fn set_generation(name: &'static str, generation: Arc<Generation>) {
    *GENERATION.lock().unwrap() = Some((name, generation));
}

pub(super) fn assert_successor_identity(row: &serde_json::Value) {
    let spawns = SPAWNS.lock().unwrap();
    assert_eq!(spawns.len(), 1);
    assert_eq!(Some(spawns[0].root_id as u64), row["wrapper"].as_u64());
    assert_eq!(
        Some(spawns[0].creation_time),
        row["wrapper_identity"]["creation_time"].as_u64()
    );
}

pub(super) fn report(expected: usize) {
    assert_observed(expected);
    assert_eq!(SPAWNS.lock().unwrap().len(), 1);
    dump_samples();
}

fn dump_samples() {
    println!("OVERLAP_OBSERVATION snapshots=sequential_non_atomic strict_no_overlap=unproven");
    let samples = SAMPLES.lock().unwrap_or_else(|error| error.into_inner());
    for sample in samples.iter() {
        println!(
            "OVERLAP_ACCOUNTING_ZERO generation={} unix_ms={} active={} total={} pid_list_ok={} assigned={:?} listed={:?} truncated={:?} list_error={:?} listed_ids={:?} entries=partial_if_truncated_counts_unknown_if_failed wrapper={} descendant={}",
            sample.generation,
            sample.unix_ms,
            sample.active,
            sample.total,
            sample.list_ok,
            sample.assigned,
            sample.listed,
            sample.truncated,
            sample.list_error,
            sample.listed_ids,
            sample.states[0],
            sample.states[1]
        );
    }
    for sample in SPAWNS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
    {
        println!(
            "OVERLAP_NATIVE_SPAWN_RETURN successor_pid={} creation_time={} unix_ms={} old_wrapper={} old_descendant={} successor_state={} unsignaled_does_not_prove_execution=true",
            sample.root_id,
            sample.creation_time,
            sample.unix_ms,
            sample.old_states[0],
            sample.old_states[1],
            sample.successor_state
        );
    }
    for (stage, unix_ms, states) in SNAPSHOTS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
    {
        println!(
            "OVERLAP_SNAPSHOT stage={stage} unix_ms={unix_ms} wrapper={} descendant={}",
            states[0], states[1]
        );
    }
}

pub(super) struct ObserverGuard;

pub(super) fn install() -> ObserverGuard {
    SAMPLES.lock().unwrap().clear();
    SPAWNS.lock().unwrap().clear();
    SNAPSHOTS.lock().unwrap().clear();
    McpClient::test_set_drain_observer(Some(observe));
    McpClient::test_set_native_observer(Some(observe_spawn));
    ObserverGuard
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            dump_samples();
        }
        McpClient::test_set_drain_observer(None);
        McpClient::test_set_native_observer(None);
        *GENERATION.lock().unwrap() = None;
    }
}
