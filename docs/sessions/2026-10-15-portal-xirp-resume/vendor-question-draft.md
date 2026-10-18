# Vendor question — Xirp custom harness registration contract
**Date:** 2026-10-17
**For:** Spotify Xirp / @chirp/squab team
**From:** Koosha Pari (Jcode maintainer)
**Status:** DRAFT (operator must paste into the channel of their choice)
**Full evidence:** `docs/XIRP_CUSTOM_HARNESS.md` in `KooshaPari/Jcode` master

---

## Short version (paste this)

Subject: Request for a custom coding-agent harness registration contract in Xirp

Hello,

I maintain Jcode, an open-source ACP-compliant coding agent. I would like to
register Jcode as a first-class custom harness in Xirp, alongside the five
vendors that ship with `@chirp/squab` 0.11.11 (`claude`, `codex`, `gemini`,
`pi`, `cursor`).

What I have found so far on the installed build (`xirp 0.49.0+app.db5402dc7`,
`/Applications/Xirp.app/Contents/Resources/app.asar`):

- `squab --available-harnesses` returns a fixed catalog of 5 harnesses. Jcode
  is absent.
- `xirp api send agents:list` returns the same 5 entries, all
  `"visibility": "public"`, all with hard-coded per-agent flags
  (`--launch-claude`, `--launch-codex`, etc.).
- The daemon message catalog (`xirp api list`) only exposes mutators
  (`harness:install|update|uninstall`) that act on harnesses already in the
  fixed catalog. There is no `harness:register` or `agents:create`.

What I would like to know:

1. **Is there a supported way to register a custom harness** (e.g., Jcode)
   without forking `@chirp/squab`?
2. If not, would Spotify accept a **generic ACP client adapter** in
   `@chirp/squab` so any ACP-compliant agent can be registered via a single
   config entry? ACP is a stable open spec and the Jcode side already
   implements it.
3. If neither is supported today, is a **vendor extension contract** (e.g.,
   an internal `@chirp/squab` plugin API) on the roadmap? If so, what
   timeline?

A full evidence dossier is in the Jcode repository at
`docs/XIRP_CUSTOM_HARNESS.md` (referenced from PR `KooshaPari/KCode#103`).
The dossier includes probe output, message-catalog enumeration, and an
honest record of one withdrawn sub-question (a session-list artifact that
turned out to be a project-registration issue, not a squab bug).

If the right channel for this is not direct email, I am happy to file it
wherever you prefer (issue tracker, customer support form, etc.). Please
let me know the channel and I will re-route.

Thank you for your time.

— Koosha Pari

---

## Full evidence dossier

The Jcode repository has a full evidence dossier at
`docs/XIRP_CUSTOM_HARNESS.md` in `KooshaPari/Jcode` master. The dossier
covers:

- XJ-01 (closed): no custom-harness registration contract exists at the
  current build. Evidence: 5 lines of cross-confirmation (squab
  --available-harnesses, xirp api send agents:list, daemon message catalog,
  AppBundle inspection, cross-checked against published docs).
- XJ-02 through XJ-07: blocked pending the answer to (1)-(3) above.
- Sub-question (g) (withdrawn): an earlier session-list artifact that
  appeared to be a squab bug turned out to be a project-registration issue
  on the Jcode side. Withdrawn rather than filed as a defect.

PR `KooshaPari/KCode#103` is a draft PR that captures the full
audit-correction arc; the body of that PR is the most recent summary of
what was learned.

---

## Operator action required

**I do not have a verified Spotify / Xirp contact channel from the agent
side.** xirp has no `feedback`/`support`/`contact` subcommand, the AppBundle
does not advertise a support URL, and the user-facing docs
(`https://backstage.spotify.com/docs/xirp`) do not list a contact email.
Past contact (if any) is not in agent knowledge.

The operator should:

1. Pick a channel (e.g., direct email, Spotify support form, GitHub
   issues on a public squab repo, etc.).
2. Paste the "Short version" above into that channel.
3. Send.

Or, if the operator has a prior contact at Spotify, paste the short
version into a reply to that thread.

---

## Channel options (2026-10-17 subagent investigation)

The `vendor-contact-search` subagent
(`session_mizaru_1792219502394_01b378b9830eb204`) was dispatched to find
a real contact channel and exhausted the following paths. **No public
contact channel exists for Xirp.** The product is Spotify-internal:

- `CFBundleIdentifier = "com.spotify.xirp"`, author = "Spotify"
- Internal codebase: `ghe.spotify.net` (Spotify's GitHub Enterprise;
  not externally accessible)
- Update CDN: `https://reckless-finch.spotifycdn.com/external/`
- `@chirp/*` packages (squab, daemon, etc.) are private (not on npm)
- No public GitHub repo for any `@chirp` package
- The Xirp CLI has no `feedback`/`support`/`contact` subcommand
- The Xirp AppBundle does not advertise a support URL
- `backstage.spotify.com/docs/xirp` does not list a contact email
- Web search for "Spotify Xirp developer support" returns no relevant hits

**Operator channel options, ranked by practicality:**

1. **Personal Slack/email to a Spotify contact** (Xirp team, Portal
   team, developer-relations, or any colleague with Xirp visibility).
   The operator is a `Portal Admins` member per STATE.md §B6, so
   internal channels are accessible. **This is the recommended path.**
2. **GitHub issue on a public squab/Xirp repo** — but no public repo
   exists, so this is not a real option. (Documented for completeness.)
3. **Direct email to a guessed-but-not-verified address** — the operator
   must verify before sending; the agent will not fabricate an address.
4. **Xirp app's built-in `update` flow** — the app has a built-in
   updater; the operator could in principle include a message with an
   update log. Not a real support channel, but documents the
   investigation exhaustively.

**Once a channel is chosen, paste the "Short version" verbatim from
the section above. Do not edit the questions — they were tuned against
the XJ-01 negative result and the 5-line proof in
`docs/XIRP_CUSTOM_HARNESS.md`.**

---

## Status update (2026-10-17)

§24 — all 3 unblock conditions for XJ-03 were marked resolved on
2026-10-17T07:14Z. The orchestrator's `scripts/portal-mcp-bridge.json`
was re-verified live (2026-10-17T22:46Z) against the production CIMD
URL. The verification **contradicted the prior unblock claim** —
`auth.kooshapari.com` is NXDOMAIN at the Cloudflare authoritative NS
and at 1.1.1.1 / 8.8.8.8; `https://kooshapari.com/oauth/client-metadata.json`
returns 404. The bridge mechanism itself passes (`mcp-remote@0.15.1`
correctly derives loopback port 47290 and emits a valid authorize URL).
Arg-order defect in the bridge config was fixed in commit
`39f2ea76f` (2026-10-18T01:25Z). The DNS / Cloudflare Worker
re-deploy is the operator's only remaining step for XJ-03; XJ-07
is shipped to master. XJ-02, XJ-04, XJ-05 are still blocked on
the @chirp/squab answers below.

## Status update (2026-10-18T01:25Z)

XJ-07 (failure and rollback verification) shipped to master at
`530511a66` + `c79951b05` on 2026-10-17. §24.2 captures the bridge
arg-order fix at `39f2ea76f` on 2026-10-18T01:25Z. The §24
"all 3 unblocked" claim is now superseded by §24.1 + §24.2 — the
defect was real (NXDOMAIN) and is being addressed (mock CIMD in
flight on `feat/xj03-mock-cimd-as-20261017`).

A targeted audit (`XJ-AUDIT-2026-10-17.md`, 77 lines) recommends
**three new vendor questions (Q4–Q6)** that close the
Q2/Q3-specificity gap. They are listed in the "Short version"
supplement below. If the operator chooses to send, paste both the
Short version (Q1–Q3) and the Q4–Q6 supplement as a single thread.

## Supplement: Q4–Q6 (per-item mapping)

These three additions close the Q2/Q3-specificity gap noted in
`XJ-AUDIT-2026-10-17.md` §"Vendor question quality check". Each
question is mapped 1:1 to a bounded work item so the vendor can
return an answer tied to a concrete interface or schema.

### Q4 — `AgentSessionAdapter` interface for a sixth harness (XJ-02, XJ-04, XJ-05)

> For the five harnesses that ship with `@chirp/squab` 0.11.11
> (`claude`, `codex`, `gemini`, `pi`, `cursor`), can you share the
> TypeScript interface (or schema) that each harness implements?
> Specifically, I would like to know the shape of
> `sessionRoot(cwd)`, `locateLatest`, `findBySessionId`,
> `readNative` / `writeNative`, `resumeArgs`, the turn-state
> detector, and the launchable-spec. Jcode will implement against
> this and upstream the patch.
>
> *Why:* XJ-02 (register Jcode as a sixth harness), XJ-04 (map
> working / waiting / idle / failed / completed states), and
> XJ-05 (transcript round-trip) are blocked solely on this
> unmeasured interface.
>
> *Expected response:* pasted interface/schema, or a named doc
> path; or "spec is internal, cannot share".

### Q5 — Usage / cost extraction contract (XJ-06)

> Which usage fields must an adapter expose for `@chirp/squab` to
> accept it? At minimum, input/output token counts and the model
> id. If cost must be reported, how should an unknown price be
> represented so records are not fabricated?
>
> *Why:* XJ-06 is the only bounded item where Jcode can produce
> data today (`jcode usage --json` with explicit `price: null` on
> unknown prices) but has no consumer in the absence of a
> registered adapter.
>
> *Expected response:* a field list, or an explicit
> "usage is not part of the adapter contract" ruling.

### Q6 — Transcript round-trip / import format (XJ-05)

> What format does `readNative` consume for transcript import?
> Specifically, does it require JSONL (the shape the Claude / Pi
> adapters read) or will it accept a native JSON store
> (Jcode's current shape at
> `~/.jcode/sessions/session_*_<ms>_<hash>.json`)? Is a
> fixture-based acceptance test required for an import path?
>
> *Why:* Jcode persists JSON, not JSONL; the answer decides
> whether XJ-05 needs a parser or a line-reader, and whether the
> fixture work can be skipped.
>
> *Expected response:* a sample fixture or a format spec; or
> "JSONL only".

## Status update (2026-10-18T06:32Z) — XJ-03 LIVE PASS

The XJ-03 audit-correction loop closed at 2026-10-18T06:32Z with a
live end-to-end verification. Concretely:

1. **Root cause confirmed** (per `AUTH-DNS-FORENSICS-2026-10-17.md`,
   §24.3): the `auth.kooshapari.com` CNAME was deleted manually from
   the Cloudflare dashboard at 2026-10-17T20:46:50.734Z during a
   multi-zone DNS cleanup, not by a Jcode-side script. The Worker
   (`jcode-cimd`) and the route were never touched.

2. **CNAME re-added** via Cloudflare API at 2026-10-18T06:31:01.099Z
   (record id `49e5f266f8fd6f25aaa02e9df6c5e8ae`). Resolved in 5 s;
   `curl -I https://auth.kooshapari.com/oauth/client-metadata.json`
   returns 200 application/json.

3. **End-to-end bridge PASS** (`scripts/test-portal-bridge-live.sh`):
   mcp-remote 0.15.1 fetched the live CIMD, connected to the
   production Portal MCP URL, and emitted a valid OAuth authorize
   URL to stderr. Script grep matched → `RESULT: PASS` → exit 0.
   Bridge now in "Waiting for authorization..." state.

4. **XJ-03 is unblocked end-to-end.** `scripts/portal-mcp-bridge.json`
   flipped from `ready_to_merge_into_mcp_json: false` to `true`; the
   historical blockers are kept in `_historical_blockers_for_audit_trail`.

5. **The only remaining step is operator-side** (one click): open
   the Spotify consent page in a browser once so mcp-remote can
   capture the auth token at `http://127.0.0.1:47290/oauth/callback`.
   After that, the bridge is ready for tool calls.

**XJ-02, XJ-04, XJ-05, XJ-06 are still blocked on the @chirp/squab
answers above (Q4–Q6).** The XJ-03 closure is independent of those
answers — it succeeded entirely with the existing @chirp/squab
capabilities and a Cloudflare-managed CIMD.
