# Contributing to jcode

Thanks for contributing.

## Optional Nix development shell

If you already use Nix, run `nix develop` from the repository root. The
`flake.nix` provides Rust, Cargo, rustfmt, Clippy, rust-analyzer, the platform
C/C++ compiler, CMake, Make, Perl, pkg-config, OpenSSL, Git, and ripgrep.
On macOS it also supplies libclang through the bindgen hook for CoreAudio.
It declares shells for x86_64 and aarch64 Linux and macOS. This is a development
environment for the CLI repository, not Jcode Desktop or a Nix release package.
The existing non-Nix setup remains supported.

Enable `nix-command` and `flakes` in your Nix configuration, or use:

```sh
nix --extra-experimental-features 'nix-command flakes' develop
```

Inside the shell, use the normal development workflow. If you have `justrust`
installed, use `justrust check` and `justrust test`. The shell does not install
justrust or configure its hosted service. The Nix-provided Cargo also works for
contributors without justrust. Leave the shell with `exit`.

`flake.lock` pins nixpkgs, including the Rust toolchain and native libraries.
`Cargo.lock` still pins Rust crates. The shell does not replace your rustup
installation or make builds hermetic: it retains your environment and may use
externally installed tools. Optional integrations and runtime tools are not
all bundled.

To update the pinned environment deliberately:

```sh
nix flake update
nix flake check --all-systems
nix develop --command rustc --version
nix develop --command pkg-config --modversion openssl
nix develop --command justrust check
# Without justrust: nix develop --command cargo check
```

Review and commit the resulting `flake.lock` change. Add new native build
requirements to `flake.nix` when the project needs them. `flake check` evaluates
the shells but does not compile Jcode or prove that every declared platform
works, so test on the platforms you are changing as well.

## Issues vs pull requests

Both issues and pull requests are welcome. Open an issue to report a bug or discuss an idea, or send a focused PR if you have a fix or improvement ready. For large changes, discuss the approach in an issue first to avoid spending time on work that may not fit the project.

A clear issue or PR includes reproduction steps, expected behavior, actual behavior, and relevant logs, screenshots, or traces. Environment-specific fixes are especially helpful when they cover systems the maintainers may not have, such as macOS, Windows, unusual shells, terminal emulators, filesystems, GPU/display setups, or provider accounts.

Every PR must link to an existing GitHub issue in this repository. If there is no issue yet, open one and reference it in the PR description, for example with `Closes #123`.

## Pull request policy

Pull requests from everyone are welcome and encouraged, including first-time contributors and people who are not maintainers or existing contributors.

PRs are reviewed as changes that can be merged directly, not merely as proposals or references for a maintainer-authored rewrite. Review is based on correctness, tests, security, architecture, maintainability, and fit with the project, not the author's contributor status.

AI-assisted and generated contributions are welcome and held to the same standards as handwritten code. Understand the changes you submit, explain their assumptions and tradeoffs, and validate them. This applies equally to maintainer and community contributions.

Maintainers may request revisions, help refine an implementation, or decline a change that does not fit the project. A rewrite is not required just because a PR comes from an outside contributor.

The best PRs include:

- a clear description of the problem being solved
- a minimal reproduction or failing test when possible
- notes about edge cases and tradeoffs
- focused changes that are easy to review independently
- any relevant logs, screenshots, traces, or benchmarks

Keep changes focused and split large changes into independently reviewable pieces when possible. Passing checks does not guarantee a merge, but author status or use of code generation is not, by itself, a reason to reject a contribution.
