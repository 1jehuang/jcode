# Portable contributor builds

Plain `cargo check`, `cargo test`, and rust-analyzer do not select Jcode's shell
compiler wrapper by default. Native Windows contributors should not need a
POSIX shell or a manual wrapper override just to invoke Cargo.

## Fast local builds on Linux and macOS

Use `scripts/dev_cargo.sh check` or `scripts/dev_cargo.sh test`. The script works
with macOS's `/bin/bash` 3.2 as well as newer Bash. It retains adaptive memory-based
Cargo job sizing and the host-wide build gate when `flock` is available. Without
`flock`, it reports that the gate is unavailable and still applies job sizing.
No shell or toolchain upgrade is needed.

For local unoptimized crates, the dev script still selects
`scripts/rustc-parallel-frontend` automatically. The existing measurements in the
wrapper record substantial Linux frontend speedups. Registry/git dependencies,
compiler probes, and optimized compilations pass through unchanged. Set
`JCODE_PARALLEL_FRONTEND=0` to disable extra frontend threads or
`JCODE_FRONTEND_THREADS=2` to choose a thread count. `JCODE_BUILD_JOBS` and
`CARGO_BUILD_JOBS` retain their existing precedence.

POSIX users who want the same frontend policy for direct Cargo and rust-analyzer
can explicitly export the wrapper before starting those tools:

```sh
export RUSTC_WRAPPER="$PWD/scripts/rustc-parallel-frontend"
cargo check
```

Run that command from the repository root so the exported path is absolute.
Keep the choice consistent across tools. Removing the global default means plain
Cargo no longer receives the measured threaded frontend speedup automatically.
Switching between threaded and unthreaded builds also invalidates the affected
local incremental artifacts. The dev script remains the recommended fast path.
Do not opt into this POSIX script from a native Windows build.

Explicit `RUSTC_WRAPPER` or `CARGO_BUILD_RUSTC_WRAPPER` environment values,
including empty values that disable wrapping, take precedence over automatic
sccache/frontend selection. An explicit sccache wrapper is no longer removed for
incremental builds. If you want Jcode to choose automatically, unset those
variables rather than exporting an empty value.

## Focused portability checks

```sh
DEV_CARGO_TEST_BASH=/bin/bash python3 scripts/test_build_portability.py -v
bash scripts/test_dev_cargo_jobs.sh
```

The Python suite copies only the scripts and Cargo config into a temporary,
dependency-free crate. It checks real Cargo wrapper selection, explicit POSIX
opt-in, shell gate behavior, override precedence, and exit-status propagation.
It never builds the Jcode workspace or contacts a remote builder. The fixture
uses the supported `JCODE_FAST_LINKER=system` setting. Controlled `uname` fixtures
exercise Linux x86_64 linker-policy branches on any POSIX host. Gate-open checks
require the actual directory-open error and matching failure log, so an earlier
linker rejection cannot make them pass. These branch checks are not native Linux
execution. Gate policy checks use a stub `flock`, with an additional real-lock
check when available.
The compiler wrapper remains POSIX shell. `dev_cargo.sh` requires Bash, not `sh`.
Native Windows execution needs a separate Windows host and is not established
by the POSIX fixture results.
