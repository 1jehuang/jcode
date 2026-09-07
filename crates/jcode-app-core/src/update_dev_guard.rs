//! Protect development builds from releases that do not contain their compiled commit.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

/// The caller has already established that this is a development build and that
/// the release's semver is newer than the build's base version. Semver alone does
/// not establish that installing it would move the running binary forward.
pub(super) fn should_install_release(release_tag: &str) -> Result<bool> {
    should_install_release_with(
        release_tag,
        jcode_build_meta::git_hash(),
        crate::build::get_repo_dir().as_deref(),
        github_comparison,
    )
}

fn should_install_release_with(
    release_tag: &str,
    current_hash: &str,
    repo: Option<&Path>,
    fallback: impl FnOnce(&str, &str) -> Result<bool>,
) -> Result<bool> {
    validate_inputs(release_tag, current_hash)?;
    if let Some(decision) = repo.and_then(|repo| local_comparison(repo, release_tag, current_hash))
    {
        return Ok(decision);
    }
    fallback(release_tag, current_hash)
}

fn validate_inputs(release_tag: &str, current_hash: &str) -> Result<()> {
    // Build metadata normally contains an abbreviated SHA-1. Also accept full
    // SHA-1/SHA-256 IDs, but never git expressions, options, or "unknown".
    if !(7..=64).contains(&current_hash.len())
        || !current_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        bail!(
            "Cannot safely install release: the development build's compiled git hash is missing or invalid ({current_hash:?})"
        );
    }
    // Release tags are version-like names, not arbitrary revision expressions.
    // This also makes both the git argument and GitHub URL segment unambiguous.
    if release_tag.is_empty()
        || !release_tag.as_bytes()[0].is_ascii_alphanumeric()
        || !release_tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-+".contains(&byte))
        || release_tag.contains("..")
        || release_tag.ends_with('.')
        || release_tag.ends_with(".lock")
    {
        bail!("Cannot safely compare development build to invalid release tag {release_tag:?}");
    }
    Ok(())
}

/// `None` means the repository, refs, or ancestry operation is unavailable.
/// Only a proven ancestor relationship permits installing. Never use HEAD: the
/// checkout can have moved independently of the binary being updated.
fn local_comparison(repo: &Path, release_tag: &str, current_hash: &str) -> Option<bool> {
    validate_inputs(release_tag, current_hash).ok()?;
    let release = resolve_commit(repo, &format!("refs/tags/{release_tag}"))?;
    let current = resolve_commit(repo, current_hash)?;
    // A hexadecimal branch name must not stand in for the compiled object ID.
    if !current.starts_with(&current_hash.to_ascii_lowercase()) {
        return None;
    }
    if is_ancestor(repo, &release, &current)? {
        crate::logging::info(&format!(
            "Keeping development build {current_hash}: release {release_tag} is an ancestor of or identical to the compiled commit"
        ));
        return Some(false);
    }
    if is_ancestor(repo, &current, &release)? {
        return Some(true);
    }
    // Divergence (including ancestry hidden by a shallow clone) is not evidence
    // that a release is safe to install, so retain the development build.
    crate::logging::info(&format!(
        "Keeping development build {current_hash}: release {release_tag} has divergent or incomplete local ancestry"
    ));
    Some(false)
}

fn git(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(repo).env("GIT_NO_REPLACE_OBJECTS", "1");
    command
}

fn resolve_commit(repo: &Path, revision: &str) -> Option<String> {
    let output = git(repo)
        .args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let hash = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    if !matches!(hash.len(), 40 | 64) || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(hash)
}

fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> Option<bool> {
    // Both arguments are resolved full hexadecimal IDs, never user options.
    let output = git(repo)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .ok()?;
    match output.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

fn github_comparison(release_tag: &str, current_hash: &str) -> Result<bool> {
    let url = format!(
        "https://api.github.com/repos/{}/compare/{release_tag}...{current_hash}",
        super::GITHUB_REPO
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(super::UPDATE_CHECK_TIMEOUT)
        .user_agent("jcode-updater")
        .build()
        .context("Cannot safely install release: failed to create GitHub comparison client")?;
    let response = super::github_api_request(&client, &url)
        .send()
        .with_context(|| {
            format!("Cannot safely install release: failed to compare {release_tag} to compiled commit {current_hash} on GitHub")
        })?;
    github_response(response, release_tag, current_hash)
}

fn github_response(
    response: reqwest::blocking::Response,
    release_tag: &str,
    current_hash: &str,
) -> Result<bool> {
    if let Some(error) = super::rate_limit_error(&response) {
        return Err(error);
    }
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        bail!(
            "Cannot safely install release: GitHub could not find release {release_tag} or compiled commit {current_hash} for comparison (404)"
        );
    }
    if !response.status().is_success() {
        bail!(
            "Cannot safely install release: GitHub comparison failed ({})",
            response.status()
        );
    }
    let comparison: serde_json::Value = response
        .json()
        .context("Cannot safely install release: invalid GitHub comparison response")?;
    let decision = comparison_decision(&comparison)?;
    if !decision {
        crate::logging::info(&format!(
            "Keeping development build {current_hash}: GitHub reports compiled commit is {} relative to release {release_tag}",
            comparison["status"].as_str().unwrap_or("unknown")
        ));
    }
    Ok(decision)
}

fn comparison_decision(comparison: &serde_json::Value) -> Result<bool> {
    // GitHub compares base...head. Here the release is base and the compiled
    // commit is head, so "behind" means the release contains the running build.
    match comparison.get("status").and_then(serde_json::Value::as_str) {
        Some("behind") => Ok(true),
        Some("ahead" | "identical" | "diverged") => Ok(false),
        status => bail!(
            "Cannot safely install release: GitHub returned an unknown or missing comparison status ({status:?})"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    struct Repo(tempfile::TempDir);

    impl Repo {
        fn new() -> Self {
            let repo = Self(tempfile::tempdir().unwrap());
            repo.run(&["init", "--quiet"]);
            repo.run(&["config", "user.name", "Dev Guard Test"]);
            repo.run(&["config", "user.email", "dev-guard@example.invalid"]);
            repo
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        fn run(&self, args: &[&str]) -> String {
            let output = git(self.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env(
                    "GIT_CONFIG_GLOBAL",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                )
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }

        fn commit(&self, message: &str) -> String {
            self.run(&[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                message,
            ]);
            self.run(&["rev-parse", "HEAD"])
        }

        fn tag(&self, commit: &str) {
            self.run(&["tag", "v9.0.0", commit]);
        }
    }

    #[test]
    fn local_ahead_retains_dev_build() {
        let repo = Repo::new();
        let release = repo.commit("release");
        repo.tag(&release);
        let current = repo.commit("development");
        assert_eq!(
            local_comparison(repo.path(), "v9.0.0", &current),
            Some(false)
        );
    }

    #[test]
    fn local_behind_installs_release() {
        let repo = Repo::new();
        let current = repo.commit("development");
        let release = repo.commit("release");
        repo.tag(&release);
        assert_eq!(
            local_comparison(repo.path(), "v9.0.0", &current),
            Some(true)
        );
    }

    #[test]
    fn local_identical_retains_dev_build() {
        let repo = Repo::new();
        let current = repo.commit("same commit");
        repo.tag(&current);
        assert_eq!(
            local_comparison(repo.path(), "v9.0.0", &current),
            Some(false)
        );
    }

    #[test]
    fn local_diverged_retains_dev_build() {
        let repo = Repo::new();
        let base = repo.commit("base");
        let current = repo.commit("development branch");
        repo.run(&["checkout", "--quiet", "--detach", &base]);
        let release = repo.commit("release branch");
        repo.tag(&release);
        assert_eq!(
            local_comparison(repo.path(), "v9.0.0", &current),
            Some(false)
        );
    }

    #[test]
    fn compiled_commit_ahead_is_not_replaced_when_checkout_is_behind() {
        let repo = Repo::new();
        let base = repo.commit("base");
        let release = repo.commit("release");
        repo.tag(&release);
        let compiled = repo.commit("compiled development build");
        repo.run(&["checkout", "--quiet", "--detach", &base]);
        assert_eq!(
            local_comparison(repo.path(), "v9.0.0", &compiled[..9]),
            Some(false)
        );
        assert_eq!(local_comparison(repo.path(), "v9.0.0", &base), Some(true));
    }

    #[test]
    fn compiled_commit_behind_can_update_when_checkout_is_ahead() {
        let repo = Repo::new();
        let compiled = repo.commit("compiled development build");
        let release = repo.commit("release");
        repo.tag(&release);
        let head = repo.commit("checkout moved forward");
        assert_eq!(
            local_comparison(repo.path(), "v9.0.0", &compiled[..9]),
            Some(true)
        );
        assert_eq!(local_comparison(repo.path(), "v9.0.0", &head), Some(false));
    }

    #[test]
    fn annotated_release_tag_is_peeled_to_commit() {
        let repo = Repo::new();
        let current = repo.commit("development");
        repo.commit("release");
        repo.run(&[
            "-c",
            "tag.gpgsign=false",
            "tag",
            "-a",
            "v9.0.0",
            "-m",
            "release",
        ]);
        assert_eq!(
            local_comparison(repo.path(), "v9.0.0", &current),
            Some(true)
        );
    }

    #[test]
    fn branch_named_like_release_does_not_substitute_for_tag() {
        let repo = Repo::new();
        let current = repo.commit("development");
        repo.commit("not a release tag");
        repo.run(&["branch", "v9.0.0"]);
        assert_eq!(local_comparison(repo.path(), "v9.0.0", &current), None);
    }

    #[test]
    fn hexadecimal_branch_name_cannot_substitute_for_compiled_hash() {
        let repo = Repo::new();
        let current = repo.commit("development");
        repo.run(&["branch", "abcdef123", &current]);
        let release = repo.commit("release");
        repo.tag(&release);
        assert_eq!(local_comparison(repo.path(), "v9.0.0", "abcdef123"), None);
    }

    #[test]
    fn missing_refs_fall_back_with_exact_compiled_hash() {
        let repo = Repo::new();
        let current = repo.commit("development");
        let result =
            should_install_release_with("v9.0.0", &current, Some(repo.path()), |tag, hash| {
                assert_eq!(tag, "v9.0.0");
                assert_eq!(hash, current);
                Ok(true)
            });
        assert!(result.unwrap());
        repo.tag(&current);
        assert_eq!(
            local_comparison(repo.path(), "v9.0.0", &"0".repeat(40)),
            None
        );
    }

    #[test]
    fn absent_and_invalid_repositories_fall_back() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");
        for repo in [None, Some(directory.path()), Some(missing.as_path())] {
            assert!(
                should_install_release_with("v9.0.0", "abcdef123", repo, |_, _| Ok(true)).unwrap()
            );
        }
    }

    #[test]
    fn definitive_local_result_never_calls_network() {
        let repo = Repo::new();
        let current = repo.commit("development");
        repo.tag(&current);
        assert!(
            !should_install_release_with("v9.0.0", &current, Some(repo.path()), |_, _| panic!(
                "unexpected network fallback"
            ))
            .unwrap()
        );
    }

    #[test]
    fn unknown_hash_and_revision_injection_fail_before_network() {
        for hash in [
            "",
            "unknown",
            "HEAD",
            "--help",
            "abcdef1^{commit}",
            "123456",
            "abcdefg",
            "abcdef1/../../HEAD",
            "abcdef1\n",
        ] {
            assert!(
                should_install_release_with("v9.0.0", hash, None, |_, _| panic!(
                    "invalid hash reached network"
                ))
                .is_err(),
                "{hash:?}"
            );
        }
        assert!(validate_inputs("v9.0.0", &"a".repeat(65)).is_err());
    }

    #[test]
    fn invalid_tags_fail_before_network() {
        for tag in [
            "",
            "--help",
            "-v9",
            "v9..0",
            "v9/0",
            "v9?query",
            "v9#fragment",
            "v9^{commit}",
            "v9.lock",
            "v9.",
            "v9\n",
        ] {
            assert!(
                should_install_release_with(tag, "abcdef123", None, |_, _| panic!(
                    "invalid tag reached network"
                ))
                .is_err(),
                "{tag:?}"
            );
        }
        for tag in ["v9.0.0", "9.0.0", "v9.0.0-rc.1", "v9.0.0+build.1"] {
            assert!(validate_inputs(tag, "ABCDEF123").is_ok());
        }
    }

    #[test]
    fn network_errors_are_not_install_permissions() {
        let error = should_install_release_with("v9.0.0", "abcdef123", None, |_, _| {
            bail!("connection timed out")
        })
        .unwrap_err();
        assert!(error.to_string().contains("connection timed out"));
    }

    #[test]
    fn github_status_direction_matches_release_base_and_compiled_head() {
        for (status, expected) in [
            ("behind", true),
            ("ahead", false),
            ("identical", false),
            ("diverged", false),
        ] {
            assert_eq!(
                comparison_decision(&json!({ "status": status })).unwrap(),
                expected,
                "{status}"
            );
        }
    }

    #[test]
    fn missing_or_unknown_github_status_fails_closed() {
        for response in [
            json!({}),
            json!({"status": null}),
            json!({"status": 1}),
            json!({"status": "new-status"}),
            json!({"status": "Behind"}),
        ] {
            assert!(comparison_decision(&response).is_err());
        }
    }

    fn mock_response(status: &str, body: &str) -> reqwest::blocking::Response {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let reply = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            let _ = stream.read(&mut buffer).unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
        });
        // Never send the user's GitHub credentials to the test server.
        let response = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .unwrap();
        server.join().unwrap();
        response
    }

    #[test]
    fn github_404_explains_unavailable_compiled_commit() {
        let error = github_response(mock_response("404 Not Found", "{}"), "v9.0.0", "abcdef123")
            .unwrap_err()
            .to_string();
        assert!(error.contains("404") && error.contains("abcdef123") && error.contains("v9.0.0"));
    }

    #[test]
    fn github_http_failure_and_malformed_body_fail_closed() {
        for (status, body) in [
            ("500 Internal Server Error", "{}"),
            ("200 OK", "not json"),
            ("200 OK", "{}"),
            ("200 OK", "{\"status\":\"unexpected\"}"),
        ] {
            assert!(github_response(mock_response(status, body), "v9.0.0", "abcdef123").is_err());
        }
    }

    #[test]
    fn github_success_response_permits_only_behind() {
        for (status, expected) in [
            ("behind", true),
            ("ahead", false),
            ("identical", false),
            ("diverged", false),
        ] {
            let body = json!({"status": status}).to_string();
            assert_eq!(
                github_response(mock_response("200 OK", &body), "v9.0.0", "abcdef123").unwrap(),
                expected
            );
        }
    }
}
