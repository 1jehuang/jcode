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
        CloudRuntimeCommand::Push { artifact } => {
            let (file, digest) = package(&artifact)?;
            eprintln!(
                "Uploading explicit cloud runtime package ({digest}). This selects dev policy for the whole host. Active daemons are not restarted."
            );
            invoke("push", Some(&digest), Some(file))
        }
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
