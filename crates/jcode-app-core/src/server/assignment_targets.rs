use super::*;

/// Eligible auto-assignment targets for a swarm task.
///
/// Auto-pick must only land on sessions that will actually *execute* the work
/// without further human action. In a shared swarm there can be many foreign
/// members: independent human TUIs and stale "zombie" sessions left over from
/// other runs. Assigning to those silently strands the task (a human session is
/// never auto-driven; a zombie has no live agent at all) and stalls `run_plan`.
///
/// So a member is only a free worker for *automatic* selection when it is a
/// worker this run owns and can drive:
///
/// - `is_headless`: a spawned in-process worker (always auto-driven), or
/// - owned by the requester (`report_back_to_session_id == req`): a worker this
///   coordinator spawned, including reusable ones that already returned `ready`.
///
/// Everything else (foreign humans, zombies) must be addressed with an explicit
/// `target_session`, which bypasses this filter; this only governs auto-pick.
pub(super) fn filter_swarm_agent_candidates<'a>(
    members: &'a HashMap<String, SwarmMember>,
    req_session_id: &str,
    swarm_id: &str,
) -> Vec<&'a SwarmMember> {
    members
        .values()
        .filter(|member| {
            member.session_id != req_session_id
                && member.swarm_id.as_deref() == Some(swarm_id)
                && member.role == "agent"
                && matches!(member.status.as_str(), "ready" | "completed")
                && is_drivable_auto_worker(member, req_session_id)
        })
        .collect()
}

/// Whether `member` can be auto-assigned a task and be relied on to run it.
/// See [`filter_swarm_agent_candidates`] for the rationale.
pub(super) fn is_drivable_auto_worker(member: &SwarmMember, req_session_id: &str) -> bool {
    member.is_headless || member.report_back_to_session_id.as_deref() == Some(req_session_id)
}

/// Whether a candidate already holds an incomplete plan assignment.
///
/// Auto-pick must treat such a member as busy rather than reusable: assigning
/// more work to it queues large tasks serially on one agent while the swarm
/// still has spawn capacity (`spawn_if_needed` exists precisely to spawn a
/// fresh agent in that case). `loads` counts non-terminal plan items per
/// assignee (see [`assignment_loads`]).
pub(super) fn member_has_active_assignment(
    session_id: &str,
    loads: &HashMap<String, usize>,
) -> bool {
    loads.get(session_id).copied().unwrap_or(0) > 0
}

/// Safety-net expiry for auto-pick claims that never reach an explicit
/// release (e.g. a request that dies between the pick and the plan write).
const AUTO_ASSIGN_CLAIM_TTL: std::time::Duration = std::time::Duration::from_secs(15);

/// In-process claims for auto-picked assignment targets, keyed by
/// `swarm_id\nsession_id`.
///
/// The busy check reads the *plan* (`assignment_loads`), but the plan only
/// records an assignment at a write that happens several awaits after the
/// target is picked. Concurrent `assign_task`/`assign_next` requests resolve
/// their targets inside that window (observed live: three auto-picks within
/// ~100ms all stacking onto the same worker), so the pick itself must be a
/// claim: the first resolver wins the member, later ones skip it and fall
/// back to another candidate or to `spawn_if_needed`. Claims are released
/// once the plan write records (or abandons) the assignment; the TTL only
/// reaps leaked claims.
pub(super) fn auto_assign_claims() -> &'static std::sync::Mutex<HashMap<String, std::time::Instant>>
{
    static CLAIMS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, std::time::Instant>>> =
        std::sync::OnceLock::new();
    CLAIMS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

pub(super) fn auto_assign_claim_key(swarm_id: &str, session_id: &str) -> String {
    format!("{swarm_id}\n{session_id}")
}

/// Claim `session_id` as an auto-pick target. Returns false when another
/// in-flight request already picked it (and that claim has not expired).
pub(super) fn try_claim_auto_assign_target(swarm_id: &str, session_id: &str) -> bool {
    let mut claims = auto_assign_claims()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = std::time::Instant::now();
    claims.retain(|_, claimed_at| now.duration_since(*claimed_at) < AUTO_ASSIGN_CLAIM_TTL);
    match claims.entry(auto_assign_claim_key(swarm_id, session_id)) {
        std::collections::hash_map::Entry::Occupied(_) => false,
        std::collections::hash_map::Entry::Vacant(vacant) => {
            vacant.insert(now);
            true
        }
    }
}

pub(super) fn release_auto_assign_claim(swarm_id: &str, session_id: &str) {
    let mut claims = auto_assign_claims()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    claims.remove(&auto_assign_claim_key(swarm_id, session_id));
}

/// Error for an auto-pick that found no assignable worker. The leading
/// sentence is a stable contract: `spawn_if_needed`/`run_plan` match on it to
/// decide to spawn a fresh agent instead of failing the assignment.
pub(super) fn no_auto_target_error(busy_skipped: usize) -> String {
    let mut message =
        "No ready or completed swarm agents are available for automatic task assignment."
            .to_string();
    if busy_skipped > 0 {
        message.push_str(&format!(
            " Skipped {busy_skipped} worker(s) that already have an active assignment or an \
             in-flight pick; spawn a fresh agent (spawn_if_needed/prefer_spawn) or wait for one \
             to finish."
        ));
    }
    message
}

/// Pick the first idle, unclaimed candidate from `candidates` (already ranked
/// by the caller) and claim it.
///
/// Members that already hold an incomplete plan assignment are skipped, as are
/// members another in-flight request just picked, so repeated auto-assignments
/// fan out across workers (or trigger a fresh spawn) instead of stacking
/// serially on one agent.
pub(super) fn select_and_claim_auto_target(
    swarm_id: &str,
    candidates: &[&SwarmMember],
    loads: &HashMap<String, usize>,
) -> Result<String, String> {
    let mut busy_skipped = 0usize;
    for member in candidates {
        if member_has_active_assignment(&member.session_id, loads) {
            busy_skipped += 1;
            continue;
        }
        if try_claim_auto_assign_target(swarm_id, &member.session_id) {
            return Ok(member.session_id.clone());
        }
        busy_skipped += 1;
    }
    Err(no_auto_target_error(busy_skipped))
}
