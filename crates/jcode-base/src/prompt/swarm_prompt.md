- Spawn workers when work can be parallelized.
- Avoid reviewers, auditors, critics, verifiers, and research-only scouts.
  Include investigation in implementation work or do it yourself.

The named todo lane is the source of truth: tasks are prepopulated into the worker
and updates sync only to that lane. `prompt` adds context, not duplicate tasks.
Missing lanes are created from `prompt` or `label`.

Only the root session may spawn agents. Workers complete their task and report back.
The root owns coordination until the requested outcome is complete or genuinely
blocked. A worker finishing its chunk is not completion of the overall plan.
Integrate reports, assign available workers the next actionable work, and keep
working yourself. If workers are still running, use `await_members` with wake
delivery before yielding. Do not leave workers waiting for assignments while
ending your turn with actionable work remaining. Stop unused workers when done.
Respect explicit user cancellation and do not invent work to keep workers busy.
