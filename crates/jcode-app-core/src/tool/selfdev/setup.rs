use super::*;

use std::process::Command;

/// A single dependency check performed by `selfdev setup`.
struct SetupCheck {
    name: &'static str,
    ok: bool,
    detail: String,
    /// Hint shown when the check fails.
    fix: Option<String>,
}

impl SetupCheck {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: true,
            detail: detail.into(),
            fix: None,
        }
    }

    fn missing(name: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            name,
            ok: false,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }

    fn marker(&self) -> &'static str {
        if self.ok { "✅" } else { "❌" }
    }
}

/// Run a command and capture trimmed stdout if it succeeds.
fn command_version(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        None
    } else {
        Some(first.to_string())
    }
}

impl SelfDevTool {
    /// `selfdev setup`: verify (and where safe, bootstrap) the prerequisites for
    /// building jcode from source: the rust toolchain, git, and a local repo
    /// checkout. This never installs anything irreversible automatically; it
    /// reports what is missing and how to fix it, and clones the source when no
    /// checkout exists yet.
    pub(super) async fn do_setup(&self, ctx: &ToolContext) -> Result<ToolOutput> {
        let mut checks: Vec<SetupCheck> = Vec::new();

        // Rust toolchain: cargo + rustc are required to build jcode.
        match command_version("cargo", &["--version"]) {
            Some(version) => checks.push(SetupCheck::ok("cargo", version)),
            None => checks.push(SetupCheck::missing(
                "cargo",
                "not found on PATH",
                "Install the Rust toolchain via rustup: \
                 `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh` \
                 (or your distro's rustup package), then restart your shell.",
            )),
        }
        match command_version("rustc", &["--version"]) {
            Some(version) => checks.push(SetupCheck::ok("rustc", version)),
            None => checks.push(SetupCheck::missing(
                "rustc",
                "not found on PATH",
                "Install the Rust toolchain via rustup (see cargo above).",
            )),
        }

        // git is required to clone/update the repository.
        match command_version("git", &["--version"]) {
            Some(version) => checks.push(SetupCheck::ok("git", version)),
            None => checks.push(SetupCheck::missing(
                "git",
                "not found on PATH",
                "Install git from your system package manager (e.g. `pacman -S git`, \
                 `apt install git`, `brew install git`).",
            )),
        }

        // Repository checkout: locate an existing repo or clone the source.
        let mut repo_dir: Option<std::path::PathBuf> =
            SelfDevTool::resolve_repo_dir(ctx.working_dir.as_deref());
        let mut clone_note: Option<String> = None;

        if repo_dir.is_none() {
            // Only attempt a clone when git is available and we're not in a
            // synthetic test session.
            let git_available = checks.iter().any(|check| check.name == "git" && check.ok);
            if SelfDevTool::is_test_session() {
                clone_note = Some("Test mode: skipped cloning the jcode source.".to_string());
            } else if git_available {
                match Self::clone_selfdev_source() {
                    Ok(path) => {
                        clone_note = Some(format!("Cloned jcode source into {}.", path.display()));
                        repo_dir = Some(path);
                    }
                    Err(err) => {
                        clone_note =
                            Some(format!("Could not clone jcode source automatically: {err}",));
                    }
                }
            }
        }

        match &repo_dir {
            Some(path) => checks.push(SetupCheck::ok("repository", path.display().to_string())),
            None => {
                let target = Self::selfdev_clone_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "~/.jcode/source/jcode".to_string());
                checks.push(SetupCheck::missing(
                    "repository",
                    "no local jcode checkout found",
                    format!(
                        "Clone the source manually: `git clone {} {}`.",
                        super::JCODE_REPO_URL,
                        target
                    ),
                ));
            }
        }

        // A reloadable/built binary, so the agent knows whether it still needs a
        // build before `selfdev reload`/`enter` can hand off into a dev binary.
        if let Some(repo) = repo_dir.as_deref() {
            match build::find_dev_binary(repo) {
                Some(binary) => {
                    checks.push(SetupCheck::ok("dev binary", binary.display().to_string()))
                }
                None => checks.push(SetupCheck::missing(
                    "dev binary",
                    "no built binary in target/selfdev or target/release",
                    "Build it once with `jcode self-dev --build`, or inside a \
                     self-dev session run `selfdev build`.",
                )),
            }
        }

        let all_ok = checks.iter().all(|check| check.ok);

        let mut output = String::from("## Self-dev setup\n\n");
        for check in &checks {
            output.push_str(&format!(
                "{} **{}** — {}\n",
                check.marker(),
                check.name,
                check.detail
            ));
        }
        if let Some(note) = &clone_note {
            output.push_str(&format!("\n{}\n", note));
        }

        let failing: Vec<&SetupCheck> = checks.iter().filter(|c| !c.ok).collect();
        if !failing.is_empty() {
            output.push_str("\n### Next steps\n\n");
            for check in &failing {
                if let Some(fix) = &check.fix {
                    output.push_str(&format!("- **{}**: {}\n", check.name, fix));
                }
            }
        } else {
            output.push_str(
                "\nAll prerequisites satisfied. Use `selfdev enter` to start working on jcode.\n",
            );
        }

        let metadata = json!({
            "ready": all_ok,
            "repo_dir": repo_dir.as_ref().map(|p| p.display().to_string()),
            "checks": checks
                .iter()
                .map(|c| json!({
                    "name": c.name,
                    "ok": c.ok,
                    "detail": c.detail,
                }))
                .collect::<Vec<_>>(),
        });

        Ok(ToolOutput::new(output).with_metadata(metadata))
    }

    /// `selfdev find-config`: report the key jcode paths (config file, home,
    /// logs, build channels, sockets, and repo checkout) so the agent can locate
    /// configuration without guessing platform-specific locations.
    pub(super) async fn do_find_config(&self, ctx: &ToolContext) -> Result<ToolOutput> {
        let jcode_home = storage::jcode_dir().ok();
        let config_path = jcode_home.as_ref().map(|home| home.join("config.toml"));
        let logs_dir = storage::logs_dir().ok();
        let repo_dir = SelfDevTool::resolve_repo_dir(ctx.working_dir.as_deref());

        let format_path = |path: Option<&std::path::Path>| match path {
            Some(p) => {
                let exists = p.exists();
                format!(
                    "{} {}",
                    p.display(),
                    if exists { "(exists)" } else { "(missing)" }
                )
            }
            None => "unavailable".to_string(),
        };

        let mut output = String::from("## jcode config & paths\n\n");
        output.push_str(&format!(
            "**Config file:** {}\n",
            format_path(config_path.as_deref())
        ));
        output.push_str(&format!(
            "**jcode home:** {}\n",
            format_path(jcode_home.as_deref())
        ));
        output.push_str(&format!(
            "**Logs dir:** {}\n",
            format_path(logs_dir.as_deref())
        ));
        output.push_str(&format!(
            "**Repository:** {}\n",
            repo_dir
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "not found (run `selfdev setup`)".to_string())
        ));

        output.push_str("\n### Build channels\n\n");
        output.push_str(&format!(
            "**current:** {}\n",
            format_path(build::current_binary_path().ok().as_deref())
        ));
        output.push_str(&format!(
            "**stable:** {}\n",
            format_path(build::stable_binary_path().ok().as_deref())
        ));
        output.push_str(&format!(
            "**shared-server:** {}\n",
            format_path(build::shared_server_binary_path().ok().as_deref())
        ));
        output.push_str(&format!(
            "**launcher:** {}\n",
            format_path(build::launcher_binary_path().ok().as_deref())
        ));

        output.push_str("\n### Sockets\n\n");
        output.push_str(&format!(
            "**main socket:** {}\n",
            server::socket_path().display()
        ));
        output.push_str(&format!(
            "**debug socket:** {}\n",
            server::debug_socket_path().display()
        ));

        let metadata = json!({
            "config_path": config_path.as_ref().map(|p| p.display().to_string()),
            "jcode_home": jcode_home.as_ref().map(|p| p.display().to_string()),
            "logs_dir": logs_dir.as_ref().map(|p| p.display().to_string()),
            "repo_dir": repo_dir.as_ref().map(|p| p.display().to_string()),
            "config_exists": config_path.as_ref().map(|p| p.exists()).unwrap_or(false),
        });

        Ok(ToolOutput::new(output).with_metadata(metadata))
    }

    /// `selfdev reload` from a non-self-dev session: a plain upgrade-in-place.
    /// Unlike the self-dev reload, this does not publish a freshly-built binary;
    /// it asks the shared server to exec into the newest installed build (if one
    /// is strictly newer than the running process).
    pub(super) async fn do_reload_to_newer_build(&self, _ctx: &ToolContext) -> Result<ToolOutput> {
        if SelfDevTool::is_test_session() {
            return Ok(ToolOutput::new("Test mode: skipped reload-to-newer-build."));
        }

        if !server::server_has_newer_binary() {
            return Ok(ToolOutput::new(
                "Already running the newest installed jcode build; no reload needed.",
            ));
        }

        let hash = jcode_build_meta::GIT_HASH.to_string();
        let request_id = server::send_reload_signal(hash.clone(), None, false);
        let timeout = std::time::Duration::from_secs(SelfDevTool::reload_timeout_secs());

        match server::wait_for_reload_ack(&request_id, timeout).await {
            Ok(ack) => Ok(ToolOutput::new(format!(
                "Reload initiated into build {}. The server is restarting into the newer binary.",
                ack.hash
            ))
            .with_metadata(json!({
                "request_id": request_id,
                "hash": ack.hash,
            }))),
            Err(err) => Ok(ToolOutput::new(format!(
                "Sent a reload request, but the server did not acknowledge within {}s: {}. \
                 It may still reload; check `selfdev status`.",
                timeout.as_secs(),
                err
            ))),
        }
    }

    /// Resolve the default location for a cloned self-dev source checkout.
    fn selfdev_clone_dir() -> Result<std::path::PathBuf> {
        Ok(storage::jcode_dir()?.join("source").join("jcode"))
    }

    /// Clone the jcode source into the default self-dev source directory.
    fn clone_selfdev_source() -> Result<std::path::PathBuf> {
        let repo_dir = Self::selfdev_clone_dir()?;
        if repo_dir.exists() {
            if build::is_jcode_repo(&repo_dir) {
                return Ok(repo_dir);
            }
            anyhow::bail!(
                "{} exists but is not a jcode repository; move it aside and retry",
                repo_dir.display()
            );
        }

        if let Some(parent) = repo_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let status = Command::new("git")
            .arg("clone")
            .arg(super::JCODE_REPO_URL)
            .arg(&repo_dir)
            .status()
            .map_err(|e| anyhow::anyhow!("failed to run git clone: {e}"))?;
        if !status.success() {
            anyhow::bail!("git clone exited with {status}");
        }
        if !build::is_jcode_repo(&repo_dir) {
            anyhow::bail!(
                "cloned source at {} is not a valid jcode repository",
                repo_dir.display()
            );
        }
        Ok(repo_dir)
    }
}
