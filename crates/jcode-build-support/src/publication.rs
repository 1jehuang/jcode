//! Installer publication as one kernel-locked transaction.

use crate::{acquire_build_store_lock, binary_name, builds_dir};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// `Current` preserves the local release install's stable + current behavior.
/// `Stable` preserves the downloaded release install's stable-only behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparedBuildChannel {
    Stable,
    Current,
}

/// Publish a prepared flat payload directory, its channels, markers and launcher.
///
/// Run this with the newly built/downloaded binary outside the version store.
/// Old binaries without this helper must fail closed, not fall back to unlocked
/// shell publication. No external locking executable or library is required.
pub fn publish_prepared_build(
    source_dir: &Path,
    version: &str,
    channel: PreparedBuildChannel,
    launcher: &Path,
    termux_launcher: bool,
) -> Result<PathBuf> {
    validate_version(version)?;
    if termux_launcher && cfg!(windows) {
        anyhow::bail!("Termux launchers are not supported on Windows");
    }
    // Validate the entire payload before mutating the store. Archives must be
    // unpacked into a dedicated directory, not the download/build scratch root.
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(source_dir).context("Reading prepared build payload")? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            anyhow::bail!(
                "Prepared build payload must contain only regular files: {}",
                entry.path().display()
            );
        }
        sources.push((entry.file_name(), entry.path()));
    }
    if !sources.iter().any(|(name, _)| name == binary_name()) {
        anyhow::bail!("Prepared build payload is missing {}", binary_name());
    }
    let _store_lock = acquire_build_store_lock()?;
    let versions = builds_dir()?.join("versions");
    std::fs::create_dir_all(&versions)?;
    let canonical_versions = std::fs::canonicalize(&versions)?;
    let source_dir = std::fs::canonicalize(source_dir)?;
    if source_dir.starts_with(&canonical_versions) {
        anyhow::bail!("Prepared build source must be outside the immutable version store");
    }
    let destination = versions.join(version);
    match std::fs::symlink_metadata(&destination) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() {
                anyhow::bail!("Installed version destination must be a real directory");
            }
            // Idempotent reinstallation is allowed only for an identical flat
            // payload. Never overwrite a published or mapped immutable binary.
            let entries: Vec<_> =
                std::fs::read_dir(&destination)?.collect::<std::io::Result<_>>()?;
            if entries.len() != sources.len() {
                anyhow::bail!(
                    "Installed version {version} already exists with a different payload"
                );
            }
            for (name, source) in &sources {
                let target = destination.join(name);
                if !std::fs::symlink_metadata(&target)?.file_type().is_file()
                    || !same_file_contents(source, &target)?
                {
                    anyhow::bail!(
                        "Installed version {version} already exists with a different payload"
                    );
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let staged = tempfile::Builder::new()
                .prefix(".publish-")
                .tempdir_in(&versions)?;
            for (name, source) in sources {
                let target = staged.path().join(name);
                std::fs::copy(source, &target)?;
                if target.file_name().is_some_and(|name| name == binary_name())
                    || target
                        .extension()
                        .is_some_and(|extension| extension == "bin")
                {
                    crate::platform_support::set_permissions_executable(&target)?;
                }
            }
            // Staging prevents partial-copy publication. The store lock, not
            // staging or an age grace period, protects the following reference gap.
            std::fs::rename(staged.path(), &destination)?;
        }
        Err(error) => return Err(error.into()),
    }
    let binary = destination.join(binary_name());
    #[cfg(all(test, unix))]
    publication_barrier()?;
    crate::update_stable_symlink(version)?;
    let channel_binary = match channel {
        PreparedBuildChannel::Stable => crate::stable_binary_path()?,
        PreparedBuildChannel::Current => {
            crate::update_current_symlink(version)?;
            crate::current_binary_path()?
        }
    };
    if let Some(parent) = launcher.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = launcher.with_file_name(format!(".jcode-publish-{}", std::process::id()));
    if termux_launcher {
        let quoted = channel_binary.to_string_lossy().replace('\'', "'\\''");
        std::fs::write(
            &temporary,
            format!("#!/usr/bin/env bash\nunset LD_PRELOAD\nexec '{quoted}' \"$@\"\n"),
        )?;
        crate::platform_support::set_permissions_executable(&temporary)?;
        std::fs::rename(&temporary, launcher)?;
    } else {
        crate::platform_support::atomic_symlink_swap(&channel_binary, launcher, &temporary)?;
    }
    Ok(binary)
}

pub(crate) fn same_file_contents(left: &Path, right: &Path) -> Result<bool> {
    use std::io::{BufReader, Read};
    if std::fs::metadata(left)?.len() != std::fs::metadata(right)?.len() {
        return Ok(false);
    }
    let mut left = BufReader::new(std::fs::File::open(left)?);
    let mut right = BufReader::new(std::fs::File::open(right)?);
    let mut left_bytes = [0; 16 * 1024];
    let mut right_bytes = [0; 16 * 1024];
    loop {
        let count = left.read(&mut left_bytes)?;
        if count == 0 {
            return Ok(true);
        }
        right.read_exact(&mut right_bytes[..count])?;
        if left_bytes[..count] != right_bytes[..count] {
            return Ok(false);
        }
    }
}

pub(crate) fn validate_version(version: &str) -> Result<()> {
    if version.is_empty()
        || version.trim() != version
        || version.starts_with('.')
        || version.contains(['/', '\\', ':'])
        || version.chars().any(char::is_control)
    {
        anyhow::bail!("Invalid installed version label: {version:?}");
    }
    Ok(())
}

#[cfg(all(test, unix))]
fn publication_barrier() -> Result<()> {
    use std::io::Write;
    if let Some(socket) = std::env::var_os("JCODE_TEST_PUBLICATION_SOCKET") {
        let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
        stream.write_all(b"copied")?;
        let mut release = [0];
        read_exact_bounded(&mut stream, &mut release)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
fn read_exact_bounded(
    stream: &mut std::os::unix::net::UnixStream,
    bytes: &mut [u8],
) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind, Read};
    use std::time::{Duration, Instant};
    // Some macOS AF_UNIX implementations reject SO_RCVTIMEO. Use explicit
    // nonblocking reads with a wall-clock deadline instead of socket timeouts.
    stream.set_nonblocking(true)?;
    let started = Instant::now();
    let mut offset = 0;
    while offset < bytes.len() {
        match stream.read(&mut bytes[offset..]) {
            Ok(0) => return Err(Error::from(ErrorKind::UnexpectedEof)),
            Ok(count) => offset += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if started.elapsed() >= Duration::from_secs(15) {
                    return Err(Error::new(
                        ErrorKind::TimedOut,
                        "publication barrier timed out",
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
