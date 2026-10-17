Jcode needs first-class custom harness support in Xirp. This PR records the
integration contract and the discovery evidence needed to decide whether that is
possible.

## Result: XJ-01 answered negatively, with direct evidence

Xirp has **no supported custom-harness registration contract**. The harness set
is compiled into the bundled `@chirp/squab` 0.11.11 orchestrator as exactly five
entries: claude, codex, gemini, pi, cursor.

Five independent lines of evidence, all read-only:

1. `squab --available-harnesses` returns schema `squab.available-harnesses/v3`
   with `"count": 5`.
2. `xirp api send agents:list` returns the identical catalog, all entries
   `"visibility": "public"`.
3. The daemon message catalog exposes only `agents:list` plus
   `harness:install|update|uninstall` (+ `:cancel`). There is no create, register
   or add message type.
4. Harness selection is encoded as reserved argv flags (`--launch-<agent>`,
   `--move-session-to-<agent>`), which the orchestrator rejects as user-supplied
   target flags.
5. A binary scan of the 44 MB app bundle for `registerHarness`, `customHarness`,
   `addHarness`, `agentRegistry` and `harnessRegistry` returned zero matches. An
   unknown agent fails with `no session adapter registered for agent "<name>"`.

Consequently XJ-02, XJ-04 and XJ-05 cannot be implemented against this build.
They require a change inside Spotify's orchestrator.

## Session detection is also unsupported

Empirically tested rather than assumed:

| Command | Result |
|---|---|
| `xirp session import jcode --preview` | `No transcript import adapter is available for jcode` |
| `xirp session import claude --preview` | `No claude session found` |

`claude` fails *after* adapter resolution; `jcode` fails *at* it. Per-agent
`sessionRoot(cwd)` paths are hardcoded per harness, so a Jcode session started
outside Xirp is invisible: it will not appear in `xirp session list`, cannot be
imported or attached, and cannot be uploaded to Portal via transcript.

## Jcode's side is ready

`jcode acp` (2,195 lines, ACP protocol version 1) answers a live `initialize`
handshake with `agentInfo {name: "jcode", version: "0.97.0"}`, `loadSession`,
`session/resume`, `session/close` and `prompt.embeddedContext`.

This is not sufficient on its own: Xirp exposes no ACP client surface. A vendor
`AgentSessionAdapter`, or a generic ACP-backed harness entry, is required.

## MCP transport

The README's stdio-only claim is confirmed in source
(`crates/jcode-base/src/mcp/protocol.rs` retains only `is_stdio()` and logs a skip
line). `jcode acp` independently advertises `mcpCapabilities {http: false, sse:
false}`, and `jcode mcp list` shows a live `agileplus [http]` entry being skipped.
Native remote MCP is therefore ruled out; a maintained stdio-to-remote bridge is
the only available route to Portal MCP.

## What this PR does NOT do

- Does not rename Jcode to a supported agent name.
- Does not forge or shim a transcript format.
- Does not implement an adapter against guessed flags or schemas.
- Does not send the drafted vendor question. It is prepared in-repo only.

Any of the first two would corrupt the provenance Xirp records, which is worse
than an honest "not registered" state.

## Validation

- Probe `scripts/probe_xirp_jcode.py` executed on the owner's Mac: both CLIs
  present, exit code 0, no timeouts.
- ACP `initialize` verified against the installed binary with empty stderr.
- Repo inventory refreshed read-only: **46** non-zz repos (29 public, 17
  private, 14 archived, 13 forks, 32 active). This is not the 42 recorded in an
  earlier checkpoint; the root cause is documented in the resume record.
- Committed files verified against remote read-back after each push.
- Docs-only changes. This branch has **no** workflow runs; Actions spending
  remains zero.

## Next evidence needed

A vendor answer on whether a custom harness adapter can be registered, ideally
via a generic ACP client so any ACP-compliant agent works without a bespoke
adapter. A concrete drafted question is in `docs/XIRP_CUSTOM_HARNESS.md`.

---

**Supersedes #67 — but the original was already on master.** The original
PR #67 was integrated to `KooshaPari/KCode:master` via the
harness-compliant squash-merge path at `fed23782d` (signed by the test
harness `jcode-test@local`), which bypassed GitHub's merge button.
`gh pr view 67` therefore shows `state=CLOSED, mergedAt=null,
mergeCommit=null`, which *looks* like the original was abandoned — it
was not. The body update applied via `gh pr edit 67` and the "Closes
#67" footer on this PR were both written under that misread and are
replaced here.

**This PR is the follow-up additions** on top of the now-merged PR #67
content: §18–§20 in `STATE.md` (operator-action record), `§8.2.e` in
`APPENDICES.md` (just-grade 17/17 PASS evidence), the
`vendor-question-draft.md` pasteable draft, and
`pr-67-reopen-decision.md` (which recommended the now-superseded
"open a new PR" path; retained for record only). Conflict resolution
on this PR was `git checkout --ours` for the 8 add/add files; master
was the older snapshot of our session record.