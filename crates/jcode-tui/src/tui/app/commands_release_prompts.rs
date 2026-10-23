use super::*;

pub(super) fn build_release_prompt(
    before_bump_instruction: &str,
    release_instruction: &str,
) -> String {
    let mut prompt = build_commit_push_prompt();
    prompt.push(' ');
    prompt.push_str("Then cut a release. Find the last release tag (git describe --tags --abbrev=0 or gh release list) and review everything that changed since it to pick the semver bump: patch for fixes and small internal changes, minor for new features, major only for breaking changes. ");
    if !before_bump_instruction.is_empty() {
        prompt.push_str(before_bump_instruction);
        prompt.push(' ');
    }
    prompt.push_str("Bump the version in the root Cargo.toml, refresh Cargo.lock (for example with cargo check), and, if the repo has a changelog/ directory, write a user-facing changelog entry changelog/v<version>.json following changelog/README.md (translate commits into user-visible effects, skip internal-only changes, update changelog/index.json). Commit the version bump together with the changelog entry as one release-metadata commit, and push. ");
    prompt.push_str("Record the exact SHA of the pushed release-metadata commit on main/master. Before invoking a tagging or staging release helper (including scripts/quick-release.sh), wait for repository CI on that exact SHA to complete successfully and verify every mandatory check and job is green. Recheck that HEAD still matches that SHA and the worktree is clean. Stop on missing, pending, skipped, cancelled, or failed checks. Never bypass CI prerequisites with flags, environment variables, manual tag pushes, or direct release publication. ");
    prompt.push_str(release_instruction);
    prompt.push_str(" Keep fast artifacts stage-only on a draft. Wait for the release workflow to finish full publication, then verify every required platform asset, signatures, checksums, published release downloads, and installer smoke checks before reporting success. Distinguish staging or a triggered workflow from completed publication. Do not force-push or move existing tags. Finally, report the new version, the commits created, the exact-SHA CI result, the tag push, and the verified release status.");
    prompt
}

pub(in crate::tui::app) fn build_fast_release_prompt() -> String {
    build_release_prompt(
        "Only after pushing the current source commit and verifying successful exact-SHA CI, and before editing Cargo.toml or the changelog for the version bump, run scripts/quick-release.sh --prepare-fast v<version>. It must refresh the warm target/selfdev cache for the Linux x86_64 binary while the existing Cargo version is unchanged and record the prepared commit.",
        "Then run scripts/quick-release.sh --fast-local v<version> only after the pushed release-metadata exact-SHA CI gate is green. It must wrap the prepared selfdev binary with the release identity and stage that Linux asset on a draft release only, never publish immediately. Let the release workflow replace it with the portable Linux artifact while adding macOS, Windows, FreeBSD, signatures, and final checksums before publication. Do not run the separate local macOS cross-build. If preparation is stale or the release-metadata commit contains code changes, stop instead of staging a binary that differs from the tag.",
    )
}

pub(in crate::tui::app) fn build_fast_macos_release_prompt() -> String {
    build_release_prompt(
        "Only after pushing the current source commit and verifying successful exact-SHA CI, and before editing Cargo.toml or the changelog for the version bump, run scripts/quick-release.sh --prepare-fast-macos v<version>. It must cross-build and record the macOS arm64 binary with the future release identity while the release metadata is still unchanged.",
        "Then run scripts/quick-release.sh --fast-macos-local v<version> only after the pushed release-metadata exact-SHA CI gate is green. It must validate and stage the prepared macOS arm64 asset on a draft release only, never publish immediately. Let the release workflow replace it with the signoff artifact and add macOS Intel, Linux, Windows, FreeBSD, signatures, and final checksums before publication. If preparation is stale or the release-metadata commit contains code changes, stop instead of staging a binary that differs from the tag.",
    )
}

pub(in crate::tui::app) fn build_remote_release_prompt() -> String {
    let jcode_release = build_release_prompt(
        "",
        "Then run scripts/quick-release.sh --remote v<version> only after the pushed release-metadata exact-SHA CI gate is green, without any local build. Let the release workflow build, sign, checksum, and publish every platform, and leave publication gated on those remote checks.",
    );
    format!(
        "First identify the repository in the current working directory from its git remote, release documentation, package manifests, existing tags, and CI workflows. Only use the following Jcode-specific procedure when this is the Jcode self-development repository and scripts/quick-release.sh exists: {jcode_release} Otherwise, use the repository's own established release conventions. Inspect its release documentation, workflows, scripts, manifests, tag format, and recent releases before changing anything. Make logical commits for current work without disturbing unrelated changes and push them normally. Determine the next version from this repository's versioning scheme and user-visible changes, update only the version files and changelog formats it actually uses, validate the metadata, commit and push it, verify successful exact-SHA CI with every mandatory job green for that pushed release-metadata commit, then trigger the repository's documented remote release mechanism. Wait for full publication and verify published downloads and installer smoke checks before claiming success. Prefer a tag-triggered or workflow-dispatch CI release that performs builds and publication remotely. Do not assume the project uses Cargo, changelog JSON, v-prefixed tags, or scripts/quick-release.sh. Do not build release artifacts locally unless this repository explicitly requires it and no remote release path exists. Never force-push, move an existing tag, bypass remote release gates, or invent a release process. Report the detected release convention, version, commits, tag or workflow invocation, and remote release status."
    )
}
