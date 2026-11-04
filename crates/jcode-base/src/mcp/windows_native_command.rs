use super::{
    windows_native_args, windows_native_env,
    windows_native_launch::{self, Spawned},
    windows_native_resolve,
};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    io,
    path::Path,
};

pub(super) fn spawn(
    program: &str,
    args: &[String],
    env: &HashMap<OsString, OsString>,
    cwd: Option<&Path>,
) -> io::Result<Spawned> {
    spawn_mode(program, args, env, cwd, false)
}

#[cfg(feature = "test-support")]
pub(super) fn spawn_natural_test(
    program: &str,
    args: &[String],
    env: &HashMap<OsString, OsString>,
    cwd: Option<&Path>,
) -> io::Result<Spawned> {
    spawn_mode(program, args, env, cwd, true)
}

fn spawn_mode(
    program: &str,
    args: &[String],
    env: &HashMap<OsString, OsString>,
    cwd: Option<&Path>,
    _natural_test: bool,
) -> io::Result<Spawned> {
    let entries: Vec<(OsString, OsString)> =
        env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let child_path = child_path(env);
    let resolved = windows_native_resolve::resolve(OsStr::new(program), child_path)?;
    let args: Vec<OsString> = args.iter().map(Into::into).collect();
    let batch = resolved
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|x| x.eq_ignore_ascii_case("cmd") || x.eq_ignore_ascii_case("bat"));
    let (app, line) = if batch {
        (
            windows_native_resolve::command_prompt()?,
            windows_native_args::batch_line(resolved.as_os_str(), &args)?,
        )
    } else {
        (
            resolved,
            windows_native_args::exe_line(OsStr::new(program), &args)?,
        )
    };
    let mut block = windows_native_env::environment_block(entries)?;
    #[cfg(feature = "test-support")]
    if _natural_test {
        if !app
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|s| s.eq_ignore_ascii_case("python.exe"))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "natural fixture requires explicit python.exe",
            ));
        }
        let broker_line =
            windows_native_args::exe_line(app.as_os_str(), &["-c".into(), "pass".into()])?;
        return windows_native_launch::spawn_natural_test(
            app.as_os_str(),
            &line,
            cwd.map(Path::as_os_str),
            Some(&mut block),
            &broker_line,
        );
    }
    windows_native_launch::spawn(
        app.as_os_str(),
        &line,
        cwd.map(Path::as_os_str),
        Some(&mut block),
    )
}

pub(super) fn child_path(env: &HashMap<OsString, OsString>) -> Option<&OsStr> {
    env.iter()
        .find(|(k, _)| windows_native_env::same_key(k, OsStr::new("PATH")))
        .map(|(_, v)| v.as_os_str())
}
