use super::*;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::{Arc, Barrier};

fn start(dir: &Path, child: bool) -> (Lease, Counts) {
    let (lease, counts) =
        Lease::begin(dir.to_owned(), &uuid::Uuid::new_v4().to_string(), child).unwrap();
    (lease, counts.expect("successful registration"))
}

#[test]
fn idle_owner_remembers_short_lived_peers_and_independent_role_peaks() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path();
    let (mut idle, at_start) = start(dir, false);
    assert_eq!(
        at_start,
        Counts {
            total: 1,
            root: 1,
            child: 0
        }
    );
    {
        let (_root, counts) = start(dir, false);
        assert_eq!(counts.total, 2);
    }
    {
        let (_child, counts) = start(dir, true);
        assert_eq!(
            counts,
            Counts {
                total: 2,
                root: 1,
                child: 1
            }
        );
    }
    // No activity or polling on idle between peer join and departure.
    assert_eq!(
        idle.finish().unwrap(),
        Counts {
            total: 2,
            root: 2,
            child: 1
        }
    );
    let (_fresh, counts) = start(dir, false);
    assert_eq!(counts.total, 1);
}

#[test]
fn live_leases_never_expire_and_fresh_stale_markers_never_count() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path();
    let (mut old, _) = start(dir, false);
    let ancient = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(100);
    old.file.as_ref().unwrap().set_modified(ancient).unwrap();
    let stale = dir.join(format!("{}.lease", uuid::Uuid::new_v4()));
    std::fs::write(&stale, "fresh but unlocked").unwrap();
    let mut snapshot = read_snapshot(dir).unwrap();
    snapshot.insert(
        stale.file_name().unwrap().to_string_lossy().into_owned(),
        Record {
            child: true,
            peak: Counts {
                total: 1,
                root: 0,
                child: 1,
            },
        }
        .value(),
    );
    write_snapshot(dir, &snapshot).unwrap();
    std::fs::write(dir.join("legacy.active"), "1").unwrap();
    std::fs::write(dir.join("not-a-uuid.lease"), "1").unwrap();
    std::fs::create_dir(dir.join(format!("{}.lease", uuid::Uuid::new_v4()))).unwrap();
    let (mut peer, counts) = start(dir, true);
    assert_eq!(counts.total, 2);
    assert!(!stale.exists());
    assert!(old.path.exists());
    assert_eq!(peer.finish().unwrap().total, 2);
    assert_eq!(old.finish().unwrap().total, 2);
}

#[test]
fn concurrent_same_process_joins_are_serialized_and_update_all_peaks() {
    const PARTICIPANTS: usize = 12;
    let home = tempfile::tempdir().unwrap();
    let joined = Arc::new(Barrier::new(PARTICIPANTS));
    let go = Arc::new(Barrier::new(PARTICIPANTS));
    let mut threads = Vec::new();
    for index in 0..PARTICIPANTS {
        let dir = home.path().to_owned();
        let joined = joined.clone();
        let go = go.clone();
        threads.push(std::thread::spawn(move || {
            go.wait();
            let (mut lease, counts) = start(&dir, index % 2 == 0);
            joined.wait();
            let peak = lease.finish().unwrap();
            (counts.total, peak)
        }));
    }
    let mut starts = Vec::new();
    for thread in threads {
        let (count, peak) = thread.join().unwrap();
        starts.push(count);
        assert_eq!(
            peak,
            Counts {
                total: 12,
                root: 6,
                child: 6
            }
        );
    }
    starts.sort_unstable();
    assert_eq!(starts, (1..=PARTICIPANTS as u32).collect::<Vec<_>>());
}

#[test]
fn failed_atomic_publication_invalidates_idle_peers_even_after_failed_owner_exits() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path();
    let (mut owner, _) = start(dir, false);
    let before = std::fs::read(dir.join("registry.json")).unwrap();
    std::fs::create_dir(dir.join("registry.pending")).unwrap();
    let (mut failed, counts) =
        Lease::begin(dir.to_owned(), &uuid::Uuid::new_v4().to_string(), true).unwrap();
    assert!(counts.is_none());
    assert_eq!(std::fs::read(dir.join("registry.json")).unwrap(), before);
    std::fs::remove_dir(dir.join("registry.pending")).unwrap();
    assert!(failed.finish().is_err());
    assert!(
        failed.path.exists(),
        "retain degraded epoch marker after failed owner exits"
    );
    let (mut during_degraded, counts) =
        Lease::begin(dir.to_owned(), &uuid::Uuid::new_v4().to_string(), true).unwrap();
    assert!(
        counts.is_none(),
        "joining a degraded epoch must not produce trusted counts"
    );
    assert!(
        owner.finish().is_err(),
        "idle survivor must not claim peak=1"
    );
    assert!(during_degraded.finish().is_err());
    let (_fresh, counts) = start(dir, false);
    assert_eq!(
        counts.total, 1,
        "a quiet point recovers trustworthy accounting"
    );
}

#[test]
fn finish_releases_lease_even_when_registry_is_unavailable() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path();
    let (mut owner, _) = start(dir, false);
    std::fs::write(dir.join("registry.json"), "corrupt").unwrap();
    assert!(owner.finish().is_err());
    assert!(owner.file.is_none());
    assert!(
        owner.path.exists(),
        "retain evidence of degraded accounting until a quiet point"
    );
}

#[test]
fn clean_finish_unlocks_even_if_a_fork_inherited_the_file_description() {
    let home = tempfile::tempdir().unwrap();
    let (mut owner, _) = start(home.path(), false);
    // try_clone shares the open file description just as fork does. Force an
    // unavailable finish so its marker remains available for lock probing.
    let inherited = owner.file.as_ref().unwrap().try_clone().unwrap();
    std::fs::write(home.path().join("registry.json"), "corrupt").unwrap();
    assert!(owner.finish().is_err());
    let probe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&owner.path)
        .unwrap();
    assert!(
        probe.try_lock().is_ok(),
        "explicit finish must unlock, not just close its own fd"
    );
    drop(probe);
    let (_fresh, counts) = start(home.path(), false);
    assert_eq!(counts.total, 1);
    drop(inherited);
}

// Invoke this exact test in subprocesses. Pipes synchronize transitions, so no
// wall-clock sleeps or assumptions about scheduling are used in crash tests.
#[test]
fn lease_subprocess_helper() {
    let Some(dir) = std::env::var_os("JCODE_TEST_CONCURRENCY_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let mut command = [0];
    std::io::stdin().read_exact(&mut command).unwrap();
    if std::env::var_os("JCODE_TEST_REGISTRY_CRASH").is_some() {
        let _registry = lock_registry(&dir).unwrap();
        std::fs::write(dir.join("registry.pending"), "interrupted publication").unwrap();
        println!("\nCONCURRENCY_READY 0");
        std::io::stdout().flush().unwrap();
        let _ = std::io::stdin().read_exact(&mut command);
        return;
    }
    let (mut lease, counts) = start(&dir, true);
    println!("\nCONCURRENCY_READY {}", counts.total);
    std::io::stdout().flush().unwrap();
    // The lease remains held until a command, hard kill, or the parent's death.
    let _ = std::io::stdin().read_exact(&mut command);
    println!("\nCONCURRENCY_FINISHED {}", lease.finish().unwrap().total);
}

struct Process {
    child: Child,
    output: BufReader<ChildStdout>,
}

impl Process {
    fn spawn(dir: &Path, registry_crash: bool) -> Self {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "concurrency::tests::lease_subprocess_helper",
                "--nocapture",
            ])
            .env("JCODE_TEST_CONCURRENCY_DIR", dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if registry_crash {
            command.env("JCODE_TEST_REGISTRY_CRASH", "1");
        }
        let mut child = command.spawn().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self { child, output }
    }

    fn command(&mut self) {
        self.child.stdin.as_mut().unwrap().write_all(b"x").unwrap();
    }

    fn read(&mut self, marker: &str) -> u32 {
        loop {
            let mut line = String::new();
            assert_ne!(
                self.output.read_line(&mut line).unwrap(),
                0,
                "child exited without {marker}"
            );
            if let Some((_, number)) = line.split_once(marker) {
                return number.trim().parse().unwrap();
            }
        }
    }

    fn kill(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn cross_process_kill_prunes_owner_but_preserves_survivor_peak() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path();
    let (mut survivor, _) = start(dir, false);
    let mut process = Process::spawn(dir, false);
    process.command();
    assert_eq!(process.read("CONCURRENCY_READY"), 2);
    process.kill();
    let (mut newcomer, counts) = start(dir, false);
    assert_eq!(
        counts,
        Counts {
            total: 2,
            root: 2,
            child: 0
        }
    );
    assert_eq!(newcomer.finish().unwrap().total, 2);
    assert_eq!(
        survivor.finish().unwrap(),
        Counts {
            total: 2,
            root: 2,
            child: 1
        }
    );
}

#[test]
fn cross_process_start_races_have_unique_counts_and_all_owners_get_peak() {
    const PARTICIPANTS: usize = 6;
    let home = tempfile::tempdir().unwrap();
    let mut processes = (0..PARTICIPANTS)
        .map(|_| Process::spawn(home.path(), false))
        .collect::<Vec<_>>();
    for process in &mut processes {
        process.command();
    }
    let mut starts = processes
        .iter_mut()
        .map(|p| p.read("CONCURRENCY_READY"))
        .collect::<Vec<_>>();
    starts.sort_unstable();
    assert_eq!(starts, (1..=PARTICIPANTS as u32).collect::<Vec<_>>());
    for process in &mut processes {
        process.command();
    }
    for process in &mut processes {
        assert_eq!(process.read("CONCURRENCY_FINISHED"), PARTICIPANTS as u32);
        assert!(process.child.wait().unwrap().success());
    }
    let (_fresh, counts) = start(home.path(), false);
    assert_eq!(counts.total, 1);
}

#[test]
fn killed_registry_writer_releases_global_lock_and_pending_write_is_ignored() {
    let home = tempfile::tempdir().unwrap();
    let (mut survivor, _) = start(home.path(), false);
    let mut process = Process::spawn(home.path(), true);
    process.command();
    assert_eq!(process.read("CONCURRENCY_READY"), 0);
    process.kill();
    let (_next, counts) = start(home.path(), true);
    assert_eq!(counts.total, 2);
    assert_eq!(survivor.finish().unwrap().total, 2);
}
