---
name: feature-planning
description: >
  Deep feature research and implementation planning for AI coding agent projects. Use this skill
  whenever a user asks about a feature they want to implement, improve, add, or design — especially
  in the context of AI coding agents, CLI tools, terminal agents, or LLM-powered developer tools.
  Triggers on: "I want to add X feature", "how do I implement X", "can we improve X", "I want to
  build X into my agent", "feature request for X", "how does X work in these tools", or any phrasing
  that implies implementing/improving a capability. This skill clones 7 reference repos, spawns
  sub-agents for deep per-repo research, runs an ultra-QA interview with the user, then produces a
  comprehensive implementation plan with code, pseudocode, test cases, benchmarks, and direct repo
  links — so the user can go from idea to working implementation with total confidence.
---

# Feature Planning Skill

Comprehensive feature research + implementation planning using 9 reference repos as the knowledge base.

## Reference Repositories

| Alias | Repo URL | Stack | What it teaches |
|-------|----------|-------|-----------------|
| `oh-my-openagent` | https://github.com/code-yeongyu/oh-my-openagent | TypeScript / OpenCode plugin | Multi-agent orchestration, model routing, tmux sessions, delegate-task patterns |
| `opencode` | https://github.com/anomalyco/opencode | TypeScript / Bun monorepo | Open-source AI coding agent architecture, provider abstraction, TUI |
| `oh-my-pi` | https://github.com/can1357/oh-my-pi | TypeScript + Rust / Bun | 40+ providers, 32 tools, LSP+DAP ops, benchmarked edits, IDE wiring |
| `codebuff` | https://github.com/CodebuffAI/codebuff | TypeScript / multi-agent | File picker + planner + editor + reviewer pipeline, beats Claude Code on evals |
| `codex` | https://github.com/openai/codex | TypeScript / Node | OpenAI Codex CLI, sandboxed execution, hardened tool use |
| `claude-code` | https://github.com/claude-code-best/claude-code | TypeScript / Bun | CCB — decompiled Claude Code with Pipe IPC, ACP, remote control, monitoring |
| `pi-agent-rust` | https://github.com/Dicklesworthstone/pi_agent_rust | Rust 2024 edition | High-perf Rust agent, SQLite sessions, SSE streaming, WASM extension security |
| `oh-my-claudecode` | https://github.com/Yeachan-Heo/oh-my-claudecode | TypeScript / Claude Code plugin | Claude Code extension with hooks, guards, permission modes, multi-agent tools |
| `oh-my-codex` | https://github.com/Yeachan-Heo/oh-my-codex | TypeScript / Codex plugin | Codex extension with approval modes, sandbox config, tool gating |

---

## Workflow (follow this order every time)

### Phase 1 — Clone & Sub-agent Research

When the skill is triggered, immediately clone all 9 repos (shallow `--depth=1`) and spawn one research sub-agent per repo. Each sub-agent gets the full repo and the feature request — its job is to autonomously explore **the entire repo** to find everything relevant. The sub-agent decides what to read; nothing is off-limits and nothing is assumed to be the right place to look.

Each sub-agent should:

1. **Map the repo first** — list all files and directories to understand the full shape before diving in. No assumptions about where things live.
2. **Follow the feature signal** — search for keywords, types, patterns, and concepts related to the requested feature across every file, every directory, every language. If a Rust file has relevant logic, read it. If a config YAML has relevant keys, read it. If a test file shows how a concept is used, read it. If a benchmark shows performance constraints, read it.
3. **Trace implementations end-to-end** — when a relevant function/type/module is found, follow its call chain in both directions (callers and callees) until the full picture is clear. Don't stop at the first hit.
4. **Extract everything useful** — architecture patterns, API surfaces, data structures, config hooks, test patterns, benchmark approaches, error handling strategies, extension points, anything that could inform the feature design.
5. **Return a structured summary** (see **Sub-agent Report Format** below)

The sub-agent must NOT limit itself to any predefined set of files or folders. If it finds something unexpected in an unusual location, it should read it. Thoroughness is the goal.

Run sub-agents in parallel. Collect all 9 reports before continuing.

```bash
# Clone command template
for repo in \
  "https://github.com/code-yeongyu/oh-my-openagent" \
  "https://github.com/anomalyco/opencode" \
  "https://github.com/can1357/oh-my-pi" \
  "https://github.com/CodebuffAI/codebuff" \
  "https://github.com/openai/codex" \
  "https://github.com/claude-code-best/claude-code" \
  "https://github.com/Dicklesworthstone/pi_agent_rust" \
  "https://github.com/Yeachan-Heo/oh-my-claudecode" \
  "https://github.com/Yeachan-Heo/oh-my-codex"; do
  git clone --depth=1 "$repo" /tmp/feature-research/$(basename $repo)
done
```

#### Sub-agent Report Format

Each sub-agent returns a structured block:

```
## [repo-name] Research Report

### Relevance Score: [HIGH / MEDIUM / LOW / NONE]
### Why relevant: [1-2 sentences]

### Key Files
- path/to/file.ts — [what it does re: the feature]

### Relevant Code Snippets
[short excerpts with file:line references]

### Architecture Pattern
[how this repo approaches the feature domain]

### Direct Links
- https://github.com/[org]/[repo]/blob/main/[file]#L[line]

### Gaps / What's Missing
[what this repo doesn't cover that the user might need]
```

---

### Phase 2 — Present Per-Repo Report to User

After collecting sub-agent reports, present a consolidated **Research Report** to the user with one section per repo. Format:

```
# Feature Research: [FEATURE NAME]

## Summary
[2-3 sentence overview of what you found across all repos]

---

## 1. oh-my-openagent
[sub-agent report content]

## 2. opencode
...

## 7. pi-agent-rust
...

---

## Cross-Repo Patterns
[What approaches are consistent across repos — these are proven patterns]

## Unique Insights
[Interesting divergences or novel approaches from individual repos]
```

---

### Phase 3 — Ultra QA Interview

After presenting the research report, enter a deep QA loop with the user. Ask questions in rounds — never dump all questions at once. Use this question bank, picking the most relevant ones for the feature at hand:

**Round 1 — Scope & Goal**
- What is the exact outcome you want after implementing this? (demo it to me in words)
- Is this a new feature or improving an existing one? If existing, what's broken/missing?
- Which repo(s) are you building in / most inspired by?
- What stack? (TypeScript, Rust, Python, other)

**Round 2 — Constraints & Context**
- What existing code does this feature touch or depend on?
- Are there performance requirements? (latency targets, memory limits, throughput)
- Security constraints? (sandboxing, capability gating, trust levels)
- Will this need to work across multiple LLM providers or just one?

**Round 3 — Design Preferences**
- Do you prefer a plugin/extension architecture or embedded implementation?
- Should this be synchronous, async, or streaming?
- How should failures be handled? (silent fallback, hard error, user prompt)
- How will users configure or toggle this feature?

**Round 4 — Testing & Quality**
- What does a successful implementation look like? How will you verify it?
- Are there existing tests in the repos we can adapt?
- Any edge cases you're already worried about?

**Round 5 — Stretch Goals**
- What would a "10x better" version of this look like?
- Are there benchmark targets you want to hit?
- Future integrations you want to leave room for?

Keep asking follow-up questions until you have clear answers to at minimum Round 1 and Round 2. Rounds 3–5 can be inferred from research if the user is in a hurry.

---

### Phase 4 — Comprehensive Implementation Plan

After the QA interview, produce the final plan. This is the deliverable the user keeps. It must include ALL of the following sections:

---

```markdown
# Implementation Plan: [FEATURE NAME]
> Generated from research across 9 repos + user interview
> Goal: [User's stated goal in 1 sentence]

---

## 1. Executive Summary
[3-5 sentences: what we're building, why this approach, expected outcome]

---

## 2. Architecture Decision
### Chosen Approach
[Which pattern from the research repos we're following, and why]

### Alternatives Considered
| Approach | Source Repo | Pros | Cons | Decision |
|----------|-------------|------|------|----------|

---

## 3. Data Structures & Types

```typescript  // or Rust, Python, etc.
// Core types for the feature
interface FeatureConfig {
  // ...
}
```

---

## 4. Pseudocode — Core Algorithm

```
FUNCTION implementFeature(input):
  // Step-by-step logic in plain pseudocode
  // No language-specific syntax
  // Shows all branches and edge cases
```

---

## 5. Implementation Code

### File: [path/to/new-or-modified-file]
```typescript
// Full implementation code
// With inline comments explaining non-obvious choices
// References to source repos where patterns were borrowed
```

### File: [path/to/another-file]
```typescript
// ...
```

---

## 6. Configuration & Wiring
[How to register/hook the feature into the existing system]
[Config file changes, env vars, flags]

---

## 7. Repo References

Direct links to the most relevant code in each source repo:

| Feature Aspect | Repo | File | Link |
|----------------|------|------|------|
| [aspect] | oh-my-openagent | src/agents/... | https://github.com/... |
| [aspect] | codebuff | packages/... | https://github.com/... |
| ... | | | |

---

## 8. Test Cases

### Happy Path Tests
```typescript
describe('[feature]', () => {
  it('should [happy case 1]', async () => {
    // setup
    // act
    // assert
  });

  it('should [happy case 2]', async () => {
    // ...
  });
});
```

### Edge Cases
```typescript
  it('should handle [edge case: empty input]', ...);
  it('should handle [edge case: provider failure]', ...);
  it('should handle [edge case: concurrent calls]', ...);
  it('should handle [edge case: large payload]', ...);
  it('should handle [edge case: timeout]', ...);
```

### Integration Tests
```typescript
// End-to-end test that exercises the full flow
```

---

## 9. Benchmarks

### What to Measure
| Metric | Baseline | Target | How to Measure |
|--------|----------|--------|----------------|
| Latency (p50) | - | [Xms] | [method] |
| Latency (p99) | - | [Xms] | [method] |
| Memory delta | - | [XMB] | [method] |
| Throughput | - | [X/s] | [method] |

### Benchmark Code
```typescript
// Benchmark harness adapted from oh-my-pi / pi-agent-rust patterns
```

---

## 10. Migration / Rollout
[If improving existing feature: how to migrate without breaking changes]
[Feature flags, gradual rollout, deprecation path]

---

## 11. Known Limitations & Future Work
- [ ] [Thing not covered in this plan]
- [ ] [Stretch goal for v2]
- [ ] [Integration left for later]

---

## 12. Success Criteria Checklist
- [ ] Core happy path works end-to-end
- [ ] All edge case tests pass
- [ ] Performance meets targets from Section 9
- [ ] No regressions in existing tests
- [ ] [User's specific success criterion from interview]
```

---

## Quality Standards

The plan must meet these bars before presenting to the user:

- **No broken links** — all GitHub links must point to real files in the cloned repos
- **No vague pseudocode** — every step in the pseudocode must be implementable
- **No placeholder tests** — every test case must have real setup/act/assert
- **Benchmark section is never empty** — even if targets are TBD, the measurement method must be specified
- **Every architectural choice has a "why"** referencing a source repo
- **The user should be able to hand this plan to a junior engineer and get working code back**

---

## References

See `references/repo-summaries.md` for static summaries of all 9 repos (useful when cloning is slow or unavailable).