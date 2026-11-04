// Restricted scratch-stage translation of Rust std 1.98.1 Windows executable search.
// Rust standard library is MIT OR Apache-2.0; see https://github.com/rust-lang/rust/blob/master/LICENSE-MIT
// and https://github.com/rust-lang/rust/blob/master/LICENSE-APACHE.
use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetWindowsDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetFullPathNameW(path: *const u16, size: u32, buf: *mut u16, file: *mut *mut u16) -> u32;
}
fn system_directory(get: unsafe extern "system" fn(*mut u16, u32) -> u32) -> io::Result<PathBuf> {
    let mut buf = vec![0u16; 32768];
    let n = unsafe { get(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return Err(io::Error::last_os_error());
    }
    buf.truncate(n);
    Ok(PathBuf::from(OsString::from_wide(&buf)))
}
fn suffix_exe(s: &OsStr) -> bool {
    let u: Vec<u16> = s.encode_wide().collect();
    let suffix = [46, 101, 120, 101];
    u.len() >= 4
        && u[u.len() - 4..]
            .iter()
            .zip(suffix)
            .all(|(x, y)| (*x | 32) == (y | 32))
}
fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}
fn full(path: &Path) -> io::Result<PathBuf> {
    let mut input: Vec<u16> = path.as_os_str().encode_wide().collect();
    if input.contains(&0) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"));
    }
    input.push(0);
    let mut out = vec![0u16; 32768];
    let n = unsafe {
        GetFullPathNameW(
            input.as_ptr(),
            out.len() as u32,
            out.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    } as usize;
    if n == 0 || n >= out.len() {
        return Err(io::Error::last_os_error());
    }
    out.truncate(n);
    Ok(PathBuf::from(OsString::from_wide(&out)))
}
fn user_path(path: &Path) -> io::Result<PathBuf> {
    let w: Vec<u16> = path.as_os_str().encode_wide().collect();
    if w.len() + 1 > 260 || !w.starts_with(&[92, 92, 63, 92]) {
        return Ok(path.to_owned());
    }
    let candidate = if w.len() >= 7 && w[5] == 58 && w[6] == 92 {
        Some(w[4..].to_vec())
    } else if w.len() >= 8 && w[4..8] == [85, 78, 67, 92] {
        let mut x = vec![92, 92];
        x.extend_from_slice(&w[8..]);
        Some(x)
    } else {
        None
    };
    if let Some(candidate) = candidate {
        let p = PathBuf::from(OsString::from_wide(&candidate));
        if full(&p)?
            .as_os_str()
            .encode_wide()
            .eq(candidate.iter().copied())
        {
            return Ok(p);
        }
    }
    Ok(path.to_owned())
}
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "relative, verbatim, or nonabsolute search path not yet supported",
    )
}
fn search_dirs(paths: &OsStr, program: &OsStr, extension: bool) -> io::Result<Option<PathBuf>> {
    for dir in env::split_paths(paths).filter(|p| !p.as_os_str().is_empty()) {
        let dir = if dir.is_absolute() { dir } else { full(&dir)? };
        let mut p = dir.join(program);
        if !extension {
            p.set_extension("exe");
        }
        if exists(&p) {
            return Ok(Some(p));
        }
    }
    Ok(None)
}
pub(super) fn resolve(program: &OsStr, child_path: Option<&OsStr>) -> io::Result<PathBuf> {
    let wide: Vec<u16> = program.encode_wide().collect();
    if wide.is_empty() || wide.contains(&0) || matches!(wide.last(), Some(47 | 92)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid program path",
        ));
    }
    let p = Path::new(program);
    let pathlike = wide.iter().any(|x| matches!(*x, 47 | 58 | 92));
    if pathlike {
        let resolved = if p.is_absolute() {
            user_path(p)?
        } else {
            user_path(&full(p)?)?
        };
        if suffix_exe(program) {
            return Ok(resolved);
        }
        let mut candidate = resolved.as_os_str().to_os_string();
        candidate.push(".exe");
        let candidate = PathBuf::from(candidate);
        return Ok(if exists(&candidate) {
            candidate
        } else {
            resolved
        });
    }
    let extension = wide.contains(&46);
    if let Some(paths) = child_path {
        if let Some(p) = search_dirs(paths, program, extension)? {
            return Ok(p);
        }
    }
    let mut dirs = vec![
        env::current_exe()?
            .parent()
            .ok_or_else(unsupported)?
            .to_path_buf(),
        system_directory(GetSystemDirectoryW)?,
        system_directory(GetWindowsDirectoryW)?,
    ];
    for dir in dirs.drain(..) {
        let mut candidate = dir.join(program);
        if !extension {
            candidate.set_extension("exe");
        }
        if exists(&candidate) {
            return Ok(candidate);
        }
    }
    if let Some(parent) = env::var_os("PATH") {
        if let Some(p) = search_dirs(&parent, program, extension)? {
            return Ok(p);
        }
    }
    Err(io::Error::new(io::ErrorKind::NotFound, "program not found"))
}

pub(super) fn command_prompt() -> io::Result<PathBuf> {
    Ok(system_directory(GetSystemDirectoryW)?.join("cmd.exe"))
}
