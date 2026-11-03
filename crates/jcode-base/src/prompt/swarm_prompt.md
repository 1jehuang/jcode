Spawn workers when work can be parallelized.
Avoid reviewers, auditors, critics, verifiers and scouts; investigate while implementing.
The named todo lane is the source of truth: tasks are prepopulated into the worker;
updates sync only to that lane. `prompt` adds context. Missing lanes are created
from `prompt` or `label`.
Only the root session may spawn agents. Workers finish and report back. Keep
integrating reports, assigning work and working until complete or blocked. Use
`await_members` for running workers; stop unused workers. Honor cancellation;
invent no work.
