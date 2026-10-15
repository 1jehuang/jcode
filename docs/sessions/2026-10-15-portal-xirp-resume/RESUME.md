# Spotify Portal + Xirp setup — resume record

Last updated: 2026-10-15 (all times UTC unless marked PDT)
Owner: KooshaPari · Host: this Mac (darwin/arm64)
Purpose: single source of truth so a future local session can resume without
reading chat history. Update after every meaningful change.

## How to read this file

**Four files, read in this order:**

- `RESUME.md` (this file) — verified evidence, including everything observed in a
  signed-in Portal session.
- `PORTAL-FINDINGS.md` — row-level detail from that authenticated inspection:
  the 42 Sources rows, the 11 affected repos, the 16 wiki suggestions and their
  accuracy audit, automation runs, the session-launch test, and the admin/CIMD
  discovery.
- `APPENDICES.md` — deeper OAuth, CIMD and bridge-verification records.
- `STATE.md` — continuation half: exact revisions, completed work, remaining
  tasks, blockers, constraints, open questions, broken gates and the evidence
  file map.

Sections separate **verified outcomes** (something observed, with the command
that observed it) from **claims** (believed, not proven) and **blockers**.
Anything in "Claims" must not be reported as working setup.

---

## 1. Verified outcomes (evidence attached)

### 1.1 Installed versions
| Component | Observed value | How |
|---|---|---|
| Xirp CLI | `xirp 0.49.0+app.db5402dc7` | `xirp --version` |
| Xirp app | `/Applications/Xirp.app`, asar 43,952,738 bytes | `ls -la` |
| Jcode CLI | `jcode v0.97.0 (94392e100)` | `jcode --version` |
| KCode fork | `kcode v0.0.0-dev (bb6174b21)` | `kcode --version` |
| Orchestrator | `@chirp/squab` **0.11.11** (bundled inside Xirp) | asar `package.json` |

Jcode launcher chain: `~/.local/bin/jcode` -> symlink ->
`~/.jcode/builds/current/jcode`.

### 1.2 XJ-01 ANSWERED NEGATIVELY — no custom-harness contract exists

This is the single most important finding of this session. Xirp cannot register a
custom harness. Five independent lines of evidence, all read-only:

1. `squab --available-harnesses` -> schema `squab.available-harnesses/v3`,
   `"count": 5` -> claude, codex, gemini, pi, cursor. Jcode absent.
2. `xirp api send agents:list` -> identical 5 entries, all
   `"visibility": "public"`, flags `--launch-{claude,codex,gemini,pi,cursor}`.
3. `xirp api list` -> daemon message catalog contains only `agents:list` and
   `harness:install|update|uninstall` (+ `:cancel`). **No create/register/add.**
4. Reserved argv flags: `--launch-*` and `--move-session-to-*` are owned by the
   orchestrator and rejected as user-supplied target flags.
5. Binary scan of the 44 MB bundle for `registerHarness`, `register-harness`,
   `customHarness`, `custom-harness`, `addHarness`, `agentRegistry`,
   `harnessRegistry` -> **zero matches**. Unknown agent yields the literal error
   `no session adapter registered for agent "<name>"`.

Consequence: XJ-02, XJ-04, XJ-05 are unimplementable against this build. They
require vendor work inside Spotify's `@chirp/squab`. XJ-06 is partly implementable
(Jcode can report tokens) but has no consumer without a squab adapter. XJ-03 and
XJ-07 can proceed on the Jcode side.

**Not done, deliberately:** Jcode was NOT renamed to a supported agent name, no
transcript format was forged, no `jcode`->`claude` binary shim was created. All
three would make Xirp display false provenance.

### 1.3 Jcode ACP adapter is real and working
- `src/cli/acp.rs` = 2,195 lines, `ACP_PROTOCOL_VERSION = 1`.
- Wired as `jcode acp` (subcommand `Acp` in `src/cli/args.rs`; `pub mod acp;`
  in `src/cli/mod.rs`).
- Live handshake returned a valid ACP `initialize` result:
  ```json
  {"id":1,"jsonrpc":"2.0","result":{"agentCapabilities":{"loadSession":true,
   "mcpCapabilities":{"http":false,"sse":false},
   "promptCapabilities":{"audio":false,"embeddedContext":true,"image":true},
   "sessionCapabilities":{"close":{},"resume":{}}},
   "agentInfo":{"name":"jcode","title":"Jcode","version":"0.97.0"},
   "authMethods":[],"protocolVersion":1}}
  ```
`agentInfo.name` = `jcode` is the correct honest identity.

### 1.4 MCP transport: native remote MCP does NOT exist (README confirmed accurate)
Verified in source, not only docs:
- `crates/jcode-base/src/mcp/protocol.rs:208` — `pub url: Option<String>`
  annotated "Unused by jcode today"; `headers` likewise.
- Same file, merge path: `merged.servers.retain(...)` keeps only `cfg.is_stdio()`
  and logs `MCP: Skipping non-stdio server '<name>' (<transport>); HTTP/SSE
  transports are not yet supported`.
- `jcode acp` advertises `mcpCapabilities {"http": false, "sse": false}`.
- Live: `jcode mcp list` shows `agileplus [http; enabled] no OAuth credentials
  stored` — that entry is `{"type":"http","url":"http://127.0.0.1:8765/mcp"}` in
  `~/.claude.json`, and Jcode is skipping it.

Route decision (per "native remote vs maintained bridge" requirement): native is
ruled out by source. The only supported path is a maintained
stdio-to-remote bridge in front of
`https://phenotype.spotifyportal.com/api/mcp-actions/v1`, carrying CIMD OAuth or
a scoped static token. **Bridge not yet selected or pinned.**

### 1.5 Repo inventory REFRESHED — the 42 figure is wrong
Read-only `gh api` pagination over owner repos, filtered `name !~ /^[Zz][Zz]/i`:

| Metric | Observed (2026-10-14) | Prior checkpoint |
|---|---|---|
| Total non-zz | **46** | 42 |
| Public | **29** | 30 |
| Private | **17** | 12 |
| Archived | **14** | 9 |
| Disabled | 0 | — |
| Forks | 13 | — |
| Active (non-archived, non-disabled) | **32** | — |

Raw owned repos before filter: 70. Dropped zz*: 24.
Owner type = **User** (verified `/users/KooshaPari` -> `type: User`). Do not invent
an organization.
Artifacts: `/tmp/xirp-probe-20261015/repo-inventory.json` (22 KB) and
`repo-inventory-summary.txt` (8.9 KB). **These are in /tmp and will not survive a
reboot — re-run or relocate if they matter.**

Jcode immutable repo ID = 1365855438, public, not archived, not disabled.
Confirms the checkpoint value.

Note: the +4 delta vs 42 is NOT explained by this run (read-only; nothing changed).
Most likely the prior figure predated several repos, or applied a fork/archive
exclusion this run did not. Do not assert which without checking Portal's stored
list. Archived repos are reference-only per the Wiki Schema rev 2 requirement.

### 1.5 SOLVED with authenticated Portal access (detail in PORTAL-FINDINGS.md)

Three long-open questions are now answered against a **signed-in** Portal
session, cross-checked row-by-row against live GitHub state:

1. **The 11 unsynced resources are private repos.** The GitHub App grant does
   not cover private repositories, so they get no sync timestamp. This matched
   **41 of 42 rows**. Archived state is *irrelevant* — `herdr-jcode`,
   `herdr-helioslite`, `herdr-forgecode` and `PhenoInfra` are all archived and
   all have fresh timestamps. The lone exception is `PhenoRegistry`, whose
   timestamp is **frozen at `1d`** while its live `pushed_at` is
   `2026-10-15T03:36:03Z` — direct proof the App can no longer read it.
2. **Everything shares one root cause: private-repo access.** The static catalog
   lives inside private PhenoRegistry *and* the App cannot read that repo. One
   fix — granting the App private-repo access — addresses the pinned catalog URL
   and the 11 unsynced rows together.
3. **Coverage is 42 attached / 36 ingested = 85.7%, with 16 pending wiki
   suggestions**, not the "6 sources / 5 suggestions" previously recorded. The
   Oct 14 force sync **finished without producing anything**:
   `Catalog Entities` reads `No catalog entities yet`. The required review was
   then performed. **Three suggestions fail it** (two cite repositories that do not
   exist, one claims `PhenoDesign` is un-ingested when it is synced). Two pass.
   The remaining 11 are identity-clean but their bodies could not be read, so
   they are unreviewed rather than approved. Nothing was accepted; bulk
   `Accept all (16)` was deliberately not used.

Full row-level tables, the 11 affected repos, the 16 suggestion titles, and the
specific disproved claim are in `PORTAL-FINDINGS.md`.

### 1.5f Forward-execution item 9: automations and launch flow, both now tested

Both surfaces had never been inspected. Neither works end to end. Detail in
`PORTAL-FINDINGS.md`.

- **Automations are healthy but barely exercised.** 3 runs, 3 completed, 100%
  success, 0 failing. Wiki Synthesis (Daily 3:00, last 21h) and Wiki Lint
  (Daily 5:00, last 2d). But the wiki has only ever had **2 accepted pages**,
  and Activity's "19 wiki pages changed" counts *suggested* changes, not
  accepted content. 100% success describes a pipeline that has processed almost
  nothing, not proof that Wiki Lint detects real drift.
- **The session launch flow is broken at the Portal boundary.** Local creation
  works: `xirp session new` returned a session id and `xirp session get` reported
  `Status running`, `Activity working`, `Cost $0.0000`. Portal `/sessions` still
  read **`No local sessions yet`** while that session existed and was running.
  The workspace header's **New Session** button is a no-op in the browser despite
  the Xirp app running with a live daemon.
- **CORRECTION: there is no Xirp enumeration defect.** An earlier version of this
  record claimed `xirp session list` returns `[]` for a running session. That was
  my error. The gate is **`--no-worktree`**: `xirp session new --goal` defaults to
  creating a git worktree, and when the project root is not a git repo it prints
  `Failed to create worktree: Project is not a git repository`, **exits 1, and
  emits no JSON**. My test captured that empty output as a session id, so the
  session was never created and the empty list was correct. Verified: the same
  directory with `--no-worktree` creates a session (exit 0) and `session list`,
  `--status running` and `--json` all enumerate it. The vendor question is
  withdrawn. Two of my three successive explanations were wrong; the detail is in
  `PORTAL-FINDINGS.md`.

The disposable test session was deleted and verified gone. No worktree or branch
was created, and the checkout's pre-existing untracked files are untouched.

### 1.6 ROOT CAUSE of the 42-vs-46 discrepancy — SOLVED

The gap is **not** a counting-rule difference. It is three distinct things.

**(a) The static catalog is a PUBLIC-ONLY subset, by design.**
`catalog/spotify-portal/catalog-info.yaml` on `chore/portal-non-zz-catalog-20261009`
lists exactly **31** repository descriptors plus `owner.yaml`. Its own description
says "Public non-zz repository snapshot ... Private records belong only in
Portal." So 31 is a *public* count, and it will never equal the 46 total.

**Correction to an earlier error in this same record:** I first reported
`kcode.yaml` as stale because I compared *filenames* to repo names. That was
wrong. `kcode.yaml` correctly declares
`github.com/project-slug: KooshaPari/Jcode`, repo id `1365855438`, and carries
`phenotype.dev/former-name: "KCode"`. It is accurate. Likewise `github.yaml`
correctly maps to `KooshaPari/.github` (id 1199812615), which my zz-filtered
inventory omitted only because the name begins with a dot. Do not "fix" either.

**(c) Eighteen current repos have no descriptor at all.** All are private or
archived, which is consistent with the public-only design, except `.github`
(see above, it IS described).
- Private, active (6): `HarnessDesk`, `job-cockpit`, `PhenoApps`, `PhenoBot`,
  `PhenoLab`, `shopping-home`
- Private, archived (10): `.audit-run-v37`, `PhenoAI`,
  `phenoDesign-visual-production-v2`, `PhenoMLX-temp`,
  `phenotype-canonicalization`, `phenotype-cvp-audit-2026-09-14`, `Pine`,
  `ResearchLedger`, `SessionLedger`, `Substrate`

**Timing evidence that no new repositories explain the delta.** Newest non-zz
repo `created_at` is `herdr` at 2026-10-09T06:32:17Z, i.e. on or before the Oct 9
snapshot date. So the +4 cannot be new repositories. Four repos (`Pine`,
`ResearchLedger`, `SessionLedger`, `Substrate`) were pushed on 2026-10-13/14 and
are now **archived**, consistent with an archive pass that ran after the
snapshot. Archived count went 9 -> 14.

Rule of thumb: **46 = total owned non-zz; 31 = public static catalog; 42 = Portal
GitHub App selected count, now suspect.**

### 1.6b PR 619 local validation — 29 of 31 clean, 2 real defects

Ran entirely locally against the PR head `8be56b97` (extracted from a tarball, no
CI, $0). No files in the registry repo were modified.

**Passing:**
- All 31 descriptors parse as YAML without error.
- Referential integrity is perfect: all 32 `catalog-info.yaml` targets resolve
  (31 repositories + `owner.yaml`), and there are zero unreferenced descriptors.
- Secret scan across the whole `catalog/spotify-portal/` tree: clean.
- 29 of 31 descriptors match live GitHub on immutable repo id **and** archived
  flag. Archived flags for `herdr-forgecode`, `herdr-helioslite`, `herdr-jcode`,
  `PhenoInfra` are all correctly `true`.

**Two genuine defects to fix in PR 619:**

1. `parpoura-experiment-0.yaml` describes a repository that **no longer exists**.
   Verified live, three ways:
   - `gh api repos/KooshaPari/parpoura-experiment-0` -> HTTP 404. Because `gh` is
     authenticated as the owner, this is **not** a private-repo artifact: it is a
     deleted or never-existed repo.
   - `gh api repos/1409529692` (the claimed immutable id, any owner) -> 404. The
     id does not correspond to anything.
   - `https://github.com/KooshaPari/parpoura-experiment-0` -> 404 with **no
     redirect**, so it was not renamed to an existing repo.
   - The id is plausible and short-lived: `1409529692` sits immediately beside
     `shopping-home` (`1409492961`, created 2026-10-07), the era the Oct 9
     snapshot was taken. So this was almost certainly a real repo that was
     deleted after the snapshot.

   **Correction to my earlier reasoning:** I first guessed it had been renamed to
   `zz-pause-inc-Parpoura`. That is wrong. `zz-pause-inc-Parpoura` has immutable
   id `1164684829`, created 2026-02-23 — a different, unrelated, *private* repo.
   There is exactly one Parpoura-named repo on the account and it is private, so
   the correct disposition is **removal of this descriptor**, not retargeting it
   at the Parpoura repo. Removing it also restores scope consistency: the only
   Parpoura work now lives under the `zz-pause-inc-*` prefix, which the non-zz
   rule excludes.

   When the descriptor was added: commit `b9869102e`, 2026-10-09T18:06:01Z
   ("chore(catalog): add catalog/spotify-portal/repositories/parpoura-experiment-0.yaml").
   Last touched by `00054b8d0`, 2026-10-14T01:31:18Z (the Prettier formatting pass).
2. `phenoregistry.yaml` describes PhenoRegistry, which is **private** on GitHub
   (verified live: `{"id":1201113097,"name":"PhenoRegistry","private":true,
   "visibility":"private"}`), yet it sits in a catalog whose own description says
   "Private records belong only in Portal." The catalog README already
   anticipated this: "PhenoRegistry changed to private after the original
   snapshot; the central descriptor now requires private repository
   authentication. Reconcile current visibility before any publication."

   Severity is **lower than it first looked**, for two verified reasons:
   - `catalog/spotify-portal/` does **not** exist on PhenoRegistry's `main`
     branch at all. `main`'s `catalog/` contains only `RUBRIC-LINEAGE.md`,
     `inventory`, `registry.schema.json`, `registry.yaml`. So publishing this
     descriptor does not publish anything to a public surface; it does not leak
     private metadata today.
   - The descriptor's `default-branch: main` annotation is **correct**
     (verified `default_branch = main`). This is not an annotation error.

   What it does mean: because the whole catalog ships inside a private repo, any
   commit-pinned `catalog-info.yaml` URL Portal is pointed at now requires
   authenticated fetch. That is a plausible contributing cause for the
   "31 resource rows had sync timestamps, 11 did not" observation recorded in the
   catalog README, and for the preview that produced only three Locations and no
   owner Group. It is a hypothesis worth testing, **not** a proven root cause.

   Disposition options, in preference order, pending eyes-on Portal observation:
   (a) keep the descriptor and give Portal's GitHub App authenticated read access
       to PhenoRegistry, which the App may already hold; or
   (b) move the catalog to a public location and exclude PhenoRegistry from it,
       consistent with the "private records belong only in Portal" rule.
   Do **not** simply delete the descriptor: PhenoRegistry is a first-class
   catalog subject and its absence would silently narrow coverage.

**Defect 1 has been FIXED** in PhenoRegistry commit `2b13afb91`: the descriptor
and its `catalog-info.yaml` target entry were removed, the README count was
corrected, and the result re-validated (30 descriptors + owner Group = 31
targets, zero dangling, zero orphans, Prettier clean). Pushed to the PR 619 draft
branch. The PR remains a draft and was not merged.

**Defect 2 (PhenoRegistry privacy) is deliberately NOT "fixed" yet.** Removing the
descriptor would silently narrow coverage of a first-class catalog subject, and
the real issue is that the catalog is unreachable rather than that the descriptor
is wrong. It needs the eyes-on Portal observation described in 1.6c first.

### 1.6c LIKELY ROOT CAUSE of Portal catalog ingestion failure — found

The static Location Portal is pointed at is a **GitHub raw URL into a repository
that is now private**. Proven with a three-way control:

| Probe | Result |
|---|---|
| Anonymous `GET raw.githubusercontent.com/KooshaPari/PhenoRegistry/b5c082a6.../catalog/spotify-portal/catalog-info.yaml` | **404** |
| Anonymous `GET raw.githubusercontent.com/KooshaPari/PhenoRegistry/main/README.md` (control) | **404** — private repo hides everything anonymously |
| Anonymous `GET raw.githubusercontent.com/KooshaPari/Jcode/master/README.md` (positive control) | **200** — public repos still work |
| **Authenticated** `GET /repos/KooshaPari/PhenoRegistry/contents/catalog/spotify-portal/catalog-info.yaml?ref=b5c082a6...` | **200, 1543 bytes** |

So the path and the file are real and intact. They are simply not anonymously
reachable, because PhenoRegistry flipped to `visibility: private` after the
catalog was written. The catalog's own README predicted this: *"PhenoRegistry
changed to private after the original snapshot; the central descriptor now
requires private repository authentication."*

This explains, with one cause, all four previously-unexplained symptoms:

1. **The force-sync that has shown "Syncing" since Oct 14.** If the fetch keeps
   retrying a 404 rather than surfacing a terminal error, a stuck progress state
   is the expected presentation.
2. **The preview that produced only three generated Locations, no owner Group and
   no repository Resources.** Backstage cannot build a Location's target set from
   entities it cannot read.
3. **"31 resource rows showed sync timestamps, 11 did not."** Consistent with rows
   sourced from the GitHub App's authenticated API (which can read private repos)
   rather than from the static catalog fetch.
4. **The catalog-import route returning 404.** Consistent with the importer
   treating the source as unavailable.

Confidence: **high but not proven.** The mechanism is demonstrated; the linkage to
the specific Portal errors is inference from consistency, not observation of the
Portal error text. Confirming it requires reading the actual processor error in
Portal, which is blocked by the browser bridge (B2).

Fix candidates, cheapest first — all require authorization or eyes-on Portal:
- (a) Repoint the static Location at an authenticated GitHub App source instead
  of a raw URL. Preferable if Portal supports it, since it survives future
  privacy changes.
- (b) Host the catalog in a public location and exclude private repositories from
  it, consistent with "private records belong only in Portal". This changes what
  is public and so needs explicit authorization.
- (c) Grant the Portal GitHub App broader access to PhenoRegistry, if the App's
  read-only grant is the limiting factor.

Do not repoint the Location silently. The current pinned URL, the visible
entities, and the two pilot workspaces must be preserved and compared before and
after any change.

### 1.7 Session detection/sync of externally-launched sessions: NOT supported for Jcode

Direct answer to "does Xirp pick up Jcode sessions I started myself?" — **no.**

| Command | Result |
|---|---|
| `xirp session list` | `No sessions found.` |
| `xirp session import jcode --preview --json` | `No transcript import adapter is available for jcode` |
| `xirp session import claude --preview --json` | `No claude session found` |
| `xirp session import --harness jcode ...` | `--harness does not apply to import; ...` |
| `xirp session attach <id> --harness jcode` | requires an interactive terminal |

The wording difference is the proof: `claude` fails *after* adapter resolution
("no session found"), `jcode` fails *at* adapter resolution ("no import adapter
is available"). Two live `jcode` processes were running at test time, so this is
not an absence-of-sessions artifact.

Mechanism: squab adapters hardcode `sessionRoot(cwd)` per agent
(`~/.claude/projects/<slug>`, `~/.codex/sessions`, `~/.gemini/projects`, pi
`sessions`). No generic transcript scanner, no configurable path. Therefore a
hand-started Jcode session cannot appear in `xirp session list`, cannot be
imported, cannot be attached, and cannot be uploaded to Portal via transcript.

This is a wiring gap, not a data gap: Jcode does persist transcripts at
`~/.jcode/sessions/session_<name>_<ms>_<hash>.json` (7,661 entries) plus an
`edit-stats/` tree. Format is JSON, not the JSONL the Claude/Pi adapters read, so
a future adapter needs a real parser.

### 1.8 Xirp daemon quirk worth knowing
`xirp features` fails with
`Could not connect to the daemon at ws://localhost:3849: ECONNREFUSED`
while `xirp api ...` succeeds against the same daemon. Use
`xirp api send agents:list` for health checks, not `xirp features`.

---


### 1.9 Portal OAuth discovery — verified, and one near-miss avoided

Probed directly, unauthenticated, with `curl`. All read-only.

**Real, verified — the authorization server metadata** at
`https://phenotype.spotifyportal.com/.well-known/oauth-authorization-server`
(genuine `application/json`):

| Field | Value |
|---|---|
| `issuer` | `https://phenotype.spotifyportal.com/api/auth` |
| `authorization_endpoint` | `https://phenotype.spotifyportal.com/api/auth/v1/authorize` |
| `token_endpoint` | `https://phenotype.spotifyportal.com/api/auth/v1/token` |
| `userinfo_endpoint` | `https://phenotype.spotifyportal.com/api/auth/v1/userinfo` |
| `jwks_uri` | `https://phenotype.spotifyportal.com/api/auth/.well-known/jwks.json` |
| `scopes_supported` | `openid`, `offline_access` |
| `grant_types_supported` | `authorization_code`, `refresh_token` |
| `code_challenge_methods_supported` | `S256`, `plain` |
| `token_endpoint_auth_methods_supported` | `client_secret_basic`, `client_secret_post`, `none` |
| `registration_endpoint` | **absent** |

**Near-miss, corrected:** the path
`https://phenotype.spotifyportal.com/.well-known/oauth-client-metadata.json`
returns **HTTP 200**, which looks like a working CIMD document. It is not. It is
the SPA fallback: `content-type: text/html`, 13,761 bytes. A control request to
`/.well-known/definitely-not-real-xyz.json` returns the identical 200 /
`text/html` / 13,761 bytes. So **any** `200` on this host proves nothing; the
content type must be checked. Do not record Portal as publishing a CIMD document.

**What this means for the bridge decision (XJ-03):**
- PKCE is supported, including `S256`. Good: no client secret is strictly
  required (`token_endpoint_auth_methods_supported` includes `none`).
- There is **no `registration_endpoint`**, so OAuth Dynamic Client Registration
  is not offered. A bridge cannot self-register a client id at runtime.
- CIMD requires the client to publish a *reachable HTTPS* client-metadata
  document and for the authorization server to allow-list its URL as a client
  pattern. Portal's allowed patterns are currently `https://claude.ai/*`,
  `https://chatgpt.com/oauth/codex/*/client.json`,
  `https://backstage.spotify.com/chirp.json`,
  `https://xirp.spotify.com/oauth/client-metadata.json`. None of those is ours,
  and widening the list is explicitly out of bounds without authorization.
- Verified reference shape, from the Xirp document that IS allow-listed
  (`https://xirp.spotify.com/oauth/client-metadata.json`, HTTP 200 JSON):
  ```json
  {"client_id":"https://xirp.spotify.com/oauth/client-metadata.json",
   "client_name":"Xirp",
   "redirect_uris":["http://127.0.0.1/callback"],
   "response_types":["code"],
   "grant_types":["authorization_code","refresh_token"],
   "token_endpoint_auth_method":"none",
   "scope":"openid offline_access"}
  ```
  Note `token_endpoint_auth_method: "none"` and a loopback redirect. That is the
  shape any Jcode bridge would need to match, once a pattern is allowed.

Net: the auth server is capable of PKCE + public client. The blocker is
**client allow-listing**, not protocol capability. That requires either a
pre-registered static client id, or operator authorization to add a CIMD URL to
the allowed patterns. Neither can be minted from this session.

Portal also publishes genuine RFC 9728 protected-resource metadata at
`/.well-known/oauth-protected-resource` and `/.well-known/oauth-protected-resource/v1`
(real `application/json`, unlike the CIMD path above). Both name
`authorization_servers: ["https://phenotype.spotifyportal.com/api/auth"]` and a
`resource` that matches the MCP path, so the whole discovery chain is present and
consistent. Details in `STATE.md` section 14.

**Allow-list enforcement is now proven, not assumed** (`STATE.md` section 15).
Read-only `GET`s to `/api/auth/v1/authorize` with a dummy PKCE challenge and no
credentials: the allow-listed Xirp CIMD URL returns **302** to a real consent
page, while `kooshapari.com`, `jcode.sh` and `example.invalid` all return
**400 `InputError` "Invalid client_id"**. The first two resolve fine over HTTPS,
so the rejection cannot be DNS or URL shape. **B6 is confirmed as the only
remaining blocker for authenticated Portal MCP** — a CIMD client needs no
pre-registration, just one allow-listed pattern for our own domain.

### 1.10 Bridge selection for XJ-03 — DECIDED, not yet deployed

**`punkpeye/mcp-remote` (npm `mcp-remote`) v0.15.1 is the only viable bridge.**
It is the only candidate implementing CIMD (SEP-991), the mechanism Portal's
allow-list is built around; the alternatives lack PKCE, CIMD, or maintenance.
Verified against the published package rather than its README: the
`--client-metadata-url` flag exists in shipped code and is validated by
`isClientMetadataUrl`, which requires `https:` and a non-root path. Comparison
table, the rejected impersonating config, the derived callback port, and the
redirect_uri mismatch to raise with the vendor are in `STATE.md` sections 13
and 15, plus `scripts/derive_callback_port.py` and the CIMD template.