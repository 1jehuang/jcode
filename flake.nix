{
  description = "Jcode — a coding agent with a blazing-fast TUI, multi-model support and swarm coordination";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    let
      packageVersion = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;

      # ── Home Manager module ────────────────────────────────────────────
      #
      # The generated file is exactly the `settings` attribute converted to
      # TOML, so the module cannot drift from jcode's config schema: unknown
      # or newly added keys pass through untouched.
      #
      # Conversion uses pkgs.formats.toml, which serialises through the Rust
      # `toml` crate and therefore handles the shapes jcode needs: nested
      # tables, arrays of tables ([providers.x.models]), quoted keys (keys
      # containing dots or spaces), floats, negative integers, unicode and
      # multi-line strings. Its option type rejects `null` at evaluation time,
      # since TOML has no null and a silently dropped key would be worse.
      homeManagerModule =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.jcode;
          toml = pkgs.formats.toml { };

          # A file generated into the store is world-readable, so a credential
          # written into `settings` is visible to every local user. The schema
          # names the environment-variable fields with an _env suffix, so
          # api_key_env and friends never match the patterns below.
          # Names mix separators ("x-api-key", "api_key", "Proxy-Authorization"),
          # so compare whole words: that keeps benign names such as keybindings,
          # x-api-version and max_context_tokens out of the match.
          credentialWords = [
            "apikey"
            "bearer"
            "cookie"
            "key"
            "passwd"
            "password"
            "secret"
            "token"
          ];

          looksSecret =
            name:
            let
              # Header names arrive in whatever case the provider uses.
              lower = lib.toLower name;
              words = lib.splitString "-" (lib.replaceStrings [ "_" "." " " ] [ "-" "-" "-" ] lower);
            in
            # Only the schema's variable-name fields (`*_env`) and identifier
            # fields (`*_id`) are exempt; a header such as X-Authorization-Env
            # carries a literal and must not slip through the exemption.
            !(lib.hasSuffix "_env" lower)
            && !(lib.hasSuffix "_id" lower)
            && (lib.hasInfix "authorization" lower || lib.any (word: builtins.elem word credentialWords) words);

          secretKeys =
            value:
            if lib.isAttrs value then
              lib.concatMap (
                name:
                (lib.optional (looksSecret name && lib.isString value.${name}) name) ++ secretKeys value.${name}
              ) (lib.attrNames value)
            else if lib.isList value then
              lib.concatMap secretKeys value
            else
              [ ];
        in
        {
          options.programs.jcode = {
            enable = lib.mkEnableOption "jcode";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.jcode;
              defaultText = lib.literalExpression "jcode from this flake";
              description = "The jcode package to install.";
            };

            extraPackages = lib.mkOption {
              type = lib.types.listOf lib.types.package;
              default = [ pkgs.git ];
              description = "Packages installed alongside jcode for its shell tools.";
            };

            settings = lib.mkOption {
              type = toml.type;
              default = { };
              example = {
                provider.default_model = "claude-sonnet-4-5";
                features.update_channel = "main";
                hooks.pre_tool = "~/bin/jcode-tool-policy";
                keybindings.side_panel_toggle = "ctrl+b";
                providers.aigate = {
                  type = "openai-compatible";
                  base_url = "https://llm.example.com/v1";
                  api_key_env = "AIGATE_API_KEY";
                  models = [
                    {
                      id = "deepseek-v4-pro";
                      name = "DeepSeek V4 Pro";
                      context_window = 200000;
                    }
                  ];
                };
              };
              description = ''
                Contents of ~/.jcode/config.toml, converted to TOML as given;
                see crates/jcode-base/src/config.rs for the available tables.
                Values must be strings, integers, floats, booleans, lists or
                attribute sets - TOML has no null. Keys with dots or spaces are
                quoted by the generator.

                The result lands in the world-readable Nix store, so
                credentials belong in the environment-variable field
                (api_key_env). Evaluation fails if `settings` contains an
                inline credential such as api_key, *_token, *_password or an
                authorization header; set manageConfig to false when a
                hand-written file with secrets is needed.

                jcode reads $JCODE_HOME/config.toml instead when JCODE_HOME is
                set, and its updater writes to ~/.jcode/builds regardless of
                this file; see manageConfig.
              '';
            };

            manageConfig = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = ''
                Write ~/.jcode/config.toml from `settings`.

                The file is a symlink into the Nix store, so it is read-only:
                jcode's own writes to it (saving settings from the TUI, the
                one-off migrations it runs at startup) fail with a permission
                error instead of persisting. Set this to false to leave the
                file alone and let jcode own a writable copy.
              '';
            };
          };

          config = lib.mkIf cfg.enable {
            assertions = [
              {
                # Only the generated file reaches the store; with
                # manageConfig disabled the file is the user's own.
                assertion = !cfg.manageConfig || secretKeys cfg.settings == [ ];
                message = ''
                  programs.jcode.settings contains inline credentials (${lib.concatStringsSep ", " (lib.unique (secretKeys cfg.settings))}).
                  The generated config.toml is a world-readable Nix store path, so those values would be readable by every local user.
                  Use the environment-variable field instead (for example api_key_env), or set programs.jcode.manageConfig = false and keep the file outside the store.
                '';
              }
            ];

            home.packages = [ cfg.package ] ++ cfg.extraPackages;

            home.file = lib.mkIf cfg.manageConfig {
              ".jcode/config.toml".source = toml.generate "jcode-config.toml" cfg.settings;
            };
          };
        };

      # ── NixOS module ───────────────────────────────────────────────────
      nixosModule =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.jcode;
        in
        {
          options.programs.jcode = {
            enable = lib.mkEnableOption "jcode system-wide";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.jcode;
              defaultText = lib.literalExpression "jcode from this flake";
              description = "The jcode package to install.";
            };
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ cfg.package ];
          };
        };
    in
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };

        # build.rs wants what `git log -1 --format=%ci` prints. Nix gives the
        # commit time as YYYYMMDDHHMMSS in UTC, or nothing for a dirty tree.
        gitDate =
          let
            stamp = self.lastModifiedDate or "";
          in
          if builtins.stringLength stamp == 14 then
            "${builtins.substring 0 4 stamp}-${builtins.substring 4 2 stamp}-${builtins.substring 6 2 stamp} ${builtins.substring 8 2 stamp}:${builtins.substring 10 2 stamp}:${builtins.substring 12 2 stamp} +0000"
          else
            "";

        jcode = pkgs.rustPlatform.buildRustPackage (
          {
            pname = "jcode";
            version = packageVersion;

            # The derivation needs the Rust sources, `docs/` (jcode-app-core's
            # build script embeds them) and `assets/` (include_bytes! icons).
            # Excluding build artefacts and Nix-only files keeps local
            # (non-git) flake evaluation from copying `target/` into the store
            # and stops edits to the flake from rebuilding all 580 crates.
            src = pkgs.lib.cleanSourceWith {
              src = self;
              filter =
                path: type:
                let
                  base = baseNameOf path;
                  rel = pkgs.lib.removePrefix (toString self + "/") (toString path);
                in
                !(builtins.elem base [
                  "target"
                  ".git"
                  "result"
                  "nix"
                  ".omp"
                ])
                && !(pkgs.lib.hasPrefix "target-" base)
                && !(builtins.elem rel [
                  "flake.nix"
                  "flake.lock"
                  "shell.nix"
                ])
                && !(pkgs.lib.hasPrefix "nix/" rel)
                && !(pkgs.lib.hasPrefix ".omp/" rel);
            };

            cargoLock = {
              lockFile = ./Cargo.lock;
              outputHashes = {
                "agentgrep-0.1.7" = "sha256-9US+dqLbe1gkXKnaRbilA/suOK5vGwe4B7c+eET05yY=";
                "mermaid-rs-renderer-0.3.1" = "sha256-uekh1vJ19dAPP7+4PiqSlJizApZLpDhBWBoyN+fgS9s=";
              };
            };

            # openssl-sys is the only crate needing a system library; the C code
            # in libsqlite3-sys, onig_sys, ring and aws-lc-sys is vendored and
            # built by the `cc` crate.
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.openssl ];

            # Everything in src/bin is a development utility or an SDK host, not
            # part of the CLI, so only the main binary is built and installed.
            cargoBuildFlags = [
              "--bin"
              "jcode"
            ];
            cargoInstallFlags = [
              "--bin"
              "jcode"
            ];

            # Release metadata for the embedded version string.
            JCODE_RELEASE_BUILD = "1";
            JCODE_BUILD_SEMVER = packageVersion;
            JCODE_BUILD_GIT_HASH = self.shortRev or self.dirtyShortRev or "unknown";
            JCODE_BUILD_GIT_DIRTY = if self ? rev then "0" else "1";
            JCODE_BUILD_GIT_DATE = gitDate;

            # The workspace test suite drives live providers and the shared
            # daemon; it is not sandbox-safe. The installed binary is smoke
            # tested instead.
            doCheck = false;
            doInstallCheck = true;
            installCheckPhase = ''
              "$out/bin/jcode" --version >/dev/null
            '';

            meta = {
              mainProgram = "jcode";
              homepage = "https://github.com/1jehuang/jcode";
              description = "Coding agent with a blazing-fast TUI, multi-model support and swarm coordination";
              license = pkgs.lib.licenses.mit;
              platforms = pkgs.lib.platforms.unix;
            };
          }
          # cpal reaches coreaudio-sys on Darwin, and its build script runs
          # bindgen, which loads libclang through clang-sys. Pinned explicitly
          # rather than assumed to be discoverable in the Darwin environment.
          // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isDarwin {
            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.llvmPackages.libclang
            ];
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          }
        );
      in
      {
        packages = {
          inherit jcode;
          default = jcode;
        };

        apps.default = {
          type = "app";
          program = "${jcode}/bin/jcode";
          meta.description = "Run the jcode coding agent";
        };

        devShells.default = import ./shell.nix { inherit pkgs; };

        formatter = pkgs.nixfmt;

        checks = import ./nix/checks.nix {
          inherit pkgs;
          module = homeManagerModule;
          inherit nixosModule;
        };
      }
    )
    // {
      homeManagerModules.default = homeManagerModule;
      nixosModules.default = nixosModule;
    };
}
