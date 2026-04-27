use super::*;

pub(super) fn can_prompt_for_external_auth() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
        && std::env::var("JCODE_NON_INTERACTIVE").is_err()
}

pub(super) fn external_auth_blocked_message(
    provider_name: &str,
    source_name: &str,
    path: &std::path::Path,
    login_hint: &str,
) -> String {
    format!(
        "Found existing {} credentials from {} at {} but jcode will not read them without confirmation. Re-run in an interactive terminal to approve this auth source for future jcode sessions, or run `{}`.",
        provider_name,
        source_name,
        path.display(),
        login_hint
    )
}

pub(super) fn prompt_to_trust_external_auth(
    provider_name: &str,
    source_name: &str,
    path: &std::path::Path,
) -> Result<bool> {
    eprintln!();
    eprintln!(
        "Found existing {} credentials from {} at {}.",
        provider_name,
        source_name,
        path.display()
    );
    eprintln!("jcode will only read that source in place after you approve it.");
    eprintln!("It will not move, delete, or rewrite the original auth there.");
    eprint!("Trust this auth source for future jcode sessions? [y/N]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExternalAuthReviewAction {
    SharedExternal(auth::external::ExternalAuthSource),
    CodexLegacy,
    ClaudeCode,
    GeminiCli,
    Copilot(auth::copilot::ExternalCopilotAuthSource),
    Cursor(auth::cursor::ExternalCursorAuthSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalAuthReviewCandidate {
    pub(crate) provider_summary: String,
    pub(crate) source_name: String,
    pub(crate) path: std::path::PathBuf,
    action: ExternalAuthReviewAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalAuthAutoImportOutcome {
    pub imported: usize,
    pub messages: Vec<String>,
}

impl ExternalAuthAutoImportOutcome {
    pub(crate) fn render_markdown(&self) -> String {
        if self.messages.is_empty() {
            return "No external auth sources were imported.".to_string();
        }
        let mut out = format!("**Auto Import**\n\nImported {} source(s).", self.imported);
        for line in &self.messages {
            out.push_str("\n- ");
            out.push_str(line);
        }
        out
    }
}

pub(crate) fn pending_external_auth_review_candidates() -> Result<Vec<ExternalAuthReviewCandidate>>
{
    let mut candidates = Vec::new();

    for source in auth::external::unconsented_sources() {
        let provider_summary = auth::external::source_provider_labels(source).join(", ");
        if provider_summary.is_empty() {
            continue;
        }
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary,
            source_name: source.display_name().to_string(),
            path: source.path()?,
            action: ExternalAuthReviewAction::SharedExternal(source),
        });
    }

    if auth::codex::has_unconsented_legacy_credentials() {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "OpenAI/Codex".to_string(),
            source_name: "Codex auth.json".to_string(),
            path: auth::codex::legacy_auth_file_path()?,
            action: ExternalAuthReviewAction::CodexLegacy,
        });
    }

    if let Some(source) = auth::claude::has_unconsented_external_auth()
        && matches!(source, auth::claude::ExternalClaudeAuthSource::ClaudeCode)
    {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "Claude".to_string(),
            source_name: source.display_name().to_string(),
            path: source.path()?,
            action: ExternalAuthReviewAction::ClaudeCode,
        });
    }

    if auth::gemini::has_unconsented_cli_auth() {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "Gemini".to_string(),
            source_name: "Gemini CLI".to_string(),
            path: auth::gemini::gemini_cli_oauth_path()?,
            action: ExternalAuthReviewAction::GeminiCli,
        });
    }

    if let Some(source) = auth::copilot::has_unconsented_external_auth()
        && !matches!(
            source,
            auth::copilot::ExternalCopilotAuthSource::OpenCodeAuth
                | auth::copilot::ExternalCopilotAuthSource::PiAuth
        )
    {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "GitHub Copilot".to_string(),
            source_name: source.display_name().to_string(),
            path: source.path(),
            action: ExternalAuthReviewAction::Copilot(source),
        });
    }

    if let Some(source) = auth::cursor::has_unconsented_external_auth() {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "Cursor".to_string(),
            source_name: source.display_name().to_string(),
            path: source.path()?,
            action: ExternalAuthReviewAction::Cursor(source),
        });
    }

    Ok(candidates)
}

pub(crate) fn parse_external_auth_review_selection(
    input: &str,
    count: usize,
) -> Result<Vec<usize>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if matches!(trimmed.to_ascii_lowercase().as_str(), "a" | "all") {
        return Ok((0..count).collect());
    }

    let mut selected = Vec::new();
    for part in trimmed.split(',') {
        let value = part.trim();
        if value.is_empty() {
            continue;
        }
        let index: usize = value.parse().map_err(|_| {
            anyhow::anyhow!(
                "Invalid selection '{}'. Enter numbers like 1,3 or 'a' for all.",
                value
            )
        })?;
        if index == 0 || index > count {
            anyhow::bail!(
                "Selection '{}' is out of range. Enter 1-{} or 'a' for all.",
                index,
                count
            );
        }
        let zero_based = index - 1;
        if !selected.contains(&zero_based) {
            selected.push(zero_based);
        }
    }
    Ok(selected)
}

fn prompt_to_review_external_auth_sources(
    candidates: &[ExternalAuthReviewCandidate],
) -> Result<Vec<usize>> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    eprintln!();
    eprintln!("Found existing logins that jcode can reuse.");
    eprintln!("Nothing has been imported yet.");
    eprintln!(
        "Approve the sources you want jcode to read in place; rejected sources stay untouched."
    );
    eprintln!();

    for (index, candidate) in candidates.iter().enumerate() {
        eprintln!(
            "  {}. {:<22} via {}",
            index + 1,
            candidate.provider_summary,
            candidate.source_name
        );
        eprintln!("     {}", candidate.path.display());
    }

    eprintln!();
    eprint!("Approve sources [a=all, Enter=skip, example: 1,3]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    parse_external_auth_review_selection(&input, candidates.len())
}

fn approve_external_auth_review_candidate(candidate: &ExternalAuthReviewCandidate) -> Result<()> {
    match candidate.action {
        ExternalAuthReviewAction::SharedExternal(source) => {
            auth::external::trust_external_auth_source(source)?
        }
        ExternalAuthReviewAction::CodexLegacy => auth::codex::trust_legacy_auth_for_future_use()?,
        ExternalAuthReviewAction::ClaudeCode => auth::claude::trust_external_auth_source(
            auth::claude::ExternalClaudeAuthSource::ClaudeCode,
        )?,
        ExternalAuthReviewAction::GeminiCli => auth::gemini::trust_cli_auth_for_future_use()?,
        ExternalAuthReviewAction::Copilot(source) => {
            auth::copilot::trust_external_auth_source(source)?
        }
        ExternalAuthReviewAction::Cursor(source) => {
            auth::cursor::trust_external_auth_source(source)?
        }
    }
    Ok(())
}

fn revoke_external_auth_review_candidate(candidate: &ExternalAuthReviewCandidate) -> Result<()> {
    match candidate.action {
        ExternalAuthReviewAction::SharedExternal(source) => {
            crate::config::Config::revoke_external_auth_source_for_path(
                source.source_id(),
                &candidate.path,
            )?
        }
        ExternalAuthReviewAction::CodexLegacy => {
            crate::config::Config::revoke_external_auth_source_for_path(
                auth::codex::LEGACY_CODEX_AUTH_SOURCE_ID,
                &candidate.path,
            )?
        }
        ExternalAuthReviewAction::ClaudeCode => {
            crate::config::Config::revoke_external_auth_source_for_path(
                auth::claude::CLAUDE_CODE_AUTH_SOURCE_ID,
                &candidate.path,
            )?
        }
        ExternalAuthReviewAction::GeminiCli => {
            crate::config::Config::revoke_external_auth_source_for_path(
                auth::gemini::GEMINI_CLI_AUTH_SOURCE_ID,
                &candidate.path,
            )?
        }
        ExternalAuthReviewAction::Copilot(source) => {
            crate::config::Config::revoke_external_auth_source_for_path(
                source.source_id(),
                &candidate.path,
            )?
        }
        ExternalAuthReviewAction::Cursor(source) => {
            crate::config::Config::revoke_external_auth_source_for_path(
                source.source_id(),
                &candidate.path,
            )?
        }
    }
    Ok(())
}

async fn validate_claude_import() -> Result<String> {
    let creds = auth::claude::load_credentials()?;
    let refreshed = crate::auth::oauth::refresh_claude_tokens(&creds.refresh_token).await?;
    Ok(format!(
        "Claude refresh probe succeeded (expires_at={}).",
        refreshed.expires_at
    ))
}

async fn validate_openai_import() -> Result<String> {
    let creds = auth::codex::load_credentials()?;
    if creds.refresh_token.trim().is_empty() {
        Ok("Loaded OpenAI API key credentials.".to_string())
    } else {
        let refreshed = crate::auth::oauth::refresh_openai_tokens(&creds.refresh_token).await?;
        Ok(format!(
            "OpenAI refresh probe succeeded (expires_at={}).",
            refreshed.expires_at
        ))
    }
}

async fn validate_gemini_import() -> Result<String> {
    let tokens = auth::gemini::load_or_refresh_tokens().await?;
    Ok(format!(
        "Gemini load/refresh probe succeeded (expires_at={}).",
        tokens.expires_at
    ))
}

async fn validate_antigravity_import() -> Result<String> {
    let tokens = auth::antigravity::load_or_refresh_tokens().await?;
    Ok(format!(
        "Antigravity load/refresh probe succeeded (expires_at={}).",
        tokens.expires_at
    ))
}

async fn validate_copilot_import() -> Result<String> {
    let github_token = auth::copilot::load_github_token()?;
    let api_token =
        auth::copilot::exchange_github_token(&reqwest::Client::new(), &github_token).await?;
    Ok(format!(
        "Copilot exchange probe succeeded (expires_at={}).",
        api_token.expires_at
    ))
}

async fn validate_cursor_import() -> Result<String> {
    let has_agent_auth = auth::cursor::has_cursor_agent_auth();
    let has_api_key = auth::cursor::has_cursor_api_key();
    let has_vscdb = auth::cursor::has_cursor_vscdb_token();
    if has_agent_auth || has_api_key || has_vscdb {
        Ok(format!(
            "Cursor source loaded (agent_session={}, api_key={}, vscdb_token={}).",
            has_agent_auth, has_api_key, has_vscdb
        ))
    } else {
        anyhow::bail!("Cursor source did not expose a usable auth token.")
    }
}

fn validate_openrouter_like_import() -> Result<String> {
    for (env_key, env_file) in crate::provider_catalog::openrouter_like_api_key_sources() {
        if crate::provider_catalog::load_api_key_from_env_or_config(&env_key, &env_file).is_some() {
            return Ok(format!("Loaded API key for `{}`.", env_key));
        }
    }
    anyhow::bail!("No reusable API key became available after import.")
}

async fn validate_shared_external_import(
    source: auth::external::ExternalAuthSource,
) -> Result<String> {
    let mut errors = Vec::new();
    for label in auth::external::source_provider_labels(source) {
        let result = match label {
            "OpenAI/Codex" => validate_openai_import().await,
            "Claude" => validate_claude_import().await,
            "Gemini" => validate_gemini_import().await,
            "Antigravity" => validate_antigravity_import().await,
            "GitHub Copilot" => validate_copilot_import().await,
            "OpenRouter/API-key providers" => validate_openrouter_like_import(),
            _ => continue,
        };
        match result {
            Ok(detail) => return Ok(detail),
            Err(err) => errors.push(format!("{}: {}", label, err)),
        }
    }
    anyhow::bail!(errors.join("; "))
}

async fn validate_external_auth_review_candidate(
    candidate: &ExternalAuthReviewCandidate,
) -> Result<String> {
    match candidate.action {
        ExternalAuthReviewAction::SharedExternal(source) => {
            validate_shared_external_import(source).await
        }
        ExternalAuthReviewAction::CodexLegacy => validate_openai_import().await,
        ExternalAuthReviewAction::ClaudeCode => validate_claude_import().await,
        ExternalAuthReviewAction::GeminiCli => validate_gemini_import().await,
        ExternalAuthReviewAction::Copilot(_) => validate_copilot_import().await,
        ExternalAuthReviewAction::Cursor(_) => validate_cursor_import().await,
    }
}

pub(crate) async fn maybe_run_external_auth_auto_import_flow() -> Result<Option<usize>> {
    if !can_prompt_for_external_auth() {
        return Ok(None);
    }

    let candidates = pending_external_auth_review_candidates()?;
    if candidates.is_empty() {
        return Ok(None);
    }

    let selected = prompt_to_review_external_auth_sources(&candidates)?;
    let outcome = run_external_auth_auto_import_candidates(&candidates, &selected).await?;
    for line in &outcome.messages {
        eprintln!("{}", line);
    }
    auth::AuthStatus::invalidate_cache();
    Ok(Some(outcome.imported))
}

pub(crate) fn format_external_auth_review_candidates_markdown(
    candidates: &[ExternalAuthReviewCandidate],
) -> String {
    let mut message = String::from(
        "**Auto Import Existing Logins**\n\nFound existing logins that jcode can reuse. Nothing has been imported yet.\n\nReply with `a` to approve all, `1,3` to approve specific sources, or `/cancel` to abort.\n",
    );
    for (index, candidate) in candidates.iter().enumerate() {
        message.push_str(&format!(
            "\n{}. **{}** via {}\n   - `{}`\n",
            index + 1,
            candidate.provider_summary,
            candidate.source_name,
            candidate.path.display()
        ));
    }
    message
}

pub(crate) async fn run_external_auth_auto_import_candidates(
    candidates: &[ExternalAuthReviewCandidate],
    selected: &[usize],
) -> Result<ExternalAuthAutoImportOutcome> {
    let mut outcome = ExternalAuthAutoImportOutcome {
        imported: 0,
        messages: Vec::new(),
    };

    for &index in selected {
        let Some(candidate) = candidates.get(index) else {
            continue;
        };
        approve_external_auth_review_candidate(candidate)?;
        match validate_external_auth_review_candidate(candidate).await {
            Ok(detail) => {
                outcome.imported += 1;
                outcome.messages.push(format!(
                    "✓ Imported {} from {}. {}",
                    candidate.provider_summary, candidate.source_name, detail
                ));
            }
            Err(err) => {
                let _ = revoke_external_auth_review_candidate(candidate);
                outcome.messages.push(format!(
                    "✕ Skipped {} from {}: {}",
                    candidate.provider_summary, candidate.source_name, err
                ));
            }
        }
    }

    auth::AuthStatus::invalidate_cache();
    Ok(outcome)
}
