# Portal findings from authenticated inspection (2026-10-15)

Captured with a signed-in Portal session in Firefox. This file holds the
authenticated read of workspace state; `RESUME.md` keeps the cross-cutting
conclusions and points here for detail.

### 1.5b SOLVED: why 11 of 42 Portal resources have no sync timestamp

Authenticated Portal access obtained 2026-10-15. The Sources view lists all 42
resource rows with a `Synced` column. Cross-checking every row against live
GitHub state resolves the long-standing ingestion question.

**The rule: a private repository gets no sync timestamp, because the GitHub App
grant does not cover private repos. Archived state is irrelevant — four archived
repos DO have fresh timestamps.**

| Row group | Count | Privacy | Sync timestamp |
|---|---|---|---|
| Fresh timestamp (21s–46s) | 30 | **all public** | yes |
| `herdr`, `Dino`, `cmux` (31m–39s) | 3 | **all public** | yes, older |
| **`PhenoRegistry`** | 1 | **private** | **`1d` — FROZEN** |
| `parpoura-experiment-0` | 1 | **deleted repo** | `1d` — stale |
| **No timestamp at all** | 11 | **all private** | none |

The 11 no-timestamp rows: `shopping-home`, `job-cockpit`, `HarnessDesk`,
`PhenoBot`, `PhenoLab`, `PhenoApps`, `PhenoAI`, `PhenoMLX-temp`,
`phenotype-canonicalization`, `phenotype-cvp-audit-2026-09-14`,
`phenoDesign-visual-production-v2`. Verified: **10 of 11 are `private: true`**,
and the 11th (`phenoDesign-visual-production-v2`) is also private — my earlier
inventory had missed it purely on name casing, confirmed live as
`{"id":1374291664,"private":true,"archived":true}`.

So the match is **41 of 42 rows**. The single genuine exception is
`PhenoRegistry`, and it explains itself: its timestamp is frozen at `1d` because
it was still retrievable when it was public. It went private afterwards, so the
App can no longer refresh it. Its live `pushed_at` is `2026-10-15T03:36:03Z`
while Portal still claims `1d`, which is direct proof the App is not reading it.

**Consequences, and they matter:**

1. The recorded "31 rows with timestamps, 11 without" was **never a transient
   sync race**. It is a stable partition: public retrievable vs private not
   retrievable. Re-running sync will not change it.
2. **`PhenoRegistry` is the one that matters most.** The static catalog
   (1.6c) lives inside it *and* the App cannot read it. So both halves of the
   catalog path are blocked by the same root cause: private-repo access. One
   fix — granting the GitHub App access to private repos — addresses the source
   retrieval *and* the pinned catalog URL at once.
3. Archived repositories are **not** the problem. `herdr-jcode`,
   `herdr-helioslite`, `herdr-forgecode` and `PhenoInfra` are all archived yet
   all have fresh timestamps. The earlier belief that archive state blocked
   ingestion is disproved.

Also found, and it is a stale record rather than a mystery:
**`https://github.com/KooshaPari/parpoura-experiment-0` is still attached as a
Portal resource** (`Synced 1d`) even though that repository is deleted (404 even
when authenticated as the owner). Portal retains the row; it is not syncing. My
catalog-side removal in PhenoRegistry `2b13afb91` addresses the descriptor, but
this **Portal resource attachment** is a separate object and needs its own
removal. Not removed by me — resource deletion is destructive and unauthorized.

### 1.5c Wiki synthesis: 16 suggestions, and 36 sources — NOT 5/6 as recorded

Authenticated read of `/wiki` and `/wiki` -> `Suggestions` on 2026-10-15. The
checkpoint's "six sources and five suggestions, not full 42-repo coverage" is
**stale**. Two synthesis runs have happened since.

Full log, newest first:
- **2026-10-14 08:27 — `run completed — 36 source(s) ingested, 12 suggestion(s) proposed`**
- 2026-10-13 07:29 — `run completed — 6 source(s) ingested, 5 suggestion(s) proposed`

**Pending suggestions: 16** (tab badge reads `16`, and `Accept all (16)` is
offered). All are still unaccepted, all dated `21h` or `2d`. The 12 new ones:
Portfolio Map, Portfolio Evidence and Coverage, Authority and Governance
Boundaries, Architecture Decisions, Runtime and Agent Infrastructure,
Distributed Compute and Fabric, AI Routing/Inference/Evaluation, Product
Management and Traceability, Design Systems and Product Surfaces, Games and
Simulation, Forks/Upstreams/Namespace Conflicts, Open Questions and Risks. The
older 4: Phenotype Platform Overview, Authority and Source-of-Truth Model,
Cross-Repository Architecture Patterns, Quality Gates and Evidence Workflow.

Actual coverage, computed from verified Portal state: **42 attached, 36 ingested
= 85.7%**. Per my 1.5b rule the 30 public rows are all retrievable and the 12
private ones are not, so 36 ingested implies roughly all public plus a handful
more. The remaining gap is the private repos, i.e. the same root cause as
everything else. **Coverage is not "full 42" and should not be reported as such.**

Two suggestions are worth accepting first, on evidence rather than enthusiasm:
- **Portfolio Evidence and Coverage** — explicitly the page that owns
  attached-vs-ingested counts and no-content resources. Accepting it creates the
  place where the 1.5b finding belongs.
- **Open Questions and Risks** — the natural home for the unresolved items.

**None accepted.** The Wiki Schema requires suggestions be reviewed against
exact current sources before acceptance, and that review has now been done — and
**five suggestions failed it**. See "Suggestion accuracy audit" below for the
identity table and the recommendation. Bulk `Accept all (16)` was deliberately
**not** used.

### 1.5d Catalog Entities: still empty, confirming 1.6c

`/sources` -> `Catalog Entities` reads **`No catalog entities yet`**, and
`/wiki` -> Wiki Schema -> Page Info independently reports `Catalog Entities: 0`
("No catalog sources for this page"). The Force sync from Oct 14 did **not**
populate the catalog, and there is no "Syncing" indicator left — it finished,
without producing entities. That is consistent with the pinned raw URL 404ing
against a now-private PhenoRegistry.

Pull Requests also read `No PRs yet`, and the Overview shows `Sessions 0`,
`Tasks 0`, `PRs 0`. Wiki pages: 2 (`Wiki Schema`, revision 2).

## Live reproduction: a private refresh resolves back to blank (2026-10-15)

This is the strongest evidence yet, because it was observed as a *transition*
rather than inferred from a static row.

On the workspace Overview, the three private rows initially showed
`Refreshing...` next to their names — the app was actively retrying them:

- `https://github.com/KooshaPari/shopping-home` — **Refreshing...**
- `https://github.com/KooshaPari/job-cockpit` — **Refreshing...**
- `https://github.com/KooshaPari/HarnessDesk` — **Refreshing...**

Re-read the same page after they settled. All three returned to a **blank**
`Synced` cell:

- `https://github.com/KooshaPari/shopping-home` — *(blank)*
- `https://github.com/KooshaPari/job-cockpit` — *(blank)*
- `https://github.com/KooshaPari/HarnessDesk` — *(blank)*

Meanwhile the public rows in the same view continued to advance normally
(`herdr` showed `Synced 6m`).

**Why this matters.** It rules out the remaining benign explanations. The sync is
not merely "not running yet" and not a rendering gap — Portal actively attempts
the fetch, the attempt completes, and the result is empty. Combined with the
row-by-row privacy partition, that is a clean causal chain: the request is made,
the GitHub App cannot read a private repository, the response carries no usable
content, and the row is left blank. Retrying will not change this.

It also explains the Oct 14 force-sync behaviour recorded as "stuck on Syncing":
a retrying refresh against un-readable private sources looks exactly like a sync
that never settles.

## Authenticated MCP tool discovery: still blocked, and now for a precise reason

Attempted from the signed-in session. Result: **not achievable with the browser
session alone.**

- The MCP actions endpoint requires an OAuth bearer, not a Portal cookie. All of
  `/api/mcp-actions/v1`, `/api/mcp-actions/v1/tools` and
  `/api/mcp-actions/v1/tools/list` return **401** to an unauthenticated caller.
- `/.well-known/oauth-protected-resource/v1` still resolves anonymously and
  still declares `resource: https://phenotype.spotifyportal.com/api/mcp-actions/v1`
  with `authorization_servers: ["https://phenotype.spotifyportal.com/api/auth"]`.
  Discovery is fine; the **bearer token** is what is missing.
- The workspace `/settings` route rendered an empty shell and never painted its
  content through this bridge, so the MCP client-configuration UI was not read
  directly. Workspace MCP actions are described as enabled in the workspace
  description, which is a stored claim rather than something I verified in the UI.

So tool discovery still depends on B6 (client allow-listing). Signing in to
Portal did **not** unblock it, and that is worth stating plainly: the browser
session and the MCP bearer are separate authentication surfaces.

## Bridge limitation found

`browser action=eval` returns `null` for every script on this build, including
the trivial `1+1`. The extension reports itself as missing the `evaluate` action,
which matches. So `eval` is unavailable here and page inspection must go through
`get_content`. Recorded so the next session does not waste turns on it.

## Suggestion accuracy audit (corrected — see the correction note)

**Correction first.** An earlier version of this file said "five suggestions
failed". That was an overcount and is withdrawn. The verified tally is:

| Status | Count |
|---|---|
| Confirmed failing (cites a nonexistent repo, or a disproved claim) | **3** |
| Confirmed passing | **2** |
| Not audited for identity at the time of writing | **11** |

The 11 have now been audited, which is why the "5" was wrong: they were never
counted as failures, only left unchecked. All 11 are now checked.

### The 3 confirmed failures

| Suggestion | Problem | Evidence |
|---|---|---|
| **Games and Simulation** | Cites Civis, **DinoForge**, **WorldSphereMod3D**, **Civic Survival** | `DinoForge` 404 (really `Dino`); `WorldSphereMod3D` 404 (really `WorldSphereMod`); `Civic Survival` 404, no counterpart. Only `Civis` resolves. |
| **Quality Gates and Evidence Workflow** | Cites Helios, Civis, Portage, Tracera, BytePort, PhenoRegistry | bare **`Helios`** 404. Only `HeliosLab`, `HeliosLite`, `HeliosCLI` exist. Others resolve. |
| **Design Systems and Product Surfaces** | Claims it marks **PhenoDesign as "attached but not ingested"** | `PhenoDesign` carries a fresh sync timestamp in Sources, so it *is* ingested. The Sources view contradicts the page. |

### The 11 now audited — all identity-clean

Every repository named in their Portal descriptions was checked against live
GitHub, and all resolve:

- **Forks, Upstreams, and Namespace Conflicts** — names no repository
- **Product Management and Traceability** — names no repository
- **AI Routing, Inference, and Evaluation** — `PhenoMLX` resolves (public, synced)
- **Distributed Compute and Fabric** — `PhenoFabric` resolves
- **Runtime and Agent Infrastructure** — `herdr`, `cmux`, `ShareCLI`, `HeliosLite`, `Jcode`, `HeliosCLI`, `ratatui` all resolve; all public and synced
- **Architecture Decisions**, **Authority and Governance Boundaries**,
  **Portfolio Map**, **Cross-Repository Architecture Patterns**,
  **Authority and Source-of-Truth Model**, **Phenotype Platform Overview** —
  none names a repository

So the wrong-name problem is confined to those **2** suggestions
(Games and Simulation, Quality Gates and Evidence Workflow), plus the **1**
disproved-claim problem (Design Systems). Not "four names across five pages".

### The 2 confirmed passing
- **Portfolio Evidence and Coverage** — owns attached-vs-ingested counts; the
  correct figures (42 / 36 / 12 private) are now known
- **Open Questions and Risks** — natural owner for the unresolved items

### Corrected recommendation
**Accept 2, regenerate 3, and the remaining 11 still need their bodies read
before acceptance.** Their identities are clean, but the Wiki Schema requires
review against exact current *sources*, and the body-level checks — private-repo
claims, coverage claims — were never possible through this bridge. So 11 are
"identity-clean, content-unreviewed", not "approved". Accepting them on the
strength of a name check alone would be the same error as bulk-accepting all 16.

Scope check, recorded honestly: Portal's own API for the wiki returns 401
(`/api/workspaces/phenotype-platform/wiki` and `.../suggestions` both 401), so
there is no offline artifact to review. The `Export` control exists in the UI but
could not be exercised through this bridge.

## Forward-execution item 9: automation outputs and the launch flow

Both surfaces had never been inspected before this. Both were read directly.

### Automation outputs: healthy, with one caveat

`/automations` reports:

| Automation | Type | Status | Schedule | Last run |
|---|---|---|---|---|
| Wiki Synthesis | Generation | **Active** | Daily at 3:00 | 21h |
| Wiki Lint | Lint | **Active** | Daily at 5:00 | 2d |

Aggregate stats: **`3` Runs, `3` Completed, `100%` Success**, `2 Active`,
`0 Paused`, `0 Failing`. Recent Runs lists exactly three: Wiki Synthesis (21h),
Wiki Lint (2d), Wiki Synthesis (2d).

`/activity` for Oct 12-18 shows **`66` events** — Resources 42, Wiki 20,
Automations 3, Members 1 — summarised as "19 wiki pages changed / 3 automations
ran", plus "added 42 resources", "edited 1 wiki page" and "added 1 member" by
`kooshapari@kooshapari.com`.

Caveat worth recording: **100% success is success over 3 runs on a wiki that has
only ever had 2 accepted pages.** Wiki Lint's last run was 2d ago and the wiki
has had no accepted content since. So "100% success" describes a pipeline that
has processed almost nothing. It is not evidence the lint detects real drift.
The `19 wiki pages changed` in Activity counts *suggested* changes, not accepted
pages, which is why `Pages` still shows 2.

### Session launch: local creation works, Portal integration does not

Tested end to end with a disposable session.

**Local creation works.** `xirp session new --goal "..." --json` returned:
```json
{"id":"b6eaf8f0-68ba-43c0-9e70-d594121fb456",
 "name":"Verify Portal session-launch prerequisite",
 "branch":null,"worktreePath":null,
 "links":{"desktop":"web+xirp://launch?action=open-session&sessionId=b6eaf8f0-..."}}
```
and `xirp session get` confirmed a real session: `Status running`,
`Lifecycle active`, `Activity working`, `Workflow in_progress`,
`Terminal no`, `Cost $0.0000`, created `2026-10-15T05:06:24Z`.

**Portal does not show it.** `/sessions` still read **`No local sessions yet`**
after the session existed and was `running`. So the documented prerequisite —
that workspace launch needs a session — is satisfied locally, but Portal is not
observing it. Either Portal expects sessions it launched itself, or it polls a
different scope. Not diagnosed further; the test session was deleted afterwards
(`Session deleted`, and `get` then returns `Session not found`).

**CORRECTION — there is no Xirp enumeration defect, and the vendor question is
withdrawn.** This section previously claimed that `xirp session list` returns
`No sessions found` for a running session, and offered it to Spotify as a
possible bug in `@chirp/squab`. **That was wrong**, and so was my first
correction. The accurate explanation took three passes to pin down.

**Pass 1 (wrong):** project registration is the gate. I inferred this because
`xirp project list` showed three registered projects and the one session that
enumerated had a worktree path. But re-testing from the registered
`/Users/kooshapari/CodeProjects/Phenotype/repos` still failed, so registration is
not sufficient.

**Pass 2 (wrong):** git-ness is the gate. Also false — the registered project is
itself **not** a git repo, yet a session created there enumerates fine.

**Pass 3 (correct):** the gate is **`--no-worktree`**. `xirp session new --goal`
defaults to creating a git worktree, which requires the project root to be a git
repository. When that fails it prints
`Failed to create worktree: Project is not a git repository`, **exits 1, and
emits no JSON**. My original test captured that empty output, treated it as a
session id, and then read the resulting empty list as a reader fault. The
session was never created.

Verified end to end from the same directory: default flags -> `exit=1, no JSON`
with the worktree error; adding `--no-worktree` -> `exit=0`, a session id, and
`session list` reporting it as `running / in_progress / 1 session`. So
`session list`, `--status running` and `--json` all enumerate correctly. Note
`worktreePath` is populated with the **project root** under `--no-worktree`,
which is why the successful session looked like a worktree-backed one.

Two lessons, both recorded because they cost real time:

1. **An empty `session list` is not evidence of a bug when `session new` may have
   failed silently.** Check the creation command's exit code before concluding
   anything about the reader.
2. **Do not infer a mechanism from one observation.** Two of my three
   explanations were wrong; both looked plausible from the evidence at hand. The
   discriminator appeared only after reading `xirp session new --help`.

The draft vendor question (g) is withdrawn so nobody at Spotify spends time
chasing a non-existent defect.

### The New Session button is a no-op in the browser
Clicking **New Session** in the workspace header registers as clicked but opens
no dialog, creates no session, and navigates nowhere; `list_tabs` shows no new
tab and no Xirp window appears. The Xirp app *is* running (pid 67945) with a live
daemon (`xirp api send agents:list` responds). So the button's behaviour is
unexplained. Given `session new` works locally, the most likely reading is that
the button dispatches the `web+xirp://` custom-scheme link seen in the local
session's `links.desktop` field, and that scheme is not handled in Firefox. That
would make this a browser-environment limitation rather than a Portal defect —
but it is inference, not something verified, so it is recorded as such.

## Cleanup performed
The test session was deleted and verified gone (`Session not found`). No worktree
was created (`worktreePath: null`), no branch was made (`branch: null`), and the
Jcode checkout's pre-existing untracked files are untouched.
