// Included by both notification modules so the same lifecycle contract exercises
// each real helper. Only the subprocess gets a private PATH, never the test runner.
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const FIXTURE_ENV: &str = "JCODE_NOTIFICATION_REAP_TEST_DIR";

#[test]
#[ignore = "entry point launched by the notification lifecycle tests"]
fn notification_probe() {
    let dir = PathBuf::from(std::env::var_os(FIXTURE_ENV).expect("subprocess fixture"));
    for _ in 0..5 {
        // In app-core this public wrapper calls the rich helper. In setup-hints
        // this is the private helper used by the shortcut notification workflow.
        super::send_desktop_notification("reaping title", "reaping body");
    }
    std::fs::write(dir.join("returned"), "").unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
}

struct Probe {
    child: Child,
    dir: tempfile::TempDir,
}

impl Probe {
    fn start(notifier_present: bool) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("jcode-notification-reaping-")
            .tempdir()
            .unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::create_dir(dir.path().join("children")).unwrap();
        std::fs::create_dir(dir.path().join("ready")).unwrap();
        if notifier_present {
            let script = bin.join("notify-send");
            std::fs::write(
                &script,
                "#!/bin/sh\n\
                 printf '%s\\n' \"$@\" > \"$JCODE_NOTIFICATION_REAP_TEST_DIR/children/$$\"\n\
                 : > \"$JCODE_NOTIFICATION_REAP_TEST_DIR/ready/$$\"\n\
                 while [ ! -e \"$JCODE_NOTIFICATION_REAP_TEST_DIR/release\" ]; do /bin/sleep 0.02; done\n",
            )
            .unwrap();
            std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let module = module_path!().split_once("::").unwrap().1;
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{module}::notification_probe"),
                "--ignored",
                "--test-threads=1",
            ])
            .env("PATH", &bin)
            .env(FIXTURE_ENV, dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        Self { child, dir }
    }

    fn wait_for(&mut self, description: &str, mut condition: impl FnMut(&Path) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "probe exited: {description}"
            );
            if condition(self.dir.path()) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out: {description}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        // Release even after a failed assertion, including a blocking wait
        // regression. Bound cleanup so a broken helper cannot hang the suite.
        let _ = std::fs::write(self.dir.path().join("release"), "");
        if let Some(mut stdin) = self.child.stdin.take() {
            let _ = stdin.write_all(b"done\n");
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn notification_children_are_reaped_without_blocking() {
    let mut probe = Probe::start(true);
    probe.wait_for(
        "five calls return before the notifier release gate opens",
        |dir| dir.join("returned").exists(),
    );
    probe.wait_for("five exact notifier PIDs recorded", |dir| {
        std::fs::read_dir(dir.join("ready")).unwrap().count() == 5
    });
    let pids: Vec<u32> = std::fs::read_dir(probe.dir.path().join("children"))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            assert_eq!(
                std::fs::read_to_string(entry.path()).unwrap(),
                "--app-name=jcode\nreaping title\nreaping body\n"
            );
            entry.file_name().to_str().unwrap().parse().unwrap()
        })
        .collect();
    for pid in &pids {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        assert_ne!(fields[0], "Z", "gated notifier must still be running");
        assert_eq!(fields[1].parse::<u32>().unwrap(), probe.child.id());
    }
    std::fs::write(probe.dir.path().join("release"), "").unwrap();
    // Require disappearance, not merely a zero zombie count before child exit.
    // No waitpid sweep: only the production code can reap these children.
    probe.wait_for(
        "all five notifier PIDs disappear while their parent stays alive",
        |_| {
            pids.iter()
                .all(|pid| !Path::new(&format!("/proc/{pid}")).exists())
        },
    );
}

#[test]
fn notification_missing_executable_is_nonfatal() {
    let mut probe = Probe::start(false);
    probe.wait_for("missing notify-send is best effort", |dir| {
        dir.join("returned").exists()
    });
    assert_eq!(
        std::fs::read_dir(probe.dir.path().join("children"))
            .unwrap()
            .count(),
        0
    );
}
