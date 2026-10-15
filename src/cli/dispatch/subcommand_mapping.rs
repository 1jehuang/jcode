//! Conversion from clap commands into runtime command requests.

use super::*;

pub(super) fn map_memory_subcommand(subcmd: MemoryCommand) -> commands::MemorySubcommand {
    match subcmd {
        MemoryCommand::List { scope, tag } => commands::MemorySubcommand::List { scope, tag },
        MemoryCommand::Search { query, semantic } => {
            commands::MemorySubcommand::Search { query, semantic }
        }
        MemoryCommand::Export { output, scope } => {
            commands::MemorySubcommand::Export { output, scope }
        }
        MemoryCommand::Import {
            input,
            scope,
            overwrite,
        } => commands::MemorySubcommand::Import {
            input,
            scope,
            overwrite,
        },
        MemoryCommand::Stats => commands::MemorySubcommand::Stats,
        MemoryCommand::ClearTest => commands::MemorySubcommand::ClearTest,
    }
}

pub(super) fn map_ambient_subcommand(subcmd: AmbientCommand) -> commands::AmbientSubcommand {
    match subcmd {
        AmbientCommand::Status => commands::AmbientSubcommand::Status,
        AmbientCommand::Log => commands::AmbientSubcommand::Log,
        AmbientCommand::Trigger => commands::AmbientSubcommand::Trigger,
        AmbientCommand::Stop => commands::AmbientSubcommand::Stop,
        AmbientCommand::RunVisible => commands::AmbientSubcommand::RunVisible,
    }
}

pub(super) fn map_cloud_sessions_subcommand(
    action: CloudSessionsCommand,
) -> commands::CloudSessionsSubcommand {
    match action {
        CloudSessionsCommand::Configure {
            api_base,
            api_token,
            api_token_env,
            api_token_id,
            user_id,
            helper,
            clear,
        } => commands::CloudSessionsSubcommand::Configure {
            api_base,
            api_token,
            api_token_env,
            api_token_id,
            user_id,
            helper,
            clear,
        },
        CloudSessionsCommand::Status { json } => commands::CloudSessionsSubcommand::Status { json },
        CloudSessionsCommand::Upload {
            session_file,
            raw,
            jade,
        } => commands::CloudSessionsSubcommand::Upload {
            session_file,
            raw,
            user_id: jade.user_id,
            profile: jade.profile,
            region: jade.region,
            helper: jade.helper,
        },
        CloudSessionsCommand::UploadLatest {
            sessions_dir,
            raw,
            jade,
        } => commands::CloudSessionsSubcommand::UploadLatest {
            sessions_dir,
            raw,
            user_id: jade.user_id,
            profile: jade.profile,
            region: jade.region,
            helper: jade.helper,
        },
        CloudSessionsCommand::Sync {
            sessions_dir,
            since_days,
            all,
            max,
            min_interval_mins,
            raw,
            dry_run,
            force,
            json,
            jade,
        } => commands::CloudSessionsSubcommand::Sync {
            sessions_dir,
            since_days,
            all,
            max,
            min_interval_mins,
            raw,
            dry_run,
            force,
            json,
            user_id: jade.user_id,
            profile: jade.profile,
            region: jade.region,
            helper: jade.helper,
        },
        CloudSessionsCommand::List { limit, json, jade } => {
            commands::CloudSessionsSubcommand::List {
                limit,
                json,
                user_id: jade.user_id,
                profile: jade.profile,
                region: jade.region,
                helper: jade.helper,
            }
        }
        CloudSessionsCommand::Verify { session_id, jade } => {
            commands::CloudSessionsSubcommand::Verify {
                session_id,
                user_id: jade.user_id,
                profile: jade.profile,
                region: jade.region,
                helper: jade.helper,
            }
        }
        CloudSessionsCommand::Dashboard {
            limit,
            output,
            open,
            with_view,
            jade,
        } => commands::CloudSessionsSubcommand::Dashboard {
            limit,
            output,
            open,
            with_view,
            user_id: jade.user_id,
            profile: jade.profile,
            region: jade.region,
            helper: jade.helper,
        },
        CloudSessionsCommand::View {
            session_id,
            format,
            output,
            open,
            jade,
        } => commands::CloudSessionsSubcommand::View {
            session_id,
            format: format.as_arg().to_string(),
            output,
            open,
            user_id: jade.user_id,
            profile: jade.profile,
            region: jade.region,
            helper: jade.helper,
        },
    }
}

pub(super) fn map_transcript_mode(mode: TranscriptModeArg) -> crate::protocol::TranscriptMode {
    match mode {
        TranscriptModeArg::Insert => crate::protocol::TranscriptMode::Insert,
        TranscriptModeArg::Append => crate::protocol::TranscriptMode::Append,
        TranscriptModeArg::Replace => crate::protocol::TranscriptMode::Replace,
        TranscriptModeArg::Send => crate::protocol::TranscriptMode::Send,
    }
}
