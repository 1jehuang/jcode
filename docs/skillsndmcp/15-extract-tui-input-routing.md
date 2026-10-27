# Task 3.3: Extract TUI Input Routing Logic

## File Tree
```
Task 3.3 (Extract TUI Input Routing)
├── Depends On: Phase 2 Synthesis (09-12)
├── Used By: Phase 4 Testing
├── Uses Modules:
│   ├── Module boundaries from Task 2.2
│   └── API contracts from Task 2.3
├── Produces:
│   ├── tui_input_router.rs (< 500 lines)
│   ├── tui_skill_activator.rs (< 500 lines)
│   └── tui_snapshot_refresher.rs (< 500 lines)
```

## Objective
Extract TUI input routing into reusable modules under 500 lines each with comprehensive documentation headers.

## Sub-Sub-Agent Assignments

### Agent A: Extract and Modularize Local TUI Input Routing
**Goal**: Create `tui_input_router.rs` with `submit_input_with_routing` logic
**Questions**:
1. How do we extract the skill invocation detection from `jcode-tui/src/tui/app/input.rs`?
2. What is the flow from `resolve_invocation` to `active_skill` setting?
3. How does it handle endorsed-but-not-installed skill hints?
4. What happens when a skill is not found in the initial snapshot?
**Output**: Modularized local input routing module with documentation header

### Agent B: Extract and Modularize Remote TUI Input Routing
**Goal**: Create remote-specific routing in `tui_input_router.rs`
**Questions**:
1. How do we extract `submit_remote_slash_input` from `jcode-tui/src/tui/app/remote/input_dispatch.rs`?
2. How does it determine if input is "command-shaped"?
3. How does it prepare only the trailing prompt for remote submission?
4. How does it preserve pasted images attached to the same user turn?
**Output**: Modularized remote input routing module with documentation header

### Agent C: Extract and Modularize Skill Activation State Management
**Goal**: Create `tui_skill_activator.rs` with `active_skill` state handling
**Questions**:
1. How do we extract the `active_skill` state management logic?
2. How is `active_skill` serialized in the wire protocol?
3. How does it validate skill names against the registry?
4. How does it show activation notices to the user?
**Output**: Modularized skill activation module with documentation header

### Agent D: Extract and Modularize Snapshot Refresh Logic
**Goal**: Create `tui_snapshot_refresher.rs` with `refresh_skills_snapshot` logic
**Questions**:
1. How do we extract `refresh_skills_snapshot` from `jcode-tui/src/tui/app/state_ui_runtime.rs`?
2. Why does it skip refresh in SSH remote mode?
3. How does `SkillRegistry::load_global()` read ONLY global skills?
4. Why does it NOT load project-local skills here (session-scoped, issue #457)?
**Output**: Modularized snapshot refresh module with documentation header

## Validation Requirements
- Verify each extracted file is under 500 lines
- Check that all TUI tests pass after extraction
- Ensure documentation headers are accurate
- Validate that local vs remote routing differences are preserved

## Deliverables
1. `tui_input_router.rs` (< 500 lines) with documentation header
2. `tui_skill_activator.rs` (< 500 lines) with documentation header
3. `tui_snapshot_refresher.rs` (< 500 lines) with documentation header
4. Updated dependency graphs showing new module relationships
5. Local vs remote routing comparison table
