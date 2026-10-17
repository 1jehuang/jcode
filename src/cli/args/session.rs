use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub(crate) enum SessionCommand {
    /// Copy a selected jcode session into KCode; never modifies the source
    ImportJcode {
        /// Session ID in ~/.jcode/sessions (no paths or credentials imported)
        session: String,
    },
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
}
