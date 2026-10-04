# Nix and NixOS

The repository ships a flake that covers both halves of the Nix story:

- `devShells.default` supplies a pinned toolchain and the native libraries
  `cargo` needs, while cargo keeps using its own registry cache and the
  repository's `target/` directory. Incremental builds stay as fast as they are
  outside Nix.
- `packages.default` is a reproducible, sandboxed release build for people who
  want jcode managed by Nix rather than by `scripts/install.sh`.
- `packages.jcode-bin` installs a hash-pinned official release binary without
  compiling Rust. Both packages install the same `bin/jcode` command.
- `homeManagerModules.default` and `nixosModules.default` install the package
  and can generate `~/.jcode/config.toml` from a Nix attribute set.
- `checks` cover the configuration conversion described below.

## Development shell

```sh
nix develop            # flake-pinned nixpkgs
nix-shell              # same environment, using the ambient <nixpkgs>
```

The shell provides `rustc`, `cargo`, `clippy`, `rustfmt`, `rust-analyzer`,
`clang`, `mold`, `git` and OpenSSL. `scripts/dev_cargo.sh`, and therefore
`jcode self-dev`, picks up `clang` and `mold` from `PATH` and links with
`-fuse-ld=mold`; without them it falls back to the system linker, which is
noticeably slower for the jcode binary because it relinks on every build.

The parallel rustc front-end that `scripts/dev_cargo.sh` enables for the
`dev`, `selfdev` and `test` profiles also works here. It is applied through the
repository's own `RUSTC_WRAPPER` (`scripts/rustc-parallel-frontend`), which sets
`RUSTC_BOOTSTRAP` for those invocations, so a nightly toolchain is not needed
and the shell stays on the stable compiler from nixpkgs.

OpenSSL is the only system library involved. The C sources in `libsqlite3-sys`,
`onig_sys`, `ring` and `aws-lc-sys` are vendored and compiled by the `cc` crate,
so sqlite, oniguruma, cmake, perl and pkg-config are not required.

Cargo continues to use `~/.cargo` and `./target`: the shell exports
`OPENSSL_DIR`, `OPENSSL_LIB_DIR` and `OPENSSL_INCLUDE_DIR` and nothing else, so
builds and caches behave exactly as they do on a non-Nix system.

## Building the package

```sh
nix build .#          # ./result/bin/jcode
nix run .#            # build and run
nix build .#jcode     # the package by name
```

The derivation runs `cargo build --release --bin jcode` inside the sandbox with
`cargoLock.lockFile`, which resolves the two git dependencies through the hashes
recorded in `flake.nix`. Only the `jcode` binary is built and installed:
`test_api` and `jcode-harness` are development utilities, and the harness API
bridge ships inside the main binary as `jcode api-bridge`.

Build metadata mirrors the release workflow (`.github/workflows/release.yml`):
`JCODE_RELEASE_BUILD=1`, `JCODE_BUILD_SEMVER` from `Cargo.toml`, and
`JCODE_BUILD_GIT_HASH`, `JCODE_BUILD_GIT_DATE` and `JCODE_BUILD_GIT_DIRTY` from
the flake revision. Without them the build script would report `unknown`,
because the sandbox has no `.git` directory.

The workspace test suite is not run during the build: it drives live providers
and the shared daemon, which the sandbox cannot provide. The derivation instead
smoke tests the installed binary with `jcode --version`.

`nix flake check` runs the checks in `nix/checks.nix`. It does not build the
package, so run `nix build .#` as well when changing anything the binary depends
on.

## Installing a release binary

```sh
nix build .#jcode-bin
nix run .#jcode-bin -- --version
nix profile install .#jcode-bin
```

The binary package supports `x86_64-linux`, `aarch64-linux`, `x86_64-darwin`
and `aarch64-darwin`. `nix/release.json` pins the release tag and the SHA256 of
each archive. Linux executables are patched to use the Nix loader and libraries;
the portable archive's launcher is replaced by the actual executable. Darwin
binaries are installed without stripping or modifying their signatures. Each
native build checks the installed command's version.

`jcode` and `default` still compile the checked-out sources. Their version comes
from `Cargo.toml`, while `jcode-bin` follows the pinned public release. These
versions can differ. Install one variant, not both, since their commands collide.

To choose the binary in either the Home Manager or NixOS module, override the
existing package option:

```nix
programs.jcode.package = inputs.jcode.packages.${pkgs.stdenv.hostPlatform.system}.jcode-bin;
```

### Release updates

After publishing a release and its assets, `release.yml` explicitly dispatches
`update-nix-release.yml`. A separate `release: published` trigger covers releases
published manually; that event alone is insufficient for publications made with
`GITHUB_TOKEN`. The updater accepts only public stable releases with all four
archives and valid entries in `SHA256SUMS`, and refuses version downgrades.

The workflow updates `nix/release.json`, validates the flake and binary package,
and proposes a pull request against the default branch. It does not merge the
pull request, rewrite release tags, or update `flake.lock`. Repository settings
must allow GitHub Actions to create pull requests. Updates only reach flake
consumers after the pull request is merged and their own flake input is updated.

Pull requests created with `GITHUB_TOKEN` do not trigger other pull-request
workflows. Nix validation runs before the PR is created; if branch protection
requires the general CI checks, a maintainer must close and reopen the PR with a
human account after the latest bot update. The updater's validation does not
replace required PR checks; wait for those checks before merging.

The tracked `flake.lock` pins build dependencies for both variants. Update those
separately with `nix flake update`; publishing a new binary does not require
changing nixpkgs or flake-utils.

Intel macOS uses the separate `nixpkgs-darwin` input on the 26.05 Darwin branch
because nixpkgs unstable dropped that platform in 26.11. Other systems retain
the unstable input.

## Home Manager

```nix
{
  inputs.jcode.url = "github:1jehuang/jcode";

  # In a home-manager configuration:
  imports = [ inputs.jcode.homeManagerModules.default ];

  programs.jcode = {
    enable = true;
    settings = {
      provider.default_model = "claude-sonnet-4-5";
      features.update_channel = "main";
      hooks.pre_tool = "~/bin/jcode-tool-policy";
      keybindings.side_panel_toggle = "ctrl+b";
      providers.example = {
        type = "openai-compatible";
        base_url = "https://llm.example.com/v1";
        api_key_env = "EXAMPLE_API_KEY";
      };
    };
  };
}
```

`settings` is written to `~/.jcode/config.toml` as-is: nested tables, arrays of
tables, floats and keys containing dots or spaces all survive the conversion,
and the option type rejects `null`, which TOML cannot represent. The generated
file is a symlink into the Nix store and is therefore read-only, so jcode's own
writes to it fail: saving settings from the TUI and the one-off config
migrations it runs at startup cannot persist. Set
`programs.jcode.manageConfig = false` to leave the file alone and let jcode own
a writable copy.

Two further points worth knowing:

- jcode reads `$JCODE_HOME/config.toml` instead of `~/.jcode/config.toml` when
  `JCODE_HOME` is set, so the module's file is ignored in that case.
- `jcode update` installs release binaries under `~/.jcode/builds` and has no
  notion of a Nix-managed install. Set `features.check_updates = false` if the
  user should only receive updates through Nix.

The file lands in the world-readable Nix store, so credentials belong in the
environment-variable field, `api_key_env`. To make that hard to get wrong,
evaluation fails when `settings` contains an inline credential. A string-valued
key counts as one when its name contains `authorization` or one of its words
(split on `-`, `_`, `.` and spaces) is `key`, `apikey`, `token`, `secret`,
`password`, `passwd`, `cookie` or `bearer`. That catches `api_key`,
`bing_api_key`, `telegram_bot_token`, `email_password`, `Authorization`,
`Proxy-Authorization`, `X-Api-Key-Env` and `Set-Cookie`, and the error names
the offending keys.

Names that only look similar are unaffected, because whole words are compared:
`keybindings`, `max_context_tokens` and headers such as `X-Api-Version` or
`X-Title` pass, as does the `auth = "api-key"` enum value. The only exemptions
are the schema's variable-name fields ending in `_env` and identifier fields
ending in `_id`, so `api_key_env` and `jade_relay_token_id` are accepted while
a literal header such as `X-Authorization-Env` is still rejected.

The assertion only applies while the module generates the file. With
`programs.jcode.manageConfig = false` nothing is written to the store, so
`settings` may hold anything and the user owns the file.

## NixOS

```nix
{
  imports = [ inputs.jcode.nixosModules.default ];
  programs.jcode.enable = true;
}
```

The system module installs the package into `environment.systemPackages`.
Per-user configuration is the Home Manager module's job.

## Limitations

- Linux on `x86_64` is the tested platform. The flake also declares
  `aarch64-linux`, `x86_64-darwin` and `aarch64-darwin`, but those outputs have
  not been built or run.
- The package follows `nixpkgs-unstable` through `flake.lock`; update it with
  `nix flake update`.
