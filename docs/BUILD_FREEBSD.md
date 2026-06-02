# Building jcode on FreeBSD

Issue [#131](https://github.com/quangdang46/jcode/issues/131) requested
pre-built FreeBSD binaries. We do not currently publish them because
GitHub Actions does not provide native FreeBSD runners and the cross-compile
path from Linux to `x86_64-unknown-freebsd` is unreliable for a binary with
this many native dependencies (ratatui, crossterm, ring, libc subprocess
plumbing, etc.).

This document covers a **native FreeBSD build** so users on FreeBSD can run
jcode without waiting for an upstream binary release.

> ⚠️ Tested on FreeBSD 14.x amd64 / arm64. Build times are comparable to a
> Linux build (~15 minutes cold for `--release` with LTO; ~2 minutes
> incremental). Memory: ~6 GB peak during LTO link.

## Prerequisites

```sh
# As root or via sudo:
pkg install -y rust git pkgconf openssl curl cmake gmake
```

- `rust` ≥ 1.79 (FreeBSD's `pkg` tracks current; if you need a specific
  toolchain, install via [rustup](https://rustup.rs) — it works fine on
  FreeBSD).
- `pkgconf` is required for the OpenSSL / libssh2 link.
- `cmake` + `gmake` are needed by a couple of native crates in the dep tree.

## Build

```sh
git clone https://github.com/quangdang46/jcode.git
cd jcode
cargo build --release --bin jcode
```

The resulting binary is at `target/release/jcode`. Copy it to a directory
on your PATH:

```sh
mkdir -p ~/.local/bin
cp target/release/jcode ~/.local/bin/jcode
```

Or use the in-tree installer (creates the same `~/.jcode/builds/`
hierarchy as the Linux installer):

```sh
./scripts/install_release.sh
```

After install, `~/.local/bin/jcode --version` should print
`jcode v0.x.x-dev (<git-sha>)`.

## Smoke test

```sh
jcode doctor
jcode --help
```

`jcode doctor` is a good first check — it reports the OS, arch, terminal,
and whether storage / config dirs are reachable. On FreeBSD the platform
section will show:

```
## platform
  os: freebsd
  arch: x86_64
  ...
```

## Known gotchas on FreeBSD

1. **Terminal raw mode**: jcode's TUI uses crossterm, which works on
   FreeBSD via `termios`. If you're inside `tmux` or `screen` on a remote
   FreeBSD shell, make sure your terminal type is set sensibly
   (`export TERM=xterm-256color` is a safe default).
2. **OpenSSL paths**: if OpenSSL is installed in a non-standard prefix,
   set `OPENSSL_DIR=/usr/local` (or wherever you installed it) before the
   `cargo build` so the `openssl-sys` crate finds it.
3. **`libgit2` link errors** on `cargo build`: install `pkgconf` first
   (see prerequisites). The vendored libgit2 build path requires it.
4. **MCP servers running as separate processes** are not yet known to
   work without modification on FreeBSD because their argv handling uses
   Linux-specific `prctl(2)`. File a follow-up issue if you hit this.

## Why no CI-built FreeBSD binaries?

GitHub-hosted runners are Linux / macOS / Windows only. The two
realistic paths to ship a FreeBSD binary from CI are:

- **Cross-compile from Linux** via `cross` or `cargo-zigbuild`. Works for
  simple Rust binaries but the dep graph here pulls in `ring`,
  `cmake`-driven crates, and OpenSSL bindings — most attempts produce a
  binary that links but segfaults at runtime.
- **External CI** like Cirrus CI or self-hosted FreeBSD runners. Possible
  but adds infra cost + a new place to keep credentials.

Until one of those is set up, building from source on the target FreeBSD
machine is the supported path.

## See also

- [README.md](../README.md) — main install + provider setup.
- [BUILDING.md](BUILDING.md) — general build notes (when present).
- Issue [#131](https://github.com/quangdang46/jcode/issues/131) — tracking.
