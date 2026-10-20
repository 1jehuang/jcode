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
/// Jcode release installed on the machine when its version is older.
pub(crate) const REMOTE_RELEASE: &str = "v0.99.0";
const DEFAULT_API_BASE: &str = "https://api.jcode.sh/v1";
const PROVISION_DEADLINE: Duration = Duration::from_secs(10 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(4);

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
    let config = format!(
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
         \x20 ServerAliveInterval 30\n\
         \x20 ServerAliveCountMax 3\n",
        key = ssh_quote(&key.to_string_lossy()),
        known = ssh_quote(&known_hosts_path()?.to_string_lossy()),
        exe = ssh_quote(&exe.to_string_lossy()),
    );
    let path = ssh_config_path()?;
    if std::fs::read_to_string(&path).ok().as_deref() != Some(config.as_str()) {
        write_private(&path, &config)?;
    }
    Ok(vec!["-F".into(), path.to_string_lossy().into_owned()])
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
        let response = req.send().context("could not reach Jcode Cloud")?;
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

/// Shell script run on the machine before a move: keep its Jcode on a release
/// that speaks the move protocol, and make sure `home` exists so repos are
/// restored at the same absolute path as on this computer.
pub(crate) fn prepare_script(home: &str) -> String {
    let home = home.replace('\'', "");
    format!(
        r#"set -eu
want='{REMOTE_RELEASE}'
current=$(/opt/jcode-cloud/jcode --version 2>/dev/null | head -n1 || true)
if ! /opt/jcode-cloud/jcode cloud receive --help >/dev/null 2>&1 || [ "${{JCODE_CLOUD_FORCE_UPGRADE:-}}" = 1 ]; then
  echo "[cloud prepare] updating jcode on the cloud machine ($current -> $want)" >&2
  tmp=$(mktemp -d)
  base="https://github.com/1jehuang/jcode/releases/download/$want"
  curl -fsSL --retry 3 "$base/jcode-linux-x86_64.tar.gz" -o "$tmp/jcode.tgz"
  curl -fsSL --retry 3 "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"
  (cd "$tmp" && grep ' jcode-linux-x86_64.tar.gz$' SHA256SUMS | sed 's/ .*//; s/$/  jcode.tgz/' | sha256sum --check --quiet -)
  sudo install -d -m 755 "/opt/jcode-cloud/$want"
  sudo tar xzf "$tmp/jcode.tgz" -C "/opt/jcode-cloud/$want"
  if [ -x "/opt/jcode-cloud/$want/jcode-linux-x86_64" ]; then entry=jcode-linux-x86_64; else entry=$(ls "/opt/jcode-cloud/$want" | head -n1); fi
  sudo ln -sfn "$want/$entry" /opt/jcode-cloud/jcode
  rm -rf "$tmp"
  XDG_RUNTIME_DIR=/run/user/$(id -u) systemctl --user restart jcode-cloud.service || true
  for i in $(seq 1 60); do [ -S /run/user/$(id -u)/jcode.sock ] && break; sleep 1; done
fi
if [ -n '{home}' ] && [ ! -d '{home}' ]; then
  sudo install -d -o "$(id -un)" -g "$(id -gn)" -m 755 '{home}'
fi
command -v git >/dev/null || sudo dnf install -y git >/dev/null
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
        let script = prepare_script("/home/me");
        assert!(script.contains(REMOTE_RELEASE));
        assert!(script.contains("'/home/me'"));
        assert!(script.contains("cloud receive --help"));
    }

    #[test]
    fn ssh_quote_wraps_spaces() {
        assert_eq!(ssh_quote("/a/b"), "/a/b");
        assert_eq!(ssh_quote("/a b/c"), "\"/a b/c\"");
    }
}
