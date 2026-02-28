# Releasing jcode

jcode has two release paths: a fast local path for hotfixes, and CI for full releases.

## Quick Release (local, ~2.5 minutes)

For hotfixes and urgent updates. Builds Linux + macOS locally and uploads directly.

```bash
scripts/quick-release.sh v0.5.5                # Build + tag + release
scripts/quick-release.sh v0.5.5 "Fix bug"      # With custom title
scripts/quick-release.sh --dry-run v0.5.5       # Build only, don't publish
```

### How it works

1. Builds Linux x86_64 natively and macOS aarch64 via osxcross **in parallel**
2. Verifies both binaries (ELF and Mach-O checks)
3. Creates a git tag and pushes it (this also triggers CI for the Windows build)
4. Uploads both binaries to a GitHub Release via `gh release create`
5. Users can immediately run `jcode update`

### Prerequisites

Already set up on the dev laptop (xps13):

- **osxcross** at `~/.osxcross` with macOS 14.5 SDK (darwin triple: `aarch64-apple-darwin23.5`)
- **rustup** with `aarch64-apple-darwin` target installed
- **`~/.cargo/config.toml`** has the osxcross linker configured
- **`gh` CLI** authenticated with GitHub

### Timeline

```
0s     Start parallel builds (Linux native + macOS cross-compile)
~90s   Linux build finishes
~150s  macOS build finishes
~153s  Binaries uploaded, release live
         ✅ Linux + macOS users can `jcode update`
~16m   CI finishes Windows build, uploads to same release
         ✅ Windows users can `jcode update`
```

## CI Release (automated, ~11 min Linux+macOS, ~16 min Windows)

Triggered automatically when a `v*` tag is pushed to GitHub.

### Workflow: `.github/workflows/release.yml`

```
Tag push (v*)
    │
    ├─► build-linux-macos (parallel)
    │     ├─► Linux x86_64   (ubuntu-latest)     ~8 min
    │     └─► macOS aarch64  (macos-latest)       ~11 min
    │
    ├─► build-windows (parallel, non-blocking)
    │     └─► Windows x86_64 (windows-latest)     ~16 min
    │
    ├─► release (after Linux + macOS complete)
    │     ├─► Create GitHub Release with binaries
    │     ├─► Update Homebrew formula (1jehuang/homebrew-jcode)
    │     └─► Update AUR package (jcode-bin)
    │
    └─► release-windows (after Windows + release complete)
          └─► Upload Windows binary to existing release
```

Key design decisions:
- **Windows does not block the release.** Linux and macOS binaries are published as soon as they're ready. Windows is added later.
- **Shallow clones** (`fetch-depth: 1`) to minimize checkout time.
- **`CARGO_INCREMENTAL=0`** for CI (incremental adds overhead on clean CI builds).
- **sccache + rust-cache** for dependency caching across runs.
- **mold linker** on Linux for faster linking.

### Package manager updates

CI handles Homebrew and AUR updates automatically:

- **Homebrew**: Updates `Formula/jcode.rb` in `1jehuang/homebrew-jcode` with new SHA256 hashes
- **AUR**: Updates `PKGBUILD` and `.SRCINFO` in `aur/jcode-bin`

Both are triggered by the `release` job after Linux + macOS builds complete.

## Which to use

| Scenario | Method | Time to Linux+macOS | Time to Windows |
|----------|--------|-------------------|-----------------|
| Hotfix / urgent bug | `scripts/quick-release.sh` | **~2.5 min** | ~16 min (CI) |
| Regular release | Push `v*` tag | ~11 min | ~16 min |
| Need Homebrew/AUR | Push `v*` tag | ~11 min | ~16 min |

For quick releases that also need Homebrew/AUR updates, use the script first (gets binaries out fast), then the CI tag push handles the package manager updates automatically. CI's `softprops/action-gh-release` will update the existing release created by the script.

## Cross-Compilation Setup

macOS binaries are cross-compiled from Linux using [osxcross](https://github.com/tpoechtrager/osxcross).

### Current configuration

| Component | Value |
|-----------|-------|
| SDK | macOS 14.5 |
| SDK source | [joseluisq/macosx-sdks](https://github.com/joseluisq/macosx-sdks) |
| Install location | `~/.osxcross/` |
| Darwin triple | `aarch64-apple-darwin23.5` |
| Linker | `aarch64-apple-darwin23.5-clang` |

### Cargo config (`~/.cargo/config.toml`)

```toml
[target.aarch64-apple-darwin]
linker = "aarch64-apple-darwin23.5-clang"

[env]
CC_aarch64_apple_darwin = "aarch64-apple-darwin23.5-clang"
CXX_aarch64_apple_darwin = "aarch64-apple-darwin23.5-clang++"
```

### Rebuilding osxcross from scratch

```bash
git clone https://github.com/tpoechtrager/osxcross /tmp/osxcross
curl -L -o /tmp/osxcross/tarballs/MacOSX14.5.sdk.tar.xz \
  https://github.com/joseluisq/macosx-sdks/releases/download/14.5/MacOSX14.5.sdk.tar.xz
cd /tmp/osxcross && UNATTENDED=1 TARGET_DIR=~/.osxcross ./build.sh
rustup target add aarch64-apple-darwin
```

Build takes ~5 minutes. Requires `clang`, `cmake`, `libxml2` (all available via pacman on Arch).

### Why osxcross (not zigbuild)

`cargo-zigbuild` can cross-compile pure Rust code to macOS, but jcode depends on crates that link against macOS system frameworks:
- `arboard` (clipboard) - links `AppKit`, `Foundation`
- `native-tls` / `security-framework` - links `Security`, `SystemConfiguration`
- `objc2` - links Objective-C runtime

These require actual macOS SDK headers and framework stubs, which osxcross provides.

## Build Performance

### Current timing (laptop, 8-core Intel Ultra 7 256V)

| Build | Clean | Cached deps |
|-------|-------|-------------|
| Linux x86_64 (native) | ~90s | ~90s |
| macOS aarch64 (cross) | ~3 min | ~2.5 min |
| Both in parallel | ~3 min | ~2.5 min |

The bottleneck is compiling jcode itself (120k lines of Rust). Dependencies are cached and don't need recompilation. The `build.rs` timestamp causes a full recompile of the main crate on every build.

### Why not faster

- `opt-level = 1`, `codegen-units = 256`, `incremental = true` are already set in `[profile.release]`
- 8 cores is the hardware limit
- Splitting into workspace crates would allow partial recompilation (~1 min for small changes)
- A 20+ core machine on LAN (not Tailscale) would cut build time to ~40-50s
