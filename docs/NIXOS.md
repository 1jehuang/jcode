# Nix and NixOS

The repository ships a flake that covers both halves of the Nix story:

- `devShells.default` supplies a pinned toolchain and the native libraries
  `cargo` needs, while cargo keeps using its own registry cache and the
  repository's `target/` directory. Incremental builds stay as fast as they are
  outside Nix.
- `packages.default` is a reproducible, sandboxed release build for people who
  want jcode managed by Nix rather than by `scripts/install.sh`.
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

The file lands in the world-readable Nix store, so credentials belong in
`api_key_env` and never inline in `api_key`.

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
