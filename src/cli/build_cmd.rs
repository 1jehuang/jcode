#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub message: Option<String>,
    pub manual: bool,
    pub no_verify: bool,
    pub max_retries: usize,
    pub release: bool,
    pub clean: bool,
    pub target: Option<String>,
    pub all_projects: bool,
    pub test: bool,
    pub parallel: bool,
    pub jobs: Option<usize>,
}

pub async fn run_build_command(options: BuildOptions) -> anyhow::Result<()> {
    let message = options.message.as_deref().unwrap_or("Build project");
    todo!("Implement build command with options: {:?}", message)
}