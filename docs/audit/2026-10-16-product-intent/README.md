# KCode source, Jcode runtime and finite finish plan

Observation: 2026-10-16 Pacific. Owned source snapshot: `6cf71ed4efb9b1f256b2fcd00e95748309fa6d0b`,default branch `master`,repository `KooshaPari/KCode`. The directory historically named jcode is not proof that its code is the installed Jcode product.

**Current conclusion: source integration advanced, but installed harness behavior, Windows/Herdr repair and migration/overflow acceptance remain unqualified.** PR95 Windows terminal-liveness detection and PR98 KCode update/storage isolation are now merged. Those are meaningful source changes,not proof they are installed or solve the exact Windows input/resume failure.

| Document                              | Purpose                                                                                          |
| ------------------------------------- | ------------------------------------------------------------------------------------------------ |
| [State and journey](STATE.md)         | Three distinct source/runtime identities and usable-harness proof boundaries                     |
| [Goal dispositions](GOALS.md)         | Core harness and operator deployment goals,implemented versus candidate/deferred/runtime-unknown |
| [Finish plan](FINISH-PLAN.md)         | Work packages,acceptance gates,CVP/MVP/finite maturity ranges and conditional calendar scenario  |
| [Evidence and decisions](EVIDENCE.md) | Source/release/PR lineage,existing audit reuse and unresolved feedback                           |

## Finite product purpose

The owned harness should install and resume a controlled multi-step task with correct durable state,scoped tools,no unauthorized mutations,bounded child ownership and a measured incremental advantage over its upstream base. The latest deployment direction additionally requires reliable Windows/Herdr operation,a preserved migration from the laptop to the stronger host,and named manager/lane ownership with demonstrated overflow and result return.

Jcode runtime evidence,KCode source evidence and KCode installed evidence remain separate throughout this set. Desktop is a separate repository per current AGENTS;this set does not silently absorb desktop/mobile products or authorize shared-service replacement.

| Horizon                      | Proposed acceptance slice                                                                                                                                                              | Remaining effort               |
| ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------ |
| Core value proposition (CVP) | Correct isolated owned harness lifecycle,Windows/Herdr fault-path qualification,preserved session migration,bounded permissions/children and demonstrated manager/overflow work return | 20-46 engineer-days            |
| Minimum usable product (MVP) | CVP plus native compact-terminal/accessibility usability,repeatable upstream comparison and release/rollback/consumer evidence                                                         | 25-58 engineer-days cumulative |
| Full finite mapped maturity  | MVP plus explicitly retained advanced discovery/ACP/poke/ambient/automation obligations and regression profiles                                                                        | 29-70 engineer-days cumulative |

These low-confidence work ranges are not completion percentages or committed dates. Existing candidate reuse and access to native hosts are assumed. One engineer-day is about eight focused hours including review and acceptance. See the plan for dependency and calendar assumptions. No wider capability is retired by selecting a first value slice.

## Assessment boundary

No product builds/tests,installation,daemon restart,session migration,prompts or captures were performed. An owned remote fetch updated tracking refs only;the canonical detached checkout and installed profiles were preserved. Publication is additive documentation to `KooshaPari/KCode` only,never upstream. Existing full-state audit PR106 is complementary evidence and is not rewritten here.
