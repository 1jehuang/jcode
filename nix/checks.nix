{
  pkgs,
  packages,
  module,
  nixosModule,
}:
let
  lib = pkgs.lib;

  # Evaluate the module with stub home-manager options so the generated
  # config.toml can be inspected without pulling in home-manager itself.
  evalConfig =
    jcodeConfig:
    (lib.evalModules {
      modules = [
        module
        { programs.jcode = jcodeConfig; }
        {
          options = {
            home.file = lib.mkOption {
              type = lib.types.attrsOf lib.types.raw;
              default = { };
            };
            home.packages = lib.mkOption {
              type = lib.types.listOf lib.types.raw;
              default = [ ];
            };
            assertions = lib.mkOption {
              type = lib.types.listOf lib.types.raw;
              default = [ ];
            };
          };
        }
        { _module.args.pkgs = pkgs; }
      ];
    }).config;

  parse =
    cfg: builtins.fromTOML (builtins.readFile (evalConfig cfg).home.file.".jcode/config.toml".source);

  # Home Manager enforces assertions itself (modules/default.nix collects
  # `config.assertions` and throws "Failed assertions"), so a bare evalModules
  # does not run them. Inspecting the list is how this check sees what a real
  # home-manager build would reject.
  failedAssertions =
    cfg: map (a: a.message) (lib.filter (a: !a.assertion) (evalConfig cfg).assertions);

  rejected = cfg: failedAssertions cfg != [ ];

  # Every value shape jcode's config.toml uses, including the ones that are easy
  # to get wrong: arrays of tables, floats, negative integers, empty containers,
  # keys that need quoting and a table key sorting before a scalar key.
  sample = {
    apple = {
      x = 1;
    };
    zebra = 2;

    provider = {
      default_model = "claude-sonnet-4-5";
      anthropic_cache_ttl_1h = true;
      openai_reasoning_effort = "low";
    };

    "my.gateway" = {
      "a b" = 1;
      note = "привет 🎉";
    };

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
        {
          id = "other";
          name = "Other";
          context_window = 8000;
        }
      ];
    };

    compaction = {
      mode = "semantic";
      ewma_alpha = 0.35;
      min_samples = -3;
      max_context_tokens = 200000;
    };

    tools = {
      enabled = [
        "bash"
        "edit"
      ];
      disabled = [ ];
      profile = "";
    };

    keybindings.side_panel_toggle = "ctrl+b";

    hooks.pre_tool = ''
      line one
      line "two"
    '';
  };

  noConfig = evalConfig {
    enable = true;
    manageConfig = false;
  };

  nixosEval =
    jcodeConfig:
    (lib.evalModules {
      modules = [
        nixosModule
        { programs.jcode = jcodeConfig; }
        {
          options.environment.systemPackages = lib.mkOption {
            type = lib.types.listOf lib.types.raw;
            default = [ ];
          };
        }
        { _module.args.pkgs = pkgs; }
      ];
    }).config;

  check =
    name: cond:
    if cond then
      pkgs.runCommand "jcode-check-${name}" { } "mkdir $out"
    else
      throw "TEST FAILED: ${name}";
in
{
  jcode-source-default = check "source-default" (
    packages.default == packages.jcode && packages.default.pname == "jcode"
  );

  jcode-binary-home-manager = check "binary-home-manager" (
    lib.elem packages.jcode-bin
      (evalConfig {
        enable = true;
        package = packages.jcode-bin;
      }).home.packages
  );

  jcode-binary-nixos = check "binary-nixos" (
    lib.elem packages.jcode-bin
      (nixosEval {
        enable = true;
        package = packages.jcode-bin;
      }).environment.systemPackages
  );

  # The whole point of the module: settings must survive the round trip
  # Nix -> TOML -> Nix unchanged.
  jcode-config-roundtrip = check "roundtrip" (
    parse {
      enable = true;
      settings = sample;
    } == sample
  );

  # A table key that sorts before a scalar key must not break serialization
  # (TOML requires scalars before tables in a section).
  jcode-config-scalar-after-table = check "scalar-after-table" (
    let
      parsed = parse {
        enable = true;
        settings = {
          apple = {
            x = 1;
          };
          zebra = 2;
        };
      };
    in
    parsed.apple.x == 1 && parsed.zebra == 2
  );

  jcode-config-arrays-of-tables = check "arrays-of-tables" (
    let
      models =
        (parse {
          enable = true;
          settings = sample;
        }).providers.aigate.models;
    in
    builtins.length models == 2
    && (builtins.elemAt models 1).id == "other"
    && (builtins.elemAt models 0).context_window == 200000
  );

  jcode-config-quoted-keys = check "quoted-keys" (
    let
      parsed = parse {
        enable = true;
        settings = sample;
      };
    in
    parsed."my.gateway"."a b" == 1
  );

  # Empty settings still produce a file; an empty TOML document is valid and
  # jcode falls back to its own defaults for every key.
  jcode-config-empty-settings = check "empty-settings" (parse { enable = true; } == { });

  # manageConfig = false installs the package without touching config.toml.
  jcode-config-manage-config-off = check "manage-config-off" (
    noConfig.home.file == { }
    && lib.any (p: lib.isDerivation p && p.pname == "jcode") noConfig.home.packages
  );

  # The system module only installs the package, and only when enabled.
  jcode-nixos-module = check "nixos-module" (
    lib.any (p: lib.isDerivation p && p.pname == "jcode")
      (nixosEval { enable = true; }).environment.systemPackages
    && (nixosEval { }).environment.systemPackages == [ ]
  );

  # A generated config lands in the world-readable store, so an inline
  # credential there must stop the build instead of being published.
  jcode-config-rejects-inline-credential = check "rejects-inline-credential" (
    let
      messages = failedAssertions {
        enable = true;
        settings.providers.example.api_key = "not-a-real-secret";
      };
    in
    messages != [ ] && lib.any (m: lib.hasInfix "api_key" m && lib.hasInfix "world-readable" m) messages
  );

  # The file source itself refuses an inline credential, so a consumer that
  # reads home.file without evaluating assertions still cannot put a secret in
  # the store.
  jcode-config-file-source-refuses-inline-credential = check "file-source-refuses-inline-credential" (
    !(builtins.tryEval (
      builtins.seq
        (evalConfig {
          enable = true;
          settings.providers.example.api_key = "not-a-real-secret";
        }).home.file.".jcode/config.toml".source
        "reached"
    )).success
  );

  # The same detector catches the other credential-bearing fields in the schema.
  jcode-config-rejects-token = check "rejects-token" (rejected {
    enable = true;
    settings.telegram_bot_token = "not-a-real-secret";
  });

  jcode-config-rejects-password = check "rejects-password" (rejected {
    enable = true;
    settings.safety.email_password = "not-a-real-secret";
  });

  # Header names arrive in any case, and a bearer token in a header leaks just
  # like an inline api_key.
  jcode-config-rejects-authorization-header = check "rejects-authorization-header" (rejected {
    enable = true;
    settings.providers.example.headers.Authorization = "Bearer not-a-real-secret";
  });

  jcode-config-rejects-proxy-authorization = check "rejects-proxy-authorization" (rejected {
    enable = true;
    settings.providers.example.headers."Proxy-Authorization" = "Bearer not-a-real-secret";
  });

  # The exemption for variable-name fields must not cover a literal header that
  # merely ends in "env".
  jcode-config-rejects-authorization-env-header = check "rejects-authorization-env-header" (rejected {
    enable = true;
    settings.providers.example.headers."X-Authorization-Env" = "Bearer not-a-real-secret";
  });

  jcode-config-rejects-api-key-env-header = check "rejects-api-key-env-header" (rejected {
    enable = true;
    settings.providers.example.headers."X-Api-Key-Env" = "not-a-real-secret";
  });

  # Credential words only match at a separator boundary, so ordinary
  # configuration must keep working: the auth enum value, a benign header, a
  # variable name field and an identifier field.
  jcode-config-accepts-non-credential-strings = check "accepts-non-credential-strings" (
    !(rejected {
      enable = true;
      settings = {
        providers.example.auth = "api-key";
        providers.example.headers."X-Api-Version" = "2024-01-01";
        providers.example.headers."X-Title" = "jcode";
        providers.example.api_key_env = "EXAMPLE_API_KEY";
        safety.jade_relay_token_id = "abcd";
        compaction.max_context_tokens = 200000;
        keybindings.side_panel_toggle = "ctrl+b";
      };
    })
  );

  # Environment-variable names are the supported way to reference credentials,
  # and the flag-style field must not trip the detector.
  jcode-config-accepts-credential-references = check "accepts-credential-references" (
    !(rejected {
      enable = true;
      settings = {
        providers.example.api_key_env = "EXAMPLE_API_KEY";
        providers.example.requires_api_key = true;
      };
    })
  );

  # With manageConfig disabled nothing is written to the store, so credentials
  # in settings are not published and the user keeps their own file.
  jcode-config-accepts-credentials-without-config-management =
    check "accepts-credentials-without-config-management"
      (
        !(rejected {
          enable = true;
          manageConfig = false;
          settings.providers.example.api_key = "not-a-real-secret";
        })
      );
}
