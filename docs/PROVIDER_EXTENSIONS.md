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
jcode provider extension list
jcode provider extension enable example-provider
jcode provider extension disable example-provider
jcode provider extension doctor
jcode provider extension remove example-provider
```

Use `--trusted` on `add` only after reviewing the executable, arguments,
permissions, and source. Use `--json` for automation. The registry is stored
under the Jcode configuration directory and is written with a temporary file
followed by an atomic rename.

Discovery helpers recognize these locations without executing anything:

1. `$JCODE_HOME/providers/<provider>/provider.toml`, or the platform Jcode
   configuration directory when `JCODE_HOME` is unset.
2. `.jcode/providers/<provider>/provider.toml` in the current project.

Explicit registration is preferred during the initial rollout. Automatic
discovery must not silently trust or start a provider.

## Upgrade and compatibility policy

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
- The extension runtime forwards only a minimal environment (`PATH`, and
  `SystemRoot` on Windows). Environment permission does not implicitly expose
  parent-process secrets.
- Native tools and filesystem access require separate permission decisions.
- A provider manifest is metadata and is never proof that an executable is
  trustworthy.

The current CLI registry records trust and permissions. The provider runtime
must continue to enforce those fields at process start and must not treat a
registered provider as trusted merely because it is discoverable. Additional
credentials should be added only through a separate explicit allowlist design.
