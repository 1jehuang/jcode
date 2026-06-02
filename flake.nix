{
  description = "jcode — high-performance multi-session coding agent harness";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane = {
      url = "github:ipetkov/crane";
    };
  };

  outputs =
    { self, nixpkgs, flake-utils, rust-overlay, crane, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # jcode currently builds on a recent nightly. Override here if you want
        # to pin a specific toolchain.
        rustToolchain = pkgs.rust-bin.nightly.latest.default.override {
          extensions = [ "rust-src" "rustfmt" "clippy" "rust-analyzer" ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Filter to keep markdown / extra files crates depend on at build time
        # (jcode reads include_str! files like src/prompt/system_prompt.md).
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter =
            path: type:
            let
              base = baseNameOf (toString path);
              ext = pkgs.lib.toLower (
                pkgs.lib.removePrefix "." (pkgs.lib.last (pkgs.lib.splitString "." base))
              );
            in
            (type == "directory")
            || craneLib.filterCargoSources path type
            || (
              ext == "md"
              || ext == "txt"
              || ext == "toml"
              || ext == "json"
              || ext == "snap"
              || ext == "html"
              || ext == "yaml"
              || ext == "yml"
            );
        };

        commonArgs = {
          inherit src;
          strictDeps = true;
          pname = "jcode";
          version = "0.12.3";

          nativeBuildInputs = with pkgs; [
            pkg-config
            cmake
            perl
            git
          ];

          buildInputs =
            with pkgs;
            [
              openssl
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
              libxcb
              xorg.libX11
              libusb1
            ]
            ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              darwin.apple_sdk.frameworks.AppKit
              darwin.apple_sdk.frameworks.Security
              darwin.apple_sdk.frameworks.SystemConfiguration
              darwin.apple_sdk.frameworks.CoreFoundation
              darwin.apple_sdk.frameworks.Metal
            ];

          # build.rs reads git metadata; supply something deterministic when the
          # store source has no .git/ (Nix typically strips it).
          JCODE_GIT_HASH = if (self ? rev) then self.rev else "nix-${self.shortRev or "unknown"}";
          JCODE_GIT_DATE = self.lastModifiedDate or "1970-01-01";
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        jcode = craneLib.buildPackage (
          commonArgs
          // {
            inherit cargoArtifacts;
            cargoExtraArgs = "-p jcode --bin jcode";
            doCheck = false;
          }
        );
      in
      {
        packages = {
          default = jcode;
          jcode = jcode;
        };

        apps.default = flake-utils.lib.mkApp {
          drv = jcode;
          name = "jcode";
        };

        devShells.default = pkgs.mkShell {
          inherit (commonArgs) JCODE_GIT_HASH JCODE_GIT_DATE;

          buildInputs = commonArgs.buildInputs;
          nativeBuildInputs =
            commonArgs.nativeBuildInputs
            ++ [
              rustToolchain
              pkgs.cargo-watch
              pkgs.cargo-nextest
              pkgs.sccache
            ];

          shellHook = ''
            export RUSTC_WRAPPER=${pkgs.sccache}/bin/sccache
            echo "jcode dev shell — rustc $(${rustToolchain}/bin/rustc --version)"
          '';
        };

        checks = {
          inherit jcode;

          jcode-clippy = craneLib.cargoClippy (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets --all-features -- -D warnings";
            }
          );

          jcode-fmt = craneLib.cargoFmt {
            inherit src;
            pname = "jcode-fmt";
            version = commonArgs.version;
          };
        };
      }
    );
}
