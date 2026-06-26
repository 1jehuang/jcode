# Keybinding conflict detection

jcode runs inside a terminal, which runs inside an OS. Both layers can claim a
key chord before it ever reaches jcode (for example Ghostty binding `Ctrl+Tab`
to "next tab", or macOS binding `Cmd+Space` to Spotlight). When that happens, a
jcode keybinding silently does nothing and it is not obvious why.

This feature discovers the key bindings that exist on the machine, compares them
against jcode's own configured bindings, and warns about overlaps.

## What it can and cannot detect

**Can detect (config-declared intercepts):**

- **macOS system shortcuts** read from `com.apple.symbolichotkeys` (Spotlight,
  Mission Control, screenshots, input-source switching, etc.). Only shortcuts
  that are *enabled* are considered.
- **Terminal emulator bindings.** Currently Ghostty, via
  `ghostty +list-keybinds`, which reports the *effective* binding set (built-in
  defaults merged with the user's config). This also catches rewrites such as
  Ghostty mapping `Alt+Left`/`Alt+Right` to word-navigation escape sequences.

**Cannot detect:**

- Ad-hoc remappers (Karabiner-Elements, BetterTouchTool), window managers, or
  global launcher hotkeys that are not stored in a file we read.
- Terminals other than Ghostty (yet). Adding one is a self-contained adapter
  (see "Adding a terminal adapter" below).

It is a snapshot taken at startup, not a live hook, so changes made while jcode
is running are not seen until the snapshot is refreshed.

## How it surfaces

- **`/keys`** prints a full report: detected terminal, discovered binding
  counts, and each conflict tied to the exact `[keybindings]` config field that
  owns it. `/keys refresh` forces a rescan of the machine (otherwise a cached
  snapshot up to a day old is reused).
- **Startup notice.** On launch, if the set of conflicts has *changed* since the
  last time we warned, jcode shows a one-time heads-up pointing at `/keys`. It is
  debounced by a signature of the conflict set, so users are warned once per
  distinct set of conflicts and never nagged on every launch.

## Resolving a conflict

The report names the conflicting jcode action and its config field, e.g.:

```
  ⚠ Ctrl+Tab
      jcode: Switch to next model (keybindings.model_switch_next = "ctrl+tab")
      taken by terminal: next_tab
```

To fix, either:

- rebind the jcode action in `~/.jcode/config.toml` under `[keybindings]`
  (e.g. `model_switch_next = "ctrl+shift+m"`), or
- change the conflicting shortcut in your terminal or OS settings.

## Implementation

All logic lives in `crates/jcode-setup-hints/src/keymap/`:

- `chord.rs` - `KeyChord`, a normalized `(cmd/ctrl/alt/shift + key)` that unifies
  the different key spellings each source uses, plus `KeyChord::parse` for
  jcode's own binding-string grammar.
- `macos_hotkeys.rs` - decode `com.apple.symbolichotkeys`
  `[ascii, keycode, modmask]` triples (pure logic + a thin subprocess wrapper).
- `terminal.rs` - parse `ghostty +list-keybinds` output (pure logic + wrapper).
- `source.rs` - `DiscoveredBinding` and its `KeySource`.
- `conflicts.rs` - enumerate `KeybindingsConfig` as chords, diff against a
  snapshot, and produce `Conflict`s keyed to config fields. `conflict_signature`
  produces the stable signature used for startup debounce.
- `report.rs` - render the human-readable report and the compact status line.
- `mod.rs` - `collect_snapshot` / `refresh_and_save` / `snapshot_cached_or_refresh`
  (persisted to `~/.jcode/keymap-snapshot.json`).

The pure parsing/decoding/diffing functions are unit-tested and do not touch the
machine; only the `read_*` wrappers shell out.

## Adding a terminal adapter

1. Add a `read_<terminal>_keybinds()` in `terminal.rs` (or a sibling module) that
   produces `Vec<DiscoveredBinding>` with `source: KeySource::Terminal`, keeping
   the parser pure and the subprocess/file read thin.
2. Call it from `collect_snapshot()` in `mod.rs`, ideally gated on the detected
   terminal so we do not shell out to tools that are not present.
3. Add unit tests for the parser using sample config/output text.
