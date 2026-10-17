# Xirp custom harness integration: Jcode
Date: 2026-10-14 (evidence refresh: 2026-10-14/15)
Status: XJ-01 answered negatively with direct evidence on the owner's Mac.
XJ-02 through XJ-07 are BLOCKED pending a vendor extension contract. Integration is
not implemented and not verified.

## Intent
Jcode must be a first-class custom harness in Xirp, retaining its native provider
credentials and permission behavior. Merely attaching the Jcode repository to
Portal or running its TUI in a shell does not satisfy this requirement.

## XJ-01 result: no supported custom-harness registration contract exists

This supersedes the earlier "evidence gap" note. The gap is now closed by direct
inspection of the installed application, not by absence of documentation.

Installed build under test:
- `xirp 0.49.0+app.db5402dc7` (`/Users/kooshapari/.local/bin/xirp` -> shell launcher)
- App bundle `/Applications/Xirp.app/Contents/Resources/app.asar` (43,952,738 bytes)

The harness contract lives in a bundled vendor package, not in Xirp's own code:

- `@chirp/squab` version `0.11.11`, binary `squab` (`dist/cli.js`). This is the
  agent orchestrator that actually owns session launch, transcript parsing, and
  cross-harness handoff.

Evidence that the harness set is fixed at compile time:

1. `squab --available-harnesses` returns schema `squab.available-harnesses/v3`
   with `"count": 5`: `claude`, `codex`, `gemini`, `pi`, `cursor`. Jcode is absent.
2. Xirp's own daemon reports the identical catalog. `xirp api send agents:list`
   returns the same five entries, every one `"visibility": "public"`, with
   per-agent flags `--launch-claude`, `--launch-codex`, `--launch-gemini`,
   `--launch-pi`, `--launch-cursor`. Note `cursor` resolves to a `binary` of
   `agent` (the Cursor agent CLI), which is a further reminder that the catalog
   entries are vendor-curated rather than user-declared.
3. The daemon message catalog (`xirp api list`) exposes only `agents:list` and
   `harness:install|update|uninstall` (+ `:cancel` variants). Every one of those
   mutates a harness that is already in the fixed catalog. There is no create,
   register, or add message type.
4. Harness selection is encoded as argv flag parsing in the orchestrator. The
   reserved-flag guard rejects any target flag beginning `--launch-` or
   `--move-session-to-`, and `squab --help` enumerates only
   `--launch-{claude,codex,cursor,gemini,pi}`.
5. A binary-level scan of the 44 MB bundle for registration entry points
   (`registerHarness`, `register-harness`, `customHarness`, `custom-harness`,
   `addHarness`, `agentRegistry`, `harnessRegistry`) returned ZERO matches. The
   adapter lookup is a compiled-in table keyed by agent name; a missing agent
   produces the literal runtime error
   `no session adapter registered for agent "<name>"`.

Conclusion for XJ-01: Xirp supports exactly five hardcoded harnesses. There is
no plugin system, no adapter interface, no config-file harness declaration, and
no ACP client surface exposed to third parties. First-class custom harness
support requires a change inside Spotify's `@chirp/squab` orchestrator.
Re-verified under Xirp 0.54.0 (`xirp 0.54.0+app.39e027324`) on 2026-10-15:
`xirp api send agents:list` still reports exactly five harnesses
(`agentName`: claude, codex, gemini, pi, cursor), and cursor now reports
`"version": "2026.10.12-d47ca18"` (was `null` under 0.49.0); claude reports
`"2.1.197 (Claude Code)"` and codex `"codex-cli 0.162.1"`. No new harness or
adapter entry appeared.

Consequences for the remaining IDs:
- XJ-02, XJ-04, XJ-05 are unimplementable against this build. They depend on
  vendor work in squab's adapter table and launchable-spec table.
- XJ-06 (usage/cost) is partially implementable independently: Jcode can report
  token totals, but squab has no Jcode adapter to receive them from.
- XJ-03 (Portal context) and XJ-07 (failure/rollback) can proceed on the Jcode
  side without waiting on squab, subject to the MCP transport finding below.

Explicitly NOT done and NOT to be done without authorization: do not rename Jcode
to a supported agent name, do not emit a fabricated `claude`/`codex` transcript,
and do not shim `jcode` onto the `claude` binary name to make the catalog light
up. All three would make Xirp display a false provenance record, which is worse
than the honest "not registered" state.

## Prepared vendor question (NOT SENT)
See "Vendor question" below. It is drafted in-repo only. Sending it to Spotify
support requires separate authorization.

## Jcode ACP status (verified working)
- `src/cli/acp.rs` is 2,195 lines and implements ACP protocol version 1.
- It is wired into the CLI as `jcode acp` (subcommand `Acp` in
  `src/cli/args.rs`, module declared in `src/cli/mod.rs`).
- Live handshake against the installed binary returned a valid ACP `initialize`
  result:
  ```json
  {"id":1,"jsonrpc":"2.0","result":{
    "agentCapabilities":{"loadSession":true,
      "mcpCapabilities":{"http":false,"sse":false},
      "promptCapabilities":{"audio":false,"embeddedContext":true,"image":true},
      "sessionCapabilities":{"close":{},"resume":{}}},
    "agentInfo":{"name":"jcode","title":"Jcode","version":"0.97.0"},
    "authMethods":[],"protocolVersion":1}}
  ```
  `agentInfo.name` is `jcode`, which is the correct honest identity and must not
  be changed to impersonate another harness.

So the Jcode side of an ACP integration is ready. The missing half is a squab
adapter that consumes this stream. Jcode cannot supply that.

## MCP transport: README is accurate, native remote MCP is not available
Verified in source, not just docs:
- `crates/jcode-base/src/mcp/protocol.rs` carries the schema fields for remote
  servers (`url`, `headers`, `transport`), each annotated
  "Unused by jcode today".
- The merge path drops them. `merged.servers.retain(...)` keeps only
  `cfg.is_stdio()` and logs
  `MCP: Skipping non-stdio server '<name>' (<transport>); HTTP/SSE transports are not yet supported`.
- `jcode acp` advertises `mcpCapabilities: {"http": false, "sse": false}`,
  independently confirming the gap at the protocol layer.
- Live confirmation: `jcode mcp list` shows `agileplus [http; enabled] no OAuth
  credentials stored`. That entry is defined in `~/.claude.json` as
  `{"type":"http","url":"http://127.0.0.1:8765/mcp"}`. Jcode is skipping it.

Route selection for Portal MCP (XJ-03), per the "native remote vs bridge"
requirement: native remote MCP support does not exist in this fork, so the only
supported option today is a maintained stdio-to-remote bridge in front of
`https://phenotype.spotifyportal.com/api/mcp-actions/v1`, carrying CIMD OAuth or
a scoped static token. Selecting and pinning that bridge is separate work and is
not started here.

## Bounded work
| ID | Work | Acceptance evidence |
| --- | --- | --- |
| XJ-01 | Discover installed Xirp custom-command/adapter contract | CLI help and app version; documented or vendor-confirmed registration schema |
| XJ-02 | Register explicit Jcode identity and launch in a worktree | Launch, interrupt and resume use Jcode; checkout isolation verified |
| XJ-03 | Connect Portal context | Authenticated tool listing and scoped Workspace read; no secrets in repo/config examples |
| XJ-04 | Map working, waiting, idle, failed and completed states | Independent state transitions observed; permission waits remain waits |
| XJ-05 | Preserve transcripts and provenance | Tool calls, results, edits and interrupted/resumed sessions survive round-trip |
| XJ-06 | Capture usage without inventing cost | Token totals reconcile; unknown price remains unknown |
| XJ-07 | Verify failure and rollback | Missing CLI, expired auth, network loss and malformed events fail clearly; unregister restores prior configuration |

First execute XJ-01. Do not write an adapter against guessed Xirp flags, hook
formats or transcript schemas. If Xirp exposes no supported custom harness
contract, obtain that contract from Spotify before implementing XJ-02/04/05.
Jcode-side transport work can proceed separately after choosing an authenticated
MCP design and inspecting the current transport implementation.

## Detection and sync of non-Xirp-launched sessions: NOT supported for Jcode

A separate question from "can Xirp launch Jcode" is "will Xirp notice a Jcode
session that was started outside Xirp". Both answers are no, and the second is
easy to test empirically.

Empirical results on the owner's Mac, all read-only (`--preview` creates nothing):

| Command | Result |
|---|---|
| `xirp session list` | `No sessions found.` |
| `xirp session import jcode --preview --json` | `No transcript import adapter is available for jcode` |
| `xirp session import claude --preview --json` | `No claude session found` (correct failure: adapter exists, no session in cwd) |
| `xirp session import --harness jcode ...` | `--harness does not apply to import; use xirp session attach <session-id> --harness <agent> after importing` |

The difference in wording between rows 2 and 3 is the whole answer. For `claude`
Xirp gets far enough to look for a session and reports that it found none. For
`jcode` it fails earlier, at adapter resolution: there is no transcript import
adapter for that agent name at all. Jcode sessions are therefore invisible to
Xirp's discovery, and cannot be synced, imported, or attached.

Why, mechanically: each supported harness has a squab `AgentSessionAdapter` that
implements `sessionRoot(cwd)`, `locateLatest`, `findBySessionId`,
`readNative`/`writeNative` and `resumeArgs`. Those are hardcoded per agent, for
example `~/.claude/projects/<slug>`, `~/.codex/sessions`, `~/.gemini/projects`,
and pi's `sessions`. There is no generic "scan any directory for a transcript"
fallback and no user-configurable transcript path, so pointing Xirp at Jcode's
own session store is not possible without an adapter.

`xirp session attach` does take a `--harness <agent>` value, but it only accepts
one of the five catalog names, and it attaches an already-imported session rather
than discovering a new one. It additionally requires an interactive terminal, so
it cannot be exercised from a non-TTY context.

Practical upshot: today, a Jcode session started by hand in Ghostty is invisible
to Xirp. It will not appear in `xirp session list`, cannot be uploaded to a
Portal workspace through the transcript path, and its live state is not mirrored.
Anything that looks like "Xirp is tracking my Jcode session" would be an illusion.

Two live Jcode processes were running during these tests, so the negative result
is not explained by absence of sessions.

Not a data problem, only a wiring problem: Jcode does persist transcripts, at
`~/.jcode/sessions/session_<name>_<ms>_<hash>.json`, with an adjacent
`~/.jcode/sessions/edit-stats/` tree. So a future squab adapter would have
something real to read. The transcripts are JSON rather than the JSONL used by
the Claude and Pi adapters, so an adapter would need a real parser, not a
path-and-line-count reader.

## Read-only Mac probe — EXECUTED 2026-10-14
From this checkout:
```sh
python3 scripts/probe_xirp_jcode.py
```
The probe invokes only each installed CLI's --help, with a 15-second timeout.
It does not launch sessions, inspect credential/config files, install packages,
change settings, or upload output. CLI implementations themselves may have
startup side effects; review before execution. Output stays on stdout. Review
output before sharing. The probe cannot establish runtime compatibility.

Result on the owner's Mac: both CLIs present, both exit code 0, no timeouts.
`xirp` help output was 552 bytes and matched the 7-command surface documented by
Spotify. `jcode` help output was 8,320 bytes.

The probe alone could not have answered XJ-01, because `xirp --help` prints no
harness registration surface. Answering XJ-01 required the daemon API
(`xirp api list`, `xirp api send agents:list`) and direct inspection of the
bundled `@chirp/squab` package. Both are read-only operations.

One environmental finding worth recording: `xirp features` fails with
`Could not connect to the daemon at ws://localhost:3849: ECONNREFUSED` while
`xirp api` succeeds against the same daemon. Treat `xirp features` as unreliable
for health checks; use `xirp api send agents:list` instead.

## Vendor question — PREPARED, NOT SENT
Drafted here only. Sending to Spotify requires separate authorization; no
support message has been sent from this work.

> Subject: Request for a custom coding-agent harness registration contract in Xirp
>
> We run Jcode (github.com/KooshaPari/Jcode), an open-source coding agent that
> speaks the Agent Client Protocol (ACP) v1. We would like Jcode to appear as a
> first-class harness in Xirp alongside Claude Code, Codex, Gemini CLI, Pi and
> Cursor, so that Jcode-native sessions get the same launch, interrupt/resume,
> live state, transcript and usage treatment as the built-in harnesses.
>
> What we established by inspecting Xirp 0.49.0 and its bundled `@chirp/squab`
> 0.11.11:
>
> 1. `squab --available-harnesses` and `xirp api send agents:list` both return
>    exactly five harnesses: claude, codex, gemini, pi, cursor. All are
>    `"visibility": "public"`.
> 2. The daemon's message catalog exposes `agents:list` plus
>    `harness:install`/`update`/`uninstall` and their `:cancel` variants. We find
>    no create/register/add message type.
> 3. Harness selection is encoded as reserved argv flags (`--launch-<agent>`,
>    `--move-session-to-<agent>`); no configuration file declares a harness.
> 4. Looking up an unknown agent fails with
>    `no session adapter registered for agent "<name>"`, which suggests a
>    compiled-in adapter table.
>
> We are not asking for guidance on configuration; we have already searched for
> a documented extension point and cannot find one. Our specific questions:
>
> a. Is there a supported, non-vendored way to register an additional harness
>    with Xirp or `@chirp/squab`? If it exists, what is the schema and where is
>    it documented?
> b. If not, is a per-agent `AgentSessionAdapter` (launch spec, transcript
>    reader/writer, resume args, usage extraction, turn-state detector)
>    contribution model planned, and what would such an adapter need to
>    implement to be accepted upstream?
> c. Our agent is already ACP v1 compliant: `jcode acp` returns
>    `agentInfo {name: "jcode", version: "0.97.0"}` with `loadSession`,
>    `session/resume`, `session/close` and `prompt.embeddedContext` support. Is
>    a generic "ACP-backed harness" entry a supported configuration today, or is
>    it on the roadmap? A generic ACP client would let any ACP agent register
>    without a bespoke adapter.
> d. We can supply the Jcode adapter implementation, tests and transcript
>    fixtures upstream if that is useful.
>
> We are explicitly not asking to have Jcode masquerade as an existing harness.
> We will not rename our binary or forge transcripts to match a supported agent,
> because that would corrupt provenance in the records Xirp keeps.
>
> ### Second, smaller question: CIMD client registration on the Portal MCP endpoint
> Separately from the harness question, we want to connect a non-Spotify agent to
> `https://phenotype.spotifyportal.com/api/mcp-actions/v1`. We have verified the
> authorization side works and is not the obstacle:
>
> - `/.well-known/oauth-authorization-server` and
>   `/.well-known/oauth-protected-resource/v1` are both published and consistent.
> - PKCE with `S256` is supported, and `token_endpoint_auth_methods_supported`
>   includes `none`, so a public client with no secret is viable.
> - There is no `registration_endpoint`, so dynamic client registration is not
>   offered; CIMD looks like the intended path.
> - Presenting the allow-listed `https://xirp.spotify.com/oauth/client-metadata.json`
>   returns 302 to a working consent page. Presenting any other client-metadata
>   URL returns `400 Invalid client_id`, including URLs on hosts that resolve
>   fine. So the gate appears to be the allowed-client-pattern list.
>
> One question, about how to request the change:
>
> f. **What is the intended way to request that a new CIMD URL be added to the
>    allowed-client-pattern list?** We can serve a client-metadata document over
>    HTTPS from a stable public URL with a non-root path, `client_name` of our
>    own, and `token_endpoint_auth_method: none`. We are not asking to reuse
>    Xirp's document, which would attribute our actions to Xirp in your audit
>    trail. We will append rather than replace, since we understand a custom list
>    supersedes the implicit defaults.
>
> *(A previous draft also asked whether `redirect_uri` is compared strictly
> against the published `redirect_uris`. We have since found your own documented
> default for "Allowed redirect URI patterns" — "When no list is configured,
> Backstage allows HTTP loopback redirects on localhost, 127.0.0.1, and [::1],
> using any port and path" — which answers it, so that question is dropped.)*

> ### Third question: WITHDRAWN — it was our error, not your bug
> An earlier draft of this note reported that `xirp session list` returns
> `No sessions found` for a session that `xirp session get` reads fine, and
> offered it as a possible defect on your side. **That report was wrong and is
> withdrawn.** On re-testing, `xirp session list`, `--status running` and `--json`
> all work correctly and enumerate a running session.
>
> What actually happened: `xirp session new --goal "<text>"` needs the current
> directory to be a **registered Xirp project** so it can create a worktree. Run
> from an unregistered path it fails with
> `Failed to create worktree: Project is not a git repository`, exits 1, and
> prints no JSON — so we believed a session had been created when it had not, and
> then read the empty list as an enumeration fault.
>
> After registering the path, `session new` succeeds, `session get` shows
> `Worktree: <path>`, and `session list` lists it. The behaviour is correct. No
> bug to report, and we would rather withdraw a claim than have you chase it.
>
> If a clearer failure mode would help — for instance distinguishing "path is not
> a registered project" from "path is not a git repository", since the current
> message says only the latter — that would be a welcome usability note, but it
> is a suggestion rather than a defect.

## Constraints
GitHub Actions spend is permanently zero. Use local validation or free included
capacity. Preserve the separate Tracera/AgilePlus pilot and specification gates.
No credential minting, permission expansion or automatic transcript upload is
part of this discovery change.

## Sources inspected
- https://backstage.spotify.com/docs/xirp
- https://backstage.spotify.com/docs/xirp/settings
- https://backstage.spotify.com/docs/xirp/sessions
- https://backstage.spotify.com/docs/xirp/workspaces/launching-sessions
- https://backstage.spotify.com/docs/portal/core-features-and-plugins/mcp/connecting-agents
- This fork: README.md, src/cli/mod.rs, src/cli/args.rs, src/cli/acp.rs
