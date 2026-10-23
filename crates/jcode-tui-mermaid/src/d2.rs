//! Local-only D2 compilation with a deliberately conservative source subset.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

pub const MAX_D2_SOURCE_BYTES: usize = 64 * 1024;
pub const MAX_D2_SVG_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum D2Layout {
    #[default]
    Dagre,
    Elk,
    Tala,
}

impl D2Layout {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dagre => "dagre",
            Self::Elk => "elk",
            Self::Tala => "tala",
        }
    }
}

/// Captured by the client at startup, not reloaded behind prepared Markdown.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct D2Options {
    pub binary: PathBuf,
    pub layout: D2Layout,
    pub timeout: Duration,
}

impl Default for D2Options {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("d2"),
            layout: D2Layout::Dagre,
            timeout: Duration::from_secs(10),
        }
    }
}

/// This is not a general D2 parser. Reject potentially dangerous spellings even
/// in labels/comments, rather than trying to recognize every quoted/escaped key.
pub fn validate_d2_source(source: &str) -> Result<(), String> {
    if source.is_empty() || source.len() > MAX_D2_SOURCE_BYTES {
        return Err("D2 source must contain 1 to 65536 bytes".into());
    }
    if source
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return Err("D2 source contains unsupported control characters".into());
    }
    // An ASCII projection after Unicode lowercase is conservative across Go's
    // simple case mapping and Rust's full mapping (notably dotted I), while
    // retaining ordinary Unicode labels that do not spell an asset keyword.
    let lower: String = source
        .to_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii() && !['"', '\''].contains(ch))
        .collect();
    if source.contains(['@', '\\', '$', '|', '*', '?'])
        || lower.contains("icon")
        || lower.contains("image")
    {
        return Err("D2 inline diagrams support self-contained source only (no imports, icons/images, escapes, variables, globs, or block labels)".into());
    }
    Ok(())
}

pub(crate) fn resolve_d2_binary(binary: &Path) -> Result<PathBuf, String> {
    fn executable(path: &Path) -> bool {
        let Ok(metadata) = path.metadata() else {
            return false;
        };
        if !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
    if binary.is_absolute() || binary.components().count() > 1 {
        if executable(binary) {
            return binary
                .canonicalize()
                .map_err(|e| format!("Resolve D2 executable: {e}"));
        }
    } else if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(binary);
            if executable(&candidate) {
                return candidate
                    .canonicalize()
                    .map_err(|e| format!("Resolve D2 executable: {e}"));
            }
            #[cfg(windows)]
            if candidate.extension().is_none() {
                let candidate = candidate.with_extension("exe");
                if executable(&candidate) {
                    return candidate
                        .canonicalize()
                        .map_err(|e| format!("Resolve D2 executable: {e}"));
                }
            }
        }
    }
    Err(format!(
        "D2 executable not found: {}. Install d2 or set diagrams.d2_binary, then restart this client",
        binary.display()
    ))
}

pub fn compile_d2(source: &str, options: &D2Options) -> Result<String, String> {
    compile_d2_cancellable(source, options, Arc::new(AtomicBool::new(false)))
}

pub(crate) fn compile_d2_cancellable(
    source: &str,
    options: &D2Options,
    cancelled: Arc<AtomicBool>,
) -> Result<String, String> {
    // Must run before executable resolution, filesystem setup, or child launch.
    validate_d2_source(source)?;
    let binary = resolve_d2_binary(&options.binary)?;
    let args: Vec<OsString> = [
        format!("--layout={}", options.layout.as_str()),
        "--watch=false".into(),
        "--bundle=false".into(),
        "--theme=200".into(),
        "--pad=20".into(),
        "--scale=1".into(),
        "--target=".into(),
        format!("--timeout={}", options.timeout.as_secs().max(1)),
        "-".into(),
        "-".into(),
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    let output = run_child(
        &binary,
        &args,
        source.as_bytes(),
        options.timeout,
        MAX_D2_SVG_BYTES,
        cancelled,
    )?;
    let svg = String::from_utf8(output).map_err(|_| "D2 returned non-UTF-8 SVG".to_string())?;
    validate_d2_svg(&svg)?;
    Ok(svg)
}

pub(crate) fn validate_d2_svg(svg: &str) -> Result<(), String> {
    let lower = svg.to_ascii_lowercase();
    if !lower.contains("<svg") || svg.len() > MAX_D2_SVG_BYTES {
        return Err("D2 returned empty, oversized, or invalid SVG".into());
    }
    if [
        "<!doctype",
        "<!entity",
        "<image",
        "<foreignobject",
        "<script",
    ]
    .iter()
    .any(|tag| lower.contains(tag))
    {
        return Err("D2 returned unsupported SVG resources or markup".into());
    }
    Ok(())
}

pub(crate) struct PrivateDirectory(PathBuf);
impl PrivateDirectory {
    pub(crate) fn new() -> Result<Self, String> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        for _ in 0..32 {
            let path = std::env::temp_dir().join(format!(
                "jcode-d2-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("Create D2 scratch directory: {e}")),
            }
        }
        Err("Could not allocate private D2 scratch directory".into())
    }
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            super::log_warn(&format!("Remove private D2 directory: {error}"));
        }
    }
}

struct OwnedChild(Child, bool);
impl OwnedChild {
    fn stop(&mut self) {
        if self.1 {
            return;
        }
        self.1 = true;
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            if let Ok(pid) = i32::try_from(self.0.id()) {
                // SAFETY: the child was placed in its own process group before
                // spawn. This signals that owned group, not the user's shell.
                unsafe {
                    kill(-pid, 9);
                }
            }
        }
        if let Err(error) = self.0.kill()
            && error.kind() != std::io::ErrorKind::InvalidInput
        {
            super::log_warn(&format!("Stop D2 child: {error}"));
        }
        if let Err(error) = self.0.wait() {
            super::log_warn(&format!("Reap D2 child: {error}"));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.stop();
    }
}

fn capture(
    mut stream: impl Read + Send + 'static,
    limit: usize,
    overflow: Arc<AtomicBool>,
) -> mpsc::Receiver<Result<Vec<u8>, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 8192];
        let result = loop {
            match stream.read(&mut chunk) {
                Ok(0) => break Ok(bytes),
                Ok(count) => {
                    let available = limit.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&chunk[..count.min(available)]);
                    if count > available {
                        overflow.store(true, Ordering::Release);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => break Err(format!("Read D2 output: {e}")),
            }
        };
        if tx.send(result).is_err() {
            super::log_info("D2 output receiver closed after cancellation or timeout");
        }
    });
    rx
}

pub(crate) fn run_child(
    binary: &Path,
    args: &[OsString],
    source: &[u8],
    timeout: Duration,
    output_limit: usize,
    cancelled: Arc<AtomicBool>,
) -> Result<Vec<u8>, String> {
    let started = Instant::now();
    let directory = PrivateDirectory::new()?;
    let mut command = Command::new(binary);
    command
        .args(args)
        .current_dir(directory.path())
        .env_clear()
        .env("PATH", OsStr::new(""));
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = OwnedChild(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Start D2: {e}"))?,
        false,
    );
    let mut stdin = child.0.stdin.take().ok_or("D2 stdin unavailable")?;
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout = capture(
        child.0.stdout.take().ok_or("D2 stdout unavailable")?,
        output_limit,
        overflow.clone(),
    );
    let stderr = capture(
        child.0.stderr.take().ok_or("D2 stderr unavailable")?,
        MAX_DIAGNOSTIC_BYTES,
        overflow.clone(),
    );
    let (written_tx, written_rx) = mpsc::channel();
    let source = source.to_vec();
    std::thread::spawn(move || {
        let result = stdin
            .write_all(&source)
            .map_err(|e| format!("Write D2 source: {e}"));
        drop(stdin);
        if written_tx.send(result).is_err() {
            super::log_info("D2 input receiver closed after cancellation or timeout");
        }
    });
    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("D2 compilation cancelled".into());
        }
        if overflow.load(Ordering::Acquire) {
            return Err("D2 output exceeds the allowed size".into());
        }
        if started.elapsed() >= timeout {
            return Err(format!(
                "D2 compilation timed out after {} ms",
                timeout.as_millis()
            ));
        }
        if let Some(status) = child
            .0
            .try_wait()
            .map_err(|e| format!("Wait for D2: {e}"))?
        {
            break status;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    // Close inherited pipe descriptors in any lingering children as well.
    child.stop();
    let remaining = || {
        timeout
            .saturating_sub(started.elapsed())
            .min(Duration::from_secs(1))
    };
    let output = stdout
        .recv_timeout(remaining())
        .map_err(|_| "Timed out draining D2 stdout")??;
    let diagnostic = stderr
        .recv_timeout(remaining())
        .map_err(|_| "Timed out draining D2 stderr")??;
    if overflow.load(Ordering::Acquire) {
        return Err("D2 output exceeds the allowed size".into());
    }
    if !status.success() {
        let diagnostic: String = String::from_utf8_lossy(&diagnostic)
            .chars()
            .filter(|c| !c.is_control() || c.is_whitespace())
            .collect();
        return Err(format!(
            "D2 exited with {status}: {}",
            diagnostic.split_whitespace().collect::<Vec<_>>().join(" ")
        ));
    }
    written_rx
        .recv_timeout(remaining())
        .map_err(|_| "Timed out feeding D2 stdin")??;
    Ok(output)
}

#[cfg(test)]
#[path = "d2_tests.rs"]
mod tests;
