//! External-auth-source review and auto-import flow.
//!
//! Discovers credentials left behind by other tools (Claude Code, Codex,
//! Copilot, Cursor, Gemini CLI, ...), asks the user to approve trusting them,
//! and imports approved sources. This is provider/auth domain logic that
//! depends only on core modules (`auth`, `config`, `provider`,
//! `provider_catalog`), so it lives in the core layer and can be driven by
//! both the CLI login flow and the TUI auth UI.

use anyhow::Result;
use std::io::{self, IsTerminal, Write};

use crate::auth;

pub fn can_prompt_for_external_auth() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
        && std::env::var("JCODE_NON_INTERACTIVE").is_err()
}

pub fn external_auth_blocked_message(
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

pub fn prompt_to_trust_external_auth(
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
    /// Claude Code's native credentials (macOS Keychain item or
    /// `CLAUDE_CODE_OAUTH_TOKEN` env var), which have no stable on-disk path.
    ClaudeCodeNative,
    GeminiCli,
    Copilot(auth::copilot::ExternalCopilotAuthSource),
    Cursor(auth::cursor::ExternalCursorAuthSource),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAuthReviewCandidate {
    pub(crate) provider_summary: String,
    pub(crate) source_name: String,
    pub(crate) path: std::path::PathBuf,
    action: ExternalAuthReviewAction,
}

// Read-only accessors. Kept available outside tests so onboarding/UI can
// summarize detected import candidates (e.g. for the first-run welcome card).
impl ExternalAuthReviewCandidate {
    pub fn provider_summary(&self) -> &str {
        &self.provider_summary
    }

    pub fn source_name(&self) -> &str {
        &self.source_name
    }

    /// Canonical Jcode provider ids (`claude`, `openai`, ...) this source would
    /// sign in, matching `jcode auth status --json` ids. Lets a UI tell which
    /// detected logins Jcode already has.
    pub fn provider_ids(&self) -> Vec<&'static str> {
        self.provider_summary
            .split(',')
            .filter_map(|label| provider_id_for_label(label.trim()))
            .collect()
    }

    /// Where the detected credential lives, for display only.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Build a synthetic candidate for tests / UI fixtures. The resulting
    /// candidate points at the legacy Codex action so it can be summarized and
    /// rendered, but is not expected to actually import successfully.
    #[doc(hidden)]
    pub fn fixture(provider_summary: impl Into<String>, source_name: impl Into<String>) -> Self {
        Self {
            provider_summary: provider_summary.into(),
            source_name: source_name.into(),
            path: std::path::PathBuf::from("/dev/null"),
            action: ExternalAuthReviewAction::CodexLegacy,
        }
    }
}

impl ExternalAuthReviewCandidate {
    /// Coarse telemetry `(provider, method)` labels for the providers this
    /// candidate activates on a successful import. Used by the onboarding flow
    /// to record `auth_success` so auto-imported logins show up in the
    /// activation funnel (they previously did not, because auto-import never
    /// flows through the manual `pending_login` telemetry path).
    ///
    /// The method is reported as `"import"` so import-driven activation can be
    /// distinguished from manual login in the funnel.
    pub fn telemetry_auth_labels(&self) -> Vec<(&'static str, &'static str)> {
        const METHOD: &str = "import";
        match &self.action {
            ExternalAuthReviewAction::CodexLegacy => vec![("openai", METHOD)],
            ExternalAuthReviewAction::ClaudeCode => vec![("claude", METHOD)],
            ExternalAuthReviewAction::ClaudeCodeNative => vec![("claude", METHOD)],
            ExternalAuthReviewAction::GeminiCli => vec![("gemini", METHOD)],
            ExternalAuthReviewAction::Copilot(_) => vec![("copilot", METHOD)],
            ExternalAuthReviewAction::Cursor(_) => vec![("cursor", METHOD)],
            ExternalAuthReviewAction::SharedExternal(source) => {
                auth::external::source_provider_labels(*source)
                    .into_iter()
                    .filter_map(|label| {
                        telemetry_provider_id_for_label(label).map(|id| (id, METHOD))
                    })
                    .collect()
            }
        }
    }
}

/// Map a human-facing provider label (as produced by
/// [`auth::external::source_provider_labels`]) to the canonical telemetry
/// provider id used by the activation funnel.
fn telemetry_provider_id_for_label(label: &str) -> Option<&'static str> {
    provider_id_for_label(label).filter(|id| *id != "cursor")
}

fn provider_id_for_label(label: &str) -> Option<&'static str> {
    match label {
        "Cursor" => Some("cursor"),
        "OpenAI/Codex" => Some("openai"),
        "Claude" => Some("claude"),
        "Gemini" => Some("gemini"),
        "Antigravity" => Some("antigravity"),
        "GitHub Copilot" => Some("copilot"),
        "OpenRouter/API-key providers" => Some("openrouter"),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAuthAutoImportOutcome {
    pub imported: usize,
    pub messages: Vec<String>,
    /// Coarse `(provider, method)` telemetry labels for each provider that was
    /// successfully imported, so callers can record `auth_success` for the
    /// activation funnel. May contain more entries than `imported` when a
    /// single source carries multiple providers.
    pub imported_auth_labels: Vec<(&'static str, &'static str)>,
}

impl ExternalAuthAutoImportOutcome {
    /// Provider to activate after a successful multi-source import. This mirrors
    /// jcode's global provider preference: Anthropic first, then OpenAI, followed
    /// by the remaining supported providers. The precise OAuth/API-key variant
    /// is resolved from `AuthStatus` by the caller when possible.
    pub fn preferred_activation_provider(&self) -> Option<&'static str> {
        const ORDER: &[&str] = &[
            "claude",
            "openai",
            "copilot",
            "gemini",
            "cursor",
            "antigravity",
            "openrouter",
        ];
        ORDER.iter().copied().find(|provider| {
            self.imported_auth_labels
                .iter()
                .any(|(imported, _)| imported == provider)
        })
    }

    pub fn render_markdown(&self) -> String {
        if self.messages.is_empty() {
            return "No external logins were imported.".to_string();
        }

        // Messages are tagged with a leading "✓"/"✕" marker by the importer.
        let imported: Vec<&String> = self
            .messages
            .iter()
            .filter(|m| m.starts_with('✓'))
            .collect();
        let skipped: Vec<&String> = self
            .messages
            .iter()
            .filter(|m| m.starts_with('✕'))
            .collect();

        let mut out = String::from("**Logins imported**\n");
        out.push('\n');
        if imported.is_empty() {
            out.push_str("No logins could be imported.");
        } else {
            out.push_str(&format!(
                "Reusing {} existing login{}:",
                imported.len(),
                if imported.len() == 1 { "" } else { "s" }
            ));
        }
        for line in &imported {
            out.push_str("\n- ");
            out.push_str(line.trim_start_matches('✓').trim());
        }

        if !skipped.is_empty() {
            out.push_str(&format!(
                "\n\nSkipped {} source{}:",
                skipped.len(),
                if skipped.len() == 1 { "" } else { "s" }
            ));
            for line in &skipped {
                out.push_str("\n- ");
                out.push_str(line.trim_start_matches('✕').trim());
            }
        }

        out
    }
}

pub fn pending_external_auth_review_candidates() -> Result<Vec<ExternalAuthReviewCandidate>> {
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

    // Claude Code's native credentials live in the macOS Keychain (or the
    // CLAUDE_CODE_OAUTH_TOKEN env var), not in the JSON file. Offer them when
    // the JSON file was not already detected above, so macOS users (where the
    // file usually does not exist) can still import their Claude login.
    if !auth::claude::native_source_allowed()
        && !candidates
            .iter()
            .any(|candidate| matches!(candidate.action, ExternalAuthReviewAction::ClaudeCode))
        && auth::claude::native_credentials_present()
    {
        candidates.push(ExternalAuthReviewCandidate {
            provider_summary: "Claude".to_string(),
            source_name: auth::claude::native_source_display_name().to_string(),
            path: auth::claude::native_source_path_hint(),
            action: ExternalAuthReviewAction::ClaudeCodeNative,
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

pub fn parse_external_auth_review_selection(input: &str, count: usize) -> Result<Vec<usize>> {
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
        ExternalAuthReviewAction::ClaudeCodeNative => {
            // Trust, then snapshot the Keychain/env credentials into jcode's own
            // auth.json so future loads and refreshes do not depend on the
            // (possibly prompting) Keychain. A successful trust with a failed
            // copy still leaves the env-token path usable.
            auth::claude::trust_native_source()?;
            if let Err(err) = auth::claude::import_native_credentials_into_account() {
                crate::logging::warn(&format!(
                    "Trusted Claude Code native credentials but could not snapshot them into jcode: {err}"
                ));
            }
        }
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
        ExternalAuthReviewAction::ClaudeCodeNative => {
            crate::config::Config::revoke_external_auth_source(
                auth::claude::CLAUDE_CODE_NATIVE_AUTH_SOURCE_ID,
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

/// Render a human-friendly note about how soon a token-based credential
/// expires, used to tell the user when a re-login is likely needed without
/// failing the import.
fn token_freshness_note(expires_at_ms: i64) -> String {
    let now_ms = chrono::Utc::now().timestamp_millis();
    if expires_at_ms <= now_ms {
        " The access token is expired; jcode will refresh it on first use, or run /login if that fails.".to_string()
    } else {
        String::new()
    }
}

// Import validation is intentionally *non-destructive*: it only checks that
// reusable credentials are present after trusting the source. It must NOT
// perform a live OAuth refresh, because OAuth refresh tokens are single-use --
// refreshing here would rotate (and thus burn) the source's refresh token and
// then discard the rotated result, breaking both jcode and the original tool.
// Expired tokens are still imported: they get refreshed lazily (and persisted)
// at request time, or the user is prompted to /login.

async fn validate_claude_import() -> Result<String> {
    let creds = auth::claude::load_credentials()?;
    if creds.access_token.trim().is_empty() && creds.refresh_token.trim().is_empty() {
        anyhow::bail!("Claude source did not expose a usable access or refresh token.");
    }
    Ok(format!(
        "Loaded Claude credentials.{}",
        token_freshness_note(creds.expires_at)
    ))
}

async fn validate_openai_import() -> Result<String> {
    let creds = auth::codex::load_credentials()?;
    if creds.refresh_token.trim().is_empty() {
        if creds.access_token.trim().is_empty() {
            anyhow::bail!("OpenAI source did not expose a usable token or API key.");
        }
        return Ok("Loaded OpenAI API key credentials.".to_string());
    }
    Ok(format!(
        "Loaded OpenAI OAuth credentials.{}",
        creds
            .expires_at
            .map(token_freshness_note)
            .unwrap_or_default()
    ))
}

async fn validate_gemini_import() -> Result<String> {
    let tokens = auth::gemini::load_tokens()?;
    Ok(format!(
        "Loaded Gemini credentials.{}",
        token_freshness_note(tokens.expires_at)
    ))
}

async fn validate_antigravity_import() -> Result<String> {
    let tokens = auth::antigravity::load_tokens()?;
    Ok(format!(
        "Loaded Antigravity credentials.{}",
        token_freshness_note(tokens.expires_at)
    ))
}

async fn validate_copilot_import() -> Result<String> {
    // Presence check only: confirm a GitHub token is readable. The
    // GitHub->Copilot exchange happens lazily at request time.
    let _github_token = auth::copilot::load_github_token()?;
    Ok("Loaded GitHub Copilot credentials.".to_string())
}

async fn validate_cursor_import() -> Result<String> {
    let has_api_key = auth::cursor::has_cursor_api_key();
    let has_vscdb = auth::cursor::has_cursor_vscdb_token();
    if has_api_key || has_vscdb {
        Ok(format!(
            "Cursor native source loaded (api_key={}, vscdb_token={}).",
            has_api_key, has_vscdb
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
        ExternalAuthReviewAction::ClaudeCodeNative => validate_claude_import().await,
        ExternalAuthReviewAction::GeminiCli => validate_gemini_import().await,
        ExternalAuthReviewAction::Copilot(_) => validate_copilot_import().await,
        ExternalAuthReviewAction::Cursor(_) => validate_cursor_import().await,
    }
}

pub async fn maybe_run_external_auth_auto_import_flow() -> Result<Option<usize>> {
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

pub fn format_external_auth_review_candidates_markdown(
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

pub async fn run_external_auth_auto_import_candidates(
    candidates: &[ExternalAuthReviewCandidate],
    selected: &[usize],
) -> Result<ExternalAuthAutoImportOutcome> {
    let mut outcome = ExternalAuthAutoImportOutcome {
        imported: 0,
        messages: Vec::new(),
        imported_auth_labels: Vec::new(),
    };

    for &index in selected {
        let Some(candidate) = candidates.get(index) else {
            continue;
        };
        approve_external_auth_review_candidate(candidate)?;
        match validate_external_auth_review_candidate(candidate).await {
            Ok(detail) => {
                outcome.imported += 1;
                outcome
                    .imported_auth_labels
                    .extend(candidate.telemetry_auth_labels());
                outcome.messages.push(format!(
                    "✓ {} (from {}): {}",
                    candidate.provider_summary, candidate.source_name, detail
                ));
            }
            Err(err) => {
                let _ = revoke_external_auth_review_candidate(candidate);
                outcome.messages.push(format!(
                    "✕ {} (from {}): {}",
                    candidate.provider_summary, candidate.source_name, err
                ));
            }
        }
    }

    auth::AuthStatus::invalidate_cache();
    Ok(outcome)
}

/// Whether Jcode already has a usable login for a canonical provider id, as
/// returned by [`ExternalAuthReviewCandidate::provider_ids`]. API keys do not
/// count for the OAuth providers, since importing an OAuth login adds to them.
pub fn provider_connected_in_jcode(status: &auth::AuthStatus, provider_id: &str) -> bool {
    use auth::AuthState::Available;
    match provider_id {
        "claude" => status.anthropic.oauth_state == Available,
        "openai" => status.openai_has_oauth && status.openai_oauth_state == Available,
        "gemini" => status.gemini == Available,
        "antigravity" => status.antigravity == Available,
        "copilot" => status.copilot == Available,
        "cursor" => status.cursor == Available,
        "openrouter" => status.openrouter == Available,
        _ => false,
    }
}

/// Like [`run_external_auth_auto_import_candidates`], but never lets an
/// imported source shadow a login Jcode already has. Connection state is
/// re-read at import time, and any selected source that would touch an
/// already connected provider is skipped and reported.
pub async fn run_external_auth_import_candidates_preserving_existing(
    candidates: &[ExternalAuthReviewCandidate],
    selected: &[usize],
) -> Result<ExternalAuthAutoImportOutcome> {
    auth::AuthStatus::invalidate_cache();
    let status = auth::AuthStatus::check();
    let mut kept = Vec::new();
    let mut skipped = Vec::new();
    for &index in selected {
        let Some(candidate) = candidates.get(index) else {
            continue;
        };
        if candidate
            .provider_ids()
            .iter()
            .any(|id| provider_connected_in_jcode(&status, id))
        {
            skipped.push(format!(
                "✕ {} (from {}): already connected in jcode, kept the existing login",
                candidate.provider_summary, candidate.source_name
            ));
        } else {
            kept.push(index);
        }
    }
    let mut outcome = run_external_auth_auto_import_candidates(candidates, &kept).await?;
    outcome.messages.extend(skipped);
    Ok(outcome)
}

#[cfg(test)]
mod render_markdown_tests {
    use super::ExternalAuthAutoImportOutcome;

    #[test]
    fn empty_outcome_reports_nothing_imported() {
        let outcome = ExternalAuthAutoImportOutcome {
            imported: 0,
            messages: Vec::new(),
            imported_auth_labels: Vec::new(),
        };
        assert_eq!(
            outcome.render_markdown(),
            "No external logins were imported."
        );
    }

    #[test]
    fn groups_imported_and_skipped_with_counts() {
        let outcome = ExternalAuthAutoImportOutcome {
            imported: 2,
            messages: vec![
                "✓ OpenAI/Codex (from Codex auth.json): Loaded OpenAI OAuth credentials."
                    .to_string(),
                "✓ Claude (from Claude Code): Loaded Claude credentials.".to_string(),
                "✕ Cursor (from Cursor native): no usable auth token.".to_string(),
            ],
            imported_auth_labels: vec![("openai", "import"), ("claude", "import")],
        };
        let md = outcome.render_markdown();
        assert!(md.starts_with("**Logins imported**"), "got: {md}");
        assert!(md.contains("Reusing 2 existing logins:"), "got: {md}");
        assert!(
            md.contains("- OpenAI/Codex (from Codex auth.json): Loaded OpenAI OAuth credentials."),
            "got: {md}"
        );
        assert!(md.contains("Skipped 1 source:"), "got: {md}");
        assert!(
            md.contains("- Cursor (from Cursor native): no usable auth token."),
            "got: {md}"
        );
        // Markers themselves should be stripped from the rendered list.
        assert!(!md.contains('✓'), "got: {md}");
        assert!(!md.contains('✕'), "got: {md}");
    }

    #[test]
    fn singular_wording_for_one_login() {
        let outcome = ExternalAuthAutoImportOutcome {
            imported: 1,
            messages: vec!["✓ Gemini (from Gemini CLI): Loaded Gemini credentials.".to_string()],
            imported_auth_labels: vec![("gemini", "import")],
        };
        let md = outcome.render_markdown();
        assert!(md.contains("Reusing 1 existing login:"), "got: {md}");
    }

    #[test]
    fn preferred_activation_provider_is_deterministic_for_multi_import() {
        let outcome = ExternalAuthAutoImportOutcome {
            imported: 3,
            messages: Vec::new(),
            imported_auth_labels: vec![
                ("gemini", "import"),
                ("claude", "import"),
                ("openai", "import"),
            ],
        };
        assert_eq!(outcome.preferred_activation_provider(), Some("claude"));

        let anthropic_only = ExternalAuthAutoImportOutcome {
            imported: 1,
            messages: Vec::new(),
            imported_auth_labels: vec![("claude", "import")],
        };
        assert_eq!(
            anthropic_only.preferred_activation_provider(),
            Some("claude")
        );
    }

    #[test]
    fn fixture_candidate_reports_import_auth_labels() {
        use super::ExternalAuthReviewCandidate;
        // The fixture points at the legacy Codex action -> OpenAI provider.
        let candidate = ExternalAuthReviewCandidate::fixture("OpenAI/Codex", "Codex auth.json");
        assert_eq!(
            candidate.telemetry_auth_labels(),
            vec![("openai", "import")]
        );
    }

    #[test]
    fn provider_ids_match_auth_status_ids() {
        use super::ExternalAuthReviewCandidate;
        let candidate = ExternalAuthReviewCandidate::fixture(
            "OpenAI/Codex, Claude, OpenRouter/API-key providers",
            "OpenCode",
        );
        assert_eq!(
            candidate.provider_ids(),
            vec!["openai", "claude", "openrouter"]
        );
        let cursor = ExternalAuthReviewCandidate::fixture("Cursor", "Cursor");
        assert_eq!(cursor.provider_ids(), vec!["cursor"]);
        assert!(
            ExternalAuthReviewCandidate::fixture("Unknown", "x")
                .provider_ids()
                .is_empty()
        );
    }

    #[test]
    fn existing_oauth_logins_block_import_but_api_keys_do_not() {
        use crate::auth::{AuthState, AuthStatus};
        let mut status = AuthStatus::default();
        assert!(!super::provider_connected_in_jcode(&status, "claude"));
        status.anthropic.state = AuthState::Available;
        status.anthropic.has_api_key = true;
        assert!(!super::provider_connected_in_jcode(&status, "claude"));
        status.anthropic.has_oauth = true;
        status.anthropic.oauth_state = AuthState::Available;
        assert!(super::provider_connected_in_jcode(&status, "claude"));
        status.gemini = AuthState::Available;
        assert!(super::provider_connected_in_jcode(&status, "gemini"));
        assert!(!super::provider_connected_in_jcode(&status, "unknown"));
    }
}
