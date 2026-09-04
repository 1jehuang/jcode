<!--
This file IS the swarm config. Swarms are complicated, dynamic systems, so
routing policy is passed to the models as a prompt rather than as options in
a standard config file. Edit freely: override globally at
~/.jcode/swarm-prompt.md or per-project at ./.jcode/swarm-prompt.md.
-->

Model routing guidance for spawned swarm agents. Worker models are selected by
the operator through `agents.swarm_model` (or inherited from the coordinator
when unset). The spawn tool has no per-spawn `model` override; passing one is
ignored and reported back. Run `swarm list_models` to see the effective pin.
Pass `effort` when spawning or assigning swarm work:

- Implementation tasks: `effort: "low"`.
- Design, investigation, debugging, review, and verification: default effort.
- Context fetching / bulk reading / summarization: `effort: "none"`.
- If the user wants workers on a different model, ask them to set
  `[agents] swarm_model` (route-pinned values like `openai-api:gpt-5.5` work).

Structure guidance for spawned swarm agents:

- Always pass `label` when spawning (e.g. `label: "api reviewer"`) so the swarm
  UI shows what each agent is for. The explicit `spawn` action rejects missing or
  blank labels.
- In normal and light-swarm mode, only the root session may spawn agents. Workers
  must complete their assigned task directly and report back rather than creating
  another generation.
- Recursive spawning is reserved for a root running in `swarm-deep` mode. In that
  mode the spawner owns its children, and manager-style decomposition may create
  deeper subtrees when it materially improves coverage.
