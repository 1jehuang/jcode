//! SSH transport, host capability checks, and private credential channels.

#[cfg(all(test, unix))]
mod tests;

use super::{
    CloudAccount, REQUIRED_REMOTE_COMMANDS, preflight_is_fresh, record_preflight, shell_quote,
    step, tar_dir, timed_move_stage,
};
use crate::auth::transfer;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// `ssh` exits 255 for connection-level failures (DNS, refused, reset, timeout
/// while the machine wakes). Those are safe to retry for idempotent calls.
const SSH_CONNECTION_FAILURE: i32 = 255;
const SSH_ATTEMPTS: u32 = 3;

fn ssh_connection_failed(status: &std::process::ExitStatus) -> bool {
    status.code() == Some(SSH_CONNECTION_FAILURE)
}

/// Marks an SSH transport drop (exit 255) so idempotent steps can retry.
#[derive(Debug)]
struct SshDropped(String);

impl std::fmt::Display for SshDropped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SshDropped {}

/// Retry an idempotent remote step when only the SSH connection dropped.
pub(super) fn retry_ssh<T>(host: &str, what: &str, mut op: impl FnMut() -> Result<T>) -> Result<T> {
    let mut attempt = 1;
    loop {
        match op() {
            Err(error)
                if error.downcast_ref::<SshDropped>().is_some() && attempt < SSH_ATTEMPTS =>
            {
                step(
                    "transfer",
                    &format!(
                        "connection to `{host}` dropped during {what}, retrying ({attempt}/{SSH_ATTEMPTS})"
                    ),
                );
                std::thread::sleep(std::time::Duration::from_secs(2 * attempt as u64));
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// Run an idempotent remote command, retrying connection-level SSH failures
/// with a short backoff. Remote command failures are returned immediately.
fn output_with_ssh_retry(
    mut build: impl FnMut() -> Result<Command>,
    host: &str,
) -> Result<std::process::Output> {
    let mut attempt = 1;
    loop {
        let output = build()?
            .output()
            .with_context(|| format!("failed to reach cloud host `{host}`"))?;
        if output.status.success()
            || !ssh_connection_failed(&output.status)
            || attempt >= SSH_ATTEMPTS
        {
            return Ok(output);
        }
        step(
            "prepare",
            &format!("connection to `{host}` dropped, retrying ({attempt}/{SSH_ATTEMPTS})"),
        );
        std::thread::sleep(std::time::Duration::from_secs(2 * attempt as u64));
        attempt += 1;
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Target {
    pub host: Option<String>,
    pub remote_binary: Option<String>,
    pub transport: Option<String>,
}

impl Target {
    pub(super) fn resolve_host(&self) -> Result<String> {
        if let Some(host) = self.host.clone().filter(|h| !h.trim().is_empty()) {
            return Ok(host);
        }
        if let Ok(host) = std::env::var("JCODE_CLOUD_HOST")
            && !host.trim().is_empty()
        {
            return Ok(host);
        }
        if super::super::cloud_managed::account_available() {
            return Ok(super::super::cloud_managed::HOST.to_string());
        }
        bail!(
            "no cloud machine: sign in with `jcode login` to use the Jcode Cloud machine included with your subscription, or pass --host <ssh-alias> / set JCODE_CLOUD_HOST"
        )
    }

    pub(super) fn remote_binary(&self) -> String {
        self.remote_binary
            .clone()
            .or_else(|| match std::env::var("JCODE_CLOUD_REMOTE_BINARY") {
                Ok(binary) => Some(binary),
                Err(std::env::VarError::NotPresent) => None,
                Err(std::env::VarError::NotUnicode(_)) => {
                    crate::logging::info(
                        "cloud move: ignoring non-Unicode JCODE_CLOUD_REMOTE_BINARY",
                    );
                    None
                }
            })
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "jcode".to_string())
    }

    /// Fail fast, with an actionable message, when the cloud host's jcode is
    /// too old to speak the move protocol.
    pub(super) fn preflight(&self, host: &str) -> Result<()> {
        if self.transport.is_none() && preflight_is_fresh(&self.preflight_key(host)) {
            crate::logging::info("cloud move: host preflight skipped, prepared moments ago");
            return Ok(());
        }
        let result = self.preflight_uncached(host);
        if result.is_ok() && self.transport.is_none() {
            record_preflight(&self.preflight_key(host));
        }
        result
    }

    fn preflight_key(&self, host: &str) -> String {
        // Capability changes invalidate markers written by older clients.
        format!(
            "{host}\n{}\n{}",
            self.remote_binary(),
            REQUIRED_REMOTE_COMMANDS.join(",")
        )
    }

    fn preflight_uncached(&self, host: &str) -> Result<()> {
        timed_move_stage("host preflight", || {
            if super::super::cloud_managed::is_managed(host) && self.transport.is_none() {
                // Usually the machine is already running and this is one
                // authorization request. `connect` reports real wake progress.
                step("prepare", "connecting to your Jcode Cloud machine");
                timed_move_stage("cloud api connect", || {
                    if super::super::cloud_managed::master_is_live() {
                        return Ok(());
                    }
                    super::super::cloud_managed::connect(&mut |m| step("prepare", m)).map(|_| ())
                })?;
                let home = dirs::home_dir()
                    .context("locate local home directory for cloud preparation")?
                    .to_string_lossy()
                    .into_owned();
                timed_move_stage("host prepare script", || {
                    let prepared =
                        self.run_shell(host, &super::super::cloud_managed::prepare_script(&home)?);
                    if let Err(error) = prepared {
                        // A development protocol may not exist in any published
                        // release yet. Use the running native executable, never
                        // a stale PATH launcher, and verify it before switching.
                        if cfg!(all(target_os = "linux", target_arch = "x86_64"))
                            && !jcode_build_meta::is_release_build()
                            && std::env::var_os("JCODE_CLOUD_RELEASE").is_none()
                            && self.remote_binary.is_none()
                            && std::env::var_os("JCODE_CLOUD_REMOTE_BINARY").is_none()
                        {
                            step(
                                "prepare",
                                "published cloud CLI is unavailable or incompatible; syncing this development binary",
                            );
                            self.prepare_development_binary(host, &home)
                                .with_context(|| format!("prepare development cloud CLI (published preparation failed: {error:#})"))?;
                        } else {
                            return Err(error).context("prepare the Jcode Cloud machine");
                        }
                    }
                    Ok(())
                })?;
            }
            // Preserve custom transports that only support one remote invocation.
            if self.transport.is_none() {
                let binary = shell_quote(&self.remote_binary());
                let mut script =
                    String::from("PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; export PATH\n");
                // Distinguish a missing or broken binary (exit 127/126, or a
                // dangling wrapper target) from an old one lacking a command.
                script.push_str(&format!(
                    "version=$({binary} --no-update --no-selfdev --version 2>&1) || {{ printf 'cloud jcode is not runnable: %s\\n' \"$version\" >&2; exit 3; }}\n"
                ));
                for command in REQUIRED_REMOTE_COMMANDS {
                    script.push_str(&format!(
                    "out=$({binary} --no-update --no-selfdev cloud {command} --help 2>&1 >/dev/null) || {{ printf '%s\\ncloud protocol probe failed: {command} (remote %s)\\n' \"$out\" \"$version\" >&2; exit 4; }}\n"
                ));
                }
                let output = output_with_ssh_retry(
                    || {
                        let mut command = Self::ssh_command(host)?;
                        command
                            .arg(&script)
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::piped());
                        Ok(command)
                    },
                    host,
                )?;
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return match output.status.code() {
                    Some(0) => Ok(()),
                    Some(3) => bail!(
                        "jcode on `{host}` could not start. Reinstall it there (or pass --remote-binary / set JCODE_CLOUD_REMOTE_BINARY): {stderr}"
                    ),
                    Some(4) => bail!(
                        "jcode on `{host}` is too old for cloud handoff. Update jcode on the cloud host (or pass --remote-binary / set JCODE_CLOUD_REMOTE_BINARY to a newer build): {stderr}"
                    ),
                    Some(SSH_CONNECTION_FAILURE) => bail!(
                        "could not connect to cloud host `{host}` after {SSH_ATTEMPTS} attempts: {stderr}"
                    ),
                    _ => bail!(
                        "could not run jcode on `{host}` ({}): {stderr}",
                        output.status
                    ),
                };
            }
            for command in REQUIRED_REMOTE_COMMANDS {
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

    /// Transfer only our executable over the existing authenticated SSH channel.
    /// The installer probes it before an atomic switch and rolls back on failure.
    /// No checkout, credentials, lease, or running daemon is touched.
    fn prepare_development_binary(&self, host: &str, home: &str) -> Result<()> {
        let executable =
            std::env::current_exe().context("locate running development executable")?;
        let bundle = tempfile::tempdir()?;
        let payload = bundle.path().join("payload");
        std::fs::create_dir(&payload)?;
        std::fs::copy(&executable, payload.join("jcode.bin"))?;
        // Bundle the current executable's dependency closure, including its
        // loader. Never install these libraries into the cloud system paths.
        let dependencies = Command::new("ldd")
            .arg(&executable)
            .env_remove("LD_PRELOAD")
            .env_remove("LD_LIBRARY_PATH")
            .output()
            .context("inspect development executable libraries")?;
        if !dependencies.status.success() {
            bail!(
                "cannot bundle development executable libraries: {}",
                String::from_utf8_lossy(&dependencies.stderr)
            );
        }
        let mut members = vec!["jcode".to_string(), "jcode.bin".to_string()];
        let mut loader = None;
        for line in String::from_utf8_lossy(&dependencies.stdout).lines() {
            if line.contains("not found") {
                bail!("missing development dependency: {line}");
            }
            let Some(path) = line.split_whitespace().find(|part| part.starts_with('/')) else {
                continue;
            };
            let path = Path::new(path);
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("invalid shared library filename")?;
            if !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            {
                bail!("unsupported shared library filename {name:?}");
            }
            if name.starts_with("ld-linux") {
                loader = Some(name.to_string());
            }
            if !members.iter().any(|member| member == name) {
                std::fs::copy(path, payload.join(name))?;
                members.push(name.to_string());
            }
        }
        let loader = loader.context("development executable has no supported Linux loader")?;
        std::fs::write(
            payload.join("jcode"),
            format!(
                "#!/bin/sh\nset -eu\np=$(readlink -f \"$0\")\nd=$(dirname \"$p\")\nexec \"$d/{loader}\" --library-path \"$d\" \"$p.bin\" \"$@\"\n"
            ),
        )?;
        let archive = bundle.path().join("development.tar.gz");
        let member_refs = members.iter().map(String::as_str).collect::<Vec<_>>();
        tar_dir(&payload, &archive, &member_refs)?;
        let mut file = std::fs::File::open(&archive)?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        let checksum = hex::encode(digest.finalize());
        // The candidate is inside a private temporary directory. The outer
        // shell owns its cleanup independently of the installer's own traps.
        let prepare =
            super::super::cloud_managed::prepare_script_with_candidate(home, Some("jcode"))?;
        let script = format!(
            "set -eu\numask 077\nupload=$(mktemp -d)\ntrap 'rm -rf \"$upload\"' EXIT\ncat > \"$upload/bundle.tar.gz\"\nprintf '%s  %s\\n' '{checksum}' \"$upload/bundle.tar.gz\" | sha256sum --check --status\ntar xzf \"$upload/bundle.tar.gz\" --no-same-owner --no-same-permissions -C \"$upload\"\nchmod 755 \"$upload/jcode\" \"$upload/jcode.bin\" \"$upload\"/ld-linux*.so*\n(\ncd \"$upload\"\n{prepare}\n)\n"
        );
        let mut command = Self::ssh_command(host)?;
        command.arg(format!("bash -c {}", shell_quote(&script)));
        command
            .stdin(Stdio::from(std::fs::File::open(&archive)?))
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let output = command
            .output()
            .context("send development executable to cloud")?;
        if !output.status.success() {
            bail!(
                "cloud development CLI install failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// `ssh` with the options every cloud call uses, including the private
    /// config for the managed machine.
    pub(super) fn ssh_command(host: &str) -> Result<Command> {
        let mut command = Command::new("ssh");
        if super::super::cloud_managed::is_managed(host) {
            command.args(super::super::cloud_managed::ssh_args()?);
        }
        command.args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=30", "--"]);
        command.arg(host);
        Ok(command)
    }

    /// Run a raw shell script on the target (managed-machine preparation).
    fn run_shell(&self, host: &str, script: &str) -> Result<()> {
        let mut attempt = 1;
        loop {
            let mut command = Self::ssh_command(host)?;
            command.arg("bash -s");
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            let mut child = command.spawn()?;
            // SSH connection status controls retries, but successful execution
            // must also confirm the entire preparation script was written.
            let written = child
                .stdin
                .take()
                .context("ssh stdin")?
                .write_all(script.as_bytes());
            let output = child.wait_with_output()?;
            if output.status.success() {
                written.context("send cloud preparation script")?;
                return Ok(());
            }
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            // The prepare script is idempotent and serialized by its own lock.
            if ssh_connection_failed(&output.status) && attempt < SSH_ATTEMPTS {
                step(
                    "prepare",
                    &format!("connection to `{host}` dropped, retrying ({attempt}/{SSH_ATTEMPTS})"),
                );
                std::thread::sleep(std::time::Duration::from_secs(2 * attempt as u64));
                attempt += 1;
                continue;
            }
            if stderr.is_empty() {
                bail!("remote script failed ({})", output.status);
            }
            bail!("{stderr}");
        }
    }

    pub(super) fn remote_command(&self, host: &str, args: &[&str]) -> Result<Command> {
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
    pub(super) fn run_remote(
        &self,
        host: &str,
        args: &[&str],
        stdin: Option<&Path>,
    ) -> Result<Vec<u8>> {
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
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if ssh_connection_failed(&output.status) {
                return Err(SshDropped(format!(
                    "lost the connection to cloud host `{host}`: {stderr}"
                ))
                .into());
            }
            bail!(
                "remote `jcode {}` on `{host}` failed ({}):\n{stderr}",
                args.iter().take(2).copied().collect::<Vec<_>>().join(" "),
                output.status,
            );
        }
        Ok(output.stdout)
    }

    /// Same private channel as [`Self::exchange_credentials`], for the router
    /// bundle. Remote stderr is discarded and output is not expected.
    pub(super) fn send_router_credentials(&self, host: &str, payload: &[u8]) -> Result<()> {
        let mut command = self.remote_command(host, &["cloud", "router-credentials"])?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .context("start private router credential transfer")?;
        let written = child
            .stdin
            .take()
            .context("router credential stdin")?
            .write_all(payload);
        let status = child
            .wait()
            .context("wait for router credential transfer")?;
        if written.is_ok() && ssh_connection_failed(&status) {
            return Err(SshDropped(format!(
                "lost the connection to cloud host `{host}` during provider key transfer"
            ))
            .into());
        }
        if written.is_err() || !status.success() {
            bail!("provider key transfer failed ({status}); the session was not moved");
        }
        Ok(())
    }

    /// Secrets travel only in an SSH pipe, never argv, a temporary file, or the
    /// session archive. Discard remote stderr: even a broken transport must not
    /// reflect a credential into the local transcript or logs.
    pub(super) fn exchange_credentials(
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
            let writer = if let Some(bytes) = payload {
                let mut stdin = child.stdin.take().context("piped credential stdin")?;
                Some(scope.spawn(move || stdin.write_all(bytes)))
            } else {
                None
            };
            let mut bytes = Vec::new();
            child
                .stdout
                .take()
                .context("cloud credential stdout")?
                .take((transfer::MAX_TRANSFER_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| anyhow::anyhow!("could not read cloud credential response"))?;
            if bytes.len() > transfer::MAX_TRANSFER_BYTES {
                kill_credential_child(&mut child);
                bail!("cloud credential response exceeds the size limit");
            }
            if let Some(writer) = writer {
                writer
                    .join()
                    .map_err(|_| anyhow::anyhow!("cloud credential writer failed"))?
                    .map_err(|_| anyhow::anyhow!("could not send cloud credentials"))?;
            }
            let status = child.wait().context("wait for cloud credential transfer")?;
            if ssh_connection_failed(&status) {
                return Err(SshDropped(format!(
                    "lost the connection to cloud host `{host}` during credential transfer"
                ))
                .into());
            }
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
            kill_credential_child(&mut child);
            if let Err(error) = child.wait() {
                crate::logging::info(&format!(
                    "cloud move: credential child cleanup wait failed: {error}"
                ));
            }
        }
        result
    }
}

/// Cleanup is best effort after the primary credential error, but never silent.
fn kill_credential_child(child: &mut std::process::Child) {
    if let Err(error) = child.kill() {
        crate::logging::info(&format!(
            "cloud move: credential child cleanup kill failed: {error}"
        ));
    }
}
