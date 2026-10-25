<!--
This file IS the swarm config. Swarms are complicated, dynamic systems, so
routing policy is passed to the models as a prompt rather than as options in
a standard config file. Edit freely: override globally at
~/.jcode/swarm-prompt.md or per-project at ./.jcode/swarm-prompt.md.
-->

Model routing guidance for spawned swarm agents. Pass `model` to choose a model
for newly spawned workers, including workers created by assignment or `launch`.
An explicit model overrides `agents.swarm_model`. When omitted, workers use that
configured default, or inherit the coordinator's model and route when unset.
Pass `model: "inherit"` to force coordinator inheritance even with a configured
default. Model selection does not change reused workers. Run `swarm list_models`
to check available models/routes. Route-prefixed values such as
`openai-api:gpt-6-astra` pin the authentication route as well as the model.
Use `[agents] swarm_model` to set the default for future worker spawns, and
the `model` parameter for task-specific choices. Worker reasoning effort is
not selectable from this tool: workers run at the user's `[agents]
swarm_effort` (default `low`).

What to spawn workers for:

- Spawn workers to implement: build features, fix bugs, write tests, migrate
  or refactor code. Give each worker a concrete, scoped change with clear file
  ownership and an acceptance check it can run itself.
- Do not spawn reviewers, auditors, critics, or verifier agents. Review and
  verify the workers' results yourself (read the diff, run the tests). Each
  implementation worker validates its own change before reporting.
- Avoid pure research or scouting workers. Fold the needed investigation into
  the implementation task, or do it yourself first.

Structure guidance for spawned swarm agents:

- Always pass `label` when spawning (e.g. `label: "auth refactor"`) so the swarm
  UI shows what each agent is for. The explicit `spawn` action rejects missing or
  blank labels. The named todo lane is the source of truth: its tasks and details
  are prepopulated into the worker and sent before it starts, with updates synced
  only to that lane. `prompt` is optional extra context, not a duplicate task list.
  If the lane does not exist, one is created from `prompt` or `label`.
- Only the root session may spawn agents. Workers must complete their assigned
  task directly and report back rather than creating another generation.
