# Session-End Learnings Capture Rules

| | |
|---|---|
| Status | Enforced ruleset (ambient on-exit task) |
| Scope | jcode TUI/CLI agent |
| Trigger | `/exit` or `/quit` (session teardown), fired as an ambient task |
| Pipeline | `trigger_save_memory_extraction` -> `trigger_final_extraction_with_dir` -> `run_final_extraction` -> `Sidecar::extract_memories_with_existing` -> `MemoryManager::remember_project` |
| Storage | Project memory (`MemoryEntry`) + ambient on-exit directive (`~/.jcode/ambient/directives.json`) |

## Purpose

Every session must end by recording what was learned and what was done, so that
future sessions start with durable context instead of cold. This is a free,
local, OAuth-only operation that runs as an ambient task when the user exits.
These rules formalize *what* gets captured and *how* it is enforced; they sit on
top of the existing final-extraction pipeline rather than replacing it.

## When the task fires

The session-end capture fires on session teardown, which `/exit` and `/quit`
trigger. It is a no-op (silently skipped) when any of the following hold, to
keep exit fast and avoid noise:

- The session is remote (`is_remote`).
- Memory is disabled (`memory_enabled == false`).
- The transcript has fewer than 4 provider messages (nothing substantive
  happened).

Otherwise the transcript is materialized and handed to the final extractor,
which runs asynchronously so exit never blocks on the model call.

## The Rules

### Rule 1: Capture is mandatory at session end
Every non-trivial session (>= 4 provider messages, memory enabled, local) MUST
run the session-end capture on `/exit`/`/quit`. The capture is fire-and-forget:
it must never block or delay the user's exit, and a failure must never prevent
exit. Failures are logged via `memory_log`, not surfaced as errors.

### Rule 2: Record durable learnings, not transient noise
Capture only what a developer would benefit from recalling weeks later. Use
exactly one category per item:

- `fact` - objective technical info about the codebase, architecture, patterns,
  dependencies, tools, environment.
- `preference` - what the USER wants or how they like things (workflow, UX,
  coding style, how the assistant should behave).
- `correction` - a mistake corrected, bug found and fixed, wrong assumption, or
  something the user explicitly corrected.
- `entity` - named people, projects, services, repos, teams worth tracking.

Categorization MUST follow:
- User wants / likes => `preference` (never `fact`).
- Bug fix / mistake => `correction` (never `fact`).
- `fact` is reserved for objective system information, never user behavior.

### Rule 3: Never record ephemera
Do NOT capture:
- Transient debugging details, compile errors, intermediate build steps.
- Commit hashes, git operations, or "changes were committed/pushed" notes.
- Line-by-line code edits ("X changed to Y in file Z") - that belongs in git
  history, not memory.
- Self-evident project context already in the system prompt (project name, repo
  URL, language).
- Redundant variations of already-known memories (check the "Already known"
  list before emitting).

### Rule 4: Deduplicate against existing memory
Before storing, the extractor MUST be given the current active project memories
and MUST NOT re-emit them or close paraphrases. The existing-memory list is
capped (80 entries, 150 chars each) to bound cost; dedup is best-effort but
required.

### Rule 5: Attribute trust honestly
Each item carries a trust level:
- `high` - the user stated it explicitly.
- `medium` - observed from the assistant's own actions/results.
- `low` - inferred.
Trust MUST reflect the actual evidence, not optimism.

### Rule 6: Record work, not just facts
The session-end capture covers both *learnings* (Rules 2-5) and *work done*. For
work, record only durable outcomes worth recalling (e.g. "added gpt-5.4-mini to
the OpenAI catalog and switched the sidecar OAuth fallback to it"), not the
mechanical diff. Mechanical change detail lives in git history (Rule 3).

### Rule 7: Bound cost and stay free
The capture uses the sidecar (cheap/fast OAuth model; OpenAI fallback is
`gpt-5.4-mini`). It MUST stay within the sidecar's existing context caps
(extraction context: <= 40 messages / 24k chars) and MUST NOT spend API-key
budget unless the user has explicitly enabled API keys for ambient work.

### Rule 8: Persist an auditable on-exit directive
When the ambient subsystem is active, the session-end task SHOULD append a
machine-readable directive to `~/.jcode/ambient/directives.json` recording that
the session ended and that capture ran, so the ambient runner has an auditable
trail and can pick up any follow-up. The directive is data only and is never
treated as executable instructions.

### Rule 9: Write task artifact files at the end of major tasks
Every jcode agent (primary, swarm subagents, ambient, server) MUST write durable
artifact files to disk at the end of any major task, the same way memex/memory
files are written. A "major task" is multi-step work, research, a feature, a
debugging session, or anything spanning many tool calls.

- Write a short markdown artifact capturing: what the task was, what was done,
  key decisions + rationale, files touched, how it was verified, and any
  follow-ups or known gaps.
- Location: `docs/<TOPIC>.md` for shareable references that belong with the repo;
  or scratch planning files (`task_plan.md`, `findings.md`, `progress.md`) in the
  working directory for in-progress working memory. Keep scratch planning files
  out of commits unless the user asks otherwise (use `.git/info/exclude`).
- Content discipline mirrors Rules 2-3 and 6: durable, useful-weeks-later content
  only. No line-by-line diffs, commit hashes, or transient build noise (that
  lives in git history).
- This complements the automatic session-end memory capture (Rules 1-8); it does
  not replace it. The agent writes the human-readable artifact; the pipeline
  writes the structured memory.
- Enforced via the agent system prompt (`crates/jcode-base/src/prompt/system_prompt.md`,
  "Task artifacts" section), which every agent surface embeds through
  `build_system_prompt_split`.

## Enforcement model

These rules are enforced at four layers:

1. **Pipeline** - the existing `run_final_extraction` already runs on teardown
   across TUI, server, comm, and desktop disconnect paths. The rules document
   its contract so it is not silently weakened by future edits.
2. **Extraction prompt** - Rules 2-5 mirror the sidecar extraction system prompt
   in `crates/jcode-base/src/sidecar.rs`. Any change to that prompt must keep
   these guarantees.
3. **Ambient directive seed** - Rule 8 wires an on-exit directive so the ambient
   task is explicit and auditable rather than implicit.
4. **Agent system prompt** - Rule 9 is instructed to every agent via the
   "Task artifacts" section of `crates/jcode-base/src/prompt/system_prompt.md`,
   which all agent surfaces embed through `build_system_prompt_split`.

## Verification

A change to the capture path is correct only if:
- `/exit` on a >= 4-message local session triggers `trigger_final_extraction*`
  (logged via `memory_log::log_final_extraction`).
- The extractor receives the existing-memory list (dedup, Rule 4).
- New memories are stored via `manager.remember_project` with a category, trust,
  and `with_source(session_id)`.
- Exit latency is unchanged (capture is async, Rule 1).
- Memory/ambient unit tests pass (`cargo test -p jcode-base memory`,
  `cargo test -p jcode-app-core ambient`).
