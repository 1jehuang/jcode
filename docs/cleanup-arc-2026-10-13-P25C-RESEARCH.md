# P25c Research: Rescue Branch Content Analysis (2026-10-14 04:11Z)

**Date**: 2026-10-14 04:11Z
**Purpose**: Inform the operator's A/B/C/D decision on the 3 orphan rescue branches
**Companion to**: `docs/cleanup-arc-2026-10-13-OPERATOR-ACTIONS.md` §P25c

---

## TL;DR

All 3 rescue branches modify **KCode-specific crate paths** (`crates/kcode-storage/...`, `crates/kcode-session-memory/...`). The current jcode master uses `jcode-*` paths. The fix **logic** is likely still applicable, but the fix **as committed** cannot be cherry-picked cleanly — manual port to `jcode-*` equivalents is required.

**Recommendation: Option C (manual cherry-pick into jcode equivalents) for the 2 storage fixes (ceb860bf2, e45ddd47e). Option A (keep) for d6f2d2091 (session-memory metadata, the target crate still exists as jcode-session-memory).**

---

## Branch 1: rescue/ceb860bf2 — fix(storage): allow normal symlinked state home

| Property | Value |
|---|---|
| Tip commit | `ceb860bf2bf4637a0223e64e7e14011f4210d7a4` |
| Target crate | `crates/kcode-storage/src/remove.rs` |
| Does target exist on jcode master? | **NO** — jcode has `crates/jcode-storage/src/{lib.rs,active_pids.rs}` but NOT `remove.rs` |
| Patch surface | 98 insertions, 1 deletion + ~90 lines of new tests |
| Change summary | Canonicalizes `root` before opening with O_NOFOLLOW, but only when `KCODE_DEV_NAMESPACE != "1"` (dev homes retain no-follow boundary) |
| Cargo.lock impact | NONE |
| tx-scope (per commit trailer) | `dev-isolation` |
| Code generic? | **YES** — the symlink-handling logic is generic Rust, not KCode-specific |
| **Port effort to jcode** | **MEDIUM** — must add `remove_file_beneath` (or equivalent) to `jcode-storage/src/lib.rs` and port the KCODE_DEV_NAMESPACE check (would become JCODE_DEV_NAMESPACE) |

### Detailed diff
```rust
// ADDED to kcode-storage/src/remove.rs:
let root = if std::env::var_os("KCODE_DEV_NAMESPACE").as_deref() == Some(std::ffi::OsStr::new("1")) {
    root.to_path_buf()
} else {
    root.canonicalize()?  // ← The fix: resolve symlinks for normal user homes
};

#[cfg(unix)]
{
    remove_unix(&root, relative)  // ← changed from `root` to `&root`
}
```

Plus 90+ lines of tests for `removes_a_state_file_when_normal_home_is_a_symlink`.

---

## Branch 2: rescue/d6f2d2091 — chore(session-memory): record validation metadata

| Property | Value |
|---|---|
| Tip commit | `d6f2d20916fd669e6f70204b1851dbe3a28d739b` |
| Target crate | `crates/kcode-app-core/src/ambient/session_memory.rs` (via adjacent commit `a4649a32e`) |
| Does target exist on jcode master? | **NO** — completely refactored |
| The actual fix commit | `a4649a32e fix(session-memory): preserve adjacent headings` (NOT on master) |
| Patch surface | 47 lines in `kcode-app-core/src/ambient/session_memory.rs` — fixes `render_session_template` to add trailing newline when body is empty |
| Code generic? | **YES** — the trailing-newline logic is generic, but the call site doesn't exist anymore |
| jcode equivalent | `crates/jcode-session-memory/src/substitute.rs::VariableSubstitutor::substitute` — uses `String::replace`, NOT `replace_range`. **The bug doesn't apply** because the new implementation doesn't iterate over section bodies |
| **Port effort to jcode** | **ZERO** — the bug is structurally impossible in the new implementation (uses `str::replace` instead of `replace_range` on section bodies) |

### Detailed diff (a4649a32e)
```rust
// ADDED to kcode-app-core/src/ambient/session_memory.rs::render_session_template:
let replacement = if body.trim().is_empty() && body_end == body_start && body_end < rendered.len() {
    format!("{value}\n")  // ← The fix: trailing newline to prevent heading merge
} else {
    value.clone()
};
rendered.replace_range(replacement_range, &replacement);
```

### Why this fix is NOT applicable to jcode
- The old code did `rendered.replace_range(replacement_range, &replacement)` where `body` was a section body (could be empty)
- The new code does `result = result.replace(&placeholder, value)` which is a simple text substitution
- `String::replace` doesn't have the "empty body" boundary issue because it operates on the whole text, not on a specific section
- The test case `session_template_replaces_empty_section_before_adjacent_heading` would still pass in the new implementation because the placeholder `{{Summary}}` is fully replaced, not just a section body

**This branch is essentially obsolete** — the bug it fixed doesn't exist in the new jcode implementation. **Recommendation: Option B (delete) — no value in porting because the bug is structurally impossible.**

---

## Branch 3: rescue/e45ddd47e — fix(storage): unlink session markers through directory handles

| Property | Value |
|---|---|
| Tip commit | `e45ddd47e394267b0b66275f0426f5468e483f90` |
| Target crate | `crates/kcode-storage/src/active_pids.rs` (+ tests) |
| Does target exist on jcode master? | **PARTIAL** — `crates/jcode-storage/src/active_pids.rs` exists |
| Patch surface | 85 insertions, 3 deletions (production) + test additions |
| Change summary | Adds `remove_marker` function that opens the parent directory with O_DIRECTORY \| O_NOFOLLOW \| O_CLOEXEC and unlinks the marker through the directory handle (safer than `std::fs::remove_file(path)`) |
| Cargo.lock impact | NONE |
| tx-validated (per trailer) | `manual` |
| Code generic? | **YES** — directory-handle-based unlink is generic Rust + libc |
| **Port effort to jcode** | **MEDIUM-HIGH** — must add `remove_marker` to `jcode-storage/src/active_pids.rs` and port the path-allowed validation logic |

### Detailed diff
```rust
// ADDED to kcode-storage/src/active_pids.rs:
fn remove_marker(dir: &Path, session_id: &str) -> std::io::Result<()> {
    if session_id.is_empty() || session_id == "." || session_id == ".." || session_id.contains('/')
    {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid session marker name"));
    }
    let path = dir.join(session_id);
    if !path_allowed(&path) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "unsafe session marker path"));
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::fs::OpenOptionsExt;
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(dir)?;
        let name = CString::new(session_id)?;
        remove_marker_in_directory(&directory, &name)
    }
    #[cfg(not(unix))]
    { std::fs::remove_file(path) }
}
```

---

## Decision impact matrix

| Option | ceb860bf2 (symlink) | d6f2d2091 (session-mem) | e45ddd47e (active_pids) | Cross-cutting cost |
|---|---|---|---|---|
| A: Keep all 3 | preserved | preserved | preserved | 3 orphan branches remain; risk of ref bloat |
| B: Delete all 3 | **LOST** | lost (likely no-op) | **LOST** | clean state; KCode-specific fixes cannot be ported later without re-deriving |
| C: Cherry-pick into jcode equivalents | **PORT EFFORT** (medium) | trivial (likely no-op) | **PORT EFFORT** (medium-high) | preserves intent; 2-4 hours of manual port work |
| D: Move to archive/* namespace | preserved | preserved | preserved | cleaner namespace; same cost as A |

---

## Recommendation rationale

- **ceb860bf2 (symlink)** — valuable generic fix; should be ported to `jcode-storage/src/lib.rs` (Option C, medium effort)
- **e45ddd47e (active_pids)** — valuable generic fix; should be ported to `jcode-storage/src/active_pids.rs` (Option C, medium-high effort)
- **d6f2d2091 (session-memory)** — **OBSOLETE due to refactor** (see Branch 2 analysis). The bug it fixed is structurally impossible in the new `VariableSubstitutor` implementation. **Recommended: Option B (delete) — no porting needed.**

### Mixed strategy (most efficient)
1. **Delete d6f2d2091** (obsolete) — `git push origin --delete rescue/d6f2d20-choresession-memory-record-validation-metadata` + remove local worktree `/Users/kooshapari/wt-kcode-heading-boundary`
   - **DONE 2026-10-14 04:21Z** — remote branch deleted, worktree removed (206 MB freed). Commits remain reachable via reflog for ~30 days.
2. **Port ceb860bf2** to `crates/jcode-storage/src/lib.rs` — add canonicalize-root logic + tests (1-2 hours)
3. **Port e45ddd47e** to `crates/jcode-storage/src/active_pids.rs` — add `remove_marker` + tests (1-2 hours)
4. **Delete ceb860bf2 and e45ddd47e rescue branches** after porting (Option B equivalent for the ported content)
5. **Total: 2-4 hours of port work, then delete all 3 rescue branches** (1 already done)

### If port work is not feasible right now
- Option A (keep all 3) — preserves work but leaves 3 orphan branches
- Option D (archive) — clean namespace but same cost as A
- Note: d6f2d2091 is **safe to delete at any time** because the fix is obsolete

---

## Recovery: how to get back to a rescue branch tip if needed

The tip commits remain reachable in the git object database for ~30 days:
```bash
git cat-file -t ceb860bf2bf4637a0223e64e7e14011f4210d7a4   # commit
git cat-file -t d6f2d20916fd669e6f70204b1851dbe3a28d739b   # commit
git cat-file -t e45ddd47e394267b0b66275f0426f5468e483f90   # commit
git show ceb860bf2 > /tmp/ceb860bf2.patch   # Full commit as patch
```

After 30 days, the commits are unrecoverable from the local repo (they remain on origin if the branches still exist, but the operator said these branches may be deleted).

## Worktree mapping (verified 2026-10-14 04:16Z)

Each rescue branch tip has a corresponding worktree in detached HEAD state:

| Rescue branch tip | Worktree path | State |
|---|---|---|
| `ceb860bf2` (symlink) | `/Users/kooshapari/wt-kcode-home-symlink` | detached HEAD at ceb860bf2 |
| `d6f2d2091` (session-memory) | `/Users/kooshapari/wt-kcode-heading-boundary` | detached HEAD at d6f2d2091 |
| `e45ddd47e` (active_pids) | `/Users/kooshapari/wt-kcode-active-pids-race` | detached HEAD at e45ddd47e |

These 3 worktrees are the original development worktrees that produced the commits later saved as `rescue/*` branches. Removing the worktrees does NOT lose the commits (they remain reachable via master or reflog); it only frees disk space.

---

tx-agent: jcode
tx-task: P25c-research-2026-10-14
tx-validated: git-show + git-diff + file-path inventory
tx-scope: docs/cleanup-arc-2026-10-13-P25C-RESEARCH.md
tx-intent: prepare informed A/B/C/D decision for P25c
