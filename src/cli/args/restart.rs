use super::*;

#[derive(Subcommand, Debug)]
pub(crate) enum RestartCommand {
    /// Save a reboot snapshot of currently active jcode windows
    Save {
        /// Restore this reboot snapshot automatically the next time plain `jcode` starts
        #[arg(long)]
        auto_restore: bool,
    },
    /// Restore the most recently saved reboot snapshot
    Restore,
    /// Show the currently saved reboot snapshot
    Status,
    /// Remove the currently saved reboot snapshot
    Clear,
}
