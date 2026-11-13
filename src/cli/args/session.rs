use super::*;
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub(crate) enum SessionCommand {
    /// Rename a saved session's human-readable name/title
    Rename {
        /// Session ID or memorable short name, e.g. fox
        session: String,

        /// New session name/title
        #[arg(required_unless_present = "clear")]
        name: Option<String>,

        /// Clear the custom session name/title
        #[arg(long, conflicts_with = "name")]
        clear: bool,

        /// Emit JSON instead of human-readable output
        #[arg(long)]
        json: bool,
    },

    /// Export user and assistant turns to Markdown
    Export {
        /// Session ID or memorable short name, e.g. fox
        session: String,

        /// Markdown file to write. Defaults to a sanitized display name in the current directory.
        output: Option<PathBuf>,
    },
}
