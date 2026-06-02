use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SshRemoteConfig {
    #[serde(default)]
    pub hosts: Vec<SshRemoteProfile>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshRemoteProfile {
    pub name: String,
    pub ssh_target: String,
    #[serde(default = "default_workspace")]
    pub workspace: String,
}

fn default_workspace() -> String {
    "~".to_string()
}

pub fn config_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("ssh_remotes.json"))
}

pub fn control_dir() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("ssh-control"))
}

pub fn control_socket_path(name: &str) -> Result<PathBuf> {
    Ok(control_dir()?.join(format!("{}.sock", sanitize_profile_name(name))))
}

pub fn load_config() -> Result<SshRemoteConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(SshRemoteConfig::default());
    }
    crate::storage::read_json(&path).with_context(|| format!("failed to read {}", path.display()))
}

pub fn save_config(config: &SshRemoteConfig) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let _ = crate::platform::set_directory_permissions_owner_only(parent);
    }
    let bytes = serde_json::to_vec_pretty(config)?;
    std::fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

pub fn upsert_profile(name: &str, ssh_target: &str) -> Result<SshRemoteProfile> {
    let mut config = load_config()?;
    let ssh_target = normalize_ssh_target(ssh_target)?;
    let profile = SshRemoteProfile {
        name: name.to_string(),
        ssh_target,
        workspace: default_workspace(),
    };
    if let Some(existing) = config.hosts.iter_mut().find(|p| p.name == name) {
        *existing = profile.clone();
    } else {
        config.hosts.push(profile.clone());
        config.hosts.sort_by(|a, b| a.name.cmp(&b.name));
    }
    save_config(&config)?;
    Ok(profile)
}

pub fn find_profile(name: &str) -> Result<Option<SshRemoteProfile>> {
    let Some(mut profile) = load_config()?.hosts.into_iter().find(|p| p.name == name) else {
        return Ok(None);
    };
    // Older MVP builds could save a pasted command like `ssh user@host`. Normalize on load so
    // users do not have to manually repair ~/.jcode/ssh_remotes.json.
    profile.ssh_target = normalize_ssh_target(&profile.ssh_target)?;
    Ok(Some(profile))
}

pub fn normalize_ssh_target(input: &str) -> Result<String> {
    let trimmed = input.trim();
    let without_ssh = trimmed
        .strip_prefix("ssh ")
        .or_else(|| trimmed.strip_prefix("ssh\t"))
        .unwrap_or(trimmed)
        .trim();

    if without_ssh.is_empty() {
        bail!("SSH target cannot be empty. Example: alice@login.school.edu");
    }
    if without_ssh.starts_with('-') || without_ssh.split_whitespace().count() != 1 {
        bail!(
            "SSH target should be just the host alias or user@host, not a full command. Example: alice@login.school.edu"
        );
    }
    if without_ssh.chars().any(|c| c.is_control()) {
        bail!("SSH target contains invalid control characters");
    }
    Ok(without_ssh.to_string())
}

pub fn sanitize_profile_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "remote".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn is_control_master_alive(profile: &SshRemoteProfile) -> bool {
    let Ok(socket) = control_socket_path(&profile.name) else {
        return false;
    };
    Command::new("ssh")
        .arg("-S")
        .arg(socket)
        .arg("-O")
        .arg("check")
        .arg(&profile.ssh_target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn can_connect_batch_mode(profile: &SshRemoteProfile) -> bool {
    Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .arg(&profile.ssh_target)
        .arg("true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn disconnect(profile: &SshRemoteProfile) -> Result<bool> {
    let socket = control_socket_path(&profile.name)?;
    let status = Command::new("ssh")
        .arg("-S")
        .arg(socket)
        .arg("-O")
        .arg("exit")
        .arg(&profile.ssh_target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("failed to run ssh disconnect")?;
    Ok(status.success())
}

pub fn build_control_master_script(profile: &SshRemoteProfile) -> Result<String> {
    std::fs::create_dir_all(control_dir()?)?;
    let socket = control_socket_path(&profile.name)?;
    let target = &profile.ssh_target;
    Ok(format!(
        r#"printf '%s\n' '========================================'
printf '%s\n' 'Jcode SSH login for {name}'
printf '%s\n' '========================================'
printf '%s\n' ''
printf '%s\n' 'Step 2/4: Authenticate with your system SSH client'
printf '%s\n' ''
printf '%s\n' 'What is happening:'
printf '%s\n' '  - This terminal is running OpenSSH, not a Jcode password form.'
printf '%s\n' '  - If a password or two-factor prompt appears, type it here.'
printf '%s\n' '  - Jcode cannot read or store what you type in this terminal.'
printf '%s\n' ''
printf '%s\n' 'After authentication:'
printf '%s\n' '  - SSH will create a temporary background control socket.'
printf '%s\n' '  - Jcode will verify that socket before this terminal closes.'
printf '%s\n' '  - If anything fails, this terminal stays open with the reason.'
printf '%s\n' ''
ssh -f -M -S {socket} -N {target}
status=$?
if [ $status -ne 0 ]; then
  printf '%s\n' ''
  printf '%s\n' 'Step 2/4 failed: SSH did not complete authentication.'
  printf '%s\n' ''
  printf '%s\n' 'Common fixes:'
  printf '%s\n' '  - Check that the SSH target is only user@host or an SSH alias, not a full command.'
  printf '%s\n' '  - Check username, hostname, password, VPN, and two-factor prompt.'
  printf '%s\n' '  - Try the same target manually with: ssh {target}'
  printf '%s' 'Press Enter to close this terminal... '
  read _
  exit $status
fi

printf '%s\n' ''
printf '%s\n' 'Step 3/4: SSH accepted the login. Verifying background control socket...'
for i in 1 2 3 4 5 6 7 8 9 10; do
  if ssh -S {socket} -O check {target} >/dev/null 2>&1; then
    printf '%s\n' ''
    printf '%s\n' 'Step 4/4: Connected and verified.'
    printf '%s\n' 'Jcode can now use this SSH connection headlessly.'
    printf '%s\n' 'This terminal will close automatically.'
    sleep 1
    exit 0
  fi
  sleep 1
done

printf '%s\n' ''
printf '%s\n' 'Step 3/4 failed: Jcode could not verify the background control socket.'
printf '%s\n' ''
printf '%s\n' 'What this means:'
printf '%s\n' '  - SSH login may have succeeded, but multiplexing did not stay available.'
printf '%s\n' '  - The server may disallow SSH ControlMaster, or the connection closed immediately.'
printf '%s\n' '  - Jcode is keeping this terminal open so you can read the reason.'
printf '%s' 'Press Enter to close this terminal... '
read _
exit 1
"#,
        name = shell_single_quote(&profile.name),
        socket = shell_single_quote(&socket.to_string_lossy()),
        target = shell_single_quote(target),
    ))
}

pub fn spawn_control_master_terminal(profile: &SshRemoteProfile) -> Result<bool> {
    let script = build_control_master_script(profile)?;
    let command = crate::terminal_launch::TerminalCommand::new(
        "sh".to_string(),
        vec!["-c".to_string(), script],
    )
    .title(format!("jcode ssh · {}", profile.name));
    crate::terminal_launch::spawn_command_in_new_terminal(&command, Path::new("."))
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_profile_name_keeps_safe_chars() {
        assert_eq!(sanitize_profile_name("school"), "school");
        assert_eq!(
            sanitize_profile_name("alice@login.school.edu"),
            "alice_login.school.edu"
        );
        assert_eq!(sanitize_profile_name("!!!"), "remote");
    }

    #[test]
    fn normalize_ssh_target_accepts_alias_and_user_host() {
        assert_eq!(normalize_ssh_target("school").unwrap(), "school");
        assert_eq!(
            normalize_ssh_target(" alice@login.school.edu ").unwrap(),
            "alice@login.school.edu"
        );
    }

    #[test]
    fn normalize_ssh_target_strips_pasted_ssh_prefix() {
        assert_eq!(
            normalize_ssh_target("ssh alice@login.school.edu").unwrap(),
            "alice@login.school.edu"
        );
    }

    #[test]
    fn normalize_ssh_target_rejects_full_commands_with_options() {
        assert!(normalize_ssh_target("ssh -p 2222 alice@login.school.edu").is_err());
        assert!(normalize_ssh_target("alice@login.school.edu true").is_err());
    }

    #[test]
    fn control_master_script_waits_for_verified_socket_before_closing() {
        let profile = SshRemoteProfile {
            name: "school".to_string(),
            ssh_target: "alice@login.school.edu".to_string(),
            workspace: "~".to_string(),
        };

        let script = build_control_master_script(&profile).unwrap();
        assert!(script.contains("Step 3/4: SSH accepted the login"));
        assert!(script.contains("Step 4/4: Connected and verified"));
        assert!(script.contains("ssh -S"));
        assert!(script.contains("-O check"));
        assert!(script.contains("Press Enter to close this terminal"));
        assert!(script.contains("Jcode cannot read or store"));
    }
}
