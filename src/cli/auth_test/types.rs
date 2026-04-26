#[expect(
    clippy::large_enum_variant,
    reason = "Generic auth-test targets carry provider descriptors until this CLI path is refactored"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedAuthTestTarget {
    Detailed(AuthTestTarget),
    Generic {
        provider: crate::provider_catalog::LoginProviderDescriptor,
        choice: super::provider_init::ProviderChoice,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthTestTarget {
    Claude,
    Openai,
    Gemini,
    Antigravity,
    Google,
    Copilot,
    Cursor,
}

impl AuthTestTarget {
    fn provider_choice(self) -> super::provider_init::ProviderChoice {
        match self {
            Self::Claude => super::provider_init::ProviderChoice::Claude,
            Self::Openai => super::provider_init::ProviderChoice::Openai,
            Self::Gemini => super::provider_init::ProviderChoice::Gemini,
            Self::Antigravity => super::provider_init::ProviderChoice::Antigravity,
            Self::Google => super::provider_init::ProviderChoice::Google,
            Self::Copilot => super::provider_init::ProviderChoice::Copilot,
            Self::Cursor => super::provider_init::ProviderChoice::Cursor,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Openai => "openai",
            Self::Gemini => "gemini",
            Self::Antigravity => "antigravity",
            Self::Google => "google",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
        }
    }

    fn supports_smoke(self) -> bool {
        !matches!(self, Self::Google)
    }

    fn from_provider_choice(choice: &super::provider_init::ProviderChoice) -> Option<Self> {
        match choice {
            super::provider_init::ProviderChoice::Claude
            | super::provider_init::ProviderChoice::ClaudeSubprocess => Some(Self::Claude),
            super::provider_init::ProviderChoice::Openai => Some(Self::Openai),
            super::provider_init::ProviderChoice::Gemini => Some(Self::Gemini),
            super::provider_init::ProviderChoice::Antigravity => Some(Self::Antigravity),
            super::provider_init::ProviderChoice::Google => Some(Self::Google),
            super::provider_init::ProviderChoice::Copilot => Some(Self::Copilot),
            super::provider_init::ProviderChoice::Cursor => Some(Self::Cursor),
            _ => None,
        }
    }

    fn credential_paths(self) -> Result<Vec<String>> {
        match self {
            Self::Claude => Ok(vec![
                crate::auth::claude::jcode_path()?.display().to_string(),
                crate::storage::user_home_path(".claude/.credentials.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Openai => Ok(vec![
                crate::storage::jcode_dir()?
                    .join("openai-auth.json")
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".codex/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Gemini => Ok(vec![
                crate::auth::gemini::tokens_path()?.display().to_string(),
                crate::auth::gemini::gemini_cli_oauth_path()?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Antigravity => Ok(vec![
                crate::auth::antigravity::tokens_path()?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Google => Ok(vec![
                crate::auth::google::credentials_path()?
                    .display()
                    .to_string(),
                crate::auth::google::tokens_path()?.display().to_string(),
            ]),
            Self::Copilot => Ok(vec![
                crate::storage::user_home_path(".copilot/config.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".config/github-copilot/hosts.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".config/github-copilot/apps.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Cursor => Ok(vec![
                dirs::config_dir()
                    .ok_or_else(|| anyhow::anyhow!("No config directory found"))?
                    .join("jcode")
                    .join("cursor.env")
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".config/Cursor/User/globalStorage/state.vscdb")?
                    .display()
                    .to_string(),
            ]),
        }
    }
}

#[derive(Debug, Serialize)]
struct AuthTestStepReport {
    name: String,
    ok: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct AuthTestProviderReport {
    provider: String,
    credential_paths: Vec<String>,
    steps: Vec<AuthTestStepReport>,
    smoke_output: Option<String>,
    tool_smoke_output: Option<String>,
    success: bool,
}

impl AuthTestProviderReport {
    fn new(target: AuthTestTarget) -> Self {
        Self {
            provider: target.label().to_string(),
            credential_paths: target.credential_paths().unwrap_or_default(),
            steps: Vec::new(),
            smoke_output: None,
            tool_smoke_output: None,
            success: true,
        }
    }

    fn new_generic(provider_id: String, credential_paths: Vec<String>) -> Self {
        Self {
            provider: provider_id,
            credential_paths,
            steps: Vec::new(),
            smoke_output: None,
            tool_smoke_output: None,
            success: true,
        }
    }

    fn push_step(&mut self, name: impl Into<String>, ok: bool, detail: impl Into<String>) {
        if !ok {
            self.success = false;
        }
        self.steps.push(AuthTestStepReport {
            name: name.into(),
            ok,
            detail: detail.into(),
        });
    }
}

impl ResolvedAuthTestTarget {
    fn from_choice(choice: &super::provider_init::ProviderChoice) -> Option<Self> {
        let provider = super::provider_init::login_provider_for_choice(choice)?;
        Some(match AuthTestTarget::from_provider_choice(choice) {
            Some(target) => Self::Detailed(target),
            None => Self::Generic {
                provider,
                choice: choice.clone(),
            },
        })
    }

    fn from_provider(provider: crate::provider_catalog::LoginProviderDescriptor) -> Option<Self> {
        let choice = super::provider_init::choice_for_login_provider(provider)?;
        Some(match AuthTestTarget::from_provider_choice(&choice) {
            Some(target) => Self::Detailed(target),
            None => Self::Generic { provider, choice },
        })
    }
}

#[derive(Clone, Copy)]
enum AuthTestSmokeKind {
    Provider,
    Tool,
}

impl AuthTestSmokeKind {
    fn step_name(self) -> &'static str {
        match self {
            Self::Provider => "provider_smoke",
            Self::Tool => "tool_smoke",
        }
    }

    fn skipped_by_flag_detail(self) -> &'static str {
        match self {
            Self::Provider => "Skipped by --no-smoke.",
            Self::Tool => "Skipped by --no-tool-smoke.",
        }
    }

    fn unsupported_detail(self) -> &'static str {
        "Skipped: provider is auth/tool-only and has no model runtime smoke step."
    }

    fn success_detail(self) -> &'static str {
        match self {
            Self::Provider => "Provider returned AUTH_TEST_OK.",
            Self::Tool => "Tool-enabled provider request returned AUTH_TEST_OK.",
        }
    }

    fn failure_detail(self, output: &str) -> String {
        match self {
            Self::Provider => {
                format!("Provider response did not contain AUTH_TEST_OK: {}", output)
            }
            Self::Tool => format!(
                "Tool-enabled provider response did not contain AUTH_TEST_OK: {}",
                output
            ),
        }
    }

    async fn run(
        self,
        target: AuthTestTarget,
        model: Option<&str>,
        prompt: &str,
    ) -> Result<String> {
        self.run_for_choice(&target.provider_choice(), model, prompt)
            .await
    }

    async fn run_for_choice(
        self,
        choice: &super::provider_init::ProviderChoice,
        model: Option<&str>,
        prompt: &str,
    ) -> Result<String> {
        match self {
            Self::Provider => run_provider_smoke_for_choice(choice, model, prompt).await,
            Self::Tool => run_provider_tool_smoke_for_choice(choice, model, prompt).await,
        }
    }

    fn set_output(self, report: &mut AuthTestProviderReport, output: String) {
        match self {
            Self::Provider => report.smoke_output = Some(output),
            Self::Tool => report.tool_smoke_output = Some(output),
        }
    }
}

fn push_result_step<T, E, F>(
    report: &mut AuthTestProviderReport,
    name: &'static str,
    result: std::result::Result<T, E>,
    detail: F,
) -> Option<T>
where
    E: std::fmt::Display,
    F: FnOnce(&T) -> String,
{
    match result {
        Ok(value) => {
            report.push_step(name, true, detail(&value));
            Some(value)
        }
        Err(err) => {
            report.push_step(name, false, err.to_string());
            None
        }
    }
}

fn auth_email_suffix(email: Option<&str>) -> String {
    email
        .map(|email| format!(" for {}", email))
        .unwrap_or_default()
}
