//! Desktop-only development commands. Never invokes CLI selfdev or manipulates focus.
use super::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

pub struct DesktopSelfDevTool;

impl DesktopSelfDevTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct Input {
    action: String,
    #[serde(default)]
    instance: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

#[async_trait]
impl Tool for DesktopSelfDevTool {
    fn name(&self) -> &str {
        "desktop_selfdev"
    }

    fn description(&self) -> &str {
        "Build, reload, test, or screenshot Jcode Desktop from its checkout (not the CLI)."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type":"object", "required":["action"], "properties": {
            "intent": super::intent_schema_property(),
            "action": {"type":"string", "enum":["status","build","reload","build-reload","reload-bridge","test","screenshot","inspect"], "description":"reload-bridge restarts the harness API bridge onto this checkout's newest target/*/jcode-harness-api-bridge as a daemon task that survives this connection dropping. Never kill or restart the bridge by hand from bash: this session talks through it."},
            "instance": {"type":"string", "enum":["main","no-sidebar"], "description":"Required when both Desktop instances exist. No arbitrary socket paths."},
            "command": {"type":"string", "description":"Optional test shell command, run with the Desktop repository as cwd. Default cargo test."},
            "output": {"type":"string", "description":"Screenshot path under target/. Default desktop-selfdev.png. Uses private Xvfb."},
            "timeout_seconds": {"type":"integer", "minimum":1, "maximum":600, "description":"Command timeout, default 120s. For longer jobs use bash in the Desktop checkout."}
        }})
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        // No process-global cwd fallback: an absent context must not grant this mode.
        let root = desktop_root(ctx.working_dir.as_deref())?;
        let input: Input = serde_json::from_value(input)?;
        let timeout = input.timeout_seconds.unwrap_or(120);
        if !(1..=600).contains(&timeout) {
            bail!("timeout_seconds must be between 1 and 600");
        }
        if input.command.is_some() && input.action != "test" {
            bail!("command is supported only for test");
        }
        if input.output.is_some() && input.action != "screenshot" {
            bail!("output is supported only for screenshot");
        }
        match input.action.as_str() {
            #[cfg(unix)]
            "reload-bridge" => {
                let launch = super::bridge_reload::desktop_bridge_launch(&root)?;
                return Ok(
                    super::bridge_reload::spawn_bridge_reload(&ctx.session_id, launch).await,
                );
            }
            "status" | "reload" | "build-reload" | "inspect" | "build" => (),
            "test" | "screenshot" => {
                let spec = command_spec(&root, &input, "debug", None)?;
                return run_command(&root, spec, timeout).await;
            }
            other => bail!("Unknown desktop_selfdev action: {other}"),
        }
        let endpoint = select_instance(input.instance.as_deref())?;
        let mut host = match endpoint {
            Some(path) => Some(connect_host(&root, &path).await?),
            None => None,
        };
        if input.action == "status" {
            return Ok(ToolOutput::new(serde_json::to_string_pretty(&json!({
                "mode":"desktop", "repo":root,
                "instance": host.as_ref().map(|h| json!({"socket":h.path,"pid":h.pid,"profile":h.profile})),
                "default_build_profile":"debug", "reload_completion_verifiable":false,
                "note":"Instance metadata only. Host R protocol acknowledges enqueueing, not build/reload completion. Inspect lists UI preview states, not full app state."
            }))?));
        }
        if matches!(input.action.as_str(), "reload" | "build-reload") {
            let host = host.as_mut().context(
                "No Desktop instance is running. Start Desktop from this checkout first.",
            )?;
            host.reload().await?;
            return Ok(ToolOutput::new(serde_json::to_string_pretty(&json!({
                "action":input.action,"socket":host.path,"pid":host.pid,
                "acknowledged":true,"completed":false,
                "message":"Host acknowledged R: the same rebuild-and-reload request as Ctrl+R, without focusing the window. Build success and UI generation are not verified by this protocol."
            }))?));
        }
        let profile = host.as_ref().map(|h| h.profile.as_str()).unwrap_or("debug");
        let pid = host.as_ref().map(|h| h.pid);
        let spec = command_spec(&root, &input, profile, pid)?;
        // Release the read-only probe connection before running a potentially long job.
        drop(host);
        run_command(&root, spec, timeout).await
    }
}

fn desktop_root(cwd: Option<&Path>) -> Result<PathBuf> {
    cwd.and_then(jcode_selfdev_types::desktop_repo_root)
        .context("desktop_selfdev is available only inside a Jcode Desktop source checkout (ctx.working_dir). It is distinct from CLI selfdev.")
}

#[derive(Debug, PartialEq, Eq)]
struct CommandSpec {
    program: String,
    args: Vec<String>,
    note: String,
}

fn command_spec(
    root: &Path,
    input: &Input,
    profile: &str,
    pid: Option<u32>,
) -> Result<CommandSpec> {
    let (program, args, note) = match input.action.as_str() {
        "build" => {
            let mut args = vec!["build", "-p", "jcode-desktop", "-p", "jcode-desktop-ui"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>();
            if profile == "release" {
                args.push("--release".into());
            } else if profile != "debug" {
                args.extend(["--profile".into(), profile.into()]);
            }
            (
                "cargo",
                args,
                format!("Paired Desktop host/UI build, profile {profile}. Does not reload."),
            )
        }
        "test" => match &input.command {
            Some(command) if !command.trim().is_empty() => (
                "bash",
                vec!["-c".into(), command.clone()],
                "Provided test command, Desktop repository cwd.".into(),
            ),
            Some(_) => bail!("test command must not be empty"),
            None => (
                "cargo",
                vec!["test".into()],
                "Desktop repository tests.".into(),
            ),
        },
        "screenshot" => {
            let output = Path::new(input.output.as_deref().unwrap_or("desktop-selfdev.png"));
            if output.as_os_str().is_empty()
                || output
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_)))
            {
                bail!("Screenshot output must be a relative path under target without . or ..");
            }
            let target = root.join("target");
            let output = target.join(output);
            // Refuse existing symlink components, including target, before the script writes.
            reject_symlink_components(&output)?;
            ("python3", vec!["scripts/screenshot.py".into(), output.display().to_string()], "Build current debug Desktop and capture a private Xvfb/offline fixture. Independent of the live host, not a capture of its window.".into())
        }
        "inspect" => {
            let pid = pid.context("No verified Desktop instance for inspection. Start Desktop with --hot-reload first.")?;
            ("python3", vec!["scripts/preview-state.py".into(), "--list".into(), "--pid".into(), pid.to_string()],
                "Read-only UI preview catalog. Requires the host's self-development preview endpoint. Not full live application state.".into())
        }
        _ => bail!("Action does not route to a subprocess"),
    };
    Ok(CommandSpec {
        program: program.into(),
        args,
        note,
    })
}

fn reject_symlink_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                bail!("Refusing symlink path: {}", current.display())
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn select_instance(name: Option<&str>) -> Result<Option<PathBuf>> {
    let names: &[&str] = match name {
        None => &["main", "no-sidebar"],
        Some("main") => &["main"],
        Some("no-sidebar") => &["no-sidebar"],
        Some(_) => bail!("instance must be main or no-sidebar"),
    };
    let runtime = std::env::var_os("XDG_RUNTIME_DIR");
    let mut paths = Vec::new();
    for name in names {
        let filename = if *name == "main" {
            "jcode-desktop.sock"
        } else {
            "jcode-desktop-no-sidebar.sock"
        };
        let path = if let Some(runtime) = &runtime {
            PathBuf::from(runtime).join(filename)
        } else {
            let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
            if user.is_empty() || user.contains('/') || user.contains('\\') {
                bail!("Unsafe USER value for Desktop instance discovery");
            }
            std::env::temp_dir().join(format!("{user}-{filename}"))
        };
        // macOS commonly aliases /tmp and /var. Resolve the directory, never the socket.
        let parent = path.parent().context("Desktop socket has no parent")?;
        let parent = match parent.canonicalize() {
            Ok(parent) => parent,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let path = parent.join(path.file_name().context("Desktop socket has no filename")?);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => paths.push(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    choose_instance(paths)
}

fn choose_instance(mut paths: Vec<PathBuf>) -> Result<Option<PathBuf>> {
    if paths.len() > 1 {
        bail!(
            "Multiple Desktop instances exist. Specify instance: main or no-sidebar. No reload was sent."
        );
    }
    Ok(paths.pop())
}

struct Host {
    path: PathBuf,
    pid: u32,
    profile: String,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    stream: tokio::net::UnixStream,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn connect_host(root: &Path, path: &Path) -> Result<Host> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    reject_symlink_components(path)?;
    let meta = std::fs::symlink_metadata(path)?;
    let uid = unsafe { libc::geteuid() };
    if !meta.file_type().is_socket() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        bail!(
            "Refusing unsafe Desktop instance socket: {}",
            path.display()
        );
    }
    let stream = tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::UnixStream::connect(path),
    )
    .await??;
    let peer = stream.peer_cred()?;
    if peer.uid() != uid {
        bail!("Desktop socket peer belongs to another user");
    }
    let (pid, exe) = peer_process(&stream)?;
    let profile = host_profile(root, &exe)?;
    Ok(Host {
        path: path.into(),
        pid,
        profile,
        stream,
    })
}

#[cfg(target_os = "linux")]
fn peer_process(stream: &tokio::net::UnixStream) -> Result<(u32, PathBuf)> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let pid = stream
        .peer_cred()?
        .pid()
        .filter(|pid| *pid > 0)
        .context("Desktop socket has no peer PID")? as u32;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))?;
    // Cargo atomically replaces the host executable during a paired UI build.
    // The authenticated running host still belongs to that checkout afterward.
    let bytes = exe.as_os_str().as_bytes();
    let bytes = bytes.strip_suffix(b" (deleted)").unwrap_or(bytes);
    Ok((
        pid,
        PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec())),
    ))
}

#[cfg(target_os = "macos")]
fn peer_process(stream: &tokio::net::UnixStream) -> Result<(u32, PathBuf)> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStringExt;
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of_val(&pid) as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    if result != 0 || pid <= 0 {
        bail!(
            "Cannot authenticate Desktop peer PID: {}",
            std::io::Error::last_os_error()
        );
    }
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let size = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if size <= 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    buffer.truncate(buffer.iter().position(|b| *b == 0).unwrap_or(size as usize));
    Ok((
        pid as u32,
        PathBuf::from(std::ffi::OsString::from_vec(buffer)),
    ))
}

fn host_profile(root: &Path, exe: &Path) -> Result<String> {
    // Reject installed binaries, other checkouts and ambiguous custom target directories.
    let relative = exe
        .strip_prefix(root.join("target"))
        .context("Desktop instance is not a build from this checkout's target directory")?;
    let parts = relative.components().collect::<Vec<_>>();
    if parts.len() != 2 || parts[1].as_os_str() != "jcode-desktop" {
        bail!(
            "Cannot safely determine the Desktop instance build profile: {}",
            exe.display()
        );
    }
    let profile = parts[0]
        .as_os_str()
        .to_str()
        .context("Non-UTF8 build profile")?;
    if profile.is_empty()
        || !profile
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("Invalid Desktop build profile");
    }
    Ok(profile.into())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn connect_host(_root: &Path, _path: &Path) -> Result<Host> {
    bail!(
        "Safe Desktop instance verification currently requires Linux or macOS peer process credentials"
    )
}

impl Host {
    async fn reload(&mut self) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use tokio::io::AsyncWriteExt;
            tokio::time::timeout(Duration::from_secs(3), async {
                self.stream.write_all(b"R").await?;
                let mut response = [0; 3];
                self.stream.read_exact(&mut response).await?;
                if &response != b"ok\n" {
                    bail!("Invalid Desktop reload acknowledgement");
                }
                Ok::<_, anyhow::Error>(())
            }).await.context("Desktop reload acknowledgement timed out. Request may have been queued, do not blindly retry.")??;
            Ok(())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        bail!("Desktop reload requires Linux or macOS instance verification")
    }
}

const OUTPUT_LIMIT: usize = 64 * 1024;

async fn bounded_output(mut reader: impl AsyncRead + Unpin) -> Result<String> {
    let mut kept = Vec::new();
    let mut buf = [0; 8192];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let take = n.min(OUTPUT_LIMIT - kept.len());
        kept.extend_from_slice(&buf[..take]);
        truncated |= take < n;
    }
    let mut text = String::from_utf8_lossy(&kept).into_owned();
    if truncated {
        text.push_str("\n[output truncated after 64 KiB]");
    }
    Ok(text)
}

// Kill the entire private process group on timeout or cancellation, including Xvfb/compiler descendants.
struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}

async fn run_command(root: &Path, spec: CommandSpec, timeout: u64) -> Result<ToolOutput> {
    let mut command = tokio::process::Command::new(&spec.program);
    command
        .args(&spec.args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("Start {} in {}", spec.program, root.display()))?;
    let _group = ProcessGroup(child.id().context("Missing child PID")?);
    let stdout = child.stdout.take().context("Missing stdout")?;
    let stderr = child.stderr.take().context("Missing stderr")?;
    let result = tokio::time::timeout(Duration::from_secs(timeout), async {
        tokio::try_join!(
            async { child.wait().await.map_err(anyhow::Error::from) },
            bounded_output(stdout),
            bounded_output(stderr)
        )
    })
    .await;
    let (status, stdout, stderr) = match result {
        Ok(result) => result?,
        Err(_) => bail!(
            "Desktop command timed out after {timeout}s. Its private process group was terminated. Use bash/background from {} for longer jobs.",
            root.display()
        ),
    };
    Ok(ToolOutput::new(serde_json::to_string_pretty(&json!({
        "program":spec.program,"args":spec.args,"cwd":root,"note":spec.note,
        "success":status.success(),"exit_code":status.code(),"stdout":stdout,"stderr":stderr
    }))?))
}

#[cfg(test)]
#[path = "desktop_selfdev_tests.rs"]
mod tests;
