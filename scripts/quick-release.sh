#!/usr/bin/env bash
set -euo pipefail

# Release helper. Every non-dry-run release requires successful exact-SHA CI:
# - --prepare-fast: refresh target/selfdev before the release metadata commit.
# - --fast-local: package that prepared binary and stage Linux on a draft, then
#   let CI replace it with portable/signoff assets and add every other platform.
# - --prepare-fast-macos/--fast-macos-local: do the same for macOS arm64 using
#   the local osxcross cache.
# - --remote: check CI, push the tag, and let the release workflow publish.
# - default: build Linux + macOS locally and stage them on the CI-owned draft.
#
# Usage:
#   scripts/quick-release.sh --prepare-fast v0.5.5 # warm selfdev before bump
#   scripts/quick-release.sh --fast-local v0.5.5  # package it on a draft
#   scripts/quick-release.sh --prepare-fast-macos v0.5.5
#   scripts/quick-release.sh --fast-macos-local v0.5.5
#   scripts/quick-release.sh --remote v0.5.5      # green CI, then tag
#   scripts/quick-release.sh v0.5.5               # local Linux + macOS draft
#   scripts/quick-release.sh --dry-run v0.5.5     # standard local build only

MODE="standard"
DRY_RUN=false
while [[ "${1:-}" == --* ]]; do
    case "$1" in
        --dry-run) DRY_RUN=true ;;
        --fast|--fast-local)
            [[ "$MODE" == "standard" ]] || { echo "Error: release modes cannot be combined." >&2; exit 1; }
            MODE="fast-local"
            ;;
        --prepare-fast)
            [[ "$MODE" == "standard" ]] || { echo "Error: release modes cannot be combined." >&2; exit 1; }
            MODE="prepare-fast"
            ;;
        --fast-macos|--fast-macos-local)
            [[ "$MODE" == "standard" ]] || { echo "Error: release modes cannot be combined." >&2; exit 1; }
            MODE="fast-macos-local"
            ;;
        --prepare-fast-macos)
            [[ "$MODE" == "standard" ]] || { echo "Error: release modes cannot be combined." >&2; exit 1; }
            MODE="prepare-fast-macos"
            ;;
        --remote|--ci-only)
            [[ "$MODE" == "standard" ]] || { echo "Error: release modes cannot be combined." >&2; exit 1; }
            MODE="remote"
            ;;
        --)
            shift
            break
            ;;
        *)
            echo "Error: Unknown option: $1" >&2
            echo "Usage: scripts/quick-release.sh [--prepare-fast | --fast-local | --prepare-fast-macos | --fast-macos-local | --remote | --dry-run] <version> [title]" >&2
            exit 1
            ;;
    esac
    shift
done

if $DRY_RUN && [[ "$MODE" == "remote" ]]; then
    echo "Error: --dry-run cannot be combined with --remote." >&2
    exit 1
fi

VERSION="${1:?Usage: scripts/quick-release.sh [--prepare-fast | --fast-local | --prepare-fast-macos | --fast-macos-local | --remote | --dry-run] <version> [title]}"
TITLE="${2:-$VERSION}"
VERSION_NUM="${VERSION#v}"

if [[ ! "$VERSION" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: Version must be in format v0.5.4"
    exit 1
fi

cd "$(git rev-parse --show-toplevel)"
RELEASE_COMMIT="$(git rev-parse HEAD)"

# Ad-hoc sign a macOS binary with a stable code-signing identifier. The linker
# signature otherwise uses cargo's hashed output name, which macOS Tahoe treats
# as a new program on every release (duplicate "Allow in the Menu Bar" entries,
# #621). Must match the identifier in .github/workflows/release.yml.
MACOS_SIGNING_IDENTIFIER="com.jcode.cli"
sign_macos_binary() {
    local bin="$1"
    if command -v codesign &>/dev/null; then
        codesign --force --sign - --identifier "$MACOS_SIGNING_IDENTIFIER" "$bin"
    else
        rcodesign sign --binary-identifier "$MACOS_SIGNING_IDENTIFIER" "$bin" >/dev/null
    fi
}

required_commands=(git)
case "$MODE" in
    remote) ;;
    prepare-fast)
        required_commands+=(cargo sha256sum)
        ;;
    fast-local)
        required_commands+=(file strip tar gzip sha256sum)
        $DRY_RUN || required_commands+=(gh)
        ;;
    prepare-fast-macos)
        required_commands+=(cargo file sha256sum rcodesign)
        ;;
    fast-macos-local)
        required_commands+=(file tar gzip sha256sum rcodesign)
        $DRY_RUN || required_commands+=(gh)
        ;;
    standard)
        required_commands+=(cargo docker file rcodesign)
        $DRY_RUN || required_commands+=(gh)
        ;;
esac
if ! $DRY_RUN && [[ "$MODE" != "prepare-fast" && "$MODE" != "prepare-fast-macos" ]]; then
    required_commands+=(gh python3)
fi
for cmd in "${required_commands[@]}"; do
    command -v "$cmd" &>/dev/null || { echo "Error: $cmd not found."; exit 1; }
done

if [[ "$MODE" == "standard" || "$MODE" == "prepare-fast-macos" ]]; then
    [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
    export PATH="$HOME/.osxcross/bin:$PATH"
    if ! command -v aarch64-apple-darwin23.5-clang &>/dev/null; then
        echo "Error: osxcross not found. Install at ~/.osxcross"
        exit 1
    fi
fi

working_tree_changes="$(git status --porcelain)"
if [[ -n "$working_tree_changes" ]]; then
    case "$MODE" in
        standard|fast-local|prepare-fast|fast-macos-local|prepare-fast-macos)
            echo "Error: --$MODE requires a clean working tree so the binary matches committed source." >&2
            printf '%s\n' "$working_tree_changes" >&2
            exit 1
            ;;
        remote)
            echo "Warning: working-tree changes are not included; only committed HEAD will be tagged."
            ;;
    esac
fi

if [[ "$MODE" == "fast-local" || "$MODE" == "prepare-fast" ]]; then
    if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
        echo "Error: --fast-local currently publishes the Linux x86_64 asset and must run on Linux x86_64." >&2
        exit 1
    fi
fi

if [[ "$MODE" == "fast-macos-local" || "$MODE" == "prepare-fast-macos" ]]; then
    if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
        echo "Error: fast macOS release currently uses osxcross and must run on Linux x86_64." >&2
        exit 1
    fi
fi

echo "=== Quick Release: $VERSION ($MODE) ==="
echo ""

DIST="$(mktemp -d)"
trap 'rm -rf "$DIST"' EXIT
OVERALL_START=$(date +%s)
TAG_PUSHED=false

elapsed() {
    echo $(( $(date +%s) - OVERALL_START ))
}

tag_and_push() {
    local head_commit remote_tags remote_commit local_commit repository origin_url package_version
    head_commit="$(git rev-parse HEAD)"
    [[ "$head_commit" == "$RELEASE_COMMIT" ]] || {
        echo "Error: HEAD changed during release preparation. Start again at the intended release commit." >&2
        exit 1
    }
    if [[ "$MODE" != "remote" && -n "$(git status --porcelain)" ]]; then
        echo "Error: working tree changed during release preparation." >&2
        exit 1
    fi
    # Read committed metadata, not a possibly dirty remote-mode working copy.
    package_version="$(git show "$head_commit:Cargo.toml" | python3 -c 'import sys, tomllib; print(tomllib.load(sys.stdin.buffer)["package"]["version"])')"
    [[ "$package_version" == "$VERSION_NUM" ]] || {
        echo "Error: committed package version $package_version does not match requested tag $VERSION." >&2
        exit 1
    }

    remote_tags="$(git ls-remote --tags origin "refs/tags/$VERSION" "refs/tags/$VERSION^{}")"
    if [[ -n "$remote_tags" ]]; then
        remote_commit="$(printf '%s\n' "$remote_tags" | awk '$2 ~ /\^\{\}$/ { print $1; found=1 } END { if (!found) print first } NR == 1 { first=$1 }')"
        if [[ "$remote_commit" != "$head_commit" ]]; then
            echo "Error: Remote tag $VERSION already points to a different commit." >&2
            exit 1
        fi
    fi

    if git show-ref --verify --quiet "refs/tags/$VERSION"; then
        local_commit="$(git rev-parse "refs/tags/$VERSION^{commit}")"
        if [[ "$local_commit" != "$head_commit" ]]; then
            echo "Error: Local tag $VERSION already points to a different commit." >&2
            exit 1
        fi
    fi

    # The workflow's publication gate is too late to prevent premature tags.
    # Check the latest exact-commit main/master push CI run and every mandatory
    # job before either creating a local tag or touching the remote. Fail fast:
    # push the release commit first, wait for CI, then rerun this command.
    echo "▸ Requiring successful exact-SHA CI for $head_commit..."
    origin_url="$(git remote get-url origin)"
    repository="$(gh repo view "$origin_url" --json nameWithOwner --jq .nameWithOwner)"
    if ! python3 scripts/check_release_ci.py "$head_commit" --repo "$repository" --timeout 0; then
        echo "Error: release blocked. Push this commit to main/master and wait for all CI checks to succeed, then retry." >&2
        exit 1
    fi
    [[ "$(git rev-parse HEAD)" == "$head_commit" ]] || {
        echo "Error: HEAD changed during CI preflight. No tag was pushed." >&2
        exit 1
    }
    if [[ "$MODE" != "remote" && -n "$(git status --porcelain)" ]]; then
        echo "Error: working tree changed during CI preflight. No tag was pushed." >&2
        exit 1
    fi

    if [[ -n "$remote_tags" ]]; then
        echo "  Remote tag already exists at the CI-verified commit"
        return
    fi
    echo "▸ Tagging $VERSION..."
    if ! git show-ref --verify --quiet "refs/tags/$VERSION"; then
        git tag "$VERSION" "$head_commit" -m "$TITLE"
    fi
    # Recheck existing local tags in case another process changed one while CI
    # was queried. Never push an unverified commit or a same-named branch.
    [[ "$(git rev-parse "refs/tags/$VERSION^{commit}")" == "$head_commit" ]] || {
        echo "Error: Local tag changed during CI preflight. No tag was pushed." >&2
        exit 1
    }
    git push origin "refs/tags/$VERSION:refs/tags/$VERSION"
    TAG_PUSHED=true
    echo "  Tag pushed"
}

generate_notes() {
    NOTES_FILE="$DIST/release_notes.md"
    if ! scripts/generate_release_notes.sh "$VERSION" > "$NOTES_FILE" || [[ ! -s "$NOTES_FILE" ]]; then
        echo "  Warning: release notes generation failed, using the release title"
        printf '%s\n' "$TITLE" > "$NOTES_FILE"
    fi
}

ensure_release_draft() {
    generate_notes
    if ! gh release view "$VERSION" >/dev/null 2>&1; then
        if ! gh release create "$VERSION" \
            --draft \
            --title "$TITLE" \
            --notes-file "$NOTES_FILE"; then
            sleep 2
            gh release view "$VERSION" >/dev/null
        fi
    fi
    [[ "$(gh release view "$VERSION" --json isDraft --jq .isDraft)" == "true" ]] || {
        echo "Error: $VERSION is already public. Refusing to replace published assets with local staging files." >&2
        exit 1
    }
    gh release edit "$VERSION" --title "$TITLE" --notes-file "$NOTES_FILE"
}

if [[ "$MODE" == "prepare-fast" ]]; then
    echo "▸ Refreshing the warm selfdev Linux build before the version bump..."
    JCODE_REMOTE_CARGO=0 scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode
    source_bin="target/selfdev/jcode"
    [[ -x "$source_bin" ]] || { echo "Error: selfdev binary not found: $source_bin" >&2; exit 1; }
    prepared_marker="target/selfdev/fast-release-prepared"
    {
        printf 'version=%s\n' "$VERSION_NUM"
        printf 'commit=%s\n' "$(git rev-parse HEAD)"
        printf 'binary_sha256=%s\n' "$(sha256sum "$source_bin" | cut -d' ' -f1)"
    } > "$prepared_marker"
    echo ""
    echo "=== Fast release prepared in $(elapsed)s ==="
    echo "  ✅ Warm selfdev binary recorded for $VERSION at $(git rev-parse --short HEAD)"
    echo "  Next: commit only Cargo.toml, Cargo.lock, and changelog release metadata, then run --fast-local."
    exit 0
fi

if [[ "$MODE" == "prepare-fast-macos" ]]; then
    echo "▸ Refreshing the macOS arm64 build before the version bump..."
    JCODE_RELEASE_BUILD=1 JCODE_BUILD_SEMVER="$VERSION_NUM" \
        CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}" \
        cargo build --release --target aarch64-apple-darwin --bin jcode
    source_bin="target/aarch64-apple-darwin/release/jcode"
    [[ -x "$source_bin" ]] || { echo "Error: macOS binary not found: $source_bin" >&2; exit 1; }
    file "$source_bin" | grep -q 'Mach-O 64-bit' || { echo "Error: bad macOS binary" >&2; exit 1; }
    prepared_marker="target/aarch64-apple-darwin/release/fast-macos-release-prepared"
    {
        printf 'version=%s\n' "$VERSION_NUM"
        printf 'commit=%s\n' "$(git rev-parse HEAD)"
        printf 'binary_sha256=%s\n' "$(sha256sum "$source_bin" | cut -d' ' -f1)"
    } > "$prepared_marker"
    echo ""
    echo "=== Fast macOS release prepared in $(elapsed)s ==="
    echo "  ✅ macOS arm64 binary recorded for $VERSION at $(git rev-parse --short HEAD)"
    echo "  Next: commit only Cargo.toml, Cargo.lock, and changelog release metadata, then run --fast-macos-local."
    exit 0
fi

if [[ "$MODE" == "remote" ]]; then
    tag_and_push
    echo ""
    echo "=== Remote release triggered in $(elapsed)s ==="
    if $TAG_PUSHED; then
        echo "  ✅ Tag pushed; release CI started"
    else
        echo "  ✅ Tag already existed at HEAD; release CI was already triggered"
    fi
    echo "  ⏳ CI will build, sign, checksum, and publish every platform"
    echo ""
    echo "No local build was run. Publication remains gated on the release workflow."
    exit 0
fi

if [[ "$MODE" == "fast-local" ]]; then
    echo "▸ Validating the prepared selfdev Linux build..."
    build_start=$(date +%s)
    source_bin="target/selfdev/jcode"
    [[ -x "$source_bin" ]] || { echo "Error: selfdev binary not found: $source_bin" >&2; exit 1; }
    prepared_marker="target/selfdev/fast-release-prepared"
    [[ -f "$prepared_marker" ]] || {
        echo "Error: fast release was not prepared. Run scripts/quick-release.sh --prepare-fast $VERSION before the release metadata commit." >&2
        exit 1
    }
    prepared_version="$(sed -n 's/^version=//p' "$prepared_marker")"
    prepared_commit="$(sed -n 's/^commit=//p' "$prepared_marker")"
    prepared_sha256="$(sed -n 's/^binary_sha256=//p' "$prepared_marker")"
    [[ "$prepared_version" == "$VERSION_NUM" ]] || {
        echo "Error: prepared version $prepared_version does not match $VERSION_NUM." >&2
        exit 1
    }
    release_parent="$(git rev-parse HEAD^)"
    [[ "$prepared_commit" == "$release_parent" ]] || {
        echo "Error: prepared binary commit $prepared_commit is not the parent of release commit $(git rev-parse HEAD)." >&2
        exit 1
    }
    actual_sha256="$(sha256sum "$source_bin" | cut -d' ' -f1)"
    [[ "$prepared_sha256" == "$actual_sha256" ]] || {
        echo "Error: target/selfdev/jcode changed after fast-release preparation." >&2
        exit 1
    }
    unexpected_release_files="$(git diff-tree --no-commit-id --name-only -r HEAD | grep -Ev '^(Cargo\.toml|Cargo\.lock|changelog/)' || true)"
    [[ -z "$unexpected_release_files" ]] || {
        echo "Error: the release metadata commit contains code or unsupported files:" >&2
        printf '%s\n' "$unexpected_release_files" >&2
        exit 1
    }

    cp "$source_bin" "$DIST/jcode-linux-x86_64.bin"
    strip --strip-unneeded "$DIST/jcode-linux-x86_64.bin"
    chmod +x "$DIST/jcode-linux-x86_64.bin"
    cat > "$DIST/jcode-linux-x86_64" <<WRAPPER
#!/usr/bin/env sh
set -eu
export JCODE_RUNTIME_RELEASE_SEMVER="$VERSION_NUM"
export JCODE_RUNTIME_RELEASE_GIT_HASH="$(git rev-parse --short HEAD)"
export JCODE_RUNTIME_RELEASE_GIT_DATE="$(git log -1 --format=%ci)"
export JCODE_RUNTIME_RELEASE_GIT_TAG="$VERSION"
self=\$0
# The launcher and channel entries are symlinks to this wrapper; resolve them
# so the payload is found next to the real file, not next to the symlink.
while [ -L "\$self" ]; do
    link=\$(readlink -- "\$self")
    case \$link in
        /*) self=\$link ;;
        *) self=\$(dirname -- "\$self")/\$link ;;
    esac
done
self_dir=\$(CDPATH= cd -- "\$(dirname -- "\$self")" && pwd)
exec "\$self_dir/jcode-linux-x86_64.bin" "\$@"
WRAPPER
    chmod +x "$DIST/jcode-linux-x86_64"
    file "$DIST/jcode-linux-x86_64.bin" | grep -q 'ELF 64-bit' || { echo "Error: bad Linux binary" >&2; exit 1; }
    version_output="$("$DIST/jcode-linux-x86_64" --version)"
    printf '%s\n' "$version_output" | grep -Fq "v$VERSION_NUM" || {
        echo "Error: fast binary reports the wrong version: $version_output" >&2
        exit 1
    }
    (cd "$DIST" && tar -cf - jcode-linux-x86_64 jcode-linux-x86_64.bin | gzip -1 > jcode-linux-x86_64.tar.gz)
    (cd "$DIST" && sha256sum jcode-linux-x86_64.tar.gz > SHA256SUMS)
    echo "  ✅ Linux artifact ready ($(( $(date +%s) - build_start ))s validation/package, $(du -h "$DIST/jcode-linux-x86_64.tar.gz" | cut -f1))"

    if $DRY_RUN; then
        echo ""
        echo "Fast dry run complete in $(elapsed)s. Artifacts in: $DIST"
        trap - EXIT
        exit 0
    fi

    tag_and_push
    echo "▸ Staging prepared Linux asset on the draft release..."
    ensure_release_draft
    gh release upload "$VERSION" \
        "$DIST/jcode-linux-x86_64.tar.gz" \
        "$DIST/SHA256SUMS" \
        --clobber

    echo ""
    echo "=== Fast release staged in $(elapsed)s ==="
    echo "  ✅ Linux x86_64: attached to draft from the warm selfdev cache"
    echo "  ⏳ CI: replacing Linux with the portable build and adding macOS, Windows, FreeBSD, signatures, and final checksums"
    echo ""
    echo "The staged Linux asset targets this build host's runtime baseline until CI replaces it."
    exit 0
fi

if [[ "$MODE" == "fast-macos-local" ]]; then
    echo "▸ Validating the prepared macOS arm64 build..."
    source_bin="target/aarch64-apple-darwin/release/jcode"
    prepared_marker="target/aarch64-apple-darwin/release/fast-macos-release-prepared"
    [[ -x "$source_bin" ]] || { echo "Error: macOS binary not found: $source_bin" >&2; exit 1; }
    [[ -f "$prepared_marker" ]] || {
        echo "Error: fast macOS release was not prepared. Run scripts/quick-release.sh --prepare-fast-macos $VERSION before the release metadata commit." >&2
        exit 1
    }
    prepared_version="$(sed -n 's/^version=//p' "$prepared_marker")"
    prepared_commit="$(sed -n 's/^commit=//p' "$prepared_marker")"
    prepared_sha256="$(sed -n 's/^binary_sha256=//p' "$prepared_marker")"
    [[ "$prepared_version" == "$VERSION_NUM" ]] || { echo "Error: prepared version $prepared_version does not match $VERSION_NUM." >&2; exit 1; }
    [[ "$prepared_commit" == "$(git rev-parse HEAD^)" ]] || { echo "Error: prepared macOS binary commit is not the parent of the release commit." >&2; exit 1; }
    [[ "$prepared_sha256" == "$(sha256sum "$source_bin" | cut -d' ' -f1)" ]] || { echo "Error: prepared macOS binary changed after preparation." >&2; exit 1; }
    unexpected_release_files="$(git diff-tree --no-commit-id --name-only -r HEAD | grep -Ev '^(Cargo\.toml|Cargo\.lock|changelog/)' || true)"
    [[ -z "$unexpected_release_files" ]] || {
        echo "Error: the release metadata commit contains code or unsupported files:" >&2
        printf '%s\n' "$unexpected_release_files" >&2
        exit 1
    }
    cp "$source_bin" "$DIST/jcode-macos-aarch64"
    chmod +x "$DIST/jcode-macos-aarch64"
    file "$DIST/jcode-macos-aarch64" | grep -q 'Mach-O 64-bit' || { echo "Error: bad macOS binary" >&2; exit 1; }
    sign_macos_binary "$DIST/jcode-macos-aarch64"
    (cd "$DIST" && tar czf jcode-macos-aarch64.tar.gz jcode-macos-aarch64)
    (cd "$DIST" && sha256sum jcode-macos-aarch64.tar.gz > SHA256SUMS)

    if $DRY_RUN; then
        echo "Fast macOS dry run complete in $(elapsed)s. Artifacts in: $DIST"
        trap - EXIT
        exit 0
    fi
    tag_and_push
    echo "▸ Staging prepared macOS arm64 asset on the draft release..."
    ensure_release_draft
    gh release upload "$VERSION" "$DIST/jcode-macos-aarch64.tar.gz" "$DIST/SHA256SUMS" --clobber
    echo ""
    echo "=== Fast macOS release staged in $(elapsed)s ==="
    echo "  ✅ macOS arm64: attached to draft from the prepared osxcross build"
    echo "  ⏳ CI: replacing it with the signoff build and adding Linux, macOS Intel, Windows, FreeBSD, signatures, and final checksums"
    exit 0
fi

# Standard local distribution build: Linux + macOS in parallel.
echo "▸ Building Linux x86_64 + macOS aarch64 in parallel..."
(
    JCODE_RELEASE_BUILD=1 JCODE_BUILD_SEMVER="$VERSION_NUM" scripts/build_linux_compat.sh "$DIST" >/dev/null
    echo "  ✅ Linux done ($(elapsed)s)"
) &
LINUX_PID=$!
(
    JCODE_RELEASE_BUILD=1 JCODE_BUILD_SEMVER="$VERSION_NUM" \
        CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}" \
        cargo build --release --target aarch64-apple-darwin --bin jcode 2>/dev/null
    cp target/aarch64-apple-darwin/release/jcode "$DIST/jcode-macos-aarch64"
    chmod +x "$DIST/jcode-macos-aarch64"
    sign_macos_binary "$DIST/jcode-macos-aarch64"
    (cd "$DIST" && tar czf jcode-macos-aarch64.tar.gz jcode-macos-aarch64)
    echo "  ✅ macOS done ($(elapsed)s)"
) &
MACOS_PID=$!
wait $LINUX_PID || { echo "Error: Linux build failed"; exit 1; }
wait $MACOS_PID || { echo "Error: macOS build failed"; exit 1; }

echo ""
echo "Build time: $(elapsed)s"
ls -lh "$DIST"/*.tar.gz
file "$DIST/jcode-linux-x86_64.bin" | grep -q 'ELF 64-bit' || { echo "Error: bad Linux binary"; exit 1; }
head -1 "$DIST/jcode-linux-x86_64" | grep -q '^#!/' || { echo "Error: bad Linux wrapper"; exit 1; }
file "$DIST/jcode-macos-aarch64" | grep -q 'Mach-O 64-bit' || { echo "Error: bad macOS binary"; exit 1; }

if $DRY_RUN; then
    echo ""
    echo "Dry run complete. Binaries in: $DIST"
    trap - EXIT
    exit 0
fi

echo ""
tag_and_push
echo "▸ Staging GitHub draft release..."
ensure_release_draft
gh release upload "$VERSION" \
    "$DIST/jcode-linux-x86_64.tar.gz" \
    "$DIST/jcode-macos-aarch64.tar.gz" \
    --clobber

echo ""
echo "=== Staged $VERSION in $(elapsed)s ==="
echo "  ✅ Linux + macOS: attached to draft"
echo "  ⏳ CI: building every required platform, checksums, and publishing"
echo ""
echo "The release becomes visible only when every required platform asset and checksum is ready."
