//! Non-destructive, time-based preview of receiving and installing an update.

use super::{App, DisplayMessage};
use crate::bus::UpdateStatus;
use std::time::{Duration, Instant};

pub(super) struct UpdateSimulator {
    started_at: Instant,
    stage: u8,
}

impl App {
    /// Start from the normal TUI and autoplay the same status transitions used by
    /// a real background update. No updater, download, install, or reload runs.
    pub fn start_update_simulator_on_launch(&mut self) {
        self.onboarding_flow = None;
        self.onboarding_startup_checked = true;
        self.onboarding_preview_mode = false;
        self.update_sim = Some(UpdateSimulator {
            started_at: Instant::now(),
            stage: 0,
        });
        self.set_status_notice("Update simulator · waiting for update signal...");
        self.force_full_redraw = true;
    }

    pub(super) fn restart_update_simulator(&mut self) {
        self.remove_simulated_update_messages();
        self.start_update_simulator_on_launch();
    }

    fn remove_simulated_update_messages(&mut self) {
        let before = self.display_messages.len();
        self.display_messages.retain(|message| {
            message.title.as_deref() != Some("Updating jcode")
                && !message.content.starts_with("Update simulator complete")
        });
        if self.display_messages.len() != before {
            self.bump_display_messages_version();
        }
        self.background_client_action = None;
        self.pending_background_client_reload = None;
    }

    /// Advance the preview from the regular TUI tick. Delays are intentionally
    /// long enough to see and critique each production UI state.
    pub(super) fn progress_update_simulator(&mut self) -> bool {
        let Some(sim) = self.update_sim.as_mut() else {
            return false;
        };
        let elapsed = sim.started_at.elapsed();
        let next_stage = match sim.stage {
            0 if elapsed >= Duration::from_millis(1200) => 1,
            1 if elapsed >= Duration::from_millis(2600) => 2,
            2 if elapsed >= Duration::from_millis(3900) => 3,
            3 if elapsed >= Duration::from_millis(5200) => 4,
            4 if elapsed >= Duration::from_millis(6500) => 5,
            5 if elapsed >= Duration::from_millis(8000) => 6,
            6 if elapsed >= Duration::from_millis(9800) => 7,
            7 if elapsed >= Duration::from_millis(11800) => 8,
            _ => return false,
        };
        sim.stage = next_stage;

        let version = "v99.0.0-simulated".to_string();
        match next_stage {
            1 => self.handle_update_status(UpdateStatus::Available {
                current: jcode_build_meta::version().to_string(),
                latest: version,
            }),
            2 => self.handle_update_status(UpdateStatus::Downloading {
                version,
                downloaded: 8 * 1_048_576,
                total: Some(100 * 1_048_576),
            }),
            3 => self.handle_update_status(UpdateStatus::Downloading {
                version,
                downloaded: 37 * 1_048_576,
                total: Some(100 * 1_048_576),
            }),
            4 => self.handle_update_status(UpdateStatus::Downloading {
                version,
                downloaded: 76 * 1_048_576,
                total: Some(100 * 1_048_576),
            }),
            5 => self.handle_update_status(UpdateStatus::Downloading {
                version,
                downloaded: 100 * 1_048_576,
                total: Some(100 * 1_048_576),
            }),
            6 => self.handle_update_status(UpdateStatus::Installing { version }),
            7 => self.handle_update_status(UpdateStatus::Installed { version }),
            8 => {
                self.handle_update_status(UpdateStatus::UpToDate);
                self.set_status_notice("Update simulator complete · restart successful");
                self.push_display_message(DisplayMessage::system(
                    "Update simulator complete. This is the post-restart TUI; no files were changed. Press Alt+_ to replay."
                        .to_string(),
                ));
                self.update_sim = None;
            }
            _ => {}
        }
        true
    }
}
