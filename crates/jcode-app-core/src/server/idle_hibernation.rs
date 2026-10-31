//! Conservative idle RAM hibernation, including sessions with attached TUIs.
//! The canonical transcript remains live. Only lazy, reproducible caches are
//! discarded, so accepted messages and scheduled wakes need no thaw protocol.

use super::{SessionAgents, SwarmMember};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const CHECK_INTERVAL: Duration = Duration::from_secs(10);

struct IdleState {
    since: Instant,
    last_status_change: Instant,
    released: bool,
}

impl IdleState {
    fn new(now: Instant, last_status_change: Instant) -> Self {
        Self {
            since: now,
            last_status_change,
            released: false,
        }
    }

    fn observe_status(&mut self, last_status_change: Instant) {
        // Compare the actual generation rather than its age: a whole turn can
        // start and finish between two sweeps, even for thresholds < 10s.
        if self.last_status_change != last_status_change {
            self.since = last_status_change;
            self.last_status_change = last_status_change;
            self.released = false;
        }
    }

    fn due(
        &self,
        now: Instant,
        threshold: Duration,
        status_idle: Duration,
        activity_idle: Option<Duration>,
    ) -> bool {
        !threshold.is_zero()
            && !self.released
            && now.saturating_duration_since(self.since) >= threshold
            && status_idle >= threshold
            && activity_idle.is_none_or(|age| age >= threshold)
    }
}

pub(super) fn spawn(sessions: SessionAgents, members: Arc<RwLock<HashMap<String, SwarmMember>>>) {
    tokio::spawn(async move {
        let mut idle = HashMap::new();
        let mut interval = tokio::time::interval(CHECK_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let threshold =
                Duration::from_secs(crate::config::config().session.hibernation_idle_secs);
            reclaim_idle_caches(&sessions, &members, &mut idle, threshold).await;
        }
    });
}

async fn reclaim_idle_caches(
    sessions: &SessionAgents,
    members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    idle: &mut HashMap<String, IdleState>,
    threshold: Duration,
) -> usize {
    if threshold.is_zero() {
        idle.clear();
        return 0;
    }
    // Never queue behind an active turn, roster change or session operation.
    let Ok(sessions) = sessions.try_read() else {
        return 0;
    };
    let Ok(members) = members.try_read() else {
        return 0;
    };
    idle.retain(|id, _| sessions.contains_key(id));
    let candidates: Vec<_> = sessions
        .iter()
        .map(|(id, agent)| {
            (
                id.clone(),
                Arc::clone(agent),
                members
                    .get(id)
                    .map(|member| (member.status == "running", member.last_status_change)),
            )
        })
        .collect();
    drop(members);
    drop(sessions);
    let mut releases = Vec::new();
    let mut before = None;
    for (id, agent, member) in candidates {
        let Some((running, last_status_change)) = member else {
            idle.remove(&id);
            continue;
        };
        if running || crate::turn_cancel_registry::has_active_turn(&id) {
            idle.remove(&id);
            continue;
        }
        let Ok(mut agent) = agent.try_lock() else {
            idle.remove(&id);
            continue;
        };
        let now = Instant::now();
        let activity_idle =
            crate::session_metrics::last_activity_age_secs(&id).map(Duration::from_secs);
        let state = idle
            .entry(id.clone())
            .or_insert_with(|| IdleState::new(now, last_status_change));
        state.observe_status(last_status_change);
        if !state.due(now, threshold, last_status_change.elapsed(), activity_idle) {
            continue;
        }
        before.get_or_insert_with(|| {
            crate::process_memory::snapshot_with_source("server_idle_hibernation_before")
        });
        let Some(released) = agent.hibernate_idle_caches() else {
            // Pending input, alerts, compaction or tools reset the idle window.
            state.since = now;
            continue;
        };
        state.released = true;
        releases.push((id, released));
    }
    if !releases.is_empty() {
        // Do the process-wide allocator purge only once, after releasing every
        // Agent and roster lock. RSS is process-wide, not a per-session metric.
        crate::process_memory::release_retained_heap("server_idle_hibernation");
        let after = crate::process_memory::snapshot_with_source("server_idle_hibernation_after");
        let before = before.unwrap_or_else(|| {
            crate::process_memory::snapshot_with_source("server_idle_hibernation_before")
        });
        let session_ids = releases
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let provider_cache_bytes: usize = releases
            .iter()
            .map(|(_, release)| release.provider_cache_bytes)
            .sum();
        let tool_index_entries: usize = releases
            .iter()
            .map(|(_, release)| release.tool_index_entries)
            .sum();
        crate::logging::info(&format!(
            "Server idle hibernation: sessions={session_ids} mode=derived_caches provider_cache_json_bytes={} tool_index_entries={} rss_before_bytes={:?} rss_after_bytes={:?} rss_released_bytes={:?}",
            provider_cache_bytes,
            tool_index_entries,
            before.rss_bytes,
            after.rss_bytes,
            before
                .rss_bytes
                .zip(after.rss_bytes)
                .map(|(before, after)| before as i128 - after as i128),
        ));
    }
    releases.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::message::{Message, ToolDefinition};
    use crate::provider::{EventStream, Provider};
    use tokio::sync::{Mutex, mpsc};

    struct IdleProvider;
    #[async_trait::async_trait]
    impl Provider for IdleProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> anyhow::Result<EventStream> {
            anyhow::bail!("hibernation must not run provider")
        }
        fn name(&self) -> &str {
            "idle-test"
        }
        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self)
        }
    }

    #[test]
    fn idle_hibernation_due_respects_disable_idle_and_recent_activity() {
        let start = Instant::now();
        let threshold = Duration::from_secs(30);
        let state = IdleState::new(start, start);
        let later = start + threshold;
        assert!(!state.due(later, Duration::ZERO, threshold, None));
        assert!(!state.due(start, threshold, threshold, None));
        assert!(!state.due(later, threshold, Duration::ZERO, None));
        assert!(!state.due(later, threshold, threshold, Some(Duration::ZERO)));
        assert!(state.due(later, threshold, threshold, None));
    }

    #[test]
    fn idle_hibernation_rearms_for_turn_entirely_between_monitor_ticks() {
        let start = Instant::now();
        let threshold = Duration::from_secs(1);
        let mut state = IdleState::new(start, start);
        state.released = true;
        let now = start + Duration::from_secs(10);
        assert!(!state.due(now, threshold, Duration::from_secs(10), None));
        let turn_finished = start + Duration::from_secs(2);
        state.observe_status(turn_finished);
        assert!(state.due(
            now,
            threshold,
            Duration::from_secs(8),
            Some(Duration::from_secs(8))
        ));
        state.released = true;
        state.observe_status(turn_finished);
        assert!(!state.due(now, threshold, Duration::from_secs(8), None));
    }

    #[test]
    fn idle_hibernation_monitor_skips_busy_and_running_then_releases_once() {
        let _env = crate::storage::lock_test_env();
        // Serialize process-wide environment outside the asynchronous runtime.
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(async {
                let agent = Arc::new(Mutex::new(Agent::new(
                    Arc::new(IdleProvider),
                    crate::tool::Registry::empty(),
                )));
                let id = agent.lock().await.session_id().to_string();
                let (tx, _rx) = mpsc::unbounded_channel();
                let mut member = SwarmMember::from_record(
                    jcode_swarm_core::SwarmMemberRecord {
                        session_id: id.clone(),
                        working_dir: None,
                        swarm_id: None,
                        swarm_enabled: false,
                        status: jcode_swarm_core::SwarmLifecycleStatus::from("ready".to_string()),
                        detail: None,
                        task_label: None,
                        friendly_name: None,
                        report_back_to_session_id: None,
                        latest_completion_report: None,
                        role: jcode_swarm_core::SwarmRole::from("agent".to_string()),
                        is_headless: false,
                    },
                    tx,
                );
                let old = Instant::now() - Duration::from_secs(60);
                member.last_status_change = old;
                let sessions = Arc::new(RwLock::new(HashMap::from([(
                    id.clone(),
                    Arc::clone(&agent),
                )])));
                let members = Arc::new(RwLock::new(HashMap::from([(id.clone(), member)])));
                let mut idle = HashMap::from([(id.clone(), IdleState::new(old, old))]);
                let threshold = Duration::from_secs(1);
                let held = agent.lock().await;
                assert_eq!(
                    reclaim_idle_caches(&sessions, &members, &mut idle, threshold).await,
                    0
                );
                assert!(idle.is_empty());
                drop(held);
                members.write().await.get_mut(&id).unwrap().status = "running".into();
                assert_eq!(
                    reclaim_idle_caches(&sessions, &members, &mut idle, threshold).await,
                    0
                );
                members.write().await.get_mut(&id).unwrap().status = "ready".into();
                idle.insert(id.clone(), IdleState::new(old, old));
                assert_eq!(
                    reclaim_idle_caches(&sessions, &members, &mut idle, threshold).await,
                    1
                );
                assert!(idle[&id].released);
                assert_eq!(
                    reclaim_idle_caches(&sessions, &members, &mut idle, threshold).await,
                    0
                );
                assert_eq!(
                    reclaim_idle_caches(&sessions, &members, &mut idle, Duration::ZERO).await,
                    0
                );
                assert!(idle.is_empty());
            });
    }
}
