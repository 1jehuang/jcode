//! Ownership and unique target resolution for swarm stop requests.

use super::*;

pub(super) fn swarm_stop_allowed_by_owner(
    req_session_id: &str,
    target_member: &SwarmMember,
    force: bool,
) -> bool {
    force || target_member.report_back_to_session_id.as_deref() == Some(req_session_id)
}

pub(super) async fn resolve_stop_target_session(
    swarm_id: &str,
    target: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> std::result::Result<String, String> {
    let target = target.trim();
    if target.is_empty() {
        return Err("target_session is required.".to_string());
    }

    let members = swarm_members.read().await;
    if members
        .get(target)
        .is_some_and(|member| member.swarm_id.as_deref() == Some(swarm_id))
    {
        return Ok(target.to_string());
    }

    let mut matches = members
        .iter()
        .filter(|(_, member)| member.swarm_id.as_deref() == Some(swarm_id))
        .filter(|(session_id, member)| {
            member.friendly_name.as_deref() == Some(target)
                || session_id.starts_with(target)
                || session_id.ends_with(target)
        })
        .map(|(session_id, member)| {
            (
                session_id.clone(),
                member
                    .friendly_name
                    .as_deref()
                    .unwrap_or(session_id)
                    .to_string(),
            )
        })
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| a.0.cmp(&b.0));

    match matches.len() {
        0 => Err(format!(
            "Unknown swarm session '{target}'. Use an exact session ID, unique friendly name, or unique session ID prefix/suffix."
        )),
        1 => Ok(matches.remove(0).0),
        _ => Err(format!(
            "Ambiguous swarm session '{target}' matched: {}. Use an exact session ID.",
            matches
                .iter()
                .map(|(session_id, friendly)| format!("{friendly} [{session_id}]"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}
