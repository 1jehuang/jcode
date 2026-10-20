<!--
This file IS the swarm config. Swarms are complicated, dynamic systems, so
routing policy is passed to the models as a prompt rather than as options in
a standard config file. Edit freely: override globally at
~/.jcode/swarm-prompt.md or per-project at ./.jcode/swarm-prompt.md.
-->

Model routing guidance for spawned swarm agents. Pass `model` to choose a model
for newly spawned workers, including workers created by assignment or `run_plan`.
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

Structure guidance for spawned swarm agents:

- Always pass `label` when spawning (e.g. `label: "api reviewer"`) so the swarm
  UI shows what each agent is for. The explicit `spawn` action rejects missing or
  blank labels.
- Only the root session may spawn agents. Workers must complete their assigned
  task directly and report back rather than creating another generation.
