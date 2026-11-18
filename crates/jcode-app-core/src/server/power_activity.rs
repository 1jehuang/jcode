//! Keep active local and cloud sessions awake.
use super::{Server, SwarmMember};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;

impl Server {
    /// Spawn the background loop that keeps the machine awake while any session
    /// is actively streaming/processing.
    ///
    /// The shared daemon owns every session, so a single inhibitor here covers
    /// all of them. We poll the swarm-member map (the authoritative "running"
    /// signal that also drives Waybar's "N streaming" indicator) on a short
    /// interval and reconcile a best-effort OS power inhibitor against it. The
    /// inhibitor blocks automatic system sleep; Linux also blocks lid-switch
    /// handling. Windows still honors explicit lid/power-button actions from the
    /// active power plan. The display can turn off. When no session is running,
    /// the guard is released so normal power management resumes immediately.
    pub(super) fn spawn_power_inhibitor(swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>) {
        // Reconcile interval. Short enough that the inhibitor engages promptly
        // when a turn starts and releases promptly when work finishes, but cheap
        // (a read lock + a scan) so it adds no meaningful load.
        const RECONCILE_INTERVAL: Duration = Duration::from_secs(5);

        let mut inhibitor = crate::power_inhibit::PowerInhibitor::new();
        if !inhibitor.is_available() {
            // Disabled via the legacy env escape hatch, or unsupported platform.
            crate::logging::info(
                "power_inhibit: unavailable (unsupported platform or JCODE_DISABLE_POWER_INHIBIT set); not monitoring",
            );
            return;
        }

        crate::logging::info(
            "power_inhibit: monitoring active sessions to prevent sleep while streaming",
        );

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(RECONCILE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last_active: Option<bool> = None;
            // macOS/Windows have no process-scoped lid lock, so lid-close
            // blocking changes a persistent OS setting. That belongs to the
            // user's desktop session, not the agent runtime: tell `jcode
            // companion`, which journals and restores it. No companion (a
            // headless or remote server) means no lid override.
            let mut lid_reported: Option<bool> = None;
            loop {
                interval.tick().await;

                // Re-evaluate the config each tick so toggling it at runtime
                // takes effect without restarting the daemon.
                let power = &crate::config::config().power;
                let enabled = power.prevent_sleep_while_streaming;
                let block_lid = enabled && power.block_lid_close;

                let active = enabled && Self::any_session_streaming(&swarm_members).await;
                if last_active != Some(active) {
                    crate::logging::info(&format!(
                        "power_inhibit: {} (streaming sessions {})",
                        if active { "engaging" } else { "releasing" },
                        if active { "present" } else { "absent" },
                    ));
                    last_active = Some(active);
                }
                inhibitor.set_active(active);
                let want_lid = active && block_lid;
                // Re-send while active so a companion started mid-turn engages
                // too. Idle state is sent once.
                if want_lid || lid_reported != Some(false) {
                    let reported = crate::companion_client::request_async(
                        crate::companion_client::CompanionRequest::PowerActivity {
                            active: want_lid,
                        },
                        Duration::from_secs(2),
                    )
                    .await
                    .is_ok();
                    lid_reported = reported.then_some(want_lid);
                }
            }
        });
    }

    /// On Jcode Cloud the host powers off after 30 minutes without an SSH
    /// client unless the busy marker is fresh. Turns keep running after the
    /// client disconnects, so mark the host busy while any session runs.
    pub(super) fn spawn_cloud_activity_keepalive(
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    ) {
        if !crate::mobile_runner::is_cloud_machine() {
            return;
        }
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if Self::any_session_streaming(&swarm_members).await {
                    crate::mobile_runner::touch_cloud_activity_marker();
                }
            }
        });
    }
}
