use super::*;
use crate::ambient::{AmbientManager, Priority, ScheduleRequest, ScheduleTarget};

struct HomeGuard(Option<std::ffi::OsString>);
impl Drop for HomeGuard {
    fn drop(&mut self) {
        if let Some(old) = self.0.take() {
            crate::env::set_var("JCODE_HOME", old);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

fn request(name: &str, due: bool) -> ScheduleRequest {
    ScheduleRequest {
        wake_in_minutes: None,
        wake_at: Some(Utc::now() + chrono::Duration::seconds(if due { -1 } else { 3600 })),
        context: name.into(),
        priority: Priority::Normal,
        target: ScheduleTarget::Session {
            session_id: name.into(),
        },
        created_by_session: name.into(),
        working_dir: None,
        task_description: None,
        relevant_files: vec![],
        git_branch: None,
        additional_context: None,
    }
}

#[test]
fn scheduled_queue_stale_managers_preserve_add_remove_and_claim() {
    let _guard = crate::storage::lock_test_env();
    let dir = tempfile::tempdir().unwrap();
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", dir.path());
    let mut first = AmbientManager::new().unwrap();
    let mut second = AmbientManager::new().unwrap();
    let a = first.schedule(request("a", true)).unwrap();
    let b = second.schedule(request("b", true)).unwrap();
    assert_eq!(AmbientManager::new().unwrap().queue().len(), 2);
    first.cancel_schedule(&a).unwrap();
    assert_eq!(AmbientManager::new().unwrap().queue().items()[0].id, b);
    assert!(second.cancel_schedule(&a).unwrap().is_none());
    let claimed = first.take_ready_direct_items();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, b);
    assert!(second.take_ready_items().is_empty());
    second.schedule(request("later", false)).unwrap();
    assert_eq!(AmbientManager::new().unwrap().queue().len(), 1);
    assert!(first.take_ready_direct_items().is_empty());
}

#[test]
fn scheduled_queue_cross_process_writer() {
    let Some(dir) = std::env::var_os("JCODE_QUEUE_TEST_CHILD_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let id = std::env::var("JCODE_QUEUE_TEST_CHILD_ID").unwrap();
    let mut queue = ScheduledQueue::load(dir.join("queue.json"));
    std::fs::write(dir.join(format!("ready-{id}")), "ready").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !dir.join("go").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "parent barrier timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    queue
        .try_push(AmbientManager::scheduled_item(request(&id, false)))
        .unwrap();
}

#[test]
fn scheduled_queue_cross_process_adds_are_not_lost() {
    let dir = tempfile::tempdir().unwrap();
    let mut children: Vec<_> = (0..4).map(|id| {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "ambient::persistence::queue_transaction_tests::scheduled_queue_cross_process_writer", "--nocapture"])
            .env("JCODE_QUEUE_TEST_CHILD_DIR", dir.path())
            .env("JCODE_QUEUE_TEST_CHILD_ID", id.to_string())
            .spawn().unwrap()
    }).collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !(0..4).all(|id| dir.path().join(format!("ready-{id}")).exists()) {
        if std::time::Instant::now() >= deadline {
            for child in &mut children {
                let _ = child.kill();
                let _ = child.wait();
            }
            panic!("child snapshot barrier timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    std::fs::write(dir.path().join("go"), "go").unwrap();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let queue = ScheduledQueue::load(dir.path().join("queue.json"));
    assert_eq!(queue.len(), 4);
    let owners: std::collections::HashSet<_> = queue
        .items()
        .iter()
        .map(|item| &item.created_by_session)
        .collect();
    assert_eq!(owners.len(), 4);
}

#[test]
fn scheduled_queue_failed_transaction_preserves_disk_and_memory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.json");
    let mut queue = ScheduledQueue::load(path.clone());
    queue
        .try_push(AmbientManager::scheduled_item(request("kept", false)))
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(
        queue
            .transact::<()>(|items| {
                items.clear();
                anyhow::bail!("abort")
            })
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(queue.len(), 1);
    std::fs::write(&path, "not json").unwrap();
    assert!(
        queue
            .try_push(AmbientManager::scheduled_item(request("new", false)))
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
    assert_eq!(queue.len(), 1);
    assert!(queue.take_ready_direct_items().is_empty());
}
