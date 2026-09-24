//! Working-tree source snapshots, not Git object/archive snapshots.
//!
//! Credential exclusions are defense in depth, NOT a complete secret detector.
//! Secrets embedded in source or stored under other names can still be uploaded.
use anyhow::{Context, Result, ensure};
use base64::Engine;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;
const MAX_FILES: usize = 10_000;
const MAX_LIST_BYTES: usize = 8 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 1024;
const MAX_PATH_COMPONENTS: usize = 64;
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, serde::Serialize)]
pub(super) struct Snapshot {
    pub files: Vec<SourceFile>,
}

#[derive(Debug, serde::Serialize)]
pub(super) struct SourceFile {
    pub path: String,
    pub content_base64: String,
    pub executable: bool,
}

/// Snapshot eligible current files, including local modifications and untracked
/// files. Requires a Git worktree root. Limits apply to decoded file contents.
/// Safe descriptor-relative file opening currently requires Unix and fails
/// closed on other platforms. This is not an atomic snapshot of concurrent edits.
pub(super) async fn snapshot(root: &Path) -> Result<Snapshot> {
    let root = tokio::fs::canonicalize(root)
        .await
        .context("canonicalize source root")?;
    let top = git_output(&root, &["rev-parse", "--show-toplevel"], MAX_LIST_BYTES).await?;
    let top = std::str::from_utf8(&top).context("Git root is not UTF-8")?;
    let top = top.strip_suffix('\n').unwrap_or(top);
    let top = tokio::fs::canonicalize(top)
        .await
        .context("resolve Git root")?;
    ensure!(root == top, "source root must be the Git worktree toplevel");
    let listing = git_output(
        &root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            ".",
        ],
        MAX_LIST_BYTES,
    )
    .await?;
    // Filesystem reads run outside the async executor. No symlink components
    // are followed, including during concurrent replacement (Unix openat).
    tokio::task::spawn_blocking(move || collect(&root, &listing)).await?
}

async fn git_output(root: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>> {
    tokio::time::timeout(GIT_TIMEOUT, async {
        let mut command = tokio::process::Command::new("git");
        // Do not let inherited Git environment redirect the index/worktree.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        let mut child = command
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.untrackedCache=false",
            ])
            .args(args)
            .current_dir(root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("launch git for source snapshot")?;
        let stdout = child.stdout.take().context("missing git stdout")?;
        let mut output = Vec::new();
        stdout
            .take(limit as u64 + 1)
            .read_to_end(&mut output)
            .await?;
        ensure!(
            output.len() <= limit,
            "Git source listing exceeds {limit} bytes"
        );
        ensure!(
            child.wait().await?.success(),
            "Git source enumeration failed (a Git worktree is required)"
        );
        Ok(output)
    })
    .await
    .context("Git source enumeration timed out")?
}

fn excluded(path: &str) -> bool {
    let lowercase = path.to_ascii_lowercase();
    let parts: Vec<_> = lowercase.split('/').collect();
    if parts.windows(2).any(|pair| {
        (pair[0] == ".cargo" && matches!(pair[1], "credentials" | "credentials.toml"))
            || (pair[0] == ".docker" && pair[1] == "config.json")
    }) {
        return true;
    }
    path.split('/').any(|part| {
        let part = part.to_ascii_lowercase();
        matches!(
            part.as_str(),
            "target"
                | "node_modules"
                | "dist"
                | ".git"
                | ".jcode"
                | ".aws"
                | ".ssh"
                | ".gnupg"
                | ".env"
                | ".npmrc"
                | ".pypirc"
                | ".netrc"
                | "credentials.json"
                | ".git-credentials"
                | "id_rsa"
                | "id_ed25519"
        ) || part.starts_with(".env.")
            || [".pem", ".key", ".p12", ".pfx"]
                .iter()
                .any(|suffix| part.ends_with(suffix))
    })
}

fn relative_path(bytes: &[u8]) -> Result<&str> {
    let path = std::str::from_utf8(bytes).context("source path is not UTF-8")?;
    ensure!(
        !path.is_empty()
            && path.len() <= MAX_PATH_BYTES
            && path.split('/').count() <= MAX_PATH_COMPONENTS
            && !path.chars().any(char::is_control)
            && !path.contains(['\\', ':'])
            && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
            && Path::new(path)
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "source path must contain only normal relative components"
    );
    Ok(path)
}

fn collect(root: &Path, listing: &[u8]) -> Result<Snapshot> {
    ensure!(
        listing.is_empty() || listing.last() == Some(&0),
        "unterminated Git source listing"
    );
    let root_dir = std::fs::File::open(root)?;
    let mut paths = BTreeSet::new();
    for bytes in listing
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
    {
        let path = relative_path(bytes)?;
        if !excluded(path) {
            paths.insert(path);
        }
    }
    let mut files = Vec::new();
    let mut total = 0;
    for path in paths {
        let Some(file) = open_source(&root_dir, path)? else {
            continue;
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let full_path = root.join(path);
        // Additional containment check; descriptor-relative opens above close
        // the symlink race that canonicalization by itself would leave open.
        match std::fs::canonicalize(&full_path) {
            Ok(canonical) if canonical.starts_with(root) => {}
            Ok(_) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("resolve source file"),
        }
        ensure!(
            files.len() < MAX_FILES,
            "source snapshot exceeds {MAX_FILES} files"
        );
        ensure!(
            metadata.len() <= MAX_FILE_BYTES as u64,
            "source file {path} exceeds {MAX_FILE_BYTES} bytes"
        );
        let content = read_content(file, path)?;
        total += content.len();
        ensure!(
            total <= MAX_TOTAL_BYTES,
            "source snapshot exceeds {MAX_TOTAL_BYTES} total bytes"
        );
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        files.push(SourceFile {
            path: path.to_owned(),
            content_base64: base64::engine::general_purpose::STANDARD.encode(content),
            executable,
        });
    }
    ensure!(!files.is_empty(), "source snapshot has no eligible files");
    Ok(Snapshot { files })
}

fn read_content(file: impl Read, path: &str) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut content)
        .with_context(|| format!("read source file {path}"))?;
    ensure!(
        content.len() <= MAX_FILE_BYTES,
        "source file {path} grew beyond {MAX_FILE_BYTES} bytes"
    );
    Ok(content)
}

#[cfg(unix)]
fn open_source(root: &std::fs::File, path: &str) -> Result<Option<std::fs::File>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let mut directory = root.try_clone()?;
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        let name = std::ffi::CString::new(part)?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if parts.peek().is_some() {
                libc::O_DIRECTORY
            } else {
                0
            };
        // SAFETY: directory is a live fd, name is NUL terminated, and a
        // successful descriptor is immediately owned by exactly one File.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(libc::ENOENT | libc::ELOOP | libc::ENOTDIR)
            ) {
                return Ok(None);
            }
            return Err(error).with_context(|| format!("open source file {path}"));
        }
        directory = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    Ok(Some(directory))
}

#[cfg(not(unix))]
fn open_source(_root: &std::fs::File, _path: &str) -> Result<Option<std::fs::File>> {
    anyhow::bail!("safe source snapshot file opening is currently supported only on Unix")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    async fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git_output(dir.path(), &["init", "-q"], 1024).await.unwrap();
        dir
    }

    fn put(root: &Path, path: &str, content: impl AsRef<[u8]>) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    async fn add(root: &Path) {
        git_output(root, &["add", "-f", "--", "."], 1024)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn snapshots_modified_untracked_deleted_and_executable_files() {
        let repo = repo().await;
        put(repo.path(), "src/main.rs", "original");
        put(repo.path(), "deleted", "deleted");
        add(repo.path()).await;
        put(repo.path(), "src/main.rs", "modified");
        std::fs::remove_file(repo.path().join("deleted")).unwrap();
        put(repo.path(), "hello world'λ.sh", "untracked");
        std::fs::set_permissions(
            repo.path().join("hello world'λ.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let result = snapshot(repo.path()).await.unwrap();
        assert_eq!(result.files.len(), 2);
        let script = &result.files[0];
        assert_eq!(script.path, "hello world'λ.sh");
        assert!(script.executable);
        let source = &result.files[1];
        assert!(!source.executable);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&source.content_base64)
                .unwrap(),
            b"modified"
        );
        assert!(serde_json::to_value(&result).unwrap()["files"].is_array());
    }

    #[tokio::test]
    async fn excludes_tracked_credentials_build_files_and_ignored_files() {
        let repo = repo().await;
        for path in [
            ".env",
            ".env.local",
            "nested/.env.example",
            "a.pem",
            "a.key",
            "a.p12",
            "a.pfx",
            ".npmrc",
            ".pypirc",
            ".netrc",
            "credentials.json",
            ".aws/config",
            ".ssh/id_rsa",
            ".gnupg/config",
            "target/a",
            "node_modules/a",
            "dist/a",
            ".jcode/a",
            "nested/target/a",
            "UPPER.PEM",
            ".git-credentials",
            ".cargo/credentials",
            ".cargo/credentials.toml",
            "nested/.cargo/credentials.toml",
            ".docker/config.json",
            "nested/id_rsa",
            "id_ed25519",
        ] {
            put(repo.path(), path, "excluded");
        }
        put(repo.path(), ".gitignore", "ignored\n");
        put(repo.path(), "keep.rs", "source");
        add(repo.path()).await;
        put(repo.path(), "ignored", "ignored");
        let result = snapshot(repo.path()).await.unwrap();
        assert_eq!(
            result
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            [".gitignore", "keep.rs"]
        );
        assert!(excluded(".git/config"));
        assert!(!excluded(".cargo/config.toml"));
        assert!(!excluded(".docker/daemon.json"));
    }

    #[tokio::test]
    async fn does_not_follow_symlink_files_or_ancestors() {
        let repo = repo().await;
        let outside = tempfile::tempdir().unwrap();
        put(outside.path(), "secret", "secret");
        put(repo.path(), "dir/secret", "old");
        put(repo.path(), "inside", "safe");
        add(repo.path()).await;
        std::fs::remove_dir_all(repo.path().join("dir")).unwrap();
        symlink(outside.path(), repo.path().join("dir")).unwrap();
        symlink(
            outside.path().join("secret"),
            repo.path().join("external-link"),
        )
        .unwrap();
        symlink("inside", repo.path().join("internal-link")).unwrap();
        symlink("missing", repo.path().join("dangling-link")).unwrap();
        let result = snapshot(repo.path()).await.unwrap();
        assert_eq!(
            result
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["inside"]
        );
        // Even a symlink ancestor pointing within the root is not traversed.
        put(repo.path(), "real/file", "safe");
        symlink("real", repo.path().join("alias")).unwrap();
        let root_fd = std::fs::File::open(repo.path()).unwrap();
        assert!(open_source(&root_fd, "alias/file").unwrap().is_none());
    }

    #[tokio::test]
    async fn requires_git_toplevel() {
        let empty = tempfile::tempdir().unwrap();
        assert!(snapshot(empty.path()).await.is_err());
        let repo = repo().await;
        std::fs::create_dir(repo.path().join("nested")).unwrap();
        assert!(
            snapshot(&repo.path().join("nested"))
                .await
                .unwrap_err()
                .to_string()
                .contains("toplevel")
        );
    }

    #[tokio::test]
    async fn enforces_file_size_and_accepts_boundary() {
        let repo = repo().await;
        put(repo.path(), "large", vec![0; MAX_FILE_BYTES]);
        assert_eq!(snapshot(repo.path()).await.unwrap().files.len(), 1);
        put(repo.path(), "large", vec![0; MAX_FILE_BYTES + 1]);
        assert!(
            snapshot(repo.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("exceeds")
        );
    }

    #[tokio::test]
    async fn enforces_total_size_and_accepts_boundary() {
        let repo = repo().await;
        for index in 0..10 {
            put(
                repo.path(),
                &format!("file-{index:02}"),
                vec![0; MAX_FILE_BYTES],
            );
        }
        assert_eq!(snapshot(repo.path()).await.unwrap().files.len(), 10);
        put(repo.path(), "overflow", "x");
        assert!(
            snapshot(repo.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("total bytes")
        );
    }

    #[tokio::test]
    async fn enforces_file_count_and_accepts_boundary() {
        let repo = repo().await;
        for index in 0..MAX_FILES {
            put(repo.path(), &format!("file-{index:05}"), "");
        }
        assert_eq!(snapshot(repo.path()).await.unwrap().files.len(), MAX_FILES);
        put(repo.path(), "overflow", "");
        assert!(
            snapshot(repo.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("10000 files")
        );
    }

    #[tokio::test]
    async fn bounds_git_output() {
        let repo = repo().await;
        put(repo.path(), "long-name", "");
        let error = git_output(repo.path(), &["ls-files", "-z", "--others"], 2)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("exceeds 2 bytes"));
    }

    #[tokio::test]
    async fn rejects_non_utf8_git_paths() {
        use std::os::unix::ffi::OsStrExt;
        let repo = repo().await;
        let name = std::ffi::OsStr::from_bytes(b"non-utf8-\xff");
        std::fs::write(repo.path().join(name), b"contents").unwrap();
        assert!(
            snapshot(repo.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("UTF-8")
        );
    }

    #[test]
    fn rejects_file_growth_after_metadata_check() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "growing", "small");
        let file = std::fs::File::open(dir.path().join("growing")).unwrap();
        assert!(file.metadata().unwrap().len() < MAX_FILE_BYTES as u64);
        put(dir.path(), "growing", vec![0; MAX_FILE_BYTES + 1]);
        assert!(
            read_content(file, "growing")
                .unwrap_err()
                .to_string()
                .contains("grew beyond")
        );
        // An unbounded reader must also terminate at the limit plus one byte.
        assert!(read_content(std::io::repeat(0), "infinite").is_err());
    }

    #[tokio::test]
    async fn rejects_empty_or_fully_excluded_snapshots() {
        let repo = repo().await;
        assert!(
            snapshot(repo.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("no eligible files")
        );
        put(repo.path(), ".env", "SECRET=private");
        add(repo.path()).await;
        assert!(
            snapshot(repo.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("no eligible files")
        );
    }

    #[test]
    fn enforces_backend_path_limits() {
        assert!(relative_path("a".repeat(MAX_PATH_BYTES).as_bytes()).is_ok());
        assert!(relative_path("a".repeat(MAX_PATH_BYTES + 1).as_bytes()).is_err());
        assert!(relative_path("λ".repeat(MAX_PATH_BYTES / 2).as_bytes()).is_ok());
        assert!(relative_path("λ".repeat(MAX_PATH_BYTES / 2 + 1).as_bytes()).is_err());
        let at_limit = vec!["a"; MAX_PATH_COMPONENTS].join("/");
        assert!(relative_path(at_limit.as_bytes()).is_ok());
        assert!(relative_path(format!("{at_limit}/a").as_bytes()).is_err());
    }

    #[test]
    fn rejects_invalid_paths_and_deduplicates_listing() {
        for path in [
            b"../secret".as_slice(),
            b"/absolute",
            b"a/../b",
            b"a/./b",
            b"a//b",
            b"a\\b",
            b"a:b",
            b"a\nb",
            b"a\rb",
            b"a\tb",
            "a\u{0085}b".as_bytes(),
            b"\xff",
        ] {
            assert!(relative_path(path).is_err(), "{path:?}");
        }
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "file", "contents");
        assert_eq!(collect(dir.path(), b"file\0file\0").unwrap().files.len(), 1);
        assert!(collect(dir.path(), b"file").is_err());
    }
}
