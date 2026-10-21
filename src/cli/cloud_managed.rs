//! Managed Jcode Cloud machine: the VM that comes with a Jcode subscription.
//!
//! The account's machine lives in Jcode's AWS account and is created, woken,
//! and authorized by the Jcode API (`/v1/cloud/host/*`) using only the Jcode
//! account key. Its root disk is an encrypted EBS volume that survives sleep,
//! so repos, sessions, and tools persist between connections.
//!
//! SSH reaches it through a private config (`~/.jcode/cloud/ssh_config`) whose
//! `ProxyCommand` is `jcode cloud proxy`. Each SSH connection therefore asks the
//! API to wake the machine (if needed) and authorize this client's key for one
//! login, writes the host keys the control plane pinned at first boot, then
//! relays the TCP stream. Nothing touches the user's own `~/.ssh`.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// Reserved host alias for the account's managed machine.
pub(crate) const HOST: &str = "jcode-cloud";
const DEFAULT_API_BASE: &str = "https://api.jcode.sh/v1";
const PROVISION_DEADLINE: Duration = Duration::from_secs(10 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(4);

/// A live multiplexed SSH master means the machine is awake and this client is
/// already authorized, so the Jcode Cloud wake/authorize call can be skipped.
pub(crate) fn master_is_live() -> bool {
    let Ok(args) = ssh_args() else {
        return false;
    };
    Command::new("ssh")
        .args(args)
        .args(["-O", "check", HOST])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) fn is_managed(host: &str) -> bool {
    host == HOST
}

/// Whether this machine is signed in to a Jcode account, which entitles it to
/// try the managed machine.
pub(crate) fn account_available() -> bool {
    crate::subscription_catalog::configured_api_key().is_some_and(|k| !k.trim().is_empty())
}

fn dir() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("cloud"))
}

fn key_path() -> Result<PathBuf> {
    Ok(dir()?.join("id_ed25519"))
}

fn known_hosts_path() -> Result<PathBuf> {
    Ok(dir()?.join("known_hosts"))
}

pub(crate) fn ssh_config_path() -> Result<PathBuf> {
    Ok(dir()?.join("ssh_config"))
}

/// Ensure the client key and private SSH config exist. Returns `-F <config>`
/// arguments for an `ssh` invocation targeting [`HOST`].
pub(crate) fn ssh_args() -> Result<Vec<String>> {
    let dir = dir()?;
    create_private_dir(&dir)?;
    let key = key_path()?;
    if !key.exists() {
        let output = Command::new("ssh-keygen")
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "jcode-cloud-client",
                "-f",
            ])
            .arg(&key)
            .stdin(std::process::Stdio::null())
            .output()
            .context("ssh-keygen is required for Jcode Cloud")?;
        if !output.status.success() {
            bail!(
                "could not create the Jcode Cloud client key: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    let exe = std::env::current_exe().context("locate the jcode binary")?;
    let namespace = account()
        .ok()
        .and_then(|account| control_namespace(&account.key, &account.base));
    let config = managed_ssh_config(&dir, &key, &known_hosts_path()?, &exe, namespace.as_deref());
    let path = ssh_config_path()?;
    if std::fs::read_to_string(&path).ok().as_deref() != Some(config.as_str()) {
        write_private(&path, &config)?;
    }
    Ok(vec!["-F".into(), path.to_string_lossy().into_owned()])
}

fn control_namespace(key: &str, base: &str) -> Option<String> {
    use sha2::{Digest, Sha256};

    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(key.as_bytes());
    hash.update([0]);
    hash.update(base.trim_end_matches('/').as_bytes());
    Some(format!("{:x}", hash.finalize())[..16].to_string())
}

fn managed_ssh_config(
    dir: &std::path::Path,
    key: &std::path::Path,
    known: &std::path::Path,
    exe: &std::path::Path,
    namespace: Option<&str>,
) -> String {
    // Keep the socket in the mode-0700 managed directory, never a shared temp
    // directory. %C keeps its name short and keys it to the SSH destination.
    // Escape literal percent signs in the directory before adding SSH's token.
    let multiplexing = if let Some(namespace) = namespace {
        let control = format!(
            "{}/{namespace}-%C",
            dir.to_string_lossy().replace('%', "%%")
        );
        format!(
            "  ControlMaster auto\n  ControlPath {}\n  ControlPersist 60\n",
            ssh_quote(&control)
        )
    } else {
        // Never reuse an earlier account's authenticated connection when signed out.
        "  ControlMaster no\n  ControlPath none\n  ControlPersist no\n".to_string()
    };
    format!(
        "# Managed by jcode. Rewritten on every use.\n\
         Host {HOST}\n\
         \x20 HostName {HOST}\n\
         \x20 User ec2-user\n\
         \x20 IdentityFile {key}\n\
         \x20 IdentitiesOnly yes\n\
         \x20 IdentityAgent none\n\
         \x20 UserKnownHostsFile {known}\n\
         \x20 GlobalKnownHostsFile /dev/null\n\
         \x20 StrictHostKeyChecking yes\n\
         \x20 HostKeyAlias {HOST}\n\
         \x20 ProxyCommand {exe} --no-update --no-selfdev cloud proxy\n\
         {multiplexing}\
         \x20 ServerAliveInterval 30\n\
         \x20 ServerAliveCountMax 3\n",
        key = ssh_quote(&key.to_string_lossy()),
        known = ssh_quote(&known.to_string_lossy()),
        exe = ssh_quote(&exe.to_string_lossy()),
    )
}

fn ssh_quote(value: &str) -> String {
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._-+@:".contains(&b))
    {
        value.to_string()
    } else {
        format!("\"{}\"", value.replace('"', "\\\""))
    }
}

// ---------------------------------------------------------------------------
// Control-plane API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub(crate) struct Reply {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub host_keys: Vec<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct Usage {
    #[serde(default)]
    pub used_seconds: u64,
    #[serde(default)]
    pub allowance_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct ErrorReply {
    error: ErrorBody,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    code: String,
    message: String,
}

struct Account {
    key: String,
    base: String,
}

fn account() -> Result<Account> {
    let key = crate::subscription_catalog::configured_api_key()
        .filter(|k| !k.trim().is_empty())
        .context("Sign in to your Jcode account (`jcode login`) to use Jcode Cloud")?;
    let base = crate::subscription_catalog::configured_api_base()
        .filter(|b| b.starts_with("https://"))
        .unwrap_or_else(|| DEFAULT_API_BASE.into());
    Ok(Account {
        key: key.trim().to_string(),
        base: base.trim_end_matches('/').to_string(),
    })
}

/// Run blocking HTTP off any async runtime the caller may be inside.
fn off_runtime<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| scope.spawn(f).join().expect("cloud API thread panicked"))
}

fn request(method: &str, path: &str, body: Option<serde_json::Value>) -> Result<Reply> {
    let account = account()?;
    let url = format!("{}/{}", account.base, path);
    off_runtime(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(40))
            .user_agent(format!("jcode/{}", env!("CARGO_PKG_VERSION")))
            .build()?;
        let mut req = match method {
            "POST" => client.post(&url),
            _ => client.get(&url),
        }
        .bearer_auth(&account.key);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req
            .send()
            .context("could not reach Jcode Cloud (check your network or DNS connection)")?;
        let status = response.status();
        let text = response.text()?;
        if status.is_success() {
            return serde_json::from_str(&text).context("unexpected Jcode Cloud reply");
        }
        let message = match serde_json::from_str::<ErrorReply>(&text) {
            Ok(reply) => describe_error(status.as_u16(), &reply.error.code, &reply.error.message),
            Err(_) => format!("Jcode Cloud request failed (HTTP {status})"),
        };
        bail!(message)
    })
}

fn describe_error(status: u16, code: &str, message: &str) -> String {
    match (status, code) {
        (401, _) => "Your Jcode sign-in expired. Run `jcode login` again.".into(),
        (402, "cloud_not_entitled") => {
            "Jcode Cloud requires an active paid Jcode subscription.".into()
        }
        _ => message.to_string(),
    }
}

fn public_key() -> Result<String> {
    ssh_args()?;
    let path = key_path()?.with_extension("pub");
    Ok(std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" "))
}

pub(crate) fn status() -> Result<Reply> {
    request("GET", "cloud/host", None)
}

pub(crate) fn stop() -> Result<Reply> {
    request("POST", "cloud/host/stop", Some(serde_json::json!({})))
}

/// Create or wake the machine and wait until it accepts this client's key.
/// `progress` receives each distinct status message.
pub(crate) fn connect(progress: &mut dyn FnMut(&str)) -> Result<Reply> {
    let key = public_key()?;
    let deadline = Instant::now() + PROVISION_DEADLINE;
    let mut last = String::new();
    loop {
        let reply = request(
            "POST",
            "cloud/host/connect",
            Some(serde_json::json!({ "public_key": key })),
        )?;
        if reply.ready {
            return Ok(reply);
        }
        let message = reply
            .message
            .clone()
            .unwrap_or_else(|| format!("Jcode Cloud machine is {}…", reply.state));
        if message != last {
            progress(&message);
            last = message;
        }
        if Instant::now() >= deadline {
            bail!("Jcode Cloud machine did not become ready in time. Retry to keep waiting.");
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn validated_host_keys(reply: &Reply) -> Result<Vec<String>> {
    let keys: Vec<String> = reply
        .host_keys
        .iter()
        .filter(|key| {
            let mut parts = key.split(' ');
            matches!(
                (parts.next(), parts.next(), parts.next()),
                (Some("ssh-ed25519" | "ecdsa-sha2-nistp256"), Some(blob), None)
                    if blob.len() <= 1024 && blob.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
            )
        })
        .cloned()
        .collect();
    if keys.is_empty() {
        bail!("Jcode Cloud did not provide verified host keys");
    }
    Ok(keys)
}

/// `jcode cloud proxy`: SSH `ProxyCommand`. Authorizes this connection, pins
/// host keys, then relays stdin/stdout to the machine's SSH port. Diagnostics
/// go to stderr only, since stdout is the SSH wire.
pub(crate) fn run_proxy() -> Result<()> {
    let reply = connect(&mut |message| eprintln!("[jcode cloud] {message}"))?;
    let address: std::net::IpAddr = reply
        .address
        .as_deref()
        .and_then(|a| a.parse().ok())
        .context("Jcode Cloud returned an invalid host address")?;
    let keys = validated_host_keys(&reply)?;
    let known: String = keys.iter().map(|k| format!("{HOST} {k}\n")).collect();
    write_private(&known_hosts_path()?, &known)?;
    let addr = std::net::SocketAddr::new(address, reply.port.unwrap_or(22));
    let stream =
        off_runtime(|| std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(20)))
            .with_context(|| format!("connect to the Jcode Cloud machine at {addr}"))?;
    stream.set_nodelay(true).ok();
    let mut upstream_read = stream.try_clone()?;
    let mut upstream_write = stream;
    // Relay raw bytes with no buffering: std's stdout is line-buffered, which
    // would hold SSH's binary packets until a newline happens to appear.
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 32 * 1024];
        loop {
            match std::io::Read::read(&mut stdin, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if upstream_write.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = upstream_write.shutdown(std::net::Shutdown::Write);
    });
    let mut stdout = raw_stdout();
    let mut buf = [0u8; 32 * 1024];
    loop {
        match std::io::Read::read(&mut upstream_read, &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stdout.write_all(&buf[..n]).is_err() || stdout.flush().is_err() {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn raw_stdout() -> std::fs::File {
    use std::os::fd::FromRawFd;
    // SAFETY: fd 1 stays open for the life of the process; ManuallyDrop-like
    // ownership is fine because the process exits right after relaying.
    unsafe { std::fs::File::from_raw_fd(libc::dup(1)) }
}

#[cfg(not(unix))]
fn raw_stdout() -> std::io::Stdout {
    std::io::stdout()
}

/// Select an exact published tag, never a guessed fallback for a development build.
/// The override is read locally, not from the remote host's environment.
fn release_target(
    release_build: bool,
    semver: &str,
    override_tag: Option<&str>,
) -> Result<Option<String>> {
    let value = match override_tag {
        Some(value) => value,
        None if release_build => semver,
        None => return Ok(None),
    };
    let version = value.strip_prefix('v').unwrap_or(value);
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|b| b.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
                || part.parse::<u64>().is_err()
        })
    {
        bail!("JCODE_CLOUD_RELEASE must be an exact release tag such as v0.99.123 (got {value:?})");
    }
    Ok(Some(format!("v{version}")))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Preflight runs before secrets are transferred. A release client pins its own
/// runtime release identity; development clients only probe unless explicitly
/// given JCODE_CLOUD_RELEASE. Existing daemons/sessions are never restarted.
pub(crate) fn prepare_script(home: &str) -> Result<String> {
    let override_tag = std::env::var("JCODE_CLOUD_RELEASE").ok();
    let target = release_target(
        jcode_build_meta::is_release_build(),
        jcode_build_meta::semver(),
        override_tag.as_deref(),
    )?;
    Ok(prepare_script_for_paths(
        home,
        target.as_deref(),
        "/opt/jcode-cloud/jcode",
        true,
    ))
}

fn prepare_script_for_paths(
    home: &str,
    target: Option<&str>,
    backend: &str,
    elevate: bool,
) -> String {
    let backend = shell_quote(backend);
    let privilege = if elevate { "sudo -n " } else { "" };
    let home = shell_quote(home);
    let want = shell_quote(target.unwrap_or(""));
    format!(
        r#"set -eu
want={want}
export PATH="$HOME/.local/bin:$PATH"
cli="$HOME/.local/bin/jcode"
# Preserve the VM's CLI/service wrappers and their host-specific environment.
# Both wrappers delegate to this stable backend, which also affects the next
# natural daemon start without disrupting the current daemon or live sessions.
backend={backend}
root="$HOME/.local/share/jcode/cloud-releases"
mkdir -p "$HOME/.local/bin" "$root"
# Serialize cooperating preflights without terminating any running process.
# Wait briefly for a concurrent prepare instead of failing the move, and
# reclaim a lock left behind by a prepare that was killed mid-run.
lock="$root/.prepare-lock"
waited=0
until mkdir "$lock" 2>/dev/null; do
  if [ -n "$(find "$lock" -maxdepth 0 -mmin +10 2>/dev/null)" ]; then
    echo '[cloud prepare] reclaiming a stale upgrade lock' >&2
    rmdir "$lock" 2>/dev/null || rm -rf "$lock"
    continue
  fi
  if [ "$waited" -ge 120 ]; then
    echo '[cloud prepare] another upgrade is still running after 2 minutes; retry after it finishes' >&2
    exit 1
  fi
  [ "$waited" -eq 0 ] && echo '[cloud prepare] waiting for another upgrade to finish' >&2
  sleep 2
  waited=$((waited + 2))
done
stage=
backup=
next=
switched=0
had_previous=0
committed=0
cleanup() {{
  rc=$?
  trap - EXIT HUP INT TERM
  if [ "$switched" = 1 ] && [ "$committed" = 0 ]; then
    rollback_ok=1
    if [ "$had_previous" = 1 ]; then
      if ! {privilege}mv -Tf "$backup" "$backend"; then
        echo "[cloud prepare] rollback failed; recovery files retained in $stage and $backup" >&2
        stage=
        rollback_ok=0
        rc=1
      fi
    else
      if ! {privilege}rm -f "$backend"; then
        echo "[cloud prepare] rollback failed; installed files retained in $stage" >&2
        stage=
        rollback_ok=0
        rc=1
      fi
    fi
    if [ "$rollback_ok" = 1 ]; then
      echo '[cloud prepare] verification failed; restored previous CLI' >&2
    fi
  fi
  if [ -n "$next" ]; then {privilege}rm -f "$next" || true; fi
  # Keep installed releases and the previous binary for rollback/recovery.
  if [ -n "$stage" ] && [ "$committed" = 0 ]; then rm -rf "$stage"; fi
  rmdir "$lock" || true
  exit "$rc"
}}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
probe() {{
  # Do not let a fast-release parent identity mask the candidate's real version.
  env -u JCODE_RUNTIME_RELEASE_SEMVER -u JCODE_RUNTIME_RELEASE_GIT_HASH \
    -u JCODE_RUNTIME_RELEASE_GIT_DATE -u JCODE_RUNTIME_RELEASE_GIT_TAG \
    "$1" --no-update --no-selfdev "$2" "${{3:-}}" "${{4:-}}"
}}
capable() {{
  probe "$1" cloud receive --help >/dev/null 2>&1 &&
    probe "$1" cloud credentials --help >/dev/null 2>&1 &&
    probe "$1" cloud router-credentials --help >/dev/null 2>&1 &&
    probe "$1" cloud cancel-return --help >/dev/null 2>&1
}}
version_output() {{
  env -u JCODE_RUNTIME_RELEASE_SEMVER -u JCODE_RUNTIME_RELEASE_GIT_HASH \
    -u JCODE_RUNTIME_RELEASE_GIT_DATE -u JCODE_RUNTIME_RELEASE_GIT_TAG \
    "$1" --no-update --no-selfdev --version 2>/dev/null
}}
version_matches() {{
  version=$(version_output "$1") || return 1
  case "$version" in
    "jcode $want"|"jcode $want "*|"$want"|"$want "*) return 0 ;;
    *) return 1 ;;
  esac
}}
version_at_least() {{
  version=$(version_output "$1") || return 1
  # A newer compatible stable release need not be downgraded. Compare decimal
  # components as strings to avoid floating-point/lexicographic version errors.
  printf '%s\n' "$version" | awk -v want="$want" '
    NR == 1 {{ version = ($1 == "jcode" ? $2 : $1) }}
    END {{
      if (NR != 1 || version !~ /^v[0-9]+\.[0-9]+\.[0-9]+$/) exit 1;
      sub(/^v/, "", version); sub(/^v/, "", want);
      split(version, a, /[.]/); split(want, b, /[.]/);
      for (i=1; i<=3; i++) {{
        sub(/^0+/, "", a[i]); sub(/^0+/, "", b[i]);
        if (length(a[i]) != length(b[i])) exit (length(a[i]) > length(b[i]) ? 0 : 1);
        if ("x" a[i] != "x" b[i]) exit ("x" a[i] > "x" b[i] ? 0 : 1);
      }}
      exit 0;
    }}'
}}
if ! capable "$cli" || ! capable "$backend" || {{ [ -n "$want" ] && {{ ! version_at_least "$cli" || ! version_at_least "$backend"; }}; }} || [ "${{JCODE_CLOUD_FORCE_UPGRADE:-}}" = 1 ]; then
  if [ -z "$want" ]; then
    echo '[cloud prepare] cloud CLI lacks required capabilities, or upgrade was forced. Development clients do not choose a release automatically. Set JCODE_CLOUD_RELEASE to an exact compatible published tag.' >&2
    exit 1
  fi
  [ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ] || {{
    echo '[cloud prepare] release requires Linux x86_64' >&2; exit 1;
  }}
  echo "[cloud prepare] installing verified $want CLI (live sessions are unchanged)" >&2
  stage=$(mktemp -d "$root/$want.XXXXXXXX")
  base="https://github.com/1jehuang/jcode/releases/download/$want"
  asset=jcode-linux-x86_64.tar.gz
  curl --proto '=https' --tlsv1.2 -fsSL --retry 3 "$base/$asset" -o "$stage/$asset"
  curl --proto '=https' --tlsv1.2 -fsSL --retry 3 "$base/SHA256SUMS" -o "$stage/SHA256SUMS"
  # Accept exactly one checksum for exactly this asset, never arbitrary paths.
  awk -v asset="$asset" '
    $2 == asset || $2 == "*" asset {{
      if (length($1) != 64 || $1 ~ /[^0-9a-fA-F]/) exit 1;
      hash=$1; count++
    }}
    END {{ if (count != 1) exit 1; print hash "  " asset }}
  ' "$stage/SHA256SUMS" > "$stage/checksum"
  (cd "$stage" && sha256sum --check --status checksum)
  # The manylinux artifact is flat: wrapper, real executable, optional OpenSSL
  # libraries. Reject directories, links, devices and unexpected/traversal names.
  tar tzf "$stage/$asset" > "$stage/members"
  awk '
    $0 !~ /^(jcode-linux-x86_64(\.bin)?|lib(ssl|crypto)\.so[.0-9A-Za-z_-]*)$/ {{ exit 1 }}
    seen[$0]++ {{ exit 1 }}
    END {{ if (!seen["jcode-linux-x86_64"] || !seen["jcode-linux-x86_64.bin"]) exit 1 }}
  ' "$stage/members"
  tar tvzf "$stage/$asset" > "$stage/types"
  awk 'substr($0,1,1) != "-" {{ exit 1 }}' "$stage/types"
  mkdir "$stage/bin"
  tar xzf "$stage/$asset" --no-same-owner --no-same-permissions -C "$stage/bin"
  candidate="$stage/bin/jcode-linux-x86_64"
  [ -f "$candidate" ] && [ -f "$candidate.bin" ]
  chmod 755 "$candidate" "$candidate.bin"
  capable "$candidate" && version_matches "$candidate" || {{
    echo '[cloud prepare] downloaded release lacks required protocol capabilities or exact version' >&2; exit 1;
  }}
  # Save regular files as well as symlinks, without changing their original target.
  if [ -e "$backend" ] || [ -L "$backend" ]; then
    [ ! -d "$backend" ] || {{ echo '[cloud prepare] backend path is a directory' >&2; exit 1; }}
    backup="$backend.previous.$(basename "$stage")"
    {privilege}cp -Pp "$backend" "$backup"
    had_previous=1
  fi
  {privilege}install -d -m 755 "$(dirname "$backend")"
  # Stage the new symlink beside its destination so rename is atomic even when
  # /opt and the user's home directory live on different filesystems.
  next="$backend.next.$(basename "$stage")"
  {privilege}ln -s "$candidate" "$next"
  # Arm rollback before rename, including interruption immediately after rename.
  switched=1
  {privilege}mv -Tf "$next" "$backend"
  capable "$backend" && version_matches "$backend" && capable "$cli" && version_matches "$cli" || exit 1
fi
# All handoff protocols are mandatory even when credentials are not needed yet.
capable "$cli" || {{ echo '[cloud prepare] cloud receive, credentials and cancel-return are required' >&2; exit 1; }}
committed=1
if [ -n {home} ] && [ ! -d {home} ]; then
  sudo -n install -d -o "$(id -un)" -g "$(id -gn)" -m 755 {home}
fi
command -v git >/dev/null || sudo -n dnf install -y git >/dev/null
"#
    )
}

/// Summary line for `jcode cloud machine`.
pub(crate) fn describe(reply: &Reply) -> String {
    let mut text = format!("Jcode Cloud machine `{HOST}`: {}", reply.state);
    if let Some(usage) = &reply.usage
        && usage.allowance_seconds > 0
    {
        text.push_str(&format!(
            " · {:.1}h of {:.0}h used this month",
            usage.used_seconds as f64 / 3600.0,
            usage.allowance_seconds as f64 / 3600.0
        ));
    }
    text
}

fn create_private_dir(path: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private(path: &std::path::Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(contents.as_bytes())?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
#[path = "cloud_managed_tests.rs"]
mod upgrade_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_keys_reject_injection() {
        let reply = Reply {
            host_keys: vec![
                "ssh-ed25519 AAAAC3Nza".into(),
                "ssh-rsa AAAA".into(),
                "ssh-ed25519 AAAA\n* ssh-ed25519 BBBB".into(),
            ],
            ..Default::default()
        };
        assert_eq!(
            validated_host_keys(&reply).unwrap(),
            vec!["ssh-ed25519 AAAAC3Nza"]
        );
        assert!(validated_host_keys(&Reply::default()).is_err());
    }

    #[test]
    fn prepare_script_targets_release_and_home() {
        let script = prepare_script_for_paths(
            "/home/me",
            Some("v0.99.123"),
            "/opt/jcode-cloud/jcode",
            true,
        );
        assert!(script.contains("want='v0.99.123'"));
        assert!(script.contains("'/home/me'"));
        assert!(script.contains("cloud receive --help"));
    }

    #[test]
    fn managed_config_multiplexes_in_private_directory() {
        let dir = std::path::Path::new("/home/test user/100%/cloud");
        let config = managed_ssh_config(
            dir,
            &dir.join("id_ed25519"),
            &dir.join("known_hosts"),
            std::path::Path::new("/opt/jcode"),
            Some("0123456789abcdef"),
        );
        assert!(config.contains("  ControlMaster auto\n"));
        assert!(
            config.contains("  ControlPath \"/home/test user/100%%/cloud/0123456789abcdef-%C\"\n")
        );
        assert!(config.contains("  ControlPersist 60\n"));
        assert!(config.contains("  StrictHostKeyChecking yes\n"));
        assert!(config.contains("  IdentityAgent none\n"));
    }

    #[test]
    fn control_namespace_is_account_and_endpoint_scoped() {
        let namespace = control_namespace("secret-a", DEFAULT_API_BASE).unwrap();
        assert_eq!(namespace.len(), 16);
        assert!(namespace.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(
            Some(namespace.clone()),
            control_namespace(" secret-a ", DEFAULT_API_BASE)
        );
        assert_ne!(
            Some(namespace.clone()),
            control_namespace("secret-b", DEFAULT_API_BASE)
        );
        assert_ne!(
            Some(namespace),
            control_namespace("secret-a", "https://other.example/v1")
        );
        assert_eq!(control_namespace("  ", DEFAULT_API_BASE), None);
    }

    #[test]
    fn managed_config_disables_multiplexing_without_account() {
        let path = std::path::Path::new("/private/cloud");
        let config = managed_ssh_config(path, path, path, path, None);
        assert!(config.contains("  ControlMaster no\n"));
        assert!(config.contains("  ControlPath none\n"));
        assert!(config.contains("  ControlPersist no\n"));
        assert!(!config.contains("%C"));
    }

    #[test]
    fn ssh_quote_wraps_spaces() {
        assert_eq!(ssh_quote("/a/b"), "/a/b");
        assert_eq!(ssh_quote("/a b/c"), "\"/a b/c\"");
    }
}
