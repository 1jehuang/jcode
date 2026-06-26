# Explorer: onboarding "searched, not found" rows (+ scroll)

Area: On jcode onboarding decision rows, ALSO render the list of things we
searched for but did **not** find, beneath the row, with scrolling when it
overflows the small centered welcome area.

This doc is exploration only. No production code changed. All anchors are real
symbols/paths in this repo.

---

## 0. TL;DR / recommended approach

- The auth probes today only *return what was found*
  (`pending_external_auth_review_candidates`, `external_auth.rs:209`). They never
  surface "we looked here and found nothing". To show "searched / not found" we
  must enumerate the **full search space** ourselves and subtract the found set.
- Add a pure, side-effect-free `external_auth_search_report()` in
  `jcode-app-core/src/external_auth.rs` that returns both `found:
  Vec<ExternalAuthReviewCandidate>` and `not_found: Vec<AuthSearchTarget>`. It
  reuses the existing per-source `*_exists()` / path helpers so it stays in lock
  step with the real detectors.
- Capture the `not_found` list once, when the flow arms the import walkthrough
  (`begin_onboarding_flow_at_login`, `onboarding_flow_control.rs:162`), and store
  it on `ImportReview` so it lives exactly as long as the decision rows.
- Render it under the existing Yes/No row in
  `ui_onboarding.rs::welcome_body_lines` (the `OnboardingWelcomeKind::Login` arm,
  lines 81-184) as a dedicated, **independently scrollable** sub-paragraph.
- Scrolling: keep the simplest mechanism that fits the current "centered
  `Paragraph`" design - a `Paragraph::scroll((offset, 0))` over just the
  not-found block, with a `u16` scroll offset stored on `App`
  (`onboarding_notfound_scroll`) and clamped to content height. Drive it with
  `PgUp`/`PgDn` and `Ctrl-U`/`Ctrl-D` (NOT Up/Down/j/k - those already toggle
  Yes/No).

This generalizes cleanly: ScrollWM (and any future probe) becomes one more
`AuthSearchTarget`/`SearchTarget` entry with `found=false`.

---

## 1. Full inventory: what `pending_external_auth_review_candidates` searches for

`jcode-app-core/src/external_auth.rs:209` runs six families of probe and pushes
an `ExternalAuthReviewCandidate` only when a usable, **unconsented** credential
is present. Below is every artifact it looks at, the path, the function that
decides "present", and how to know "not found".

Paths are sandbox-aware: all `crate::storage::user_home_path(...)` lookups honor
`JCODE_HOME` (the onboarding sandbox / tests), so the doc shows the real-home
form. The Copilot/Cursor helpers special-case `JCODE_HOME` explicitly.

### 1a. Shared external sources (`auth::external::unconsented_sources`)
`external.rs:63`, iterating `SOURCES = [OpenCode, Pi]` (`external.rs:45`):

| Source | Path (`ExternalAuthSource::path`, `external.rs:37`) | "present" gate | "not found" signal |
|---|---|---|---|
| OpenCode `auth.json` | `~/.local/share/opencode/auth.json` | `path.exists()` && `!source_allowed` && `source_has_supported_auth` | file absent, or parses but holds no supported provider/API key |
| pi `auth.json` | `~/.pi/agent/auth.json` | same | same |

These are multi-provider files: `source_provider_labels` (`external.rs:71`)
scans each for OpenAI/Codex, Claude, Gemini, Antigravity, GitHub Copilot, and
OpenRouter/API-key providers. `source_has_supported_auth` (`external.rs:239`) is
the catch-all "is there anything importable here" check. For the **not-found**
UI we treat the *file family* (OpenCode / pi) as the search target; whether a
given provider key is inside is a secondary detail.

`source_allowed` (`external.rs:178`) means already-consented -> not a *pending*
candidate. See §1g for how consent interacts with "not found".

### 1b. Codex legacy (`auth::codex`)
- Path: `~/.codex/auth.json` (`legacy_auth_path`, `codex.rs:117`).
- Present: `legacy_auth_source_exists()` (`codex.rs:155`, just `path.exists()`).
- Pending candidate: `has_unconsented_legacy_credentials()` (`codex.rs:161`) =
  `exists && !legacy_auth_allowed()`.
- **Not found**: `legacy_auth_source_exists() == false`.

### 1c. Claude Code (`auth::claude`)
- Path probed by onboarding: `~/.claude/.credentials.json`
  (`claude_code_path`, `claude.rs:173`).
- `preferred_external_auth_source()` (`claude.rs:362`) actually checks
  `[ClaudeCode, OpenCode]` in order, but `external_auth.rs:234` only pushes a
  candidate when the winner is the **ClaudeCode** variant (OpenCode-anthropic is
  covered by §1a). So for this row the search target is the Claude Code file.
- Pending: `has_unconsented_external_auth()` (`claude.rs:371`) returns
  `Some(ClaudeCode)` when present and not consented.
- **Not found**: `claude_code_path()` does not exist (i.e.
  `preferred_external_auth_source()` is `None` or resolves to OpenCode only).

### 1d. Gemini CLI (`auth::gemini`)
- Path: `~/.gemini/oauth_creds.json` (`gemini_cli_oauth_path`, `gemini.rs:169`).
- Present: `gemini_cli_auth_source_exists()` (`gemini.rs:173`).
- Pending: `has_unconsented_cli_auth()` (`gemini.rs:179`) = `exists && !allowed`.
- **Not found**: `gemini_cli_auth_source_exists() == false`.

### 1e. GitHub Copilot (`auth::copilot`)
`preferred_external_auth_source()` (`copilot.rs:351`) checks, in order:
1. `~/.copilot/config.json` (`ConfigJson`; `copilot_cli_dir`, `copilot.rs:425`)
2. `~/.config/github-copilot/hosts.json` (`HostsJson`; `legacy_copilot_config_dir`, `copilot.rs:434`, honors `XDG_CONFIG_HOME`)
3. `~/.config/github-copilot/apps.json` (`AppsJson`)
4. OpenCode `auth.json` w/ copilot oauth (excluded here - belongs to §1a)
5. pi `auth.json` w/ copilot oauth (excluded here)

`external_auth.rs:254` pushes a candidate only when the winner is **not**
`OpenCodeAuth | PiAuth`. Pending: `has_unconsented_external_auth()`
(`copilot.rs:385`).
- **Not found**: none of `config.json` / `hosts.json` / `apps.json` exist (the
  three Copilot-native files), i.e. preferred source is `None` or only the
  shared OpenCode/pi variants.

### 1f. Cursor (`auth::cursor`)
`preferred_external_auth_source()` (`cursor.rs:145`) prefers:
1. `~/.cursor/auth.json` (macOS) (`cursor_auth_file_path`, `cursor.rs:359`;
   Windows `%APPDATA%/Cursor/auth.json`, Linux `~/.config/cursor/auth.json`).
2. Cursor IDE `state.vscdb` (`cursor_vscdb_paths`, `cursor.rs:235`), macOS:
   `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb` (+
   lowercase `cursor` variant); Linux `~/.config/Cursor/...`; Windows
   `%AppData%/Roaming/Cursor/...`.
- Pending: `has_unconsented_external_auth()` (`cursor.rs:160`).
- **Not found**: `preferred_external_auth_source() == None` (neither auth.json
  nor any vscdb path exists).

### 1g. Important nuance: "not found" vs "found but already trusted"

The pending-candidate probes fold two axes together: **presence** and
**consent** (`source_allowed` / `*_allowed`). For the "searched, not found" UI we
want presence only:

- "Searched, **not found**" = the credential artifact does not exist at all.
- A file that exists but was already consented is *found and imported*, not "not
  found"; it simply won't appear as a pending row. (Optionally render it as a
  third state "already trusted" later - out of scope for the first PR.)

So the not-found detector must call the `*_exists()` / `preferred_*().is_some()`
helpers, **not** the `has_unconsented_*` helpers (which return `None` for the
already-trusted case and would mislabel a trusted login as "not found").

### 1h. Canonical search-target list (what the UI enumerates)

| family id | display | representative path | present check |
|---|---|---|---|
| `codex` | Codex (`~/.codex/auth.json`) | `codex::legacy_auth_file_path()` | `codex::legacy_auth_source_exists()` |
| `claude_code` | Claude Code | `~/.claude/.credentials.json` | claude code path exists |
| `gemini_cli` | Gemini CLI | `gemini::gemini_cli_oauth_path()` | `gemini::gemini_cli_auth_source_exists()` |
| `copilot` | GitHub Copilot CLI | `~/.copilot/config.json` (+hosts/apps) | any copilot-native file exists |
| `cursor` | Cursor | `~/.cursor/auth.json` or `state.vscdb` | `cursor::preferred_external_auth_source().is_some()` |
| `opencode` | OpenCode | `~/.local/share/opencode/auth.json` | `ExternalAuthSource::OpenCode.path().exists()` |
| `pi` | pi | `~/.pi/agent/auth.json` | `ExternalAuthSource::Pi.path().exists()` |
| `scrollwm` *(future)* | ScrollWM | `~/Library/Application Support/ScrollWM/control.sock` / app bundle | socket/app present |

`not_found = { target | !present(target) && target not in found }`.

---

## 2. Data model: searched set vs found set

Add to `jcode-app-core/src/external_auth.rs` (core layer; usable by CLI + TUI):

```rust
/// One artifact the auto-import flow probes for, with whether it was located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSearchTarget {
    /// Stable family id ("codex", "claude_code", "cursor", "scrollwm", ...).
    pub family: &'static str,
    /// Human label for the row ("Codex", "Claude Code", "GitHub Copilot CLI").
    pub label: String,
    /// Representative path we looked at (best-effort; may not be the only one).
    pub path: String,
    /// True when a credential artifact exists (consent is a separate axis).
    pub present: bool,
}

/// Outcome of a single import-detection sweep: what we found and what we
/// looked for but did not find. `found` mirrors today's
/// `pending_external_auth_review_candidates`; `not_found` is the new part.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExternalAuthSearchReport {
    pub found: Vec<ExternalAuthReviewCandidate>,
    pub not_found: Vec<AuthSearchTarget>,
}

pub fn external_auth_search_report() -> Result<ExternalAuthSearchReport> { ... }
```

Implementation shape (pure, no mutation, no network):

1. `found = pending_external_auth_review_candidates()?` (unchanged behavior).
2. Build the canonical target list (§1h). For each family compute `present` via
   the existing `*_exists()` / path helpers.
3. `not_found = targets.into_iter().filter(|t| !t.present).collect()`. (We key
   "not found" purely on presence; a present-but-trusted source is excluded from
   `found` candidates anyway and can be added as a separate "already trusted"
   list later.)
4. Keep `pending_external_auth_review_candidates()` as the thin public API the
   rest of the code already uses; `external_auth_search_report` is additive.

TUI side - the not-found list must outlive a single render and be available to
the stateless `ui_onboarding` renderer via `TuiState`:

- Extend `ImportReview` (`onboarding_flow.rs:62`) with:
  ```rust
  /// Auth/source families we probed but did not find, for the
  /// "Searched, not found" panel under the decision row.
  pub(crate) not_found: Vec<crate::external_auth::AuthSearchTarget>,
  ```
  Populate it in `ImportReview::new` from the same report. Even when
  `found.is_empty()` we may still want to show not-found, so also see §5 note on
  the `LoginOpenAi` arm.
- Add a small render-friendly snapshot to `OnboardingWelcomeKind::Login` /
  `LoginImportPrompt` (`mod.rs:641` / `:668`), e.g.
  `pub not_found: Vec<NotFoundRow>` where `NotFoundRow { label: String, path:
  String }`, populated in `onboarding_welcome_kind`
  (`state_ui_input_helpers.rs:1151`).
- Scroll offset is *view* state, so it lives on `App`, not in the flow model:
  `onboarding_notfound_scroll: u16` (init `0` in both `App` constructors,
  `tui_lifecycle.rs:360` and `:738`). Expose it through `TuiState`
  (`onboarding_notfound_scroll()` + the not-found rows accessor) mirroring the
  existing `onboarding_welcome_kind` wiring (`tui_state.rs:1586`, `mod.rs:424`).

---

## 3. Render sketch (under the decision row)

Today the `Login` arm (`ui_onboarding.rs:81-184`) builds centered `Line`s ending
with the Yes/No row and the hint/countdown lines, then the whole `Vec<Line>` is
drawn as one centered `Paragraph` (`draw_onboarding_welcome`, `:355`). The
not-found block should sit **beneath** that, and because it can overflow it needs
its own scroll region rather than being folded into the single centered
paragraph.

Proposed layout change in `draw_onboarding_welcome`: when the welcome kind is
`Login { not_found }` and `!not_found.is_empty()`, split off a bottom band:

```
[ top pad ]
[ telemetry header ]
[ donut ]
[ body: title ... Import X? ... Yes/No ... hints ]   <- existing centered Paragraph
[ gap ]
[ "Searched, not found:" panel ]                     <- NEW, scrollable
```

Panel content (`build_not_found_lines`):

```
Searched, not found:
  • Cursor            ~/.cursor/auth.json
  • Gemini CLI        ~/.gemini/oauth_creds.json
  • GitHub Copilot    ~/.copilot/config.json
  • pi                ~/.pi/agent/auth.json
  ↓ 3 more   (PgDn / Ctrl-D to scroll)
```

- Header line dim+italic; each entry dim, with a bullet + label + faded path.
- The panel is its own `Rect` of height `H = min(rows_needed, available_band)`.
- Centered to match the rest, but left-aligned *within* the band reads better for
  a list; either is fine. Keep the existing `Alignment::Center` for v1 to match
  the card aesthetic, or left-align the panel only.

---

## 4. Scrolling design (state + keys + widget)

### Widget: `Paragraph::scroll` (recommended, minimal)
The current code is already `Paragraph`-based, so the smallest viable mechanism
is `Paragraph::new(not_found_lines).scroll((offset, 0))` into the panel `Rect`.
- Pros: zero new widget state machinery, matches existing rendering, trivial to
  clamp.
- Cons: we compute clamping ourselves (offset must be bounded by
  `content_height.saturating_sub(panel_height)`).

Alternative considered: `List` + `ListState` + `Scrollbar`. Gives built-in
offset tracking and a visible scrollbar, but adds a stateful widget and a
`ListState` to thread through `TuiState` for a read-only, non-selectable list.
Overkill for v1; revisit if we later want per-row selection (e.g. "retry this
probe"). Recommendation: ship `Paragraph::scroll`, optionally add a thin
`Scrollbar` overlay on the panel `Rect` for the affordance (it's stateless given
`content_len`, `position`, `viewport_len`).

### State
- `App.onboarding_notfound_scroll: u16` (offset in lines). Reset to `0` whenever
  the reviewed candidate advances (`commit_current`) or the phase leaves
  `Login`, so each row starts at the top of its (shared) not-found list.
- Clamp on render: `offset = offset.min(max_offset)` where
  `max_offset = content_height.saturating_sub(panel_height)`.

### Keys
Handled in the import-review key path
(`handle_onboarding_import_review_key`, `onboarding_flow_control.rs:417`) BEFORE
the Yes/No movement keys, and also in the `import.is_none()` recovery branch so
scrolling works even when the list shows after all candidates are declined.

Avoid collisions: `Up/Down/j/k/Tab` already toggle Yes/No; `Left/Right/h/l` set
Yes/No. Use a disjoint set for the not-found scroll:

| Key | Action |
|---|---|
| `PgDn` / `Ctrl-D` | scroll not-found down (offset += page/half-page) |
| `PgUp` / `Ctrl-U` | scroll not-found up |
| (optional) `Ctrl-E` / `Ctrl-Y` | line down / line up |

Each returns `true` (consumed) only when the not-found list is non-empty and
actually overflows; otherwise fall through so the keys keep any global meaning.
The dispatch already routes through `handle_onboarding_continue_prompt_key`
(`input.rs:2061`, remote `key_handling.rs:272`), so no new dispatch site is
needed - just add the branch inside the existing handler.

---

## 5. Risks / edge cases

- **Consent axis confusion** (§1g): must detect not-found via presence helpers,
  not `has_unconsented_*`. Otherwise an already-trusted Codex login shows up as
  "not found", which is wrong and alarming. This is the single biggest
  correctness trap.
- **`path()` returns `Result`**: several path helpers can error (no `$HOME`).
  Render best-effort (`unwrap_or` the relative path string); never panic.
- **Sandbox/`JCODE_HOME` divergence**: Copilot/Cursor special-case `JCODE_HOME`
  (`copilot.rs:425/434`, `cursor.rs:383`); use the same helpers so the displayed
  path matches what was actually probed in a sandbox/test.
- **`LoginOpenAi` / no-imports path**: when nothing is found,
  `begin_onboarding_flow_at_login` builds `import = None` and shows
  `LoginOpenAi` (`onboarding_flow_control.rs:177`, `:269`). That's exactly the
  case where "searched, not found" is most useful (everything is not-found).
  v1 can scope to the `Login{import:Some}` arm; a fast follow should also thread
  the not-found list into `LoginOpenAi` so the empty-handed first run still shows
  what was checked.
- **Vertical budget**: the centered card already shrinks the donut to fit
  (`:368`). Adding a panel competes for rows; clamp panel height and rely on
  scrolling rather than letting it push the Yes/No row off-screen. Guard the
  `area.height < N` small-terminal fallback (`:356`).
- **Probe cost / flicker**: compute the report once at flow start (it touches the
  filesystem), store it on `ImportReview`; do NOT re-probe every frame inside
  `welcome_body_lines` (that runs each render and would hit disk on the draw
  path). Cursor's vscdb path even shells out to `sqlite3` for *loading* - the
  presence check is only `path.exists()`, so keep it to existence, never load.
- **Key collisions**: `Ctrl-U` is a common "clear input" binding elsewhere; since
  onboarding consumes keys via `handle_onboarding_continue_prompt_key` before the
  global handlers (`input.rs:2061`), scoping the scroll keys to the active Login
  phase avoids hijacking them globally. Verify against `input.rs` global
  shortcuts before binding `Ctrl-U/Ctrl-D`; `PgUp/PgDn` are the safer default.
- **Remote mode parity**: the welcome kind is rendered from `TuiState`, so this
  works in remote mode as long as the not-found rows are snapshotted into
  `OnboardingWelcomeKind` (don't reach into local-only state from the renderer).

---

## 6. Minimal first PR

Scope: show + scroll a "Searched, not found" panel for the import-walkthrough
decision rows only (the `OnboardingPhase::Login { import: Some }` case).

1. `jcode-app-core/src/external_auth.rs`: add `AuthSearchTarget`,
   `ExternalAuthSearchReport`, and `external_auth_search_report()` built on the
   existing presence helpers (§1h). Add a unit test asserting that a sandbox with
   only `~/.codex/auth.json` present yields `found=[codex]` and `not_found`
   contains claude_code/gemini_cli/copilot/cursor/opencode/pi.
2. `onboarding_flow.rs`: add `ImportReview.not_found: Vec<AuthSearchTarget>`,
   populate in `ImportReview::new` (callers in `begin_onboarding_flow_at_login`,
   `onboarding_flow_control.rs:168`, pass the report's `not_found`).
3. `mod.rs`: extend `LoginImportPrompt` with `not_found: Vec<NotFoundRow>`;
   populate in `onboarding_welcome_kind` (`state_ui_input_helpers.rs:1151`).
4. `app.rs` + `tui_lifecycle.rs`: add `onboarding_notfound_scroll: u16` (init 0);
   `TuiState` accessor (`tui_state.rs`, default `0` in `mod.rs:424`).
5. `ui_onboarding.rs`: render the panel beneath the body via a dedicated
   `Paragraph::scroll((offset,0))` `Rect`, with clamp + a "↓ N more" / scrollbar
   affordance; reserve the band in `draw_onboarding_welcome`.
6. `onboarding_flow_control.rs`: in `handle_onboarding_import_review_key` add
   `PgUp/PgDn` (+`Ctrl-U/Ctrl-D`) -> adjust `app.onboarding_notfound_scroll`,
   clamped; reset to 0 on `commit_current`. Add a test that PgDn increments and
   clamps, and that it does not disturb the Yes/No highlight.
7. Golden/render test: extend `tests/onboarding_golden.rs` /
   `tests/onboarding_flow.rs` to assert the "Searched, not found:" header and at
   least one absent source appear under the decision row.

Explicitly out of scope for PR1 (fast follows): the `LoginOpenAi` empty-handed
arm, an "already trusted" third state, and the ScrollWM probe row (slots in as
one more `AuthSearchTarget` once §1h's `scrollwm` present-check exists).
