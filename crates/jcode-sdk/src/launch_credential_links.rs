use super::*;

/// Share rotating login records with an instance and copy mutable config.
/// Returns the instance-relative paths which were inherited.
pub fn inherit_credentials(from_home: &Path, to_home: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(to_home).map_err(launch_io)?;
    let metadata = fs::symlink_metadata(to_home).map_err(launch_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::new(
            ErrorKind::InvalidInstanceHome,
            format!(
                "instance home must be a real directory, not a link or file: {}",
                to_home.display()
            ),
        ));
    }
    if from_home.exists() && fs::canonicalize(from_home).ok() == fs::canonicalize(to_home).ok() {
        return Err(Error::new(
            ErrorKind::InvalidInstanceHome,
            "instance home must be different from the user's jcode home",
        ));
    }

    let mut inherited = Vec::new();
    for name in CREDENTIAL_FILES {
        let source = from_home.join(name);
        if !source.is_file() {
            continue;
        }
        let destination = to_home.join(name);
        if *name == "config.toml" {
            fs::copy(&source, &destination).map_err(launch_io)?;
            set_owner_only_file(&destination)?;
        } else {
            link_credential_file(&source, to_home, Path::new(name))?;
        }
        inherited.push(PathBuf::from(name));
    }

    // Provider env files live in the platform config directory. Link exact
    // files, never the directory, so cleanup cannot traverse into user data.
    if let Ok(entries) = fs::read_dir(user_app_config_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().ends_with(".env") || !entry.path().is_file() {
                continue;
            }
            let relative = PathBuf::from("config").join("jcode").join(&name);
            link_credential_file(&entry.path(), to_home, &relative)?;
            inherited.push(relative);
        }
    }

    let home = home_dir();
    for relative in EXTERNAL_CREDENTIAL_FILES {
        let source = home.join(relative);
        if !source.is_file() {
            continue;
        }
        let instance_relative = PathBuf::from("external").join(relative);
        link_credential_file(&source, to_home, &instance_relative)?;
        inherited.push(instance_relative);
    }

    // OpenClaw's current credential store is per-agent. Enumerate only one
    // agent-id level and only the two filenames its loader recognizes.
    if let Ok(agents) = fs::read_dir(home.join(".openclaw/agents")) {
        for agent in agents.flatten().filter(|entry| entry.path().is_dir()) {
            for name in ["auth-profiles.json", "auth.json"] {
                let relative = PathBuf::from(".openclaw")
                    .join("agents")
                    .join(agent.file_name())
                    .join("agent")
                    .join(name);
                let source = home.join(&relative);
                if source.is_file() {
                    let instance_relative = PathBuf::from("external").join(&relative);
                    link_credential_file(&source, to_home, &instance_relative)?;
                    inherited.push(instance_relative);
                }
            }
        }
    }
    Ok(inherited)
}

pub(super) fn link_credential_file(source: &Path, root: &Path, relative: &Path) -> Result<()> {
    let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
    let parent = ensure_instance_directory(root, parent_relative)?;
    let destination = parent.join(relative.file_name().unwrap_or_default());
    let _ = fs::remove_file(&destination);
    link_file(source, &destination)
}

#[cfg(unix)]
pub(super) fn link_file(source: &Path, destination: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, destination).map_err(launch_io)
}

#[cfg(windows)]
pub(super) fn link_file(source: &Path, destination: &Path) -> Result<()> {
    // Preserve rotating credentials rather than silently creating a stale copy.
    // Windows may require Developer Mode or symlink privileges; report that OS
    // error to the caller when links are unavailable.
    std::os::windows::fs::symlink_file(source, destination).map_err(launch_io)
}

pub(super) fn ensure_instance_directory(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(Error::new(
            ErrorKind::InvalidInstanceHome,
            format!("unsafe instance path: {}", relative.display()),
        ));
    }
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                fs::remove_file(&current).map_err(launch_io)?;
                fs::create_dir(&current).map_err(launch_io)?;
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(Error::new(
                    ErrorKind::InvalidInstanceHome,
                    format!(
                        "instance credential path is not a directory: {}",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(launch_io)?;
            }
            Err(error) => return Err(launch_io(error)),
        }
        set_owner_only_dir(&current)?;
    }
    Ok(current)
}
