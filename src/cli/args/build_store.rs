#[derive(clap::Args, Debug)]
pub(crate) struct PublishBuildArgs {
    #[arg(long)]
    pub source: std::path::PathBuf,
    #[arg(long)]
    pub version: String,
    #[arg(long)]
    pub launcher: std::path::PathBuf,
    #[arg(long, default_value = "stable", value_parser = ["stable", "current"])]
    pub channel: String,
    #[arg(long)]
    pub termux_launcher: bool,
}
