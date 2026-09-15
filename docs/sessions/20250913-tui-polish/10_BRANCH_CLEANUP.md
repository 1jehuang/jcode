# Remote Branch Cleanup Report

**Date:** 2026-09-15
**Repository:** KooshaPari/jcode (fork)
**Branch analyzed:** master

## Summary

| Metric | Count |
|--------|-------|
| Total branches deleted | 21 |
| Agent branches deleted | 5 |
| Stale feature/fix branches deleted | 16 |
| Branches kept | 7 |
| All kept branches <10 days old | ✓ |

## Deleted Branches

### Agent Branches (5) - Automated, temporary, safe to delete
| Branch | Age |
|--------|-----|
| origin/agent/release-v0.71.1 | 37 days |
| origin/agent/sdk-release-followup | 38 days |
| origin/agent/triage-2026-08-07 | 38 days |
| origin/agent/triage-2026-08-07-pr | 39 days |
| origin/agent/triage-safe-fixes-20260813 | 29 days |

### Feature Branches (3) - Abandoned, >30 days old
| Branch | Age |
|--------|-----|
| origin/feat/computer-observability-348 | 97 days |
| origin/feat/issue-664-auto-poke-config | 47 days |
| origin/feat/windows-setup-copilot-key | 59 days |

### Fix Branches (10) - Abandoned, >30 days old
| Branch | Age |
|--------|-----|
| origin/fix-set-route-model-alias | 95 days |
| origin/fix/computer-tool-schema-and-element-at | 80 days |
| origin/fix/installer-path-idempotency | 49 days |
| origin/fix/issue-543-mcp-format | 54 days |
| origin/fix/menubar-root-sessions | 58 days |
| origin/fix/skill-invocation-multi-word-619 | 49 days |
| origin/fix/soft-interrupt-images | 49 days |
| origin/fix/stream-first-byte-timeout | 49 days |
| origin/fix/transport-retry-classification | 98 days |
| origin/fix/windows-global-jcode-path | 59 days |

### iOS Branches (2) - Abandoned, >30 days old
| Branch | Age |
|--------|-----|
| origin/ios/mobile-real-nav | 37 days |
| origin/ios/ux-production | 53 days |

### Other (1) - Abandoned, >30 days old
| Branch | Age |
|--------|-----|
| origin/jcode/configurable-colors | 47 days |

## Kept Branches (7)

All branches kept are within 10 days of last commit (actively developed):

| Branch | Age | Reason |
|--------|-----|--------|
| origin/fix/api-attach-preserve-cwd | 9 days | Recent activity |
| origin/fix/desktop-observer-turn-completion | 7 days | Recent activity |
| origin/fix/fix-responses-api | 3 days | Very recent |
| origin/fix/issue-triage-20260907 | 7 days | Recent activity |
| origin/fix/issue-triage-20260910-pr | 4 days | Very recent |
| origin/fix/native-reasoning-markdown | 7 days | Recent activity |
| origin/oauth-api-equivalent-usage-20260907 | 8 days | Recent activity |

## Execution Notes

1. **Pre-flight:** Killed stale cargo/rustc processes before starting
2. **Fetch:** Ran `git fetch origin --prune` to sync remote state
3. **Agent branches:** All 5 deleted successfully (automated/temporary)
4. **Stale branches:** All 16 branches >30 days old deleted
5. **Safety:** No branches with commits within 7 days were touched
6. **No force deletion:** All deletions were normal `git push --delete`

## Upstream Branches

39 `upstream/*` branches exist but were NOT modified (not our fork to clean).
These include agent, arch, docs, feat, fix, ios, and misc branches from the upstream repository.
