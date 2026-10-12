use crate::build;
use crate::cli::args::Command;
use anyhow::Result;

pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::PruneBuilds { keep, dry_run } => build::run_prune_builds_command(keep, dry_run),
        Command::PublishBuild(args) => {
            let channel = match args.channel.as_str() {
                "current" => build::PreparedBuildChannel::Current,
                "stable" => build::PreparedBuildChannel::Stable,
                _ => anyhow::bail!("invalid build publication channel"),
            };
            build::publish_prepared_build(
                &args.source,
                &args.version,
                channel,
                &args.launcher,
                args.termux_launcher,
            )?;
            Ok(())
        }
        _ => anyhow::bail!("not a build-store maintenance command"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn maintenance_handler_rejects_unrelated_commands() {
        let error = super::run(crate::cli::args::Command::Usage { json: false }).unwrap_err();
        assert_eq!(error.to_string(), "not a build-store maintenance command");
    }
}
