//! Tests for the short git hash export and the reload repo-dir resolver.

use super::*;

/// The exported hash must be the one the publish gate compares, in the same
/// SHORT form.
///
/// Bound to the helper rather than to the spawn site: the spawn site is a
/// `tokio::process::Command` builder, and a test asserting on it could only
/// check that some `.env(...)` call exists, not that the value is the hash
/// the gate compares against. That gap is stated rather than papered over.
#[test]
fn short_git_hash_matches_the_hash_the_publish_gate_compares() {
    let repo_dir = SelfDevTool::resolve_repo_dir(None).expect("repo dir");
    let hash = short_git_hash(&repo_dir).expect("short hash in the jcode repo");

    assert!(
        hash.chars().all(|c| c.is_ascii_hexdigit()),
        "expected hex, got {hash:?}"
    );

    // Must be the SHORT form specifically. A range check such as
    // `len >= 7 && len <= 40` accepts a full 40-character hash, and that is
    // the exact bug this function exists to prevent. An earlier version of
    // this test used that range and passed against a helper that had no
    // length logic at all.
    let full = std::process::Command::new("git")
        .current_dir(&repo_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse HEAD");
    assert!(full.status.success());
    let full = String::from_utf8_lossy(&full.stdout).trim().to_string();
    assert_eq!(
        hash,
        &full[..full.len().min(9)],
        "expected the short hash, not a prefix guess"
    );
    assert_ne!(
        hash, full,
        "a full 40-char hash can never equal source.short_hash, so the \
             publish gate would refuse every build"
    );

    // And it must be the commit that is actually checked out.
    let expected = std::process::Command::new("git")
        .current_dir(&repo_dir)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .expect("git rev-parse");
    assert!(expected.status.success());
    assert_eq!(
        hash,
        String::from_utf8_lossy(&expected.stdout).trim(),
        "the exported hash must be the checked-out commit"
    );
}

#[test]
fn short_git_hash_is_none_outside_a_repository() {
    // The temp dir must be OUTSIDE the checkout: git walks upward, so a temp
    // dir created inside the repo resolves to this repository and the test
    // silently proves nothing. Probed: a temp dir under the repo returned
    // this repo's own hash.
    let dir = tempfile::tempdir().expect("temp dir");
    let repo = SelfDevTool::resolve_repo_dir(None).expect("repo dir");
    assert!(
        !dir.path().starts_with(&repo),
        "the temp dir must not sit inside the repo, or git walks up into it"
    );
    assert_eq!(short_git_hash(dir.path()), None);
}

/// Whatever comes back is never blank.
///
/// The blank rule itself lives in `jcode_build_meta::present` and is tested
/// there. It cannot be reached from here: outside a repository `git
/// rev-parse` exits 128 with empty stdout, so the `!status.success()` early
/// return fires before any trimming happens. A mutation that deleted the
/// blank check from this helper went undetected for exactly that reason.
///
/// So what is assertable here is the delegation, plus the observable
/// guarantee the caller relies on.
#[test]
fn short_git_hash_never_yields_a_blank_value() {
    let repo_dir = SelfDevTool::resolve_repo_dir(None).expect("repo dir");
    if let Some(hash) = short_git_hash(&repo_dir) {
        assert_eq!(
            jcode_build_meta::present(Some(hash.clone())),
            Some(hash),
            "the helper must return the same trimmed, non-blank value it was given"
        );
    }
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(
        short_git_hash(dir.path()).is_none(),
        "must be None, never Some of a blank string"
    );
}

#[test]
fn reload_repo_resolver_uses_working_dir_when_primary_detection_fails() {
    let repo = create_repo_fixture();
    let nested = repo.path().join("crates").join("jcode-build-support");
    std::fs::create_dir_all(&nested).expect("nested dir");

    let resolved = reload::resolve_selfdev_reload_repo_dir_from(None, Some(&nested));
    assert_eq!(resolved.as_deref(), Some(repo.path()));
}
