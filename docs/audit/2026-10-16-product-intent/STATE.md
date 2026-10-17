# Source/runtime identity and actual user journey

Observed 2026-10-16 Pacific. Current assessed owned source: `6cf71ed4efb9b1f256b2fcd00e95748309fa6d0b`.

## Identity map

| Object                                        | Observed or explicitly historical identity                                                 | Acceptance limit                                                                            |
| --------------------------------------------- | ------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------- |
| Canonical directory historically called jcode | Origin `KooshaPari/KCode`;detached checkout `748eeaa1e1f45a87a5d6d230963c6ad98495525b`     | Preserved;not automatically updated to remote master                                        |
| Refreshed owned master                        | `6cf71ed4efb9b1f256b2fcd00e95748309fa6d0b`                                                 | Documentation worktree source;not installed runtime                                         |
| Installed Jcode path                          | `jcode` wrapper -> `jcode.real` -> `~/.jcode/builds/versions/0.98.1/jcode`,138378816 bytes | Earlier bounded version receipt says `jcode v0.98.1 (1c7426eb1)`;no new execution here      |
| Installed KCode path                          | `kcode` -> `~/.kcode/builds/versions/bb6174b21a/kcode`,110365376 bytes                     | Different installed candidate;not current source6cf71 or installed Jcode                    |
| Long-lived Jcode daemon                       | Earlier process receipt observed stable-path serve and laptop resume clients               | A resolved launcher is not the daemon's loaded-build handshake;not restarted or pinged here |
| Latest listed owned release                   | `v0.85.0-k1.0.0`,published2026-09-15                                                       | Historical KCode/fork artifact;not current native qualification                             |
| Upstream Jcode release                        | Earlier query lists `v0.98.1`,published2026-10-15                                          | Version correspondence supports provenance lead,not fork benefit or Windows behavior        |

Root AGENTS correctly warns that fresh compilation does not qualify a long-lived daemon. Its shared-server-path example was absent in the earlier local observation while a stable-path daemon existed. Resolve the actual running process/socket/build identity before any future runtime change. Do not reconfigure either profile to make this documentation agree with an older convention.

## User journey

```text
resolve product+artifact -> isolated install/provider setup -> submit task
                                                        |
                        tool permissions -> parent/child ownership
                                                        |
                disconnect/rate limit/cancel -> durable session state
                                                        |
                         restart/resume -> correct bounded continuation
                                                        |
          named manager/lane -> overflow host/pool -> result -> acceptance
```

| Stage                             | Current source or prior evidence                                              | Remaining user-facing proof                                                                                              |
| --------------------------------- | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| Install/update without collision  | PR98 merged;source disables KCode self-update and changes storage/model paths | Installed fork must preserve Jcode launcher,profile,sessions and updater channel under failure/rollback                  |
| Select intended provider/model    | Providers/config/picker infrastructure;PR93 and PR98 merged                   | Effective served route under config changes,quota,retry/fallback and credential isolation                                |
| Submit in Windows/Herdr           | Terminal integration source;PR95 detects abandoned controlling terminal       | Exact failing launch/resume/resize/Enter sequence still not bound to a passing installed artifact;PR97 remains candidate |
| Persist and resume                | Storage/session/compaction source and extensive inherited tests               | Restriction preservation,no duplicate effects after uncertain completion,corruption/reload/cancel fault receipt          |
| Parent/child tools                | Swarm/tool/permission components exist                                        | Child ends with parent when required,cleanup is observed,denial cannot be bypassed and side effects reconcile            |
| Migrate sessions to stronger host | Shared/multi-session/SSH architecture source                                  | Complete source->destination session map,original preservation,authority,rollback and remaining laptop sessions          |
| Manager and overflow dispatch     | Integration/swarm primitives and configured pools                             | Authoritative manager->lane->host map plus one real overflow/result-return witness through both required paths           |
| Claim usable advantage            | Inherited README efficiency claims and comparison intent                      | Matched owned/upstream workload,tail latency,memory,recoveries and forbidden-action negatives on exact binaries          |

## Recent movement and current blockers

PR95 merged at `6b2361a0aa21a05fd3f5029927705e51d05de731`;PR93 merged at `bb22754a0`;PR98 merged as the assessed tip. Earlier reports calling these open are superseded. PR94 was closed unmerged;issue104 documents its rejected detection/override/size-budget concerns. Source `update_product.rs` now explicitly disables both automatic and explicit KCode self-update independently of semver/environment/renaming. That is a stronger source boundary,but installed behavior remains separate.

Windows terminal abandonment and keyboard-decoder negotiation are different failure classes. Neither upstream non-reproduction of issue1812 nor PR95 merge establishes the original inline input/Enter anchor repair. PR97 and fork issue45 remain relevant. No new reproduction or smoke was launched here.

Earlier local gate/test receipts and old TUI failures belong to their exact snapshots. Do not repeat resolved size/fmt/warning blockers or import author-reported tests from a candidate into the installed product. Current owned Actions listings expose stale failure/cancellation evidence,not a green test run for6cf71. Local required checks are the free-tier fallback;no billing upgrade is required.
