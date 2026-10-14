//! Filesystem confinement for provider processes that were not granted
//! filesystem permission.
//!
//! On Linux this installs a Landlock ruleset in the child before `exec`. The
//! child can read and execute loader paths (and its own executable directory)
//! so it can start, and it cannot write anywhere. Other platforms cannot make
//! the same guarantee, so confinement fails closed instead of launching a
//! process with ordinary host write access.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub fn install(command: &mut Command, program: &OsStr) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        install_landlock(command, program)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (command, program);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "filesystem confinement requires Landlock and is unavailable on this platform",
        ))
    }
}

#[cfg(target_os = "linux")]
fn install_landlock(command: &mut Command, program: &OsStr) -> io::Result<()> {
    use landlock::{
        ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus,
    };

    let abi = ABI::V1;
    let read_exec = AccessFs::from_read(abi) | AccessFs::Execute;
    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|error| io::Error::other(error.to_string()))?
        .create()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mut allowed = false;
    for path in readable_roots(program) {
        let fd = match PathFd::new(&path) {
            Ok(fd) => fd,
            Err(_) => continue,
        };
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, read_exec))
            .map_err(|error| io::Error::other(error.to_string()))?;
        allowed = true;
    }
    if !allowed {
        return Err(io::Error::other(
            "filesystem confinement found no readable loader paths",
        ));
    }
    let mut ruleset = Some(ruleset);
    // Safety: the closure runs in the forked child before exec. It only takes
    // the prepared ruleset and issues the Landlock restrict syscall.
    unsafe {
        command.pre_exec(move || {
            let ruleset = ruleset
                .take()
                .ok_or_else(|| io::Error::other("filesystem confinement already applied"))?;
            let status = ruleset
                .restrict_self()
                .map_err(|error| io::Error::other(error.to_string()))?;
            if status.ruleset != RulesetStatus::FullyEnforced {
                return Err(io::Error::other(format!(
                    "filesystem confinement was not fully enforced ({:?})",
                    status.ruleset
                )));
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn readable_roots(program: &OsStr) -> Vec<PathBuf> {
    let mut roots = ["/usr", "/lib", "/lib64", "/bin", "/etc", "/proc", "/dev"]
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if let Some(executable) = resolve_program(program)
        && let Some(parent) = executable.parent()
    {
        if parent.as_os_str().is_empty() {
            roots.push(PathBuf::from("."));
        } else {
            roots.push(parent.to_path_buf());
        }
    }
    roots
}

#[cfg(target_os = "linux")]
fn resolve_program(program: &OsStr) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() || path.components().count() > 1 {
        return path.canonicalize().ok();
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|directory| {
        let candidate = directory.join(path);
        candidate.is_file().then_some(candidate)
    })
}
