//! `[power]` section of the config: sleep and lid-close inhibition.

use serde::{Deserialize, Serialize};

/// Power-management configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerConfig {
    /// Prevent automatic system sleep while any jcode session is actively
    /// streaming/processing. Linux also asks logind to block lid-switch suspend.
    /// Windows cannot override a user-initiated lid close or power-button action;
    /// those remain controlled by the active Windows power plan. The display is
    /// still allowed to sleep. Default: true.
    ///
    /// Honored by the shared `jcode serve` daemon. The `JCODE_DISABLE_POWER_INHIBIT`
    /// environment variable forces this off regardless of the config value.
    pub prevent_sleep_while_streaming: bool,

    /// Also keep working when the lid closes on macOS and Windows while a
    /// session is streaming. Linux always blocks lid-close suspend as part of
    /// `prevent_sleep_while_streaming`. On Windows jcode temporarily sets the
    /// active power plan's lid close action to "Do nothing". On macOS it runs
    /// `sudo -n pmset -a disablesleep 1`, which requires a passwordless sudoers
    /// rule and is skipped otherwise. Original settings are journaled and
    /// restored when work finishes, including after a crash. Default: true.
    pub block_lid_close: bool,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            prevent_sleep_while_streaming: true,
            block_lid_close: true,
        }
    }
}
