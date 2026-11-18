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

#[cfg(test)]
mod tests {
    use super::super::Args;
    use clap::CommandFactory;

    #[test]
    fn publication_is_hidden_but_its_explicit_help_preserves_options() {
        let mut command = Args::command();
        assert!(!command.render_help().to_string().contains("publish-build"));
        let helper = command.find_subcommand_mut("publish-build").unwrap();
        let help = helper.render_help().to_string();
        for option in [
            "--source",
            "--version",
            "--launcher",
            "--channel",
            "--termux-launcher",
        ] {
            assert!(help.contains(option), "missing publication option {option}");
        }
    }
}
