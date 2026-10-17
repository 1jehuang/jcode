# Evidence, fork boundaries and unresolved feedback

Observed 2026-10-16 Pacific. Publication destination is **KooshaPari/KCode**,default branch `master`. All GitHub PR operations explicitly target that owned repository. No upstream write is authorized or performed.

## Source and runtime provenance

| ID  | Evidence                                                                                                                                                                                                                         | Meaning and limit                                                                                                                     |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| E01 | [Assessed master](https://github.com/KooshaPari/KCode/commit/6cf71ed4efb9b1f256b2fcd00e95748309fa6d0b)                                                                                                                           | Refreshed owned source;canonical checkout remained detached at748eeaa1e                                                               |
| E02 | [Merged PR98](https://github.com/KooshaPari/KCode/pull/98),merge6cf71ed4e;[update product source](https://github.com/KooshaPari/KCode/blob/6cf71ed4efb9b1f256b2fcd00e95748309fa6d0b/crates/jcode-app-core/src/update_product.rs) | Explicit self-update disablement and storage/model separation;source is not installed proof                                           |
| E03 | [Merged PR95](https://github.com/KooshaPari/KCode/pull/95),merge6b2361a0aa21a05fd3f5029927705e51d05de731                                                                                                                         | Windows controlling-terminal abandonment handling;does not alone qualify original keyboard/anchor issue                               |
| E04 | [Merged PR93](https://github.com/KooshaPari/KCode/pull/93),merge bb22754a0                                                                                                                                                       | Config-declared model picker work;effective served model still needs runtime receipt                                                  |
| E05 | [Windows candidate PR97](https://github.com/KooshaPari/KCode/pull/97),[fork issue45](https://github.com/KooshaPari/KCode/issues/45),[upstream issue1812](https://github.com/1jehuang/jcode/issues/1812)                          | Decoder/ConPTY/input investigation;upstream non-reproduction is not proof of an installed repair                                      |
| E06 | [Closed PR94](https://github.com/KooshaPari/KCode/pull/94),[issue104](https://github.com/KooshaPari/KCode/issues/104),[open PR96](https://github.com/KooshaPari/KCode/pull/96)                                                   | Prior fork-detection/override/size feedback and overlapping update candidate;reconcile against merged98                               |
| E07 | [Owned release](https://github.com/KooshaPari/KCode/releases/tag/v0.85.0-k1.0.0),[upstream0.98.1](https://github.com/1jehuang/jcode/releases/tag/v0.98.1)                                                                        | Different products/releases;neither proves installed source6cf71 or native behavior                                                   |
| E08 | [Repository guidelines](../../../AGENTS.md),[project handoff](../../PROJECT_OWNERSHIP_HANDOFF.md)                                                                                                                                | Correct ownership/install/daemon boundaries;historical path examples require actual runtime resolution                                |
| E09 | [Herdr contract](../../HERDR.md),[integration document](../../HERDR_INTEGRATION.md),[multi-session architecture](../../MULTI_SESSION_CLIENT_ARCHITECTURE.md)                                                                     | Source/planning contracts;blocked-state authority and external integration requirements are not all guaranteed by lifecycle reporting |
| E10 | [Complementary full-state audit PR106](https://github.com/KooshaPari/KCode/pull/106)                                                                                                                                             | Existing detailed multi-lane investigation;reuse its exact receipts and keep author claims distinct from independent acceptance       |
| E11 | [Latest completed listed CI failure](https://github.com/KooshaPari/KCode/actions/runs/38548850946),source29c85cbdf7e9c33bf610167dd35222348495545b                                                                                | Stale source CI,not a current6cf71 build result;no paid-CI remedy implied                                                             |

Earlier local observation recorded installed Jcode0.98.1 at1c7426eb1 and a different installed KCodebb6174b21a. This task rechecked launcher resolution and file sizes,not executable version or daemon behavior. No new product command,update,restart or session mutation was run. Existing local test claims belong to their original exact snapshots and cannot qualify either current installed product.

## Upstream divergence limits

Cached upstream/master and a read-only queried upstream head both identified `6f2926e0c63d885f5858daed5246566a7838e948` during this assessment. The cached merge-base with assessed owned source was `752df77d3c13fa7a648eda257dbfcb8d3ea5974d`;history comparison reported2838 upstream-only and338 owned-only commits. These are graph counts,not a measured number of unique semantic patches or proof that every upstream change is required.

PR98's integration changed a large source/dependency surface relative to the earlier748ee snapshot. Do not equate a merged isolation fix with a tiny deployment-risk delta or assume old gate receipts cover the new tree. Preserve an explicit retained-patch/consumer ledger,borrow upstream improvements where valid and measure carry cost before another broad convergence. Upstream remains read-only;all publication is owned-fork-only.

## Intent provenance and privacy

Core contract was summarized from the local docs5 KCode dossier:owned native install/resume,fault workload correctness,tool permissions,child lifetime,updater identity and incremental upstream advantage. Latest deployment direction was summarized from local archive `jcode-prompts-2026-10-16.zip`,member `jcode-prompts/jcode-hermes-delegation.md`:Windows/Herdr repair,preserved stronger-host migration,live manager/lane map,bounded dispatch and demonstrated overflow/results through two paths. Raw private prompts,logs,credentials and session contents are not published here.

The local folder's old jcode name is not an authority to write upstream,replace Jcode profiles or present KCode source tests as upstream runtime verification. User-authorized docs publication is separate from any future binary promotion or migration authority.

## Existing feedback and intent decisions

| Topic                                            | Existing item                                               | Required resolution                                                                      |
| ------------------------------------------------ | ----------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| Windows input/resume versus terminal abandonment | Issue45,PR97,merged95                                       | Reproduce exact profile and keep failure classes distinct                                |
| Updater/profile collision                        | Merged98,open96,closed94,issue104                           | Qualify installed denial/isolation/rollback;avoid resurrecting rejected detection logic  |
| Honest ambient/dream behavior                    | PR99                                                        | Distinguish parked/disabled from actual consolidation;no placeholder success             |
| Swarm rename wire-up                             | PR105                                                       | Review actual dispatch/result identity,not UI-only rename                                |
| Required CI crate coverage                       | PR100 and stale Actions state                               | Final candidate local gate denominator,then native proof;no billing dependency           |
| Provider count/config/model drift                | PR101,merged93/98,issue102                                  | Separate documentation counts,picker list and effective provider route                   |
| Git commit identity                              | Issue87 and current AGENTS                                  | Preserve configured user identity;ledger agent trailers must not replace author config   |
| Compact resume/ACP/poke                          | Existing history,PR10/25                                    | Retain only accepted consumer promises and qualify actual installed journey              |
| Migration/manager/overflow                       | Latest private source direction and public integration docs | Concrete identity map,task-return receipts and unambiguous authority/resource boundaries |
| Fork feature retirement                          | Existing audit/patch ledger                                 | Explicit consumer-qualified decision;no sunset or deletion authorized by this set        |

Fresh owned Dependabot query returned no open alerts;earlier source reports listed alerts. Neither result is an exploitability/native safety verdict. No security secret material was fetched. No old source result is silently upgraded to current proof.

## Documentation validation

Five additive Markdown files only. Publication receipt records formatting,local link/ASCII/privacy/conflict/file-size checks,estimate arithmetic and staged diff validation. Hooks are not bypassed or reconfigured. Product compile/lint/test/native suites are not newly asserted. Existing profile/session/daemon and other agents' worktrees remain preserved.
