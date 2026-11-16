use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub(crate) enum CloudEnvCommand {
    /// Push an explicit environment request from this local repository.
    Push {
        #[arg(long)]
        host: Option<String>,
        /// Metadata-only request JSON. Never put secret values in arguments.
        #[arg(long)]
        request: String,
    },
    /// Internal lease-bound callback. Reads request JSON on stdin.
    #[command(hide = true)]
    Fulfill {
        #[arg(long)]
        session: String,
    },
    /// Internal bounded, opaque SSH receiver.
    #[command(hide = true)]
    Receive,
}
