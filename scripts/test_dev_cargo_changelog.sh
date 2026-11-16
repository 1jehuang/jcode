#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Load only the function under test, without starting a build.
eval "$(awk '/^export_git_build_metadata\(\) \{/{copy=1} copy {print} copy && /^\}/{exit}' "$repo_root/scripts/dev_cargo.sh")"
unset JCODE_BUILD_CHANGELOG_RAW JCODE_BUILD_GIT_HASH JCODE_BUILD_GIT_TAG
export_git_build_metadata
expected="$(git -C "$repo_root" log -700 --format='%h|%ct|%D|%s')"
[[ -n "$JCODE_BUILD_CHANGELOG_RAW" ]]
[[ "$JCODE_BUILD_CHANGELOG_RAW" == "$expected" ]]
base_version="$(awk '/^\[package\]/{package=1; next} /^\[/{package=0} package && /^version *=/{gsub(/"/, "", $3); print $3; exit}' "$repo_root/Cargo.toml")"
expected_tag="$(git -C "$repo_root" describe --tags --long --match "v${base_version}" HEAD)"
[[ "$JCODE_BUILD_GIT_TAG" == "$expected_tag" ]]
distance="$(git -C "$repo_root" rev-list --count "v${base_version}..HEAD")"
[[ "$JCODE_BUILD_GIT_TAG" == "v${base_version}-${distance}-g"* ]]
# History is still populated when a caller already supplied the identity.
unset JCODE_BUILD_CHANGELOG_RAW
unset JCODE_BUILD_GIT_TAG
JCODE_BUILD_GIT_HASH=caller-hash
export_git_build_metadata
[[ "$JCODE_BUILD_CHANGELOG_RAW" == "$expected" ]]
[[ "$JCODE_BUILD_GIT_HASH" == caller-hash ]]
[[ "$JCODE_BUILD_GIT_TAG" == "$expected_tag" ]]
JCODE_BUILD_GIT_TAG=caller-tag
export_git_build_metadata
[[ "$JCODE_BUILD_GIT_TAG" == caller-tag ]]
JCODE_BUILD_GIT_TAG=''
export_git_build_metadata
[[ -z "$JCODE_BUILD_GIT_TAG" ]]
# Explicit caller history, including empty history, must not be overwritten.
JCODE_BUILD_CHANGELOG_RAW=caller-history
export_git_build_metadata
[[ "$JCODE_BUILD_CHANGELOG_RAW" == caller-history ]]
JCODE_BUILD_CHANGELOG_RAW=''
export_git_build_metadata
[[ -z "$JCODE_BUILD_CHANGELOG_RAW" ]]
echo 'dev build changelog metadata tests passed'
