//! Only launches this test executable as an isolated synthetic child, never Jcode turns.
use super::*;
use std::io::Write;
use tokio::io::{AsyncBufReadExt, BufReader};

#[cfg(unix)]
#[tokio::test]
async fn cancelling_supervision_removes_marker_and_leaves_no_child() {
    let temp = tempfile::tempdir().unwrap();
    let marker = temp.path().join("busy");
    let pid_file = temp.path().join("fixture-pid");
    {
        let mut command = fixture("hang");
        command.env("JCODE_MOBILE_FIXTURE_PID_FILE", &pid_file);
        let future = process::run(
            command,
            None,
            Some(marker.clone()),
            Duration::from_secs(5),
            Duration::from_millis(20),
        );
        tokio::pin!(future);
        let ready = async {
            while std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::select! {
            result = &mut future => panic!("fixture unexpectedly finished: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(2), ready) => result.unwrap(),
        }
        assert!(marker.exists());
        // Dropping supervision cancels both inline reader futures and kills the child.
    }
    assert!(!marker.exists());
    let pid: libc::pid_t = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            // SAFETY: signal zero observes only the isolated fixture process.
            if unsafe { libc::kill(pid, 0) } == -1 {
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::ESRCH)
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled fixture must be killed and reaped");
}

#[test]
#[ignore = "subprocess fixture, invoked only by process regression tests"]
fn fake_child() {
    let Ok(mode) = std::env::var("JCODE_MOBILE_PROCESS_FIXTURE") else {
        return;
    };
    if let Some(path) = std::env::var_os("JCODE_MOBILE_FIXTURE_PID_FILE") {
        std::fs::write(path, std::process::id().to_string()).unwrap();
    }
    if mode == "hang" || mode == "eof" {
        if mode == "eof" {
            // This fixture is its own process, so closing its pipe descriptors cannot
            // affect the parent test harness. Keep running after both pipe EOFs.
            #[cfg(unix)]
            unsafe {
                libc::close(1);
                libc::close(2);
            }
        }
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let mut stderr = std::io::stderr().lock();
    stderr.write_all("🦀界".repeat(200_000).as_bytes()).unwrap();
    if mode == "error" {
        stderr.write_all("界🦀".repeat(300).as_bytes()).unwrap();
        stderr.flush().unwrap();
        std::process::exit(7);
    }
    drop(stderr);
    if mode != "missing" {
        println!("not-json");
        println!("{{\"type\":\"start\",\"session_id\":\"synthetic-session\"}}");
    }
}

fn fixture(mode: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "mobile_runner::process_tests::fake_child",
            "--ignored",
            "--nocapture",
        ])
        .env("JCODE_MOBILE_PROCESS_FIXTURE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

#[tokio::test]
async fn sequential_baseline_times_out_but_concurrent_drain_completes() {
    let mut child = fixture("success").spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let baseline = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_millis(300), async {
        let mut lines = BufReader::new(stdout).lines();
        while lines.next_line().await.unwrap().is_some() {}
    })
    .await;
    assert!(
        result.is_err(),
        "undrained stderr must block before stdout EOF"
    );
    let baseline_elapsed = baseline.elapsed();
    child.kill().await.unwrap();
    let fixed = std::time::Instant::now();
    let session = process::run(
        fixture("success"),
        None,
        None,
        Duration::from_secs(5),
        Duration::from_millis(20),
    )
    .await
    .unwrap();
    assert_eq!(session, "synthetic-session");
    eprintln!(
        "synthetic child: sequential baseline timeout={}ms, concurrent completion={}ms",
        baseline_elapsed.as_millis(),
        fixed.elapsed().as_millis()
    );
}

#[tokio::test]
async fn unicode_error_is_exactly_last_600_characters() {
    let error = process::run(
        fixture("error"),
        None,
        None,
        Duration::from_secs(5),
        Duration::from_millis(20),
    )
    .await
    .unwrap_err()
    .to_string();
    let (_, tail) = error.split_once("): ").unwrap();
    assert_eq!(tail, "界🦀".repeat(300));
    assert_eq!(tail.chars().count(), 600);
}

#[tokio::test]
async fn resumed_session_wins_and_missing_session_is_an_error() {
    let result = process::run(
        fixture("success"),
        Some("resumed"),
        None,
        Duration::from_secs(5),
        Duration::from_millis(20),
    )
    .await
    .unwrap();
    assert_eq!(result, "resumed");
    let error = process::run(
        fixture("missing"),
        None,
        None,
        Duration::from_secs(5),
        Duration::from_millis(20),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("did not report a session id"));
}

#[tokio::test]
async fn busy_marker_touched_and_removed_on_timeout_including_after_stdout_eof() {
    for mode in ["hang", "eof"] {
        if mode == "eof" && !cfg!(unix) {
            continue;
        }
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("busy");
        let future = process::run(
            fixture(mode),
            None,
            Some(marker.clone()),
            Duration::from_millis(250),
            Duration::from_millis(20),
        );
        tokio::pin!(future);
        tokio::select! {
            result = &mut future => panic!("finished before marker observation: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(80)) => {}
        }
        assert!(marker.exists());
        // Removing it verifies that a later heartbeat recreates it.
        std::fs::remove_file(&marker).unwrap();
        tokio::select! {
            result = &mut future => panic!("finished before heartbeat observation: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(60)) => {}
        }
        assert!(marker.exists());
        assert!(
            future
                .await
                .unwrap_err()
                .to_string()
                .contains("turn timed out")
        );
        assert!(!marker.exists());
    }
}

#[tokio::test]
async fn busy_marker_removed_on_success_and_failure() {
    for mode in ["success", "error", "missing"] {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("busy");
        let result = process::run(
            fixture(mode),
            None,
            Some(marker.clone()),
            Duration::from_secs(5),
            Duration::from_millis(20),
        )
        .await;
        assert_eq!(result.is_ok(), mode == "success");
        assert!(!marker.exists());
    }
}
