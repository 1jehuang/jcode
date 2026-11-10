use super::*;

#[cfg(all(test, target_os = "linux"))]
type StagingProbe = Box<dyn Fn(&Path)>;

#[cfg(all(test, target_os = "linux"))]
thread_local! {
    static BEFORE_CONTENT_WRITE: std::cell::RefCell<Option<StagingProbe>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn apply(dir: &Path, change: &Change, reverse: bool) -> Result<()> {
    let (expected, desired) = if reverse {
        (&change.after, &change.before)
    } else {
        (&change.before, &change.after)
    };
    let actual = state(dir, &change.path)?;
    ensure!(
        expected.matches(&actual),
        "File changed outside tracked tools: {}",
        change.path.display()
    );
    let Some(data) = bytes(dir, desired)? else {
        if expected.blob.is_some() {
            fs::remove_file(&change.path)?;
            sync_parent(&change.path)?;
        }
        return Ok(());
    };
    let parent = change.path.parent().context("File has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".jcode-rewind-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        #[cfg(unix)]
        let mode = desired.mode.or(actual.mode);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Without a recorded mode this is a new file: create it like an
            // ordinary write so umask, default ACLs and setgid apply natively.
            // Start tracked siblings without access while ownership changes.
            // The creating handle retains write access even with mode 0000.
            options.mode(if mode.is_some() { 0 } else { 0o666 });
        }
        let mut file = options.open(&temporary)?;
        // Restore the original DACL onto the still-empty sibling before any
        // contents land: it was created with the parent's inherited access, so
        // writing first would briefly expose a private file. The open handle
        // keeps write access. A new file records no descriptor and keeps the
        // inherited access, like an ordinary write.
        #[cfg(windows)]
        if let Some(metadata) = desired.metadata.as_ref().or(actual.metadata.as_ref()) {
            write_security_descriptor(&temporary, &metadata.security_descriptor)?;
        }
        // macOS: an inheritable parent ACL reaches the sibling despite
        // restrictive mode bits. Apply (or clear) it before any bytes land;
        // reapply it after owner and mode changes below, as the final state.
        #[cfg(target_os = "macos")]
        if let Some(metadata) = desired.metadata.as_ref().or(actual.metadata.as_ref()) {
            write_acl(&temporary, &metadata.acl)?;
        }
        #[cfg(unix)]
        if let Some(metadata) = desired.metadata.as_ref().or(actual.metadata.as_ref()) {
            use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};
            let current = file.metadata()?;
            if current.uid() != metadata.uid || current.gid() != metadata.gid {
                let c_path = std::ffi::CString::new(temporary.as_os_str().as_bytes())?;
                let result = unsafe { libc::chown(c_path.as_ptr(), metadata.uid, metadata.gid) };
                if result != 0 {
                    return Err(std::io::Error::last_os_error())
                        .context("Could not preserve file owner and group");
                }
            }
        }
        // Set the original access ACL before chmod can enable an inherited
        // reader. An open of the empty sibling could retain access to later
        // bytes. Other xattrs wait: user attrs require write permission, and a
        // content write can clear capabilities.
        #[cfg(target_os = "linux")]
        if let Some(metadata) = desired.metadata.as_ref().or(actual.metadata.as_ref()) {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(temporary.as_os_str().as_bytes())?;
            let acl = c"system.posix_acl_access";
            let result = if let Some(value) = metadata.xattrs.get("system.posix_acl_access") {
                unsafe {
                    libc::lsetxattr(
                        path.as_ptr(),
                        acl.as_ptr(),
                        value.as_ptr().cast(),
                        value.len(),
                        0,
                    )
                }
            } else if read_xattrs(&temporary)?.contains_key("system.posix_acl_access") {
                unsafe { libc::lremovexattr(path.as_ptr(), acl.as_ptr()) }
            } else {
                0
            };
            ensure!(
                result == 0,
                "Could not restore file ACL: {}",
                std::io::Error::last_os_error()
            );
        }
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        #[cfg(all(test, target_os = "linux"))]
        BEFORE_CONTENT_WRITE.with(|probe| {
            if let Some(probe) = probe.borrow().as_ref() {
                probe(&temporary);
            }
        });
        file.write_all(&data)?;
        // A write can clear set-id bits; restoring them is safe now that the
        // sibling already has the original access rules and ownership.
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        // chown/chmod can clear capabilities or alter ACL masks. Restore access
        // attributes last, and leave the original untouched if any cannot be set.
        #[cfg(unix)]
        if let Some(metadata) = desired.metadata.as_ref().or(actual.metadata.as_ref()) {
            write_xattrs(&temporary, &metadata.xattrs)?;
            // macOS ACLs live outside named xattrs; restore them separately so
            // a deny rule on the original cannot vanish across the replace.
            #[cfg(target_os = "macos")]
            write_acl(&temporary, &metadata.acl)?;
        }
        #[cfg(not(unix))]
        if let Some(mode) = desired.mode {
            let mut permissions = file.metadata()?.permissions();
            permissions.set_readonly(mode != 0);
            file.set_permissions(permissions)?;
        }
        file.sync_all()?;
        // Recheck after staging, rather than overwriting a newer external edit.
        ensure!(
            actual.matches(&state(dir, &change.path)?),
            "File changed during restore: {}",
            change.path.display()
        );
        fs::rename(&temporary, &change.path)?;
        sync_parent(&change.path)?;
        Ok(())
    })();
    if result.is_err()
        && let Err(error) = fs::remove_file(&temporary)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return result.context(format!("Could not remove staged file: {error}"));
    }
    result
}

#[cfg(all(test, unix))]
#[path = "file_history_acl_tests.rs"]
mod tests;
