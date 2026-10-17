# KCode product isolation

KCode and jcode are independent installations. Internal Rust library crate
names remain compatible with this source base; the executable is `kcode`
(`kcode.exe` on Windows), and the root package version carries a `-k` suffix.

| Resource | KCode | jcode (never a KCode destination) |
| --- | --- | --- |
| User state/config/sessions | `~/.kcode` | `~/.jcode` |
| Platform provider config | platform config directory + `kcode` | platform config directory + `jcode` |
| Windows binaries/builds | `%LOCALAPPDATA%\kcode` | `%LOCALAPPDATA%\jcode` |
| Unix launcher | `~/.local/bin/kcode` | `~/.local/bin/jcode` |
| Runtime sockets | `kcode.sock`, `kcode-api.sock` | `jcode.sock`, `jcode-api.sock` |
| Process overrides | `KCODE_*` | `JCODE_*` (not inherited as fallbacks) |

The executable, SDK, SSH launcher, self-development builds, installer, and
hotkey listener use KCode identity. Storage rejects explicit jcode destinations
and existing symlink/junction aliases into jcode storage.

## Updates and installation

Self-update is disabled, including background checks, explicit `/update`, and
direct download/install entry points. This does not depend on a version suffix
or an environment setting. The old updater used upstream's release repository;
the Windows installer independently used upstream metadata and install paths.
Neither is an acceptable KCode update source.

The fork's older `v0.85.0-k1.0.0` release is not a maintained compatible update
channel for this `0.93.0` source base. Re-enabling updates requires a deliberate
product-specific channel and artifact verification change. There is no fallback
to jcode. Source discovery uses only the KCode root package and fork URL.

Build and install a trusted local artifact on Windows:

```powershell
cargo build --release --bin kcode
$artifactVersion = ((& .\target\release\kcode.exe --version) -split '\s+')[1]
.\scripts\install.ps1 -ArtifactExePath .\target\release\kcode.exe -Version $artifactVersion
```

On Unix, use `scripts/install_release.sh --fast` to build and install from this
checkout. The download installer intentionally refuses to run. No shared PATH
shim is installed.

## One-way session import

```text
kcode session import-jcode <session-id>
```

This explicit operation copies the selected snapshot, backup, and journal from
`~/.jcode/sessions` to KCode storage, before normal startup side effects. It
never loads or repairs a source session, imports credentials/config/indexes,
or writes a reverse-sync record. Existing KCode session files are not replaced.
These are independent snapshots, rather than a continuous bidirectional mirror.

Regression tests import two sessions, edit the KCode copies, attempt repeated
imports, and require the entire jcode source tree to retain identical file
bytes, modification times, and permissions. Negative tests cover overlap,
traversal, and existing destinations. Installer tests execute Windows rejection
paths and verify that no install directory is created.

The earlier decollision and development-isolation branches supplied the separate
namespace conventions. Their older wholesale crate rename and Unix-only dev
importer were not suitable for merging over this newer source tree.

The TypeScript SDK uses KCode binaries, homes, config and sockets. Upstream npm
runtime dependencies are removed and its fork package is private until a
maintained channel exists. Existing `JcodeClient` API names remain compatible;
credential inheritance rejects jcode sources and aliases before creating files
or links. Build locally with `npm ci --ignore-scripts --omit=optional` and
`npm run build` from `sdk/typescript`.

## Verification

Windows executable check: `cargo check -p kcode --bin kcode
--no-default-features -j 1` passed. The six configured-model regressions and six
SDK boundary regressions passed. Storage's no-write-back and path-boundary
tests passed; the observed non-Unix PID-liveness test failure was explicitly
filtered on the focused storage rerun. Storage clippy passed with `-D warnings`.

The complete Windows build-support suite and the repository code-size budget
are not green. Full workspace/default-feature release validation and live
ConPTY behavior are not claimed by these checks. The anchor and kitty fixes
remain separate stacked changes.
