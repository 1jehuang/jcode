//! Explicit managed-host runtime updates. Upload only the package the owner
//! selected, never source, credentials, or the current laptop executable.
use super::args::CloudRuntimeCommand;
use super::cloud_managed;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};

const HELPER: &str = "/opt/jcode-cloud/runtime-update";
/// Must match `jcode-app-core`'s managed live reload capability marker. A
/// daemon binary containing it re-execs the managed selection on reload.
const LIVE_RELOAD_MARKER: &str = "jcode-managed-cloud-live-reload-v1";
const MAX_PACKAGE_BYTES: u64 = 256 * 1024 * 1024;

/// All remote arguments are fixed verbs, validated release tags, or hex digests.
/// No local filename is interpreted by the remote shell.
fn remote_command(verb: &str, value: Option<&str>) -> Result<String> {
    match (verb, value) {
        ("status" | "check" | "use-stable", None) => {}
        ("push", Some(digest))
            if digest.len() == 64
                && digest
                    .bytes()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) => {}
        ("pin", Some(version)) if valid_release_tag(version) => {}
        _ => bail!("invalid managed runtime operation"),
    }
    let mut command = format!(
        "test -x {HELPER} || {{ echo 'Managed runtime updater is not installed on this host. Ask the cloud operator to migrate it.' >&2; exit 1; }}; sudo -n {HELPER} {verb}"
    );
    if let Some(value) = value {
        command.push(' ');
        command.push_str(value);
    }
    Ok(command)
}

/// Remote reload: wait until no session is streaming, then restart the
/// managed service. `ExecStartPre` activates any pending runtime and
/// `ExecStopPost` rolls back a runtime that fails to start. Only an integer
/// and a fixed flag are interpolated.
fn reload_command(wait_secs: u64, force: bool) -> String {
    let force = if force { 1 } else { 0 };
    let marker = LIVE_RELOAD_MARKER;
    format!(
        r#"set -u
export XDG_RUNTIME_DIR=/run/user/$(id -u)
say() {{ echo "[cloud reload] $*" >&2; }}
streaming() {{
  activity=$(/opt/jcode-cloud/jcode --no-update --no-selfdev menubar --json 2>/dev/null) || return 1
  printf '%s' "$activity" | python3 -c 'import json,sys; n=json.load(sys.stdin).get("streaming"); sys.exit(1) if type(n) is not int or n < 0 else print(n)' 2>/dev/null
}}
deadline=$(( $(date +%s) + {wait_secs} ))
last=""
# A daemon with managed live reload keeps in-flight turns across the re-exec,
# so it never needs to wait for idle. Only the restart fallback does.
daemon=$(pgrep -u "$(id -u)" -f '/opt/jcode-cloud/.* serve( |$)' | head -n1)
live_capable=0
if [ -n "$daemon" ] && grep -qa {marker} "/proc/$daemon/exe" 2>/dev/null; then live_capable=1; fi
wait_idle() {{
  [ {force} = 1 ] && return 0
  while :; do
    n=$(streaming) || {{ say "cannot verify cloud activity; not restarting"; exit 3; }}
    [ "$n" = 0 ] && return 0
    if [ "$(date +%s)" -ge "$deadline" ]; then say "still $n session(s) mid-turn after {wait_secs}s; not restarting. Retry later or use --force."; exit 3; fi
    [ "$n" != "$last" ] && say "waiting for $n session(s) to finish their turn"
    last=$n; sleep 5
  done
}}
if [ "$live_capable" = 0 ]; then wait_idle; fi
before=$(readlink /opt/jcode-cloud/jcode)
pending=0
sudo -n /usr/local/sbin/jcode-cloud-runtime status 2>/dev/null | grep -q '"pending": {{' && pending=1
# Live path: a daemon built with managed live reload re-execs the selected
# runtime in place, so sessions, clients and in-flight turns survive the same
# way a local self-dev reload does. Older daemons fall through to a restart.
if [ "$live_capable" = 1 ]; then
  live_ok=1
  if [ "$pending" = 1 ]; then
    sudo -n /usr/local/sbin/jcode-cloud-runtime activate-live >/dev/null || live_ok=0
  fi
  if [ "$live_ok" = 1 ]; then
    target=$(readlink -f /opt/jcode-cloud/jcode)
    say "live reloading daemon (sessions stay attached)"
    JCODE_SOCKET="$XDG_RUNTIME_DIR/jcode.sock" /opt/jcode-cloud/jcode --no-update --no-selfdev server reload --force --json >/dev/null 2>&1 || live_ok=0
    if [ "$live_ok" = 1 ] && [ "$(readlink /proc/$daemon/exe 2>/dev/null)" = "$target" ] && [ -S "$XDG_RUNTIME_DIR/jcode.sock" ]; then
      [ "$pending" = 1 ] && say "activated new runtime"
      say "daemon ready: $(/opt/jcode-cloud/jcode --version 2>/dev/null)"
      exit 0
    fi
  fi
  say "live reload did not land; falling back to a restart"
  if [ "$pending" = 1 ]; then
    sudo -n /usr/local/sbin/jcode-cloud-runtime rollback-live >/dev/null 2>&1 && say "rolled back the live-activated runtime"
    pending=0
  fi
  before=$(readlink /opt/jcode-cloud/jcode)
  # The restart fallback interrupts turns, so it still requires idle.
  wait_idle
fi
say "restarting cloud daemon"
# A daemon may have handed itself off outside systemd on an earlier stop. If it
# keeps the socket, the unit start fails and the new runtime is falsely rolled
# back. Turns are idle here (or --force), so stop every daemon first.
systemctl --user stop jcode-cloud.service 2>/dev/null
for p in $(pgrep -u "$(id -u)" -f '/opt/jcode-cloud/.* serve( |$)'); do say "stopping detached daemon pid $p"; kill -TERM "$p" 2>/dev/null; done
for i in $(seq 1 20); do pgrep -u "$(id -u)" -f '/opt/jcode-cloud/.* serve( |$)' >/dev/null || break; sleep 1; done
pkill -KILL -u "$(id -u)" -f '/opt/jcode-cloud/.* serve( |$)' 2>/dev/null
rm -f "$XDG_RUNTIME_DIR/jcode.sock"
systemctl --user reset-failed jcode-cloud.service 2>/dev/null
systemctl --user start jcode-cloud.service || {{ say "start failed"; exit 1; }}
for i in $(seq 1 60); do
  pid=$(systemctl --user show jcode-cloud.service --property=MainPID --value)
  if [ "${{pid:-0}}" != 0 ] && [ -S "$XDG_RUNTIME_DIR/jcode.sock" ]; then
    sleep 3
    pid2=$(systemctl --user show jcode-cloud.service --property=MainPID --value)
    if [ "$pid2" = "$pid" ]; then
      after=$(readlink /opt/jcode-cloud/jcode)
      if [ "$pending" = 1 ] && [ "$after" = "$before" ]; then
        say "new runtime failed to start and was rolled back; daemon is on: $(/opt/jcode-cloud/jcode --version 2>/dev/null)"
        exit 1
      fi
      [ "$after" = "$before" ] || say "activated new runtime"
      say "daemon ready: $(/opt/jcode-cloud/jcode --version 2>/dev/null)"
      exit 0
    fi
  fi
  sleep 1
done
say "daemon did not become ready; see: journalctl --user -u jcode-cloud.service"
sudo -n /usr/local/sbin/jcode-cloud-runtime status >&2 || true
exit 1
"#
    )
}

fn run_remote(script: &str, input: Option<File>) -> Result<std::process::ExitStatus> {
    let args = cloud_managed::ssh_args()?;
    Command::new("ssh")
        .args(args)
        .args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=30"])
        .arg(cloud_managed::HOST)
        .arg(script)
        .stdin(input.map(Stdio::from).unwrap_or_else(Stdio::null))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("connect to the managed cloud host over SSH")
}

fn reload(wait_secs: u64, force: bool) -> Result<()> {
    let status = run_remote(&reload_command(wait_secs, force), None)?;
    match status.code() {
        Some(0) => Ok(()),
        Some(3) => bail!("cloud reload deferred: turns were still running. Nothing was restarted."),
        _ => bail!("cloud reload failed ({status})"),
    }
}

/// Build a portable Linux x86_64 package from the jcode checkout containing
/// the current directory.
fn build_from_source() -> Result<std::path::PathBuf> {
    let root = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("run git")?;
    if !root.status.success() {
        bail!("--from-source must run inside a jcode checkout");
    }
    let root = std::path::PathBuf::from(String::from_utf8_lossy(&root.stdout).trim());
    let script = root.join("scripts/package_cloud_runtime.sh");
    if !script.is_file() {
        bail!("{} is not a jcode checkout", root.display());
    }
    let target = "x86_64-unknown-linux-gnu";
    eprintln!(
        "[cloud build] building portable Linux runtime in {}",
        root.display()
    );
    let builder = if Command::new("justrust").arg("--version").output().is_ok() {
        "justrust"
    } else {
        "cargo"
    };
    let status = Command::new(builder)
        .current_dir(&root)
        .args([
            "zigbuild",
            "--profile",
            "selfdev",
            "-p",
            "jcode",
            "--bin",
            "jcode",
            "--target",
            &format!("{target}.2.17"),
            "--features",
            "linux-compat-vendored-openssl",
        ])
        .status()
        .with_context(|| format!("run {builder} zigbuild"))?;
    if !status.success() {
        bail!("portable Linux build failed ({status})");
    }
    let binary = root
        .join("target")
        .join(target)
        .join("selfdev")
        .join("jcode");
    let output = root.join("target/cloud-runtime/jcode-linux-x86_64.tar.gz");
    eprintln!("[cloud build] packaging");
    let status = Command::new(&script)
        .arg(&binary)
        .arg(&output)
        .stdout(Stdio::null())
        .status()
        .context("run package_cloud_runtime.sh")?;
    if !status.success() {
        bail!("packaging the cloud runtime failed ({status})");
    }
    Ok(output)
}

fn valid_release_tag(value: &str) -> bool {
    let Some(version) = value.strip_prefix('v') else {
        return false;
    };
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|c| c.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
                && part.parse::<u64>().is_ok()
        })
}

fn package(path: &Path) -> Result<(File, String)> {
    let mut file = File::open(path).with_context(|| format!("open artifact {}", path.display()))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() < 2 || metadata.len() > MAX_PACKAGE_BYTES {
        bail!("runtime artifact must be a regular gzip package between 2 bytes and 256 MiB");
    }
    let mut magic = [0u8; 2];
    file.read_exact(&mut magic)?;
    if magic != [0x1f, 0x8b] {
        bail!(
            "expected a compatible jcode-linux-x86_64.tar.gz package, not a raw local executable"
        );
    }
    file.rewind()?;
    let mut hash = Sha256::new();
    let bytes = std::io::copy(&mut file, &mut hash)?;
    if bytes != metadata.len() || bytes > MAX_PACKAGE_BYTES {
        bail!("runtime package changed while hashing");
    }
    file.seek(SeekFrom::Start(0))?;
    Ok((file, format!("{:x}", hash.finalize())))
}

fn invoke(verb: &str, value: Option<&str>, input: Option<File>) -> Result<()> {
    let remote = remote_command(verb, value)?;
    let args = cloud_managed::ssh_args()?;
    let status = Command::new("ssh")
        .args(args)
        .args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=30"])
        .arg(cloud_managed::HOST)
        .arg(remote)
        .stdin(input.map(Stdio::from).unwrap_or_else(Stdio::null))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("run the managed cloud runtime updater over SSH")?;
    if !status.success() {
        bail!("cloud runtime operation failed ({status}). Active cloud work was not restarted.");
    }
    Ok(())
}

pub(crate) fn run_cli(action: CloudRuntimeCommand) -> Result<()> {
    match action {
        CloudRuntimeCommand::Status => invoke("status", None, None),
        CloudRuntimeCommand::Check => invoke("check", None, None),
        CloudRuntimeCommand::Stable { yes } => {
            if !yes {
                bail!("returning the whole host to stable requires --yes");
            }
            invoke("use-stable", None, None)
        }
        CloudRuntimeCommand::Pin { version, yes } => {
            if !yes {
                bail!("changing the whole host's runtime pin requires --yes");
            }
            let version = if version.starts_with('v') {
                version
            } else {
                format!("v{version}")
            };
            invoke("pin", Some(&version), None)
        }
        CloudRuntimeCommand::Push {
            artifact,
            from_source,
            reload: then_reload,
            wait_secs,
        } => {
            let artifact = match (artifact, from_source) {
                (Some(path), false) => path,
                (None, true) => build_from_source()?,
                _ => bail!("pass exactly one of --artifact or --from-source"),
            };
            let (file, digest) = package(&artifact)?;
            eprintln!(
                "[cloud push] uploading runtime package {digest}. This selects dev policy for the whole host."
            );
            invoke("push", Some(&digest), Some(file))?;
            if then_reload {
                reload(wait_secs, false)
            } else {
                eprintln!(
                    "Active daemons are not restarted. Run `jcode cloud runtime reload` to switch."
                );
                Ok(())
            }
        }
        CloudRuntimeCommand::Reload { force, wait_secs } => reload(wait_secs, force),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn runtime_remote_arguments_cannot_inject_shell_commands() {
        for version in [
            "v1.2.3;id",
            "v1.2.3\nreboot",
            "$(id)",
            "v01.2.3",
            "v1.2",
            "v1.2.3-dev",
        ] {
            assert!(remote_command("pin", Some(version)).is_err());
        }
        assert!(remote_command("pin", Some("v987.654.321")).is_ok());
        assert!(remote_command("push", Some(&"a".repeat(64))).is_ok());
        assert!(remote_command("push", Some(&"A".repeat(64))).is_err());
        assert!(remote_command("status; reboot", None).is_err());
        assert!(remote_command("status", Some("anything")).is_err());
        for verb in ["status", "check", "use-stable"] {
            assert!(
                remote_command(verb, None)
                    .unwrap()
                    .ends_with(&format!(" {verb}"))
            );
        }
        assert!(
            remote_command("pin", Some("v987.654.321"))
                .unwrap()
                .ends_with(" pin v987.654.321")
        );
        let digest = "a".repeat(64);
        assert!(
            remote_command("push", Some(&digest))
                .unwrap()
                .ends_with(&format!(" push {digest}"))
        );
    }

    #[test]
    fn live_reload_marker_matches_daemon_capability() {
        assert_eq!(LIVE_RELOAD_MARKER, crate::server::MANAGED_CLOUD_LIVE_RELOAD_MARKER);
        let script = reload_command(0, false);
        assert!(script.contains(&format!("grep -qa {LIVE_RELOAD_MARKER} ")));
        assert!(script.contains("activate-live"));
        assert!(script.contains("rollback-live"));
        // Live reload keeps in-flight turns, so only the restart fallback waits.
        assert!(script.contains(r#"if [ "$live_capable" = 0 ]; then wait_idle; fi"#));
        let fallback = script.split("live reload did not land").nth(1).unwrap();
        let restart = fallback.split("restarting cloud daemon").next().unwrap();
        assert!(restart.contains("wait_idle"));
    }

    #[test]
    fn runtime_reload_requires_a_verified_nonnegative_idle_count() {
        // Execute only the idle gate. No test can reach service stop/start code.
        let generated = reload_command(0, false);
        let gate = generated.split("before=$(readlink").next().unwrap();
        for (json, status, idle) in [
            (r#"{"streaming":0}"#, 0, true),
            (r#"{"streaming":2}"#, 0, false),
            (r#"{"streaming":0}"#, 1, false),
            ("", 0, false),
            ("not json", 0, false),
            ("{}", 0, false),
            (r#"{"streaming":false}"#, 0, false),
            (r#"{"streaming":-1}"#, 0, false),
            (r#"{"streaming":"0"}"#, 0, false),
        ] {
            let mock = r#"sh -c 'printf %s "$MOCK_ACTIVITY"; exit $MOCK_STATUS' --"#;
            let script = gate.replace("/opt/jcode-cloud/jcode", mock) + "printf verified_idle";
            let result = Command::new("sh")
                .args(["-c", &script])
                .env("MOCK_ACTIVITY", json)
                .env("MOCK_STATUS", status.to_string())
                .output()
                .unwrap();
            assert_eq!(result.status.success(), idle, "{json:?}, status={status}");
            assert_eq!(result.stdout == b"verified_idle", idle);
            if !idle {
                assert_eq!(result.status.code(), Some(3));
                assert!(String::from_utf8_lossy(&result.stderr).contains("not restarting"));
            }
        }
    }

    #[test]
    fn runtime_upload_hashes_and_rewinds_the_same_open_file() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let bytes = b"\x1f\x8bexplicit package fixture";
        file.write_all(bytes).unwrap();
        let (mut upload, digest) = package(file.path()).unwrap();
        assert_eq!(digest, format!("{:x}", Sha256::digest(bytes)));
        let mut contents = Vec::new();
        upload.read_to_end(&mut contents).unwrap();
        assert_eq!(contents, bytes);
    }

    #[test]
    fn runtime_rejects_laptop_executables_and_oversized_artifacts() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"\x7fELFlocal executable").unwrap();
        assert!(package(file.path()).is_err());
        file.as_file().set_len(MAX_PACKAGE_BYTES + 1).unwrap();
        assert!(package(file.path()).is_err());
        assert!(package(tempfile::tempdir().unwrap().path()).is_err());
    }
}
