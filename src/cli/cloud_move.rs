//! `jcode cloud move` / `jcode cloud return`: carry a live session to another
//! machine and back.
//!
//! # Model
//!
//! - **Conversation: one owner.** A transcript cannot be merged, so exactly one
//!   machine runs turns at a time. Ownership is a per-machine lease
//!   (`jcode_storage::session_lease`) with a shared, monotonic migration epoch.
//!   The old owner's in-memory agent refuses to start turns or persist once the
//!   lease says the session moved.
//! - **Code: git.** The move ships the exact local state (HEAD, branch,
//!   uncommitted and untracked work) as a git bundle plus a *snapshot commit*.
//!   The cloud continues from that snapshot at the same absolute path. Return
//!   brings the cloud snapshot back as refs and 3-way merges it into whatever
//!   the local checkout looks like now, so local edits made meanwhile survive.
//!   Conflicts are ordinary git conflicts.
//! - **Staged, local-coordinated handoff.** prepare → transfer → verify →
//!   commit. Until commit the local session remains the owner, so a failure
//!   leaves the conversation local. A staged repo and an imported account may
//!   already exist remotely, but no remote turns have been activated.
//!
//! The remote half is the hidden `jcode cloud receive|activate|export`
//! commands, invoked over `ssh <host>` (or `--transport` for tests).
//! The selected Jcode-owned OpenAI/Claude OAuth account travels separately over
//! SSH, never in the repository/session tarball. Moving is consent to copy that
//! account to the chosen host. Other accounts and external logins stay local.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::auth::transfer::{self, CloudCredentialTransfer, TransferProvider};
use crate::{session, storage};

#[cfg(test)]
#[path = "cloud_move_repo_tests.rs"]
mod repo_tests;

#[cfg(all(test, unix))]
#[path = "cloud_move_latency_tests.rs"]
mod latency_tests;

const FORMAT_VERSION: u32 = 1;
const SNAPSHOT_REF_PREFIX: &str = "refs/jcode-cloud";
/// Environment variables worth carrying. Everything else (DISPLAY, WAYLAND_*,
/// SSH_AUTH_SOCK, DBUS_*, XDG_RUNTIME_DIR, secrets) is machine-local.
const ENV_ALLOWLIST: &[&str] = &[
    "LANG",
    "LC_ALL",
    "TZ",
    "EDITOR",
    "VISUAL",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "RUST_BACKTRACE",
    "CARGO_TERM_COLOR",
    "NODE_ENV",
];
/// Ignored files that projects commonly need at runtime. Copied only when present
/// and small. Never copies build output.
const IGNORED_ALLOWLIST: &[&str] = &[".env", ".env.local", ".env.development", ".envrc"];
const MAX_IGNORED_FILE_BYTES: u64 = 256 * 1024;

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MoveManifest {
    pub format_version: u32,
    pub session_id: String,
    pub epoch: u64,
    pub source_host: String,
    pub target_host: String,
    pub created_at: String,
    pub repo: Option<RepoManifest>,
    pub env: Vec<(String, String)>,
    pub toolchain_hints: Vec<String>,
    pub not_transferred: Vec<String>,
    pub mcp_servers: Vec<String>,
    /// Account identity only. Tokens never enter this manifest or its tarball.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_account: Option<CloudAccount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CloudAccount {
    provider: TransferProvider,
    label: String,
}

/// Only a hash of high-entropy credentials is retained locally, never another
/// token copy. It proves that the local login did not change while away.
#[derive(Serialize, Deserialize)]
struct CloudAuthBaseline {
    account: CloudAccount,
    fingerprint: String,
}

fn credential_fingerprint(payload: &CloudCredentialTransfer) -> String {
    format!("{:x}", Sha256::digest(payload.as_bytes()))
}

fn baseline_path(session_id: &str) -> Result<PathBuf> {
    if session_id.is_empty()
        || session_id.starts_with('.')
        || !session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
    {
        bail!("invalid session ID for cloud credential baseline");
    }
    Ok(storage::jcode_dir()?
        .join("cloud_auth")
        .join(format!("{session_id}.json")))
}

fn import_return_credentials(session_id: &str, account: &CloudAccount, bytes: &[u8]) -> Result<()> {
    let home = storage::jcode_dir()?;
    let path = baseline_path(session_id)?;
    if path.exists() {
        let baseline: CloudAuthBaseline = read_json(&path)?;
        if baseline.account == *account {
            let local =
                transfer::export_cloud_account_at(&home, account.provider, Some(&account.label))?;
            if credential_fingerprint(&local) != baseline.fingerprint {
                // Allow an idempotent return, but never replace a credential
                // refreshed or re-authenticated locally while the session was away.
                if local.as_bytes() == bytes {
                    return Ok(());
                }
                bail!(
                    "the local OAuth account changed while this session was on the cloud; it was not overwritten"
                );
            }
            return transfer::import_cloud_return_at(
                &home,
                account.provider,
                &account.label,
                bytes,
                &local,
            )
            .map_err(Into::into);
        }
    }
    transfer::import_cloud_at(&home, account.provider, &account.label, bytes).map_err(Into::into)
}

impl CloudAccount {
    fn route(&self) -> String {
        let method = match self.provider {
            TransferProvider::OpenAi => "openai-oauth",
            TransferProvider::Claude => "claude-oauth",
        };
        format!("{method}@{}", self.label)
    }
}

/// Never infer OAuth from a bare provider name: it may actually use an API key.
fn oauth_selection(sess: &session::Session) -> Result<Option<(TransferProvider, Option<&str>)>> {
    let route = sess.route_api_method.as_deref().or_else(|| {
        sess.model
            .as_deref()
            .and_then(|model| model.split_once(':').map(|(route, _)| route))
    });
    let Some(route) = route else { return Ok(None) };
    let method = route.split('@').next().unwrap_or(route);
    let provider = match method {
        "openai-oauth" => TransferProvider::OpenAi,
        "claude-oauth" => TransferProvider::Claude,
        _ => return Ok(None),
    };
    let label = crate::provider::oauth_account_label(route);
    if route.contains('@') && label.is_none() {
        bail!("session has an invalid saved OAuth account route");
    }
    Ok(Some((provider, label)))
}

fn session_credentials(sess: &session::Session) -> Result<Option<CloudCredentialTransfer>> {
    let Some((provider, label)) = oauth_selection(sess)? else {
        return Ok(None);
    };
    Ok(Some(transfer::export_cloud_account_at(
        &storage::jcode_dir()?,
        provider,
        label,
    )?))
}

fn run_credentials(provider: &str, account: &str, export: bool) -> Result<()> {
    let provider = provider.parse::<TransferProvider>()?;
    let home = storage::jcode_dir()?;
    if export {
        let payload = transfer::export_cloud_account_at(&home, provider, Some(account))?;
        std::io::stdout().write_all(payload.as_bytes())?;
        std::io::stdout().flush()?;
    } else {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take((transfer::MAX_TRANSFER_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        transfer::import_cloud_at(&home, provider, account, &bytes)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RepoManifest {
    /// Absolute repo root. Restored at the same path so every path in the
    /// transcript stays valid.
    pub root: String,
    /// Session working directory (may be a subdirectory of `root`).
    pub working_dir: String,
    pub branch: Option<String>,
    pub head: String,
    /// Commit capturing HEAD + staged + unstaged + untracked (non-ignored) files.
    pub snapshot: String,
    pub dirty: bool,
    pub origin_url: Option<String>,
    pub git_user_name: Option<String>,
    pub git_user_email: Option<String>,
    pub ignored_files: Vec<String>,
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub(crate) struct Target {
    pub host: Option<String>,
    pub remote_binary: Option<String>,
    pub transport: Option<String>,
}

impl Target {
    fn resolve_host(&self) -> Result<String> {
        if let Some(host) = self.host.clone().filter(|h| !h.trim().is_empty()) {
            return Ok(host);
        }
        if let Ok(host) = std::env::var("JCODE_CLOUD_HOST")
            && !host.trim().is_empty()
        {
            return Ok(host);
        }
        if super::cloud_managed::account_available() {
            return Ok(super::cloud_managed::HOST.to_string());
        }
        bail!(
            "no cloud machine: sign in with `jcode login` to use the Jcode Cloud machine included with your subscription, or pass --host <ssh-alias> / set JCODE_CLOUD_HOST"
        )
    }

    fn remote_binary(&self) -> String {
        self.remote_binary
            .clone()
            .or_else(|| std::env::var("JCODE_CLOUD_REMOTE_BINARY").ok())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "jcode".to_string())
    }

    /// Fail fast, with an actionable message, when the cloud host's jcode is
    /// too old to speak the move protocol.
    fn preflight(&self, host: &str) -> Result<()> {
        timed_move_stage("host preflight", || {
            if super::cloud_managed::is_managed(host) && self.transport.is_none() {
                step("prepare", "waking your Jcode Cloud machine");
                super::cloud_managed::connect(&mut |m| step("prepare", m))?;
                let home = dirs::home_dir()
                    .map(|h| h.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.run_shell(host, &super::cloud_managed::prepare_script(&home)?)
                    .context("prepare the Jcode Cloud machine")?;
            }
            // Preserve custom transports that only support one remote invocation.
            if self.transport.is_none() {
                let binary = shell_quote(&self.remote_binary());
                let mut script =
                    String::from("PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; export PATH\n");
                for command in ["receive", "credentials", "cancel-return"] {
                    script.push_str(&format!(
                    "{binary} --no-update --no-selfdev cloud {command} --help || {{ printf '%s\\n' 'cloud protocol probe failed: {command}' >&2; exit 1; }}\n"
                ));
                }
                let probe = (|| -> Result<()> {
                    let output = Self::ssh_command(host)?
                        .arg(&script)
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::piped())
                        .output()
                        .with_context(|| format!("failed to reach cloud host `{host}`"))?;
                    if !output.status.success() {
                        bail!(
                            "remote cloud protocol probes failed ({}): {}",
                            output.status,
                            String::from_utf8_lossy(&output.stderr).trim()
                        );
                    }
                    Ok(())
                })();
                if let Err(error) = probe {
                    if format!("{error:#}").contains("unrecognized") {
                        bail!(
                            "jcode on `{host}` does not support the complete cloud handoff protocol. Update jcode on the cloud host (or pass --remote-binary / set JCODE_CLOUD_REMOTE_BINARY to a newer build): {error:#}"
                        );
                    }
                    return Err(error.context(format!("could not run jcode on `{host}`")));
                }
                return Ok(());
            }
            for command in ["receive", "credentials", "cancel-return"] {
                if let Err(error) = self.run_remote(host, &["cloud", command, "--help"], None) {
                    let text = format!("{error:#}");
                    if text.contains("unrecognized") {
                        bail!(
                            "jcode on `{host}` does not support the complete cloud handoff protocol (`{command}`). Update jcode on the cloud host (or pass --remote-binary / set JCODE_CLOUD_REMOTE_BINARY to a newer build)"
                        );
                    }
                    return Err(error.context(format!("could not run jcode on `{host}`")));
                }
            }
            Ok(())
        })
    }

    /// `ssh` with the options every cloud call uses, including the private
    /// config for the managed machine.
    fn ssh_command(host: &str) -> Result<Command> {
        let mut command = Command::new("ssh");
        if super::cloud_managed::is_managed(host) {
            command.args(super::cloud_managed::ssh_args()?);
        }
        command.args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=30", "--"]);
        command.arg(host);
        Ok(command)
    }

    /// Run a raw shell script on the target (managed-machine preparation).
    fn run_shell(&self, host: &str, script: &str) -> Result<()> {
        let mut command = Self::ssh_command(host)?;
        command.arg("bash -s");
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        child
            .stdin
            .take()
            .context("ssh stdin")?
            .write_all(script.as_bytes())?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(())
    }

    fn remote_command(&self, host: &str, args: &[&str]) -> Result<Command> {
        let binary = shell_quote(&self.remote_binary());
        let quoted: Vec<String> = args.iter().map(|arg| shell_quote(arg)).collect();
        let remote_cmd = format!(
            "PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; export PATH; exec {binary} --no-update --no-selfdev {}",
            quoted.join(" ")
        );
        let command = match self.transport.as_deref() {
            Some(transport) => {
                let mut parts = transport.split_whitespace();
                let program = parts
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("empty --transport"))?;
                let mut command = Command::new(program);
                command.args(parts).arg(&remote_cmd);
                command
            }
            None => {
                let mut command = Self::ssh_command(host)?;
                command.arg(&remote_cmd);
                command
            }
        };
        Ok(command)
    }

    /// Run `jcode <args>` on the target, feeding `stdin` and returning stdout.
    fn run_remote(&self, host: &str, args: &[&str], stdin: Option<&Path>) -> Result<Vec<u8>> {
        let mut command = self.remote_command(host, args)?;
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        command.stdin(match stdin {
            Some(path) => Stdio::from(
                std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?,
            ),
            None => Stdio::null(),
        });
        let output = command
            .output()
            .with_context(|| format!("failed to reach cloud host `{host}`"))?;
        if !output.status.success() {
            bail!(
                "remote `jcode {}` on `{host}` failed ({}):\n{}",
                args.first().copied().unwrap_or_default(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    }

    /// Secrets travel only in an SSH pipe, never argv, a temporary file, or the
    /// session archive. Discard remote stderr: even a broken transport must not
    /// reflect a credential into the local transcript or logs.
    fn exchange_credentials(
        &self,
        host: &str,
        account: &CloudAccount,
        payload: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let mut args = vec![
            "cloud",
            "credentials",
            "--provider",
            account.provider.as_str(),
            "--account",
            account.label.as_str(),
        ];
        if payload.is_none() {
            args.push("--export");
        }
        let mut command = self.remote_command(host, &args)?;
        command.stdin(if payload.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        command.stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = command
            .spawn()
            .context("start private cloud credential transfer")?;
        let result = std::thread::scope(|scope| -> Result<Vec<u8>> {
            let writer = payload.map(|bytes| {
                let mut stdin = child.stdin.take().expect("piped credential stdin");
                scope.spawn(move || stdin.write_all(bytes))
            });
            let mut bytes = Vec::new();
            child
                .stdout
                .take()
                .context("cloud credential stdout")?
                .take((transfer::MAX_TRANSFER_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| anyhow::anyhow!("could not read cloud credential response"))?;
            if bytes.len() > transfer::MAX_TRANSFER_BYTES {
                let _ = child.kill();
                bail!("cloud credential response exceeds the size limit");
            }
            if let Some(writer) = writer {
                writer
                    .join()
                    .map_err(|_| anyhow::anyhow!("cloud credential writer failed"))?
                    .map_err(|_| anyhow::anyhow!("could not send cloud credentials"))?;
            }
            let status = child.wait().context("wait for cloud credential transfer")?;
            if !status.success() {
                bail!(
                    "cloud credential transfer failed ({status}); destination credentials were not accepted"
                );
            }
            if payload.is_some() && !bytes.is_empty() {
                bail!("unexpected cloud credential import response");
            }
            Ok(bytes)
        });
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct MoveReport {
    pub session_id: String,
    pub host: String,
    pub epoch: u64,
    pub repo_root: Option<String>,
    pub snapshot: Option<String>,
    pub dry_run: bool,
    pub attach_command: String,
    pub return_command: String,
}

fn timed_move_stage<T>(stage: &str, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let started = std::time::Instant::now();
    let result = operation();
    crate::logging::info(&format!(
        "cloud move: {stage} {} in {:.3}s",
        if result.is_ok() {
            "completed"
        } else {
            "failed"
        },
        started.elapsed().as_secs_f64()
    ));
    result
}

pub(crate) fn run_move(
    session_ref: Option<&str>,
    target: &Target,
    allow_active: bool,
    dry_run: bool,
) -> Result<MoveReport> {
    let host = target.resolve_host()?;
    let session_id = resolve_session_id(session_ref)?;
    if let Some(lease) = storage::read_session_lease(&session_id)
        && let Some(away) = lease.away_host
    {
        bail!("session {session_id} already lives on `{away}`. Use `jcode cloud return` first");
    }
    if !allow_active && storage::streaming_session_ids().contains(&session_id) {
        bail!(
            "session {session_id} is mid-turn. Wait for the turn to finish, or pass --allow-active when the agent itself is running the move"
        );
    }

    // ---- prepare -------------------------------------------------------
    step("prepare", "checking the cloud host");
    let (staging, sess, auth_account, epoch, repo) = std::thread::scope(|scope| {
        let preflight = scope.spawn(|| target.preflight(&host));
        let prepared = timed_move_stage("local snapshot preparation", || -> Result<_> {
            step("prepare", "snapshotting session, repo and environment");
            let mut sess = session::Session::load(&session_id)
                .with_context(|| format!("load session {session_id}"))?;
            let auth_account = session_credentials(&sess)?.map(|credentials| CloudAccount {
                provider: credentials.provider(),
                label: credentials.label().to_string(),
            });
            if let Some(account) = &auth_account {
                // Pin an implicit OAuth route to the exact account being transferred.
                sess.route_api_method = Some(account.route());
            }
            let epoch = storage::read_session_lease(&session_id)
                .map(|lease| lease.epoch)
                .unwrap_or(0)
                .max(sess.migration_epoch)
                + 1;
            let working_dir = sess
                .working_dir
                .clone()
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok())
                .context("session has no working directory")?;
            let staging = tempfile::Builder::new()
                .prefix("jcode-cloud-move-")
                .tempdir()?;
            let stage = staging.path();

            let repo = prepare_repo_snapshot(&working_dir, &session_id, stage)?;
            let not_transferred = describe_local_only_state(&session_id);
            let manifest = MoveManifest {
                format_version: FORMAT_VERSION,
                session_id: session_id.clone(),
                epoch,
                source_host: local_host_label(),
                target_host: host.clone(),
                created_at: chrono::Utc::now().to_rfc3339(),
                repo: repo.clone(),
                env: ENV_ALLOWLIST
                    .iter()
                    .filter_map(|key| std::env::var(key).ok().map(|v| (key.to_string(), v)))
                    .collect(),
                toolchain_hints: toolchain_hints(repo.as_ref().map(|r| Path::new(&r.root))),
                not_transferred,
                mcp_servers: configured_mcp_servers(),
                auth_account: auth_account.clone(),
            };

            // The shipped transcript: current conversation plus a hidden notice that
            // tells the agent where it now runs and what did not come along.
            sess.migration_epoch = epoch;
            let dangling = dangling_tool_use_ids(&sess);
            append_arrival(&mut sess, &dangling, &host, &arrival_notice(&manifest));
            let session_json = serde_json::to_vec(&sess)?;
            std::fs::write(stage.join("session.json"), &session_json)?;
            std::fs::write(
                stage.join("manifest.json"),
                serde_json::to_vec_pretty(&manifest)?,
            )?;
            copy_if_exists(&todo_file(&session_id)?, &stage.join("todos.json"))?;
            if let Some(repo) = &repo {
                copy_if_exists(
                    &crate::memory::project_memory_file(Path::new(&repo.working_dir))?,
                    &stage.join("project_memory.json"),
                )?;
            }
            Ok((staging, sess, auth_account, epoch, repo))
        });
        // Always join, including when local preparation failed. Nothing may
        // continue preparing the host after this move returns.
        let checked = preflight
            .join()
            .map_err(|_| anyhow::anyhow!("cloud host preflight thread panicked"))
            .and_then(|result| result);
        match (prepared, checked) {
            (Ok(prepared), Ok(())) => Ok(prepared),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(preflight_error)) => Err(error.context(format!(
                "cloud host preflight also failed: {preflight_error:#}"
            ))),
        }
    })?;
    // Negotiate only after preflight is joined. Snapshotting can overlap host
    // startup, while packing waits so known remote history need not be shipped.
    if let Some(repo) = &repo {
        let base = timed_move_stage("common repository history", || {
            Ok(common_remote_commit(target, &host, repo))
        })?;
        timed_move_stage("repository bundle", || {
            create_repo_bundle(repo, &session_id, staging.path(), base.as_deref())
        })?;
    }
    let tarball = staging.path().join("move.tar");
    timed_move_stage("transfer archive", || {
        tar_dir(
            staging.path(),
            &tarball,
            &[
                "manifest.json",
                "session.json",
                "todos.json",
                "project_memory.json",
                "repo.bundle",
                "ignored.tar",
            ],
        )
    })?;
    // Keep the temporary archive alive until the move finishes.
    let _staging = staging;

    // ---- transfer + verify ---------------------------------------------
    step("transfer", &format!("uploading to `{host}`"));
    let out = timed_move_stage("upload and receive", || {
        target.run_remote(&host, &["cloud", "receive", "--json"], Some(&tarball))
    })?;
    let verify: ReceiveReport = serde_json::from_slice(&out).with_context(|| {
        format!(
            "cloud host returned an unexpected receive report: {}",
            String::from_utf8_lossy(&out)
        )
    })?;
    step("verify", "checking the cloud copy matches");
    if verify.session_id != session_id || verify.epoch != epoch {
        bail!("cloud host staged the wrong session/epoch: {verify:?}");
    }
    if verify.messages != sess.messages.len() {
        bail!(
            "cloud transcript has {} messages, expected {}",
            verify.messages,
            sess.messages.len()
        );
    }
    if let Some(repo) = &repo
        && verify.snapshot.as_deref() != Some(repo.snapshot.as_str())
    {
        bail!(
            "cloud repo snapshot {:?} does not match local {}",
            verify.snapshot,
            repo.snapshot
        );
    }

    let report = MoveReport {
        session_id: session_id.clone(),
        host: host.clone(),
        epoch,
        repo_root: repo.as_ref().map(|r| r.root.clone()),
        snapshot: repo.as_ref().map(|r| r.snapshot.clone()),
        dry_run,
        attach_command: format!("jcode cloud attach --session {session_id}"),
        return_command: format!("jcode cloud return --session {session_id}"),
    };
    if dry_run {
        step(
            "done",
            "dry run: cloud copy staged and verified, ownership unchanged",
        );
        return Ok(report);
    }

    if let Some(account) = &auth_account {
        step(
            "credentials",
            "securely transferring the session's OAuth account",
        );
        // Snapshotting a large repository may take long enough for another
        // local session to refresh the account. Do not ship the earlier token.
        let credentials = transfer::export_cloud_account_at(
            &storage::jcode_dir()?,
            account.provider,
            Some(&account.label),
        )?;
        timed_move_stage("credential transfer", || {
            target.exchange_credentials(&host, account, Some(credentials.as_bytes()))
        })?;
        storage::write_json_secret(
            &baseline_path(&session_id)?,
            &CloudAuthBaseline {
                account: account.clone(),
                fingerprint: credential_fingerprint(&credentials),
            },
        )?;
    }

    // ---- commit ----------------------------------------------------------
    // Local lease first: from here on local turns stop. If activation fails
    // we roll the lease back so the session stays usable locally.
    step("commit", "handing ownership to the cloud host");
    let previous = storage::read_session_lease(&session_id);
    storage::write_session_lease(&storage::SessionLease {
        session_id: session_id.clone(),
        epoch,
        away_host: Some(host.clone()),
        repo_root: repo.as_ref().map(|r| r.root.clone()),
        last_sync_snapshot: repo.as_ref().map(|r| r.snapshot.clone()),
        last_sync_head: repo.as_ref().map(|r| r.head.clone()),
        updated_at: chrono::Utc::now().to_rfc3339(),
    })?;
    let epoch_arg = epoch.to_string();
    if let Err(error) = timed_move_stage("activation", || {
        target.run_remote(
            &host,
            &[
                "cloud",
                "activate",
                "--session",
                &session_id,
                "--epoch",
                &epoch_arg,
            ],
            None,
        )
    }) {
        match previous {
            Some(lease) => storage::write_session_lease(&lease)?,
            None => storage::remove_session_lease(&session_id),
        }
        return Err(error.context("cloud activation failed; the session stays local"));
    }
    if let Some(repo) = &repo {
        let _ = git(
            Path::new(&repo.root),
            &[
                "update-ref",
                &format!("{SNAPSHOT_REF_PREFIX}/{session_id}/base"),
                &repo.snapshot,
            ],
        );
    }
    step("done", &format!("session now lives on `{host}`"));
    Ok(report)
}

#[derive(Debug, Serialize)]
pub(crate) struct ReturnReport {
    pub session_id: String,
    pub host: String,
    pub epoch: u64,
    pub messages: usize,
    pub merge: MergeOutcome,
}

#[derive(Debug, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub(crate) enum MergeOutcome {
    NoRepo,
    Unchanged,
    RefsOnly {
        cloud_snapshot: String,
    },
    Applied {
        cloud_snapshot: String,
        branch: Option<String>,
        head: String,
    },
    Conflicts {
        cloud_snapshot: String,
        files: Vec<String>,
    },
}

pub(crate) fn run_return(
    session_ref: Option<&str>,
    target: &Target,
    refs_only: bool,
) -> Result<ReturnReport> {
    let session_id = resolve_session_id(session_ref)?;
    let lease = storage::read_session_lease(&session_id)
        .with_context(|| format!("session {session_id} was never moved"))?;
    let host = match (&lease.away_host, &target.host) {
        (Some(away), _) => away.clone(),
        (None, _) => bail!("session {session_id} already lives here"),
    };
    let epoch = lease.epoch + 1;

    // ---- export (cloud stops owning at `epoch`) --------------------------
    step("export", &format!("collecting the session from `{host}`"));
    let epoch_arg = epoch.to_string();
    let bytes = target.run_remote(
        &host,
        &[
            "cloud",
            "export",
            "--session",
            &session_id,
            "--epoch",
            &epoch_arg,
        ],
        None,
    )?;
    let staging = tempfile::Builder::new()
        .prefix("jcode-cloud-return-")
        .tempdir()?;
    let stage = staging.path();
    let tarball = stage.join("return.tar");
    std::fs::write(&tarball, &bytes)?;
    untar(&tarball, stage)?;
    let manifest: MoveManifest = read_json(&stage.join("manifest.json"))?;
    if manifest.session_id != session_id || manifest.epoch != epoch {
        bail!("cloud returned the wrong session/epoch");
    }

    // The cloud has stopped running turns, so export its final refreshed tokens.
    // On conflict, restore cloud ownership rather than strand the conversation.
    if let Some(account) = &manifest.auth_account {
        step(
            "credentials",
            "bringing refreshed OAuth credentials back securely",
        );
        let imported = target
            .exchange_credentials(&host, account, None)
            .and_then(|bytes| import_return_credentials(&session_id, account, &bytes));
        if let Err(error) = imported {
            target
                .run_remote(
                    &host,
                    &[
                        "cloud",
                        "cancel-return",
                        "--session",
                        &session_id,
                        "--epoch",
                        &epoch_arg,
                    ],
                    None,
                )
                .context("credential return failed and cloud ownership could not be restored")?;
            storage::write_session_lease(&storage::SessionLease {
                epoch,
                ..lease.clone()
            })?;
            return Err(
                error.context("credential return refused; the session remains on the cloud host")
            );
        }
    }

    // ---- code: fetch cloud snapshot as refs, then 3-way merge -----------
    let merge = match (&manifest.repo, lease.repo_root.as_deref()) {
        (Some(remote_repo), Some(root)) => {
            step("merge", "bringing cloud git work back");
            merge_back(
                Path::new(root),
                &session_id,
                remote_repo,
                &stage.join("repo.bundle"),
                lease.last_sync_snapshot.as_deref(),
                refs_only,
            )?
        }
        _ => MergeOutcome::NoRepo,
    };

    // ---- conversation: install the cloud transcript ----------------------
    step("session", "installing the cloud transcript locally");
    let mut sess: session::Session = read_json(&stage.join("session.json"))?;
    sess.migration_epoch = epoch;
    let merge_note = match &merge {
        MergeOutcome::Conflicts { files, .. } => format!(
            " Git merge of the cloud work into the local checkout left conflicts in: {}. Resolve them (the cloud side is `{SNAPSHOT_REF_PREFIX}/{session_id}/cloud`) before continuing.",
            files.join(", ")
        ),
        MergeOutcome::RefsOnly { cloud_snapshot } => format!(
            " Cloud work was fetched as ref `{SNAPSHOT_REF_PREFIX}/{session_id}/cloud` ({cloud_snapshot}) but not merged into the working tree."
        ),
        MergeOutcome::Applied { .. } => {
            " Cloud commits and uncommitted work were merged into the local checkout.".to_string()
        }
        _ => String::new(),
    };
    append_system_notice(
        &mut sess,
        &format!(
            "Session returned from cloud host `{host}` to local machine `{}` at {}. Same repo path.{merge_note} Processes you started on the cloud host (dev servers, background tasks, terminals) did not come back. Restart anything you still need.",
            local_host_label(),
            chrono::Utc::now().to_rfc3339(),
        ),
    );
    install_session(&sess, stage)?;
    storage::write_session_lease(&storage::SessionLease {
        session_id: session_id.clone(),
        epoch,
        away_host: None,
        repo_root: lease.repo_root.clone(),
        last_sync_snapshot: manifest.repo.as_ref().map(|r| r.snapshot.clone()),
        last_sync_head: manifest.repo.as_ref().map(|r| r.head.clone()),
        updated_at: chrono::Utc::now().to_rfc3339(),
    })?;
    let _ = std::fs::remove_file(baseline_path(&session_id)?);
    step("done", "session is local again");
    Ok(ReturnReport {
        session_id,
        host,
        epoch,
        messages: sess.messages.len(),
        merge,
    })
}

// ---------------------------------------------------------------------------
// Remote side (hidden commands)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct ReceiveReport {
    session_id: String,
    epoch: u64,
    messages: usize,
    snapshot: Option<String>,
    repo_root: Option<String>,
}

/// Stage an incoming move. Does not take ownership: `activate` does.
pub(crate) fn run_receive() -> Result<()> {
    let inbox = inbox_dir()?;
    std::fs::create_dir_all(&inbox)?;
    let staging = tempfile::Builder::new()
        .prefix("recv-")
        .tempdir_in(&inbox)?;
    let stage = staging.path();
    let tarball = stage.join("move.tar");
    let mut data = Vec::new();
    std::io::stdin().read_to_end(&mut data)?;
    std::fs::write(&tarball, &data)?;
    untar(&tarball, stage)?;
    let manifest: MoveManifest = read_json(&stage.join("manifest.json"))?;
    if manifest.format_version != FORMAT_VERSION {
        bail!(
            "move format {} not supported by this jcode (expects {FORMAT_VERSION}); update jcode on the cloud host",
            manifest.format_version
        );
    }
    if let Some(lease) = storage::read_session_lease(&manifest.session_id)
        && lease.epoch >= manifest.epoch
    {
        bail!(
            "this host already has session {} at epoch {} (incoming {})",
            manifest.session_id,
            lease.epoch,
            manifest.epoch
        );
    }
    let sess: session::Session = read_json(&stage.join("session.json"))?;

    // Restore the repo at the same absolute path.
    let snapshot = match &manifest.repo {
        Some(repo) => Some(restore_repo(
            repo,
            &stage.join("repo.bundle"),
            &stage.join("ignored.tar"),
            &manifest.session_id,
        )?),
        None => None,
    };

    let dest = inbox.join(format!("{}.{}", manifest.session_id, manifest.epoch));
    let _ = std::fs::remove_dir_all(&dest);
    let kept = staging.keep();
    std::fs::rename(&kept, &dest)?;
    let report = ReceiveReport {
        session_id: manifest.session_id,
        epoch: manifest.epoch,
        messages: sess.messages.len(),
        snapshot,
        repo_root: manifest.repo.map(|r| r.root),
    };
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}

/// Take ownership of a staged move: install transcript, todos and memory.
pub(crate) fn run_activate(session_id: &str, epoch: u64) -> Result<()> {
    let dir = inbox_dir()?.join(format!("{session_id}.{epoch}"));
    if !dir.is_dir() {
        bail!("no staged move for {session_id} at epoch {epoch}");
    }
    let manifest: MoveManifest = read_json(&dir.join("manifest.json"))?;
    let sess: session::Session = read_json(&dir.join("session.json"))?;
    if sess.migration_epoch != epoch {
        bail!("staged transcript epoch mismatch");
    }
    if let Some(account) = &manifest.auth_account {
        let credentials =
            session_credentials(&sess)?.context("staged session lost its OAuth route")?;
        if credentials.provider() != account.provider || credentials.label() != account.label {
            bail!("staged OAuth account does not match the session route");
        }
    }
    install_session(&sess, &dir)?;
    storage::write_session_lease(&storage::SessionLease {
        session_id: session_id.to_string(),
        epoch,
        away_host: None,
        repo_root: manifest.repo.as_ref().map(|r| r.root.clone()),
        last_sync_snapshot: manifest.repo.as_ref().map(|r| r.snapshot.clone()),
        last_sync_head: manifest.repo.as_ref().map(|r| r.head.clone()),
        updated_at: chrono::Utc::now().to_rfc3339(),
    })?;
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("activated {session_id} at epoch {epoch}");
    Ok(())
}

/// Hand the session back: stop owning it here, snapshot repo, stream tarball.
pub(crate) fn run_export(session_id: &str, epoch: u64) -> Result<()> {
    let lease = storage::read_session_lease(session_id)
        .with_context(|| format!("session {session_id} is not a migrated session on this host"))?;
    if !lease.is_here() {
        bail!("session {session_id} does not live on this host");
    }
    if epoch <= lease.epoch {
        bail!("stale export epoch {epoch} (host is at {})", lease.epoch);
    }
    if storage::streaming_session_ids().contains(&session_id.to_string()) {
        bail!(
            "session {session_id} is mid-turn on the cloud host; wait for the turn to finish (or cancel it) and retry"
        );
    }
    // Stop owning first so nothing new lands after the snapshot.
    storage::write_session_lease(&storage::SessionLease {
        away_host: Some("returned".to_string()),
        epoch,
        updated_at: chrono::Utc::now().to_rfc3339(),
        ..lease.clone()
    })?;
    let result = (|| -> Result<Vec<u8>> {
        let sess = session::Session::load(session_id)?;
        let staging = tempfile::Builder::new()
            .prefix("jcode-cloud-export-")
            .tempdir()?;
        let stage = staging.path();
        let working_dir = sess
            .working_dir
            .clone()
            .map(PathBuf::from)
            .or_else(|| lease.repo_root.clone().map(PathBuf::from));
        let repo = match working_dir {
            Some(dir) => snapshot_repo(&dir, session_id, stage)?,
            None => None,
        };
        let manifest = MoveManifest {
            format_version: FORMAT_VERSION,
            session_id: session_id.to_string(),
            epoch,
            source_host: local_host_label(),
            target_host: "local".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            repo,
            env: Vec::new(),
            toolchain_hints: Vec::new(),
            not_transferred: describe_local_only_state(session_id),
            mcp_servers: Vec::new(),
            auth_account: session_credentials(&sess)?
                .as_ref()
                .map(|credentials| CloudAccount {
                    provider: credentials.provider(),
                    label: credentials.label().to_string(),
                }),
        };
        std::fs::write(stage.join("session.json"), serde_json::to_vec(&sess)?)?;
        std::fs::write(
            stage.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        copy_if_exists(&todo_file(session_id)?, &stage.join("todos.json"))?;
        let tarball = stage.join("return.tar");
        tar_dir(
            stage,
            &tarball,
            &["manifest.json", "session.json", "todos.json", "repo.bundle"],
        )?;
        Ok(std::fs::read(&tarball)?)
    })();
    match result {
        Ok(bytes) => {
            std::io::stdout().write_all(&bytes)?;
            std::io::stdout().flush()?;
            Ok(())
        }
        Err(error) => {
            // Could not export: keep owning the session here.
            storage::write_session_lease(&lease)?;
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct WhereEntry {
    pub session_id: String,
    pub location: String,
    pub epoch: u64,
    pub repo_root: Option<String>,
    pub local_ahead_of_sync: Option<usize>,
}

pub(crate) fn run_where(session_ref: Option<&str>) -> Result<Vec<WhereEntry>> {
    let mut leases = Vec::new();
    if let Some(reference) = session_ref {
        let id = resolve_session_id(Some(reference))?;
        leases.extend(storage::read_session_lease(&id));
    } else if let Some(dir) = storage::session_leases_dir()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str())
                && let Some(lease) = storage::read_session_lease(stem)
            {
                leases.push(lease);
            }
        }
    }
    Ok(leases
        .into_iter()
        .map(|lease| {
            let ahead = match (&lease.repo_root, &lease.last_sync_head) {
                (Some(root), Some(head)) => git(
                    Path::new(root),
                    &["rev-list", "--count", &format!("{head}..HEAD")],
                )
                .ok()
                .and_then(|s| s.trim().parse().ok()),
                _ => None,
            };
            WhereEntry {
                location: lease
                    .away_host
                    .clone()
                    .unwrap_or_else(|| "local".to_string()),
                session_id: lease.session_id,
                epoch: lease.epoch,
                repo_root: lease.repo_root,
                local_ahead_of_sync: ahead,
            }
        })
        .collect())
}

pub(crate) fn away_host(session_ref: Option<&str>) -> Result<(String, String)> {
    let id = resolve_session_id(session_ref)?;
    let lease = storage::read_session_lease(&id).context("session was never moved")?;
    let host = lease
        .away_host
        .filter(|h| h != "returned")
        .context("session lives on this machine")?;
    Ok((id, host))
}

// ---------------------------------------------------------------------------
// Git
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    git_env(dir, args, &[])
}

fn git_env(dir: &Path, args: &[&str], env: &[(&str, &std::ffi::OsStr)]) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().context("failed to run git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Build a commit capturing the full working tree state (tracked changes plus
/// untracked non-ignored files) on top of HEAD without touching the index or
/// working tree, bundle it with the branch history, and copy allowlisted
/// ignored files.
fn snapshot_repo(
    working_dir: &Path,
    session_id: &str,
    stage: &Path,
) -> Result<Option<RepoManifest>> {
    let repo = prepare_repo_snapshot(working_dir, session_id, stage)?;
    if let Some(repo) = &repo {
        create_repo_bundle(repo, session_id, stage, None)?;
    }
    Ok(repo)
}

/// Find a bounded, locally known ancestor without modifying either repository.
/// Any unavailable/unsupported probe falls back to a self-contained bundle.
fn common_remote_commit(target: &Target, host: &str, repo: &RepoManifest) -> Option<String> {
    if target.transport.is_some() {
        return None;
    }
    let probe = (|| -> Result<Option<String>> {
        let script = format!(
            "PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; export PATH; git -C {} rev-list --max-count=128 HEAD",
            shell_quote(&repo.root)
        );
        let output = Target::ssh_command(host)?
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Ok(None);
        }
        let candidates: Vec<&str> = std::str::from_utf8(&output.stdout)?
            .lines()
            .take(128)
            .filter(|line| {
                matches!(line.len(), 40 | 64) && line.bytes().all(|b| b.is_ascii_hexdigit())
            })
            .collect();
        if candidates.is_empty() {
            return Ok(None);
        }
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&repo.root)
            .args(["cat-file", "--batch-check=%(objectname) %(objecttype)"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = format!("{}\n", candidates.join("\n"));
        if let Err(error) = child
            .stdin
            .take()
            .context("git cat-file stdin")?
            .write_all(input.as_bytes())
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        let checked = child.wait_with_output()?;
        if !checked.status.success() {
            return Ok(None);
        }
        for (candidate, checked) in candidates
            .iter()
            .zip(std::str::from_utf8(&checked.stdout)?.lines())
        {
            if checked == format!("{candidate} commit")
                && *candidate != repo.head
                && git(
                    Path::new(&repo.root),
                    &["merge-base", "--is-ancestor", candidate, &repo.head],
                )
                .is_ok()
            {
                return Ok(Some((*candidate).to_string()));
            }
        }
        Ok(None)
    })();
    match probe {
        Ok(base) => base,
        Err(error) => {
            crate::logging::info(&format!(
                "cloud move: common-history probe unavailable, using full bundle: {error:#}"
            ));
            None
        }
    }
}

fn create_repo_bundle(
    repo: &RepoManifest,
    session_id: &str,
    stage: &Path,
    base: Option<&str>,
) -> Result<()> {
    // Older receivers fetch the branch ref explicitly. Excluding the branch
    // tip omits that advertised ref, so only a strict ancestor is safe.
    let base = base.filter(|base| {
        *base != repo.head
            && git(
                Path::new(&repo.root),
                &["merge-base", "--is-ancestor", base, &repo.head],
            )
            .is_ok()
    });
    let bundle_path = stage.join("repo.bundle");
    let bundle_str = bundle_path.to_string_lossy().to_string();
    let snap_ref = format!("{SNAPSHOT_REF_PREFIX}/{session_id}/outgoing");
    let branch_ref = repo
        .branch
        .as_ref()
        .map(|branch| format!("refs/heads/{branch}"));
    let excluded = base.map(|base| format!("^{base}"));
    let mut args = vec!["bundle", "create", "--quiet", &bundle_str, &snap_ref];
    if let Some(branch_ref) = &branch_ref {
        args.push(branch_ref);
    }
    if let Some(excluded) = &excluded {
        args.push(excluded);
    }
    git(Path::new(&repo.root), &args)?;
    crate::logging::info(&format!(
        "cloud move: {} repository bundle, {} bytes",
        if base.is_some() {
            "incremental"
        } else {
            "full"
        },
        std::fs::metadata(&bundle_path)?.len()
    ));
    Ok(())
}

fn prepare_repo_snapshot(
    working_dir: &Path,
    session_id: &str,
    stage: &Path,
) -> Result<Option<RepoManifest>> {
    let Ok(root) = git(working_dir, &["rev-parse", "--show-toplevel"]) else {
        return Ok(None);
    };
    let root_path = PathBuf::from(&root);
    let head = git(&root_path, &["rev-parse", "--verify", "HEAD"])
        .context("repository has no commits yet; commit once before moving")?;
    let branch = git(&root_path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();

    // Temporary index so the user's real index is untouched.
    let index = stage.join("snapshot.index");
    let git_dir = PathBuf::from(git(&root_path, &["rev-parse", "--absolute-git-dir"])?);
    std::fs::copy(git_dir.join("index"), &index).ok();
    let env = [("GIT_INDEX_FILE", index.as_os_str())];
    git_env(&root_path, &["add", "-A", "--", "."], &env)?;
    let tree = git_env(&root_path, &["write-tree"], &env)?;
    let head_tree = git(&root_path, &["rev-parse", "HEAD^{tree}"])?;
    let dirty = tree != head_tree;
    let snapshot = git_env(
        &root_path,
        &[
            "commit-tree",
            &tree,
            "-p",
            &head,
            "-m",
            &format!("jcode cloud snapshot for {session_id}"),
        ],
        &identity_env(&root_path),
    )?;
    let snap_ref = format!("{SNAPSHOT_REF_PREFIX}/{session_id}/outgoing");
    git(&root_path, &["update-ref", &snap_ref, &snapshot])?;

    // Allowlisted ignored files (for example .env) that the project needs.
    let mut ignored = Vec::new();
    for name in IGNORED_ALLOWLIST {
        let path = root_path.join(name);
        if path.is_file()
            && std::fs::metadata(&path)
                .map(|m| m.len())
                .unwrap_or(u64::MAX)
                <= MAX_IGNORED_FILE_BYTES
            && git(&root_path, &["check-ignore", "-q", name]).is_ok()
        {
            ignored.push(name.to_string());
        }
    }
    if !ignored.is_empty() {
        let mut command = Command::new("tar");
        command
            .arg("-cf")
            .arg(stage.join("ignored.tar"))
            .arg("-C")
            .arg(&root_path)
            .args(&ignored);
        run_ok(command, "tar ignored files")?;
    }

    Ok(Some(RepoManifest {
        root,
        working_dir: working_dir.to_string_lossy().to_string(),
        branch,
        head,
        snapshot,
        dirty,
        origin_url: git(&root_path, &["remote", "get-url", "origin"]).ok(),
        git_user_name: git(&root_path, &["config", "user.name"]).ok(),
        git_user_email: git(&root_path, &["config", "user.email"]).ok(),
        ignored_files: ignored,
    }))
}

fn identity_env(root: &Path) -> Vec<(&'static str, &'static std::ffi::OsStr)> {
    // commit-tree needs an identity. Only fall back when the repo has none, so
    // the user's configured identity is always used when present.
    if git(root, &["var", "GIT_COMMITTER_IDENT"]).is_ok() {
        return Vec::new();
    }
    vec![
        ("GIT_AUTHOR_NAME", std::ffi::OsStr::new("jcode snapshot")),
        (
            "GIT_AUTHOR_EMAIL",
            std::ffi::OsStr::new("snapshot@jcode.invalid"),
        ),
        ("GIT_COMMITTER_NAME", std::ffi::OsStr::new("jcode snapshot")),
        (
            "GIT_COMMITTER_EMAIL",
            std::ffi::OsStr::new("snapshot@jcode.invalid"),
        ),
    ]
}

/// Cloud side: materialize the snapshot at the same absolute path.
fn restore_repo(
    repo: &RepoManifest,
    bundle: &Path,
    ignored_tar: &Path,
    session_id: &str,
) -> Result<String> {
    let root = PathBuf::from(&repo.root);
    let bundle_str = bundle.to_string_lossy().to_string();
    let snap_ref = format!("{SNAPSHOT_REF_PREFIX}/{session_id}/outgoing");
    if root.join(".git").exists() {
        // Existing checkout (an earlier move, dry run, or return). Overwrite
        // only when its working state still equals a snapshot jcode itself
        // placed or exported here. Anything else is work that never came back.
        let scratch = tempfile::tempdir()?;
        let current = snapshot_worktree_tree(&root, scratch.path())?;
        let known: Vec<String> = ["base", "outgoing"]
            .iter()
            .filter_map(|name| {
                git(
                    &root,
                    &[
                        "rev-parse",
                        &format!("{SNAPSHOT_REF_PREFIX}/{session_id}/{name}^{{tree}}"),
                    ],
                )
                .ok()
            })
            .collect();
        let head_tree = git(&root, &["rev-parse", "HEAD^{tree}"]).ok();
        if !known.contains(&current) && head_tree.as_deref() != Some(current.as_str()) {
            let owners = git(
                &root,
                &["for-each-ref", "--format=%(refname)", SNAPSHOT_REF_PREFIX],
            )
            .unwrap_or_default();
            let mut owners: Vec<&str> = owners
                .lines()
                .filter_map(|name| name.strip_prefix("refs/jcode-cloud/"))
                .filter_map(|name| name.strip_suffix("/base"))
                .collect();
            owners.sort_unstable();
            owners.dedup();
            let recovery = if owners.len() == 1 {
                format!(
                    "Run `jcode cloud return --session {}` locally to recover that session's work first",
                    owners[0]
                )
            } else {
                "Run `jcode cloud where` to identify the earlier session, then `jcode cloud return --session <id>` locally to recover its work first".to_string()
            };
            bail!(
                "{} on the cloud host has changes that were never returned; refusing to overwrite. {recovery}. No cloud working files were overwritten",
                repo.root,
            );
        }
    } else {
        std::fs::create_dir_all(&root)
            .with_context(|| format!("create {} on the cloud host", repo.root))?;
        git(&root, &["init", "--quiet"])?;
        if let Some(url) = &repo.origin_url {
            let _ = git(&root, &["remote", "add", "origin", url]);
        }
    }
    // An incremental bundle may omit the branch advertisement when that
    // branch's tip is already a prerequisite on this host. The snapshot has
    // the complete parent chain we need, so fetch it rather than requiring an
    // advertised branch. Verify prerequisites before touching checkout refs.
    git(&root, &["bundle", "verify", &bundle_str])?;
    let mut fetch_args = vec!["fetch", "--quiet", "--force", bundle_str.as_str()];
    let snap_spec = format!("{snap_ref}:{snap_ref}");
    fetch_args.push(&snap_spec);
    git(&root, &fetch_args)?;
    git(
        &root,
        &["cat-file", "-e", &format!("{}^{{commit}}", repo.head)],
    )?;

    match &repo.branch {
        Some(branch) => {
            git(
                &root,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
            )?;
        }
        None => {
            git(&root, &["update-ref", "--no-deref", "HEAD", &repo.head])?;
        }
    }
    // Branch at original HEAD, working tree + index = snapshot, so uncommitted
    // work stays uncommitted exactly as it was locally.
    git(&root, &["reset", "--quiet", "--hard", &repo.head])?;
    git(&root, &["read-tree", "--reset", "-u", &snap_ref])?;
    git(&root, &["reset", "--quiet", "--mixed", &repo.head])?;
    git(
        &root,
        &[
            "update-ref",
            &format!("{SNAPSHOT_REF_PREFIX}/{session_id}/base"),
            &repo.snapshot,
        ],
    )?;
    if let Some(name) = &repo.git_user_name
        && git(&root, &["config", "user.name"]).is_err()
    {
        git(&root, &["config", "user.name", name])?;
    }
    if let Some(email) = &repo.git_user_email
        && git(&root, &["config", "user.email"]).is_err()
    {
        git(&root, &["config", "user.email", email])?;
    }
    if ignored_tar.is_file() {
        let mut command = Command::new("tar");
        command.arg("-xf").arg(ignored_tar).arg("-C").arg(&root);
        run_ok(command, "restore ignored files")?;
    }
    git(&root, &["rev-parse", &snap_ref])
}

/// Local side of return: fetch the cloud snapshot as refs and 3-way merge it
/// into the current local state (which may have moved on meanwhile).
fn merge_back(
    root: &Path,
    session_id: &str,
    remote: &RepoManifest,
    bundle: &Path,
    base_snapshot: Option<&str>,
    refs_only: bool,
) -> Result<MergeOutcome> {
    let cloud_ref = format!("{SNAPSHOT_REF_PREFIX}/{session_id}/cloud");
    let cloud_head_ref = format!("{SNAPSHOT_REF_PREFIX}/{session_id}/cloud-head");
    let bundle_str = bundle.to_string_lossy().to_string();
    let snap_ref = format!("{SNAPSHOT_REF_PREFIX}/{session_id}/outgoing");
    let mut specs = vec![format!("+{snap_ref}:{cloud_ref}")];
    if let Some(branch) = &remote.branch {
        specs.push(format!("+refs/heads/{branch}:{cloud_head_ref}"));
    }
    let mut fetch = Command::new("git");
    fetch
        .arg("-C")
        .arg(root)
        .args(["fetch", "--quiet", &bundle_str])
        .args(&specs);
    run_ok(fetch, "git fetch cloud bundle")?;
    let cloud_snapshot = remote.snapshot.clone();

    if base_snapshot == Some(cloud_snapshot.as_str()) {
        return Ok(MergeOutcome::Unchanged);
    }
    if refs_only {
        return Ok(MergeOutcome::RefsOnly { cloud_snapshot });
    }

    // Step 1: commits. If the cloud branch strictly moved ahead of the
    // local branch, fast-forward the local branch ref without touching the
    // working tree yet.
    let local_head = git(root, &["rev-parse", "HEAD"])?;
    let local_branch = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let cloud_head = remote.head.clone();
    let same_branch = local_branch.is_some() && local_branch == remote.branch;
    let mut new_head = local_head.clone();
    if same_branch && cloud_head != local_head {
        if git(
            root,
            &["merge-base", "--is-ancestor", &local_head, &cloud_head],
        )
        .is_ok()
        {
            new_head = cloud_head.clone();
        } else if git(
            root,
            &["merge-base", "--is-ancestor", &cloud_head, &local_head],
        )
        .is_ok()
        {
            // Local already contains the cloud commits.
        } else {
            // Both sides committed. Leave commits alone, merge trees only.
            // The cloud commits remain reachable from `cloud-head`.
        }
    }

    // Step 2: working tree. 3-way merge of trees:
    //   base   = snapshot both sides agreed on at move time
    //   ours   = current local working tree (as a temp snapshot)
    //   theirs = cloud working tree snapshot
    let base = base_snapshot
        .map(str::to_string)
        .or_else(|| git(root, &["merge-base", &local_head, &cloud_snapshot]).ok())
        .context("cannot find a merge base with the cloud work")?;
    let staging = tempfile::tempdir()?;
    let ours = snapshot_worktree_tree(root, staging.path())?;
    let merged = git(
        root,
        &[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            &format!("--merge-base={base}"),
            &ours,
            &cloud_snapshot,
        ],
    );
    let (tree, conflicts) = match merged {
        Ok(out) => (
            out.lines().next().unwrap_or_default().to_string(),
            Vec::new(),
        ),
        Err(_) => {
            // merge-tree exits 1 on conflicts but still prints the tree.
            let output = Command::new("git")
                .arg("-C")
                .arg(root)
                .args([
                    "merge-tree",
                    "--write-tree",
                    "--name-only",
                    "--no-messages",
                    &format!("--merge-base={base}"),
                    &ours,
                    &cloud_snapshot,
                ])
                .output()?;
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            let mut lines = text.lines();
            let tree = lines.next().unwrap_or_default().to_string();
            let files: Vec<String> = lines
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            if tree.is_empty() {
                bail!(
                    "git merge-tree failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            (tree, files)
        }
    };

    if !conflicts.is_empty() {
        // Do not write conflict markers into the user's files automatically.
        // Keep everything as refs and let the agent/user resolve.
        return Ok(MergeOutcome::Conflicts {
            cloud_snapshot,
            files: conflicts,
        });
    }

    // Apply: move branch (if fast-forwarded), then check out merged tree as
    // uncommitted changes relative to the new HEAD. `read-tree -m -u` with two
    // trees refuses to overwrite files that differ from the old tree, so we
    // go from the "ours" tree we just captured (== current worktree).
    if new_head != local_head
        && let Some(branch) = &local_branch
    {
        git(
            root,
            &[
                "update-ref",
                &format!("refs/heads/{branch}"),
                &new_head,
                &local_head,
            ],
        )?;
    }
    let index = staging.path().join("apply.index");
    let env = [("GIT_INDEX_FILE", index.as_os_str())];
    git_env(root, &["read-tree", &ours], &env)?;
    // A fresh index has no stat data, so every file looks modified. Refresh
    // it against the worktree (which equals `ours`) so two-way read-tree can
    // update files safely.
    let _ = git_env(root, &["update-index", "-q", "--refresh"], &env);
    git_env(root, &["read-tree", "-m", "-u", &ours, &tree], &env)?;
    // Real index: reflect new HEAD, keep changes unstaged like before.
    git(root, &["reset", "--quiet", "--mixed", &new_head])?;
    // Files deleted by the merge that were untracked locally are handled by
    // read-tree -u. Report.
    Ok(MergeOutcome::Applied {
        cloud_snapshot,
        branch: local_branch,
        head: new_head,
    })
}

/// Tree object for the current working tree (tracked + untracked, not ignored).
fn snapshot_worktree_tree(root: &Path, scratch: &Path) -> Result<String> {
    let index = scratch.join("ours.index");
    let git_dir = PathBuf::from(git(root, &["rev-parse", "--absolute-git-dir"])?);
    std::fs::copy(git_dir.join("index"), &index).ok();
    let env = [("GIT_INDEX_FILE", index.as_os_str())];
    git_env(root, &["add", "-A", "--", "."], &env)?;
    git_env(root, &["write-tree"], &env)
}

// ---------------------------------------------------------------------------
// CLI glue
// ---------------------------------------------------------------------------

/// Prepare only the destination. In particular, do not read the transcript,
/// snapshot the repository, copy credentials, or change session ownership.
/// This may safely run while the source agent is still working.
fn prepare_host(target: &Target) -> Result<String> {
    let host = target.resolve_host()?;
    step(
        "prepare",
        "preparing destination while the current turn continues",
    );
    target.preflight(&host)?;
    step(
        "ready",
        "destination ready; waiting for a safe turn boundary",
    );
    Ok(host)
}

pub(crate) fn run_cli(command: super::args::CloudCommand) -> Result<()> {
    use super::args::CloudCommand as C;
    let target_of = |t: super::args::CloudMoveTarget| Target {
        host: t.host,
        remote_binary: t.remote_binary,
        transport: t
            .transport
            .or_else(|| std::env::var("JCODE_CLOUD_TRANSPORT").ok())
            .filter(|s| !s.trim().is_empty()),
    };
    match command {
        C::Sessions { .. } => unreachable!("handled by the cloud sessions dispatcher"),
        C::Prepare { target, json } => {
            let host = prepare_host(&target_of(target))?;
            if json {
                println!("{}", serde_json::json!({ "host": host }));
            } else {
                println!("Cloud host `{host}` is ready. No session was moved.");
            }
            Ok(())
        }
        C::Move {
            session,
            target,
            allow_active,
            dry_run,
            attach,
            json,
        } => {
            let target = target_of(target);
            let report = run_move(session.as_deref(), &target, allow_active, dry_run)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else if report.dry_run {
                println!(
                    "Dry run OK: {} staged on `{}` (epoch {}). Nothing changed locally.",
                    report.session_id, report.host, report.epoch
                );
            } else {
                println!(
                    "Moved {} to `{}`.\n  Attach:        {}\n  Bring it back: {}",
                    report.session_id, report.host, report.attach_command, report.return_command
                );
            }
            if attach && !report.dry_run {
                exec_attach(
                    &report.session_id,
                    &report.host,
                    report.repo_root.as_deref(),
                    &target,
                )?;
            }
            Ok(())
        }
        C::Return {
            session,
            target,
            refs_only,
            attach,
            json,
        } => {
            let report = run_return(session.as_deref(), &target_of(target), refs_only)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                let merge = match &report.merge {
                    MergeOutcome::NoRepo => "no repository".to_string(),
                    MergeOutcome::Unchanged => "no code changes on the cloud side".to_string(),
                    MergeOutcome::RefsOnly { cloud_snapshot } => format!(
                        "cloud work fetched to {SNAPSHOT_REF_PREFIX}/{}/cloud ({}) and not merged",
                        report.session_id,
                        &cloud_snapshot[..12.min(cloud_snapshot.len())]
                    ),
                    MergeOutcome::Applied { head, .. } => format!(
                        "cloud work merged into your checkout (HEAD {})",
                        &head[..12.min(head.len())]
                    ),
                    MergeOutcome::Conflicts { files, .. } => format!(
                        "CONFLICTS, working tree untouched. Cloud work is at {SNAPSHOT_REF_PREFIX}/{}/cloud. Conflicting files: {}",
                        report.session_id,
                        files.join(", ")
                    ),
                };
                println!(
                    "Returned {} from `{}` ({} messages). Code: {merge}.\n  Resume: jcode --resume {}",
                    report.session_id, report.host, report.messages, report.session_id
                );
            }
            if attach {
                let exe = std::env::current_exe()?;
                let mut command = Command::new(exe);
                command.arg("--resume").arg(&report.session_id);
                let error = crate::platform::replace_process(&mut command);
                bail!("failed to resume locally: {error}");
            }
            Ok(())
        }
        C::Where { session, json } => {
            let entries = run_where(session.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("No moved sessions.");
            } else {
                for entry in entries {
                    println!(
                        "{}  {}  epoch {}{}{}",
                        entry.session_id,
                        entry.location,
                        entry.epoch,
                        entry
                            .repo_root
                            .map(|r| format!("  {r}"))
                            .unwrap_or_default(),
                        entry
                            .local_ahead_of_sync
                            .filter(|n| *n > 0)
                            .map(|n| format!("  (local is {n} commit(s) ahead)"))
                            .unwrap_or_default()
                    );
                }
            }
            Ok(())
        }
        C::Attach { session } => {
            let (id, host) = away_host(session.as_deref())?;
            let root = storage::read_session_lease(&id).and_then(|l| l.repo_root);
            exec_attach(&id, &host, root.as_deref(), &Target::default())
        }
        C::Machine { action } => {
            use super::args::CloudMachineAction as A;
            let reply = match action {
                A::Status => super::cloud_managed::status()?,
                A::Wake => super::cloud_managed::connect(&mut |m| eprintln!("{m}"))?,
                A::Stop => super::cloud_managed::stop()?,
            };
            println!("{}", super::cloud_managed::describe(&reply));
            Ok(())
        }
        C::Proxy => super::cloud_managed::run_proxy(),
        C::Credentials {
            provider,
            account,
            export,
        } => run_credentials(&provider, &account, export),
        C::Receive { .. } => run_receive(),
        C::Activate { session, epoch } => run_activate(&session, epoch),
        C::Export { session, epoch } => run_export(&session, epoch),
        C::CancelReturn { session, epoch } => {
            let lease =
                storage::read_session_lease(&session).context("missing cloud session lease")?;
            if lease.epoch != epoch || lease.away_host.as_deref() != Some("returned") {
                bail!("cloud return can no longer be cancelled");
            }
            let mut sess = session::Session::load(&session)?;
            sess.migration_epoch = epoch;
            let empty = tempfile::tempdir()?;
            install_session(&sess, empty.path())?;
            storage::write_session_lease(&storage::SessionLease {
                away_host: None,
                ..lease
            })
        }
    }
}

/// Replace this process with a native SSH attach to the cloud session.
fn exec_attach(
    session_id: &str,
    host: &str,
    repo_root: Option<&str>,
    target: &Target,
) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = Command::new(exe);
    command.arg("--ssh").arg(host);
    if let Some(binary) = &target.remote_binary {
        command.arg("--ssh-binary").arg(binary);
    }
    if let Some(root) = repo_root {
        command.arg("--remote-working-dir").arg(root);
        command.env("JCODE_SSH_LOCAL_WORKING_DIR", root);
    }
    command.arg("--resume").arg(session_id);
    let error = crate::platform::replace_process(&mut command);
    bail!("failed to attach to `{host}`: {error}")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_session_id(session_ref: Option<&str>) -> Result<String> {
    if let Some(reference) = session_ref.filter(|s| !s.trim().is_empty()) {
        if storage::read_session_lease(reference).is_some() {
            return Ok(reference.to_string());
        }
        return session::find_session_by_name_or_id(reference);
    }
    if let Ok(id) = std::env::var("JCODE_SESSION_ID")
        && !id.trim().is_empty()
    {
        return Ok(id);
    }
    bail!(
        "no session given: pass --session <id> (inside a jcode agent shell JCODE_SESSION_ID is used)"
    )
}

fn install_session(sess: &session::Session, stage: &Path) -> Result<()> {
    let path = session::session_path(&sess.id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Replace snapshot and drop the old journal: the incoming snapshot is
    // the complete, newer transcript.
    let journal = session::session_journal_path_from_snapshot(&path);
    let _ = std::fs::remove_file(&journal);
    storage::write_json(&path, sess)?;
    let todos = stage.join("todos.json");
    if todos.is_file() {
        let dest = todo_file(&sess.id)?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&todos, dest)?;
    }
    let memory = stage.join("project_memory.json");
    if memory.is_file()
        && let Some(dir) = sess.working_dir.as_deref()
    {
        let dest = crate::memory::project_memory_file(Path::new(dir))?;
        if !dest.exists() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&memory, dest)?;
        }
    }
    Ok(())
}

fn append_system_notice(sess: &mut session::Session, text: &str) {
    append_arrival(sess, &[], "", text);
}

/// Tool calls in the final assistant message that have no result yet. When
/// the agent itself runs `/cloud` mid-turn, its own bash call is one of these.
fn dangling_tool_use_ids(sess: &session::Session) -> Vec<String> {
    use crate::message::{ContentBlock, Role};
    let Some(last_assistant) = sess
        .messages
        .iter()
        .rposition(|m| m.role == Role::Assistant)
    else {
        return Vec::new();
    };
    let answered: std::collections::HashSet<&str> = sess.messages[last_assistant + 1..]
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    sess.messages[last_assistant]
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } if !answered.contains(id.as_str()) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn append_arrival(sess: &mut session::Session, dangling: &[String], host: &str, text: &str) {
    let mut blocks: Vec<crate::message::ContentBlock> = dangling
        .iter()
        .map(|id| crate::message::ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: format!(
                "jcode cloud move completed. This session now runs on cloud host `{host}`. This tool call was interrupted by the move; rerun it here if its result still matters."
            ),
            is_error: None,
        })
        .collect();
    blocks.push(crate::message::ContentBlock::Text {
        text: format!("<system-reminder>\n{text}\n</system-reminder>"),
        cache_control: None,
    });
    // Tool results must stay provider-visible and paired with their calls.
    let role = dangling
        .is_empty()
        .then_some(session::StoredDisplayRole::System);
    sess.add_message_with_display_role(crate::message::Role::User, blocks, role);
}

fn arrival_notice(manifest: &MoveManifest) -> String {
    let mut lines = vec![format!(
        "This session was moved from `{}` to cloud host `{}` at {} (migration epoch {}). You are now running on the cloud host. The conversation, todos and project memory came along.",
        manifest.source_host, manifest.target_host, manifest.created_at, manifest.epoch
    )];
    if let Some(repo) = &manifest.repo {
        lines.push(format!(
            "The repository is restored at the same path `{}` on branch `{}` at {}{}. Paths in earlier messages are still valid.",
            repo.root,
            repo.branch.as_deref().unwrap_or("(detached)"),
            &repo.head[..repo.head.len().min(12)],
            if repo.dirty {
                ", with the same uncommitted and untracked changes"
            } else {
                ""
            }
        ));
        if !repo.ignored_files.is_empty() {
            lines.push(format!(
                "Ignored files copied: {}.",
                repo.ignored_files.join(", ")
            ));
        }
        lines.push("Build caches (target/, node_modules/, etc.) did not come along. The first build here will be cold. Install dependencies if a command fails for that reason.".to_string());
    }
    if !manifest.toolchain_hints.is_empty() {
        lines.push(format!(
            "Toolchain hints from the repo: {}. Check these tools exist on this host before relying on them.",
            manifest.toolchain_hints.join(", ")
        ));
    }
    if !manifest.not_transferred.is_empty() {
        lines.push(format!(
            "These were running locally and did NOT move: {}. Restart them here if you still need them.",
            manifest.not_transferred.join("; ")
        ));
    }
    if !manifest.mcp_servers.is_empty() {
        lines.push(format!(
            "Local MCP servers ({}) are not connected here unless this host configures them too.",
            manifest.mcp_servers.join(", ")
        ));
    }
    lines.push("There is no local display, desktop, browser or ssh-agent on this host. Keep working on the task exactly where you left off. The user can bring the session back with /local (`jcode cloud return`). Git will merge your work with any local edits.".to_string());
    lines.join("\n")
}

fn describe_local_only_state(session_id: &str) -> Vec<String> {
    let mut items = Vec::new();
    let dir = std::env::temp_dir().join("jcode-bg-tasks");
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(raw) = std::fs::read(&path) else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else {
                continue;
            };
            if value.get("session_id").and_then(|v| v.as_str()) != Some(session_id) {
                continue;
            }
            if value.get("status").and_then(|v| v.as_str()) != Some("running") {
                continue;
            }
            let name = value
                .get("display_name")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("tool_name").and_then(|v| v.as_str()))
                .unwrap_or("task");
            let id = value.get("task_id").and_then(|v| v.as_str()).unwrap_or("?");
            items.push(format!("background task {id} ({name})"));
        }
    }
    items
}

fn toolchain_hints(root: Option<&Path>) -> Vec<String> {
    let Some(root) = root else { return Vec::new() };
    [
        ("rust-toolchain.toml", "rust (rust-toolchain.toml)"),
        ("rust-toolchain", "rust (rust-toolchain)"),
        ("Cargo.toml", "cargo"),
        (".nvmrc", "node (.nvmrc)"),
        ("package.json", "node/npm"),
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("pyproject.toml", "python (pyproject)"),
        ("requirements.txt", "python (requirements.txt)"),
        ("go.mod", "go"),
        (".tool-versions", "asdf (.tool-versions)"),
        ("flake.nix", "nix flake"),
        (".devcontainer/devcontainer.json", "devcontainer"),
        ("Dockerfile", "docker"),
    ]
    .iter()
    .filter(|(file, _)| root.join(file).exists())
    .map(|(_, label)| label.to_string())
    .collect()
}

fn configured_mcp_servers() -> Vec<String> {
    let Ok(dir) = storage::jcode_dir() else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read(dir.join("mcp.json")) else {
        return Vec::new();
    };
    serde_json::from_slice::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| {
            v.get("servers")
                .and_then(|s| s.as_object())
                .map(|m| m.keys().cloned().collect())
        })
        .unwrap_or_default()
}

fn local_host_label() -> String {
    std::env::var("JCODE_CLOUD_HOST_LABEL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".to_string())
}

fn todo_file(session_id: &str) -> Result<PathBuf> {
    Ok(storage::jcode_dir()?
        .join("todos")
        .join(format!("{session_id}.json")))
}

fn inbox_dir() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("cloud_inbox"))
}

fn copy_if_exists(from: &Path, to: &Path) -> Result<()> {
    if from.is_file() {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&raw).with_context(|| format!("parse {}", path.display()))
}

fn tar_dir(dir: &Path, out: &Path, names: &[&str]) -> Result<()> {
    let present: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| dir.join(n).exists())
        .collect();
    let mut command = Command::new("tar");
    command
        .arg("-cf")
        .arg(out)
        .arg("-C")
        .arg(dir)
        .args(&present);
    run_ok(command, "tar")
}

fn untar(tarball: &Path, dir: &Path) -> Result<()> {
    let mut command = Command::new("tar");
    command
        .arg("--no-same-owner")
        .arg("-xf")
        .arg(tarball)
        .arg("-C")
        .arg(dir);
    run_ok(command, "untar")
}

fn run_ok(mut command: Command, what: &str) -> Result<()> {
    let output = command
        .output()
        .with_context(|| format!("failed to run {what}"))?;
    if !output.status.success() {
        bail!(
            "{what} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn step(phase: &str, detail: &str) {
    if std::env::var_os("JCODE_CLOUD_QUIET").is_none() {
        eprintln!("[cloud {phase}] {detail}");
    }
}

// ---------------------------------------------------------------------------
// TUI handoff (`/cloud`, `/local`)
// ---------------------------------------------------------------------------

static STASHED_HANDOFF: std::sync::Mutex<Option<crate::tui::CloudHandoff>> =
    std::sync::Mutex::new(None);

pub(crate) fn stash_handoff(handoff: crate::tui::CloudHandoff) {
    *STASHED_HANDOFF
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(handoff);
}

pub(crate) fn take_stashed_handoff() -> Option<crate::tui::CloudHandoff> {
    STASHED_HANDOFF
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Replace this client with one attached where the session now lives.
pub(crate) fn exec_handoff(handoff: crate::tui::CloudHandoff) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = Command::new(&exe);
    // Never inherit the previous SSH attach identity.
    for var in [
        "JCODE_SSH_REMOTE",
        "JCODE_SSH_BINARY",
        "JCODE_SSH_WORKING_DIR",
        "JCODE_SSH_SERVER_SOCKET",
        "JCODE_SSH_LOCAL_WORKING_DIR",
        "JCODE_SSH_LOCAL_IS_SOURCE",
        "JCODE_SSH_LAST_TRANSFER_AT",
        "JCODE_SSH_CONFIG",
        "JCODE_SOCKET",
    ] {
        command.env_remove(var);
    }
    match handoff {
        crate::tui::CloudHandoff::Remote {
            session_id,
            host,
            working_dir,
        } => {
            command.arg("--ssh").arg(&host);
            if let Ok(binary) = std::env::var("JCODE_CLOUD_REMOTE_BINARY") {
                command.arg("--ssh-binary").arg(binary);
            }
            if let Some(dir) = working_dir {
                command.arg("--remote-working-dir").arg(&dir);
                command.env("JCODE_SSH_LOCAL_WORKING_DIR", &dir);
            }
            command.arg("--resume").arg(&session_id);
            // Only an explicit handoff may preview the matching local transcript.
            command.env("JCODE_CLOUD_HANDOFF_PREVIEW_SESSION", &session_id);
            command.env(
                "JCODE_CLOUD_CONTINUE_MESSAGE",
                "[jcode cloud] This session just moved to the cloud host. Read the migration notice above, verify the environment you need is here, then continue the task exactly where you left off.",
            );
        }
        crate::tui::CloudHandoff::Local { session_id } => {
            if let Some(root) = storage::read_session_lease(&session_id).and_then(|l| l.repo_root) {
                command.current_dir(root);
            }
            command.arg("--resume").arg(&session_id);
        }
    }
    let error = crate::platform::replace_process(&mut command);
    bail!("failed to reattach after cloud move: {error}")
}
