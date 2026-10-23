# D2 diagrams

Jcode can render self-contained `d2` fences with an installed local D2 executable. Mermaid remains the default syntax requested from the model, and TALA is the default layout for D2. A fence's language selects its renderer, so choosing D2 doesn't reinterpret existing Mermaid diagrams.

To prefer D2 with TALA, add this to `~/.jcode/config.toml`:

```toml
[features]
mermaid = true
d2 = true

[diagrams]
preferred_engine = "d2"
d2_layout = "tala"
d2_binary = "d2"
d2_timeout_secs = 10
```

`d2_binary` accepts an executable name on PATH or a path such as `/opt/homebrew/bin/d2`, not a shell command. Jcode doesn't install D2 or download layout plugins. Use a D2 build with the requested layout available. `tala` is the default layout; set `d2_layout = "dagre"` or `d2_layout = "elk"` if you want another layout or your D2 build doesn't provide TALA. A layout failure keeps the source visible instead of silently switching engines.

`JCODE_ENABLE_D2=0` or `JCODE_ENABLE_D2=false` disables D2 independently of the config file. Restart the TUI after changing D2 rendering settings or its feature flag, and after installing or replacing D2 following a failed render. The client snapshots those settings at startup so cached Markdown and images use consistent options. The generation preference is separate: the daemon uses it when assembling model guidance. Both the daemon and client need a build with D2 support. An explicit full system-prompt override still replaces assembled guidance.

For example, the following source uses the configured layout:

````markdown
```d2
client -> api: request
api -> database: query
```
````

Explicit `mermaid` fences still render when `features.mermaid` is enabled. If the preferred engine is disabled, model guidance uses the other enabled engine. Setting both feature flags to false removes diagram guidance. This preference doesn't convert Jcode-generated swarm graphs or translate diagram languages.

## Local rendering limits

D2 runs on the TUI client's machine, not a remote rendering service. Compilation runs outside the UI frame, uses standard input, and has a host-enforced deadline. `d2_timeout_secs` is clamped to 1 through 30 seconds. Source is limited to 64 KiB and SVG output to 8 MiB. Unsupported source, missing tools, compiler failures, and unavailable native terminal graphics leave source available rather than a blank reserved image.

Automatically rendered fences support a conservative, self-contained subset. Imports, icons, image nodes, substitutions, wildcard globs, backslash escapes, and block labels are rejected before D2 runs. The guard rejects `@`, `\`, `$`, `|`, `*`, `?`, and case-insensitive `icon` or `image` spellings anywhere in source, including harmless labels and comments. Asset-key checks also lowercase Unicode, remove quotes, and discard non-ASCII characters, so some additional labels can be rejected. Keep labels simple or use another rendering workflow for richer D2 files. The restriction prevents fenced content from reading local assets or fetching images while compiling.

The child runs without inherited D2 option overrides or external layout-plugin discovery. SVG image resources are also disabled during rasterization. These checks are not a general-purpose sandbox for an arbitrary executable configured as `d2_binary`.

D2 references: `https://d2lang.com/tour/layouts/` and `https://d2lang.com/tour/tala/`.
