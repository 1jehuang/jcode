//! Persist one lifecycle transition without discarding a swarm's existing plan.
use super::*;

pub(in crate::server) async fn persist_member_lifecycle(
    session_id: &str,
    members: &Arc<tokio::sync::RwLock<HashMap<String, SwarmMember>>>,
) {
    let swarm_id = members
        .read()
        .await
        .get(session_id)
        .and_then(|member| member.swarm_id.clone());
    let Some(swarm_id) = swarm_id else {
        return;
    };
    let operation = swarm_operation_lock(&swarm_id);
    let _operation = operation.lock().await;
    let member = members.read().await.get(session_id).cloned();
    let Some(member) = member else {
        return;
    };
    let file_lock = swarm_file_lock(&swarm_id);
    let _file = file_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let path = state_path(&swarm_id);
    // Startup also accepts an orphan backup after a lost primary. Preserve
    // its plan while writing the terminal member back to the primary path.
    let source = if path.exists() {
        path.clone()
    } else {
        let backup = path.with_extension("bak");
        if !backup.exists() {
            return;
        }
        backup
    };
    let result = (|| -> anyhow::Result<()> {
        let mut snapshot: PersistedSwarmState = storage::read_json(&source)?;
        let now = now_unix_ms();
        let current = to_persisted_member(&member, now);
        if let Some(record) = snapshot
            .members
            .iter_mut()
            .find(|record| record.record.session_id == session_id)
        {
            *record = current;
        } else {
            snapshot.members.push(current);
        }
        snapshot.updated_at_unix_ms = now;
        storage::write_json_fast(&path, &snapshot)?;
        Ok(())
    })();
    if let Err(error) = result {
        crate::logging::warn(&format!(
            "Could not persist lifecycle for {session_id}: {error}"
        ));
    }
}
