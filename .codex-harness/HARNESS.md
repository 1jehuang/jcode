# Codex Harness Workspace

This directory stores durable harness state for Codex-style agent work.

Use it for:

- execution contracts with required inputs, budgets, permissions, completion conditions, and output paths
- raw traces from attempts, failures, verification, and decisions
- persistent knowledge from research sources and implementation lessons
- harness profiles and eval records for measuring harness changes
- harness proposals and promotion decisions for optimizing harness behavior safely
- explicit gates before declaring work complete
- compact context blocks for session handoff or recovery

Default project path:

```text
<project-root>
```

Recommended loop:

1. Create a small contract.
2. Work only inside the contract boundaries.
3. Record raw traces when something succeeds or fails.
4. Record research findings and implementation lessons as knowledge.
5. Query knowledge before repeating research or implementation work.
6. Record harness profiles, eval runs, proposals, and promotion decisions when changing harness behavior.
7. Ask for the next step when signals are unclear.
8. Run the eval gate before claiming completion.
9. Keep useful decisions as durable notes.
