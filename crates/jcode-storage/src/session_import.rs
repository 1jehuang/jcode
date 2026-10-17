//! Explicit, one-way import. Never load a source through Session::load: that
//! path may repair snapshots, prune backups, or replay journals in place.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Copy a selected jcode session into KCode's private storage. No credentials,
/// config, indexes, sockets, or other product state are imported.
pub fn import_jcode_session(session_id: &str) -> io::Result<Vec<PathBuf>> {
    let home = dirs::home_dir().ok_or_else(|| io::Error::other("No home directory"))?;
    let destination = crate::jcode_dir().map_err(io::Error::other)?;
    import_session(&home.join(".jcode"), &destination, session_id)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn reject_links(path: &Path) -> io::Result<()> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(invalid("session import does not follow links"));
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err(invalid("session import does not follow reparse points"));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn import_session(source: &Path, destination: &Path, id: &str) -> io::Result<Vec<PathBuf>> {
    if id.is_empty()
        || id.len() > 160
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(invalid("invalid session ID"));
    }
    // Check before creating anything. In particular, a destination nested in
    // the source must not change even the source directory's mtime.
    reject_links(source)?;
    reject_links(destination)?;
    let source = source.canonicalize()?;
    let mut existing = destination;
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| invalid("invalid destination"))?;
    }
    let existing = existing.canonicalize()?;
    if existing.starts_with(&source) || source.starts_with(&existing) && destination.exists() {
        return Err(invalid("source and destination storage must be disjoint"));
    }
    let source_sessions = source.join("sessions");
    reject_links(&source_sessions)?;
    let mut inputs = Vec::new();
    for suffix in ["json", "bak", "journal.jsonl"] {
        let name = format!("{id}.{suffix}");
        let path = source_sessions.join(&name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                reject_links(&path)?;
                let mut options = OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
                }
                let input = options.open(&path)?; // read-only; no chmod/repair/lock writes
                if !input.metadata()?.is_file() {
                    return Err(invalid("source session must be a regular file"));
                }
                inputs.push((name, input));
            }
            Ok(_) => return Err(invalid("source session must be a regular file")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if !inputs.iter().any(|(name, _)| name == &format!("{id}.json")) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "session snapshot not found",
        ));
    }
    let output_dir = destination.join("sessions");
    reject_links(&output_dir)?;
    fs::create_dir_all(&output_dir)?;
    let mut created = Vec::new();
    let result: io::Result<()> = (|| {
        for (name, mut input) in inputs {
            let output_path = output_dir.join(name);
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut output = options.open(&output_path)?;
            created.push(output_path);
            io::copy(&mut input, &mut output)?;
            output.sync_all()?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        // Only exact paths successfully created by this invocation.
        for path in &created {
            let _ = fs::remove_file(path);
        }
        return Err(error);
    }
    Ok(created)
}

#[cfg(test)]
#[path = "session_import_tests.rs"]
mod tests;
