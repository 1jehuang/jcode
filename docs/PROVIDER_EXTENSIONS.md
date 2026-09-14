# External provider extensions

Jcode external providers are independent processes. They communicate with
Jcode using the versioned JSONL contract in
[`PROVIDER_EXTENSION_PROTOCOL.md`](./PROVIDER_EXTENSION_PROTOCOL.md), rather
than linking to Jcode's Rust implementation or loading an in-process dynamic
library.

## Manifest

Each provider is described by a `provider.toml` file:

```toml
manifest_version = 1
id = "example-provider"
name = "Example Provider"
version = "1.0.0"
executable = "/opt/example-provider/bin/provider"
args = ["--stdio"]
protocol_version = "0.1"
capabilities = ["streaming", "cancellation"]
models = ["example-model"]
permissions = ["network"]
```

The provider ID is a stable lowercase identifier matching
`[a-z0-9][a-z0-9_-]{0,63}`. The executable and arguments are passed directly to
the operating system. Jcode never invokes a provider through a shell.

Supported permission declarations are:

- `network`
- `filesystem`
- `environment`
- `subprocess`
- `native_tools`

The declaration is an explicit request, not an implicit grant. A runtime
caller must provide a matching `PermissionPolicy` before a process can start.
Untrusted, disabled, incompatible, or invalid providers are rejected before
the subprocess is used.

## Registration

The public CLI exposes the initial local lifecycle:

```text
jcode provider extension add ./provider.toml
jcode provider extension add ./plugin --trusted
jcode provider extension list
jcode provider extension enable example-provider
jcode provider extension disable example-provider
jcode provider extension doctor
jcode provider extension run example-provider "hello" --json
jcode provider extension remove example-provider
```

`run` is an explicit integration seam for testing or invoking an extension. It
requires the record to be trusted and enabled. Each declared permission must be
approved for that invocation with its matching `--allow-*` flag. No permission
flag grants undeclared permissions.

## Portable plugin bundles

Jcode can inspect the common metadata and skill layout used by Claude Code and
Codex-style plugin bundles without executing any bundle component:

```text
plugin/
  plugin.json
  provider.toml                 # optional Jcode provider adapter
  skills/<name>/SKILL.md        # optional reusable workflow metadata
  agents/                        # reported, not executed yet
  hooks/hooks.json               # reported, not executed yet
  .mcp.json                      # reported, not executed yet
  .lsp.json                      # reported, not executed yet
  monitors/monitors.json         # reported, not executed yet
```

The metadata file may be `plugin.json`, `.codex-plugin/plugin.json`, or
`.claude-plugin/plugin.json`. Exactly one must exist. The initial supported
metadata fields are `name`, `version`, `description`, `author`, `homepage`,
`repository`, `license`, and `keywords`. Unknown fields are ignored so newer
Claude or Codex metadata can be inspected without breaking older Jcode builds.

Inspect a bundle with:

```text
jcode provider extension inspect ./plugin --json
```

Inspection is bounded to 256 KiB for the plugin manifest and 64 KiB per
`SKILL.md`. It reads metadata only, never starts executables, never runs hook
commands, and never grants permissions. Skills are sorted deterministically and
duplicate or unsafe names are rejected. Components that Jcode does not execute
yet are reported explicitly rather than silently ignored.

The current bundle layer is intentionally an inspection and compatibility
foundation. `provider extension run` remains the only execution path until each
additional component has its own permission model, lifecycle tests, and failure
containment.

Use `--trusted` on `add` only after reviewing the executable, arguments,
permissions, and source. Use `--json` for automation. The registry is stored
under the Jcode configuration directory and is written with a temporary file
followed by an atomic rename.

When `add` receives a bundle directory, it requires a root-level `provider.toml`
and registers that provider while retaining the bundle root as its source. A
bundle containing only skills or metadata can be inspected, but cannot be
registered as an executable provider.

Discovery helpers recognize these locations without executing anything:

1. `$JCODE_HOME/providers/<provider>/provider.toml`, or the platform Jcode
   configuration directory when `JCODE_HOME` is unset.
2. `.jcode/providers/<provider>/provider.toml` in the current project.

Explicit registration is preferred during the initial rollout. Automatic
discovery must not silently trust or start a provider.

## Application integration boundary

The initial release keeps external IDs out of the built-in `--provider` enum and
uses `provider extension run` as an explicit, permission-gated routing seam.
This avoids coupling the stable CLI provider catalog to third-party manifests.
A future first-class session integration should add a capability-based adapter
registry that resolves external IDs separately, preserves the existing built-in
provider enum, and maps protocol events into the common provider event stream.
That work should land behind compatibility tests for handshake, streaming,
tool events, cancellation, and provider replacement.


Provider identity is the manifest `id`, not the executable path or version.
Changing the executable version while preserving the ID keeps user selection
and registry state stable. The wire protocol version is negotiated separately
from the Jcode application version.

Patch-compatible protocol changes may add optional fields or capabilities.
Unknown fields and capabilities must be ignored. Incompatible wire changes
require a new protocol version and adapter path. A provider should fail with a
structured error instead of silently changing behavior.

Providers are currently installed and updated by the user. Remote downloads,
signatures, binary verification, rollback storage, and marketplace discovery
are intentionally deferred until a distribution design exists.

## Security model

- Providers are separate processes and are killed when their adapter is
  dropped.
- Handshake, request, frame-size, and process lifecycle limits are bounded by
  `jcode-provider-subprocess`.
- Credentials must not be embedded in protocol frames or logs.
- The public `provider extension run` command is permission-gated and forwards
  only a minimal environment (`PATH`, and `SystemRoot` on Windows). Environment
  permission does not implicitly expose parent-process secrets.
- Native tools and filesystem access require separate permission decisions.
- A provider manifest is metadata and is never proof that an executable is
  trustworthy.
- Plugin bundle inspection is metadata-only, size-bounded, deterministic, and
  does not execute skills, hooks, agents, MCP servers, LSP servers, monitors, or
  scripts.

The bundle inspection path adds no work to normal built-in provider sessions.
It performs a bounded directory scan only when explicitly requested through the
`provider extension inspect` command. Performance equivalence with full Claude
or Codex plugin runtimes is not claimed until those optional execution surfaces
are implemented and benchmarked separately.

The current CLI registry records trust and permissions. The provider runtime
must continue to enforce those fields at process start and must not treat a
registered provider as trusted merely because it is discoverable. Additional
credentials should be added only through a separate explicit allowlist design.
