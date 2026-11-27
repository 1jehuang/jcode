use super::*;

pub(super) async fn reclaim_coordinator_slot_on_resume(
    swarm_id: &str,
    session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarm_coordinators: &Arc<RwLock<HashMap<String, String>>>,
) {
    let previous = {
        let members = swarm_members.read().await;
        let Some(me) = members.get(session_id) else {
            return;
        };
        if me.is_headless
            || me.report_back_to_session_id.is_some()
            || me.swarm_id.as_deref() != Some(swarm_id)
        {
            return;
        }
        let coordinators = swarm_coordinators.read().await;
        let current = coordinators.get(swarm_id).cloned();
        let should_claim = match current.as_deref() {
            None => true,
            Some(current) if current == session_id => false,
            Some(current) => match members.get(current) {
                None => true,
                Some(holder) => {
                    holder.is_headless
                        || holder.swarm_id.as_deref() != Some(swarm_id)
                        || super::super::swarm_is_self_or_ancestor(&members, session_id, current)
                }
            },
        };
        if !should_claim {
            return;
        }
        current
    };
    swarm_coordinators
        .write()
        .await
        .insert(swarm_id.to_string(), session_id.to_string());
    let mut members = swarm_members.write().await;
    if let Some(previous) = previous.as_deref()
        && let Some(holder) = members.get_mut(previous)
    {
        holder.role = "agent".to_string();
    }
    if let Some(me) = members.get_mut(session_id) {
        me.role = "coordinator".to_string();
    }
    crate::logging::info(&format!(
        "Resumed session {session_id} reclaimed coordinator slot for swarm {swarm_id} (previous: {previous:?})"
    ));
}

pub(super) async fn rename_swarm_member_session(
    old_session_id: &str,
    new_session_id: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) -> SwarmMemberRename {
    if old_session_id == new_session_id {
        let members = swarm_members.read().await;
        let swarm_id = members
            .get(new_session_id)
            .and_then(|member| member.swarm_id.clone());
        return SwarmMemberRename {
            old_swarm_id: swarm_id.clone(),
            new_swarm_id: swarm_id,
            healed_swarm_id: None,
        };
    }
    // Never hold both swarm maps at once. Coordinator cleanup reads them in the
    // opposite order, so retaining the member write guard while waiting for the
    // swarm map can permanently deadlock reconnects and every later subscribe.
    let rename = {
        let wait = Instant::now();
        let mut members = swarm_members.write().await;
        let members_wait_ms = wait.elapsed().as_millis();
        if members_wait_ms >= 50 {
            crate::logging::info(&format!(
                "[TIMING] swarm_rename members write wait={members_wait_ms}ms session={new_session_id}"
            ));
        }
        let mut rename = SwarmMemberRename::default();
        if let Some(mut temporary) = members.remove(old_session_id) {
            rename.old_swarm_id = temporary.swarm_id.clone();
            // Issue #1650: a session restored from durable swarm state already
            // has a member recording its own swarm, role and spawn owner.
            // Overwriting it with the temporary connection member attributed
            // the resumed session to the temporary swarm, so every reconnect
            // left another `session:<temp>` swarm naming it as a member.
            let existing = members.get(new_session_id).map(|member| {
                (
                    member.swarm_id.as_deref(),
                    member.report_back_to_session_id.is_some(),
                )
            });
            let new_swarm_id = resumed_member_swarm_id(
                old_session_id,
                new_session_id,
                temporary.swarm_id.as_deref(),
                existing,
            );
            if let Some(existing) = members.get_mut(new_session_id) {
                if existing.swarm_enabled {
                    if existing.swarm_id != new_swarm_id {
                        rename.healed_swarm_id = existing.swarm_id.clone();
                    }
                    existing.swarm_id = new_swarm_id.clone();
                }
                existing.status = "ready".to_string();
                existing.detail = None;
                if existing.friendly_name.is_none() {
                    existing.friendly_name = temporary.friendly_name.take();
                }
            } else {
                temporary.session_id = new_session_id.to_string();
                temporary.status = "ready".to_string();
                temporary.detail = None;
                temporary.swarm_id = if temporary.swarm_enabled {
                    new_swarm_id.clone()
                } else {
                    None
                };
                members.insert(new_session_id.to_string(), temporary);
            }
            rename.new_swarm_id = members
                .get(new_session_id)
                .and_then(|member| member.swarm_id.clone());
        } else {
            rename.new_swarm_id = members
                .get(new_session_id)
                .and_then(|member| member.swarm_id.clone());
        }

        // Keep the spawn tree intact across the rename: children that reported
        // back to the old session id must follow it.
        for member in members.values_mut() {
            if member.report_back_to_session_id.as_deref() == Some(old_session_id) {
                member.report_back_to_session_id = Some(new_session_id.to_string());
            }
        }
        rename
    };

    let wait = Instant::now();
    let mut swarms = swarms_by_id.write().await;
    let swarms_wait_ms = wait.elapsed().as_millis();
    if swarms_wait_ms >= 50 {
        crate::logging::info(&format!(
            "[TIMING] swarm_rename swarms_by_id write wait={swarms_wait_ms}ms session={new_session_id}"
        ));
    }
    if let Some(old_swarm_id) = rename.old_swarm_id.as_ref()
        && let Some(swarm) = swarms.get_mut(old_swarm_id)
    {
        swarm.remove(old_session_id);
        if swarm.is_empty() {
            swarms.remove(old_swarm_id);
        }
    }
    // The resumed session belongs to exactly one swarm. Drop it from every
    // other swarm index entry so no persisted swarm keeps reporting it.
    swarms.retain(|swarm_id, session_ids| {
        if rename.new_swarm_id.as_deref() != Some(swarm_id.as_str()) {
            session_ids.remove(new_session_id);
        }
        !session_ids.is_empty()
    });
    if let Some(new_swarm_id) = rename.new_swarm_id.as_ref() {
        swarms
            .entry(new_swarm_id.clone())
            .or_default()
            .insert(new_session_id.to_string());
    }
    rename
}
