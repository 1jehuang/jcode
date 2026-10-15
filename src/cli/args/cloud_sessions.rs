//! Cloud session command-line options.

use super::*;

#[derive(Subcommand, Debug)]
pub(crate) enum CloudSessionsCommand {
    /// Configure Jade API defaults for cloud sessions on this machine
    Configure {
        /// Jade Session API base URL
        #[arg(long)]
        api_base: Option<String>,

        /// Jade Session API bearer token. Prefer --api-token-env to avoid shell history.
        #[arg(long, conflicts_with = "api_token_env")]
        api_token: Option<String>,

        /// Read the Jade Session API bearer token from this environment variable
        #[arg(long, conflicts_with = "api_token")]
        api_token_env: Option<String>,

        /// Optional Jade token id, e.g. dev-admin
        #[arg(long)]
        api_token_id: Option<String>,

        /// Default Jade user id for commands that do not pass --user-id
        #[arg(long)]
        user_id: Option<String>,

        /// Default private Jade session helper path
        #[arg(long)]
        helper: Option<String>,

        /// Remove the saved cloud sessions config
        #[arg(long)]
        clear: bool,
    },

    /// Show saved Jade API defaults for cloud sessions without printing secrets
    Status {
        /// Emit JSON instead of human-readable text
        #[arg(long)]
        json: bool,
    },

    /// Upload a specific local session JSON file to Jade cloud storage
    Upload {
        /// Path to a local Jcode session JSON file
        session_file: String,

        /// Upload without Jade's redaction pass
        #[arg(long)]
        raw: bool,

        #[command(flatten)]
        jade: JadeCloudOptions,
    },

    /// Upload the newest local Jcode session to Jade cloud storage
    UploadLatest {
        /// Directory containing local Jcode session JSON files
        #[arg(long, default_value = "~/.jcode/sessions")]
        sessions_dir: String,

        /// Upload without Jade's redaction pass
        #[arg(long)]
        raw: bool,

        #[command(flatten)]
        jade: JadeCloudOptions,
    },

    /// Sync new or changed local sessions to Jade cloud storage (idempotent; safe to schedule)
    Sync {
        /// Directory containing local Jcode session JSON files (default: ~/.jcode/sessions)
        #[arg(long)]
        sessions_dir: Option<String>,

        /// Only consider sessions modified within this many days (ignored with --all)
        #[arg(long)]
        since_days: Option<u64>,

        /// Sync all matching sessions regardless of age
        #[arg(long)]
        all: bool,

        /// Maximum number of sessions to upload in this run
        #[arg(long, default_value_t = 50)]
        max: usize,

        /// Skip this run if the last sync ran fewer than this many minutes ago (for cron/timers)
        #[arg(long)]
        min_interval_mins: Option<u64>,

        /// Upload without Jade's redaction pass
        #[arg(long)]
        raw: bool,

        /// Show what would be uploaded without uploading or recording state
        #[arg(long)]
        dry_run: bool,

        /// Re-upload sessions even if local sync state says they are unchanged
        #[arg(long)]
        force: bool,

        /// Emit JSON instead of human-readable text
        #[arg(long)]
        json: bool,

        #[command(flatten)]
        jade: JadeCloudOptions,
    },

    /// List cloud-uploaded sessions from the Jade index
    List {
        /// Maximum number of sessions to show
        #[arg(long, default_value_t = 25)]
        limit: usize,

        /// Emit JSON instead of human-readable text
        #[arg(long)]
        json: bool,

        #[command(flatten)]
        jade: JadeCloudOptions,
    },

    /// Verify that cloud metadata and the S3 session blob both exist
    Verify {
        /// Session ID to verify
        session_id: String,

        #[command(flatten)]
        jade: JadeCloudOptions,
    },

    /// Render a local HTML dashboard of cloud-uploaded sessions from the Jade index
    Dashboard {
        /// Maximum number of sessions to include
        #[arg(long, default_value_t = 100)]
        limit: usize,

        /// Write the dashboard HTML to this path (default: a temp file)
        #[arg(long)]
        output: Option<String>,

        /// Open the generated dashboard in the default browser
        #[arg(long)]
        open: bool,

        /// Also download each session and link rows to a local per-session viewer
        #[arg(long)]
        with_view: bool,

        #[command(flatten)]
        jade: JadeCloudOptions,
    },

    /// Download and view a cloud-uploaded session
    View {
        /// Session ID to view
        session_id: String,

        /// Output format
        #[arg(long, default_value = "summary")]
        format: CloudSessionViewFormat,

        /// Write HTML output to this path when --format html is used
        #[arg(long)]
        output: Option<String>,

        /// Open the generated HTML file when --format html is used
        #[arg(long)]
        open: bool,

        #[command(flatten)]
        jade: JadeCloudOptions,
    },
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct JadeCloudOptions {
    /// Jade user id to pass to the dev helper
    #[arg(long, default_value = "dev")]
    pub(crate) user_id: String,

    /// AWS CLI profile used by the private dev Jade helper. If omitted, the helper decides.
    #[arg(long)]
    pub(crate) profile: Option<String>,

    /// AWS region used by the private dev Jade helper. If omitted, the helper decides.
    #[arg(long)]
    pub(crate) region: Option<String>,

    /// Path to the private Jade session helper. Defaults to $JCODE_JADE_SESSIONS_HELPER or ~/jade/scripts/jade_sessions.py.
    #[arg(long)]
    pub(crate) helper: Option<String>,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub(crate) enum CloudSessionViewFormat {
    Summary,
    Json,
    Html,
}

impl CloudSessionViewFormat {
    pub(crate) fn as_arg(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Json => "json",
            Self::Html => "html",
        }
    }
}
