//! Disposable cloud machines. Never address or mutate the persistent host.
//! Each machine gets separate SSH keys/config and a server-enforced hard TTL.
use super::args::CloudEphemeralCommand;
use super::cloud_managed;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const HOST_PREFIX: &str = "jcode-ephemeral-";

#[derive(Debug, Deserialize, Serialize)]
struct Machine {
    id: String,
    #[serde(default)]
    name: Option<String>,
    expires_at: u64,
    #[serde(flatten)]
    connection: cloud_managed::Reply,
}

#[derive(Deserialize)]
struct Machines {
    machines: Vec<Machine>,
}

fn validate_id(id: &str) -> Result<()> {
    if id.len() != 35
        || !id.starts_with("cm_")
        || !id[3..]
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        bail!("invalid ephemeral machine ID: expected cm_ followed by 32 lowercase hex digits");
    }
    Ok(())
}

pub(super) fn host_id(host: &str) -> Option<&str> {
    host.strip_prefix(HOST_PREFIX)
        .filter(|id| validate_id(id).is_ok())
}

fn machine_host(id: &str) -> Result<String> {
    validate_id(id)?;
    Ok(format!("{HOST_PREFIX}{id}"))
}

fn machine_path(id: &str) -> Result<String> {
    validate_id(id)?;
    Ok(format!("cloud/machines/{id}"))
}

fn machine_dir(id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(cloud_managed::dir()?.join("ephemeral").join(id))
}

pub(super) fn ssh_config_path(id: &str) -> Result<PathBuf> {
    Ok(machine_dir(id)?.join("ssh_config"))
}

fn ssh_config(id: &str, dir: &Path, exe: &Path) -> Result<String> {
    let host = machine_host(id)?;
    let quote = cloud_managed::ssh_quote;
    Ok(format!(
        "# Managed ephemeral Jcode Cloud machine. No persistent-host state.\n\
         Host {host}\n\
         \x20 HostName {host}\n\
         \x20 User ec2-user\n\
         \x20 IdentityFile {key}\n\
         \x20 IdentitiesOnly yes\n\
         \x20 IdentityAgent none\n\
         \x20 UserKnownHostsFile {known}\n\
         \x20 GlobalKnownHostsFile /dev/null\n\
         \x20 StrictHostKeyChecking yes\n\
         \x20 HostKeyAlias {host}\n\
         \x20 ProxyCommand {exe} --no-update --no-selfdev cloud ephemeral proxy {id}\n\
         \x20 ControlMaster no\n\
         \x20 ControlPath none\n\
         \x20 ForwardAgent no\n\
         \x20 ClearAllForwardings yes\n\
         \x20 ServerAliveInterval 15\n\
         \x20 ServerAliveCountMax 2\n",
        key = quote(&dir.join("id_ed25519").to_string_lossy()),
        known = quote(&dir.join("known_hosts").to_string_lossy()),
        exe = quote(&exe.to_string_lossy()),
    ))
}

pub(super) fn ssh_args(id: &str) -> Result<Vec<String>> {
    let dir = machine_dir(id)?;
    cloud_managed::create_private_dir(&dir)?;
    let key = dir.join("id_ed25519");
    if !key.exists() {
        let result = Command::new("ssh-keygen")
            .args([
                "-q",
                "-t",
                "ed25519",
                "-N",
                "",
                "-C",
                "jcode-ephemeral",
                "-f",
            ])
            .arg(&key)
            .stdin(std::process::Stdio::null())
            .output()
            .context("ssh-keygen is required for ephemeral cloud machines")?;
        if !result.status.success() {
            bail!(
                "cannot create ephemeral SSH key: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
    let exe = cloud_managed::proxy_executable(std::env::current_exe()?);
    let path = ssh_config_path(id)?;
    cloud_managed::write_private(&path, &ssh_config(id, &dir, &exe)?)?;
    Ok(vec!["-F".into(), path.to_string_lossy().into_owned()])
}

fn connect(id: &str) -> Result<Machine> {
    let path = machine_path(id)?;
    ssh_args(id)?;
    let key = std::fs::read_to_string(machine_dir(id)?.join("id_ed25519.pub"))?
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut last = String::new();
    loop {
        let machine: Machine = cloud_managed::request_json(
            "POST",
            &format!("{path}/connect"),
            Some(serde_json::json!({"public_key": key})),
        )?;
        if machine.id != id {
            bail!("cloud reply identifies a different machine");
        }
        if machine.connection.ready {
            return Ok(machine);
        }
        if matches!(
            machine.connection.state.as_str(),
            "terminated" | "terminating" | "expired" | "failed"
        ) {
            bail!("ephemeral machine is {}", machine.connection.state);
        }
        if machine.connection.state != last {
            eprintln!("[jcode cloud] {id}: {}", machine.connection.state);
            last.clone_from(&machine.connection.state);
        }
        if Instant::now() >= deadline {
            bail!(
                "machine startup timed out; it remains bounded by its TTL. Inspect with `jcode cloud ephemeral status {id}`"
            );
        }
        std::thread::sleep(Duration::from_secs(4));
    }
}

/// Provisioning can take minutes. Finish it before OpenSSH/native protocol
/// handshake timeouts start, then let the proxy obtain a fresh short-lived key.
pub(super) fn wait_ready(id: &str) -> Result<()> {
    connect(id).map(|_| ())
}

fn print_machine(machine: &Machine, json: bool) -> Result<()> {
    validate_id(&machine.id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(machine)?);
    } else {
        let name_suffix = match &machine.name {
            Some(name) => format!("  {name}"),
            None => String::new(),
        };
        println!(
            "{}  {}  expires {}{}",
            machine.id, machine.connection.state, machine.expires_at, name_suffix
        );
    }
    Ok(())
}

fn create_body(
    ttl_seconds: u32,
    request_id: Option<String>,
    name: Option<String>,
) -> Result<serde_json::Value> {
    if !(300..=86400).contains(&ttl_seconds) {
        bail!("TTL must be between 300 and 86400 seconds");
    }
    let request_id = match request_id {
        Some(value) => uuid::Uuid::parse_str(&value)
            .context("request-id must be a UUID")?
            .to_string(),
        None => uuid::Uuid::new_v4().to_string(),
    };
    if let Some(name) = &name {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            bail!("name must be 1..64 letters, digits, hyphens or underscores");
        }
    }
    let mut body = serde_json::json!({"request_id": request_id, "ttl_seconds": ttl_seconds});
    if let Some(name) = name {
        body["name"] = name.into();
    }
    Ok(body)
}

fn remote_command(command: &[String]) -> Result<String> {
    if command.is_empty() || command[0].is_empty() || command.iter().any(|a| a.contains('\0')) {
        bail!("a nonempty remote command without NUL bytes is required");
    }
    Ok(command
        .iter()
        .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" "))
}

pub(crate) fn run_cli(action: CloudEphemeralCommand) -> Result<()> {
    use CloudEphemeralCommand as C;
    match action {
        C::Create {
            ttl_seconds,
            request_id,
            name,
            json,
        } => {
            let body = create_body(ttl_seconds, request_id, name)?;
            // Printed before I/O so an uncertain network outcome can be retried safely.
            eprintln!(
                "[jcode cloud] request-id: {} (reuse with --request-id if interrupted)",
                body["request_id"]
                    .as_str()
                    .context("missing provisioning request ID")?
            );
            let machine: Machine =
                cloud_managed::request_json("POST", "cloud/machines", Some(body))?;
            print_machine(&machine, json)?;
            if !json {
                println!(
                    "Attach: jcode cloud ephemeral attach {}\nDestroy: jcode cloud ephemeral terminate {} --yes",
                    machine.id, machine.id
                );
            }
        }
        C::List { json } => {
            let list: Machines = cloud_managed::request_json("GET", "cloud/machines", None)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list.machines)?);
            } else if list.machines.is_empty() {
                println!("No ephemeral machines.");
            } else {
                for machine in list.machines {
                    print_machine(&machine, false)?;
                }
            }
        }
        C::Status { id, json } => {
            let machine: Machine = cloud_managed::request_json("GET", &machine_path(&id)?, None)?;
            print_machine(&machine, json)?;
        }
        C::Terminate { id, yes, json } => {
            if !yes {
                bail!("--yes is required to destroy the machine and its files");
            }
            let machine: Machine =
                cloud_managed::request_json("DELETE", &machine_path(&id)?, None)?;
            print_machine(&machine, json)?;
        }
        C::Attach {
            id,
            remote_binary,
            workdir,
            server_socket,
            session,
        } => {
            ssh_args(&id)?;
            let mut child = Command::new(std::env::current_exe()?);
            child.args([
                "--no-update",
                "--no-selfdev",
                "--ssh",
                &machine_host(&id)?,
                "--ssh-binary",
                &remote_binary,
            ]);
            if let Some(dir) = workdir {
                child.args(["--remote-working-dir", &dir]);
            }
            if let Some(socket) = server_socket {
                child.args(["--ssh-server-socket", &socket]);
            }
            if let Some(session) = session {
                child.args(["--resume", &session]);
            }
            let status = child
                .status()
                .context("start local TUI for ephemeral machine")?;
            if !status.success() {
                bail!("cloud TUI exited with {status}");
            }
        }
        C::Exec { id, command } => {
            let shell = remote_command(&command)?;
            wait_ready(&id)?;
            let status = Command::new("ssh")
                .args(ssh_args(&id)?)
                .args([
                    "-T",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "ConnectTimeout=30",
                    "--",
                    &machine_host(&id)?,
                    &shell,
                ])
                .status()
                .context("execute command on ephemeral machine")?;
            if !status.success() {
                bail!("remote command exited with {status}");
            }
        }
        C::Proxy { id } => {
            let machine = connect(&id)?;
            cloud_managed::relay_proxy(
                &machine.connection,
                &machine_host(&id)?,
                &machine_dir(&id)?.join("known_hosts"),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "cm_0123456789abcdef0123456789abcdef";

    #[test]
    fn ephemeral_ids_cannot_target_persistent_host_or_inject_paths() {
        assert!(validate_id(ID).is_ok());
        for invalid in [
            "jcode-cloud",
            "../cloud",
            "cm_",
            "cm_0123456789ABCDEF0123456789abcdef",
            "cm_0123456789abcdef0123456789abcdef/stop",
        ] {
            assert!(validate_id(invalid).is_err(), "{invalid}");
        }
        assert_eq!(host_id(&machine_host(ID).unwrap()), Some(ID));
        assert!(host_id("jcode-cloud").is_none());
        assert_eq!(machine_path(ID).unwrap(), format!("cloud/machines/{ID}"));
    }

    #[test]
    fn ephemeral_config_is_isolated_and_never_uses_persistent_proxy() {
        let text = ssh_config(
            ID,
            Path::new("/private/with space"),
            Path::new("/bin/jcode"),
        )
        .unwrap();
        assert!(text.contains(&format!("cloud ephemeral proxy {ID}")));
        assert!(text.contains("ControlPath none"));
        assert!(text.contains("ForwardAgent no"));
        assert!(text.contains("StrictHostKeyChecking yes"));
        assert!(text.contains("\"/private/with space/id_ed25519\""));
        assert!(!text.contains("cloud proxy\n"));
        assert!(!text.contains("Host jcode-cloud\n"));
    }

    #[test]
    fn create_requests_are_bounded_and_idempotent() {
        let request_id = "01234567-89ab-4def-8123-456789abcdef";
        let body = create_body(600, Some(request_id.into()), Some("bench-1".into())).unwrap();
        assert_eq!(body["request_id"], request_id);
        assert_eq!(body["ttl_seconds"], 600);
        assert_eq!(
            body,
            create_body(600, Some(request_id.into()), Some("bench-1".into())).unwrap()
        );
        for ttl in [0, 299, 86401, u32::MAX] {
            assert!(create_body(ttl, None, None).is_err());
        }
        assert!(create_body(600, Some("bad/id".into()), None).is_err());
        for name in ["", "bad\nname", "../foo", "bad;cmd"] {
            assert!(create_body(600, None, Some(name.into())).is_err());
        }
    }

    #[test]
    fn remote_arguments_are_quoted_individually() {
        assert_eq!(
            remote_command(&["printf".into(), "%s".into(), "a'b;$(id)".into()]).unwrap(),
            "'printf' '%s' 'a'\\''b;$(id)'"
        );
        assert!(remote_command(&[]).is_err());
        assert!(remote_command(&["\0".into()]).is_err());
    }

    #[test]
    fn machine_reply_preserves_connection_and_expiry() {
        let machine: Machine = serde_json::from_value(serde_json::json!({
            "id": ID, "state": "running", "expires_at": 1800000000,
            "ready": true, "host_keys": ["ssh-ed25519 AAAA"], "address": "192.0.2.1", "port": 22,
        }))
        .unwrap();
        assert_eq!(machine.expires_at, 1800000000);
        assert!(machine.connection.ready);
        assert_eq!(machine.connection.host_keys.len(), 1);
        assert_eq!(serde_json::to_value(machine).unwrap()["state"], "running");
    }
}
