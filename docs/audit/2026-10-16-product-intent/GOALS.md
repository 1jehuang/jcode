# Core harness and deployment goal dispositions

Source `6cf71ed4efb9b1f256b2fcd00e95748309fa6d0b`. Implemented means meaningful source exists;candidate means unmerged work;runtime-unknown is an explicit evidence limit. This maps the recovered core horizon,not every file or speculative feature request.

| Goal                                         | Current disposition/evidence                              | Required acceptance                                                                                                   |
| -------------------------------------------- | --------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Own harness identity and artifact            | Implemented fork/source lineage;installed products differ | Binary hash/build/socket identity;separate fork version and install path                                              |
| Provider connection,streaming,retry/fallback | Implemented/inherited provider architecture               | Disconnect/rate-limit/EOF/timeout with correct tool-call/effect state and served route                                |
| Configured model discovery/picker            | PR93/98 merged                                            | Effective configuration,picker and provider route agree;no silent wrong model                                         |
| Durable sessions/resume                      | Implemented storage/session source                        | Restart/corruption/compaction preserve original restrictions,todos,tool/effect state and ownership                    |
| Context/memory/compaction correctness        | Implemented architecture,behavior unqualified here        | Critical permissions survive compression;memory cannot grant authority or leak profile state                          |
| Tool/filesystem permission envelope          | Implemented tools/policy components                       | Protected file/forbidden repo remain unchanged under parent/child/retry/recovery negatives                            |
| Parent/child ownership,cancellation          | Implemented swarm/tool source                             | Failed or cancelled parent leaves no unauthorized child;observed cleanup and result provenance                        |
| Windows native input/terminal liveness       | PR95 merged;PR97 open;issue45 open                        | Exact resume/resize/input/Enter sequence on installed Jcode versus owned KCode independently                          |
| Tiny terminal and terminal UX                | Existing candidate/history,not newly qualified            | Compact terminal render/resize/resume and accessibility semantics without dropped controls                            |
| Herdr reporting/session restore              | Implemented source contracts                              | Real session IDs and correct idle/working/blocked/approval/exit transitions;no false-ready suppression                |
| Native SSH/shared-session clients            | Implemented/inherited architecture                        | Correct cwd/profile/permissions across reconnect and source/destination artifact identity                             |
| Laptop-to-stronger-host migration            | Requested operational outcome;incomplete prior witness    | Full migration or concrete approved schedule,preserved originals,remote resume and rollback                           |
| Manager/lane/host map                        | Requested;completeness unknown                            | Authoritative live assignments,not inferred from pane names or agent types                                            |
| Overflow routing                             | Existing primitives;real witness unknown                  | Load-bounded task moves to another active pool and returns verifiable output;no queued work disguised as active width |
| Cursor headless and Herdr dispatch returns   | Requested integration acceptance,unproven here            | One authorized real task per path with parent/child/served-model/host/result receipt                                  |
| Updater/storage/profile separation           | PR98 merged;source self-update disabled                   | Native install/update-denial/rollback cannot overwrite Jcode or cross-contaminate sessions                            |
| Release/local free validation                | Scripts/gates exist;current hosted proof stale            | Exact local gates and native receipts for final candidate;publish verified owned artifact                             |
| ACP/discovery/poke                           | Existing docs/candidate PR10/25                           | Retain only accepted consumer obligations;real broker/resume behavior rather than API presence                        |
| Ambient/dream and autonomous refinements     | Existing feature source/candidate PR99                    | Honest disabled/parked behavior;no placeholder completion claim or silent new authority                               |
| Swarm rename and other audit feedback        | Candidate PR105;complementary audit PR106                 | Reuse current owner review/results;do not reimplement or treat audit publication as repair                            |
| Upstream incremental advantage               | Hypothesis,not accepted benchmark result                  | Matched full task/fault workload,resource use,safety,recoveries and patch carry cost                                  |
| Desktop/mobile applications                  | Separate repository/scope per AGENTS                      | External consumer contract only;no silent absorption into this finite harness plan                                    |

## Scope and sunset decisions still owed

- Which exact installed Jcode and KCode profiles must pass Windows acceptance? A fork repair must not be reported as an upstream installation repair.
- Which updater/install path is authoritative now that KCode self-update is intentionally disabled? Keep explicit verified installation and rollback procedures rather than restoring upstream auto-update as a convenience.
- Which session and tool/effect invariants define safe migration? File transfer alone does not prove resumable work or enforce original permissions.
- Which manager owns each lane and accepts overflow results? Preserve task-bound authority and resource limits instead of treating a shared API credential as universal permission.
- Reconcile new audit PR106 with existing owner tasks,PR97/96/99/100/101/105 and issue104 before dispatching duplicates. Closed PR94 is not pending integration.
- Advanced discovery/ACP/poke/ambient capabilities need retained/replaced/deferred decisions tied to consumers. No capability is retired or deleted by this document.
- README upstream badge/install links and KCode fork identity must be clearly distinguished in an owned release. Do not overwrite upstream products or profiles while correcting documentation.
