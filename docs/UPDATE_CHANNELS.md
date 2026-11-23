# Update channels

Jcode uses the **stable** update channel by default. Existing `main` channel users keep their source-build update behavior.

## Switch to prereleases in one command

From any machine or existing jcode version (including older stable installs), run:

```sh
curl -fsSL https://jcode.sh/install | bash -s -- --prerelease
```

This single command:
1. Installs the latest published prerelease build for your platform (currently Linux x86_64).
2. Saves `update_channel = "preview"` in `[features]` inside `~/.jcode/config.toml` so future automatic updates stay on prereleases.
3. Live-reloads any running jcode server and connected TUI sessions onto the new build without dropping sessions.

If your installed jcode binary already includes prerelease channel support, you can also switch directly from the CLI or inside a running TUI session:

```sh
# From your shell (persists channel + installs + live-reloads running server/sessions)
jcode update --prerelease

# Or inside an active jcode TUI session
/update prerelease
```

(`--preview` and `--channel preview` / `--channel prerelease` are accepted aliases.)

## How prerelease discovery works

Prerelease discovery checks published, non-draft GitHub prereleases matching either:
- `selfdev-<platform>-<40-hex-sha>` (e.g. `selfdev-linux-<commit>`)
- `v<major>.<minor>.<patch>-preview.<N>` (positive integer `N` without leading zeroes)

It selects the newest published prerelease with an asset for your platform and `SHA256SUMS`. Downloads must pass SHA-256 checksum verification. Drafts, incomplete uploads, and stable releases are never selected as prerelease candidates. Automatic checks never downgrade an installed build.

## Return to stable

```sh
jcode update --stable
```

You can also run `/update stable` inside jcode, or `curl -fsSL https://jcode.sh/install | bash -s -- --stable`.

This persists `update_channel = "stable"` and explicitly permits installing the latest stable release even when returning from a prerelease build, then live-reloads running sessions. Normal automatic stable checks never downgrade. `jcode update` without flags keeps your saved channel.

## Config & environment overrides

You can also set the channel directly in `~/.jcode/config.toml`:

```toml
[features]
update_channel = "preview" # stable (default), preview, or main
```

`JCODE_UPDATE_CHANNEL` overrides the saved preference for normal checks. An explicit `jcode update --prerelease` / `--stable` / `--channel ...` takes priority for that invocation and saves the requested preference without persisting unrelated environment overrides.

## Preview binary identity

Release builders must supply the full preview identity, not just the numeric Cargo package version:

```sh
JCODE_RELEASE_BUILD=1
JCODE_BUILD_SEMVER=1.0.0-preview.N
JCODE_BUILD_GIT_TAG=v1.0.0-preview.N
```

Continue providing the existing build git hash and date inputs as appropriate. The updater compares commit hashes and publication timestamps for `selfdev-*` prereleases and semantic version identity (ignoring SemVer build metadata `+...`) for `vX.Y.Z-preview.N` prereleases.
