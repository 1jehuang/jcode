# Memory Architecture Design

> **Status:** Ready for Implementation
> **Updated:** 2026-01-19

All cost and latency thresholds have been met. Local embeddings + Haiku sidecar are viable now.

## Overview

A multi-layered memory system for cross-session learning that mimics how human memory works - relevant memories "pop up" when triggered by context rather than requiring explicit recall.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Main Agent (Opus)                         │
│  - Has explicit memory tools (remember/recall/search)       │
│  - Can be interrupted by sidecar with relevant info         │
└──────────────▲──────────────────────────▲───────────────────┘
               │                          │
     explicit  │               sidecar    │
     tool use  │               interrupt  │
               │                          │
┌──────────────┴────────┐    ┌────────────┴───────────────────┐
│   Memory Tools        │    │   Memory Sidecar (Haiku/fast)  │
│   (direct access)     │    │   - Spins up on embedding hit  │
│                       │    │   - Does session_search        │
│                       │    │   - Verifies relevance         │
│                       │    │   - Follows metadata links     │
└───────────────────────┘    └────────────▲───────────────────┘
                                          │ trigger
                                          │
              ┌───────────────────────────┴───────────────────┐
              │        Embedding Process (background)          │
              │  - Runs occasionally while Opus works         │
              │  - Embeds current context snippet             │
              │  - Quick similarity search                    │
              │  - Triggers sidecar if hits found             │
              └───────────────────────────▲───────────────────┘
                                          │
              ┌───────────────────────────┴───────────────────┐
              │              Memory Store                      │
              │  - Categorized memories with embeddings       │
              │  - Rich metadata for source tracking          │
              │  - Project-local and user-global scopes       │
              └───────────────────────────────────────────────┘
```

## Components

### 1. Memory Store

**Location:** `~/.jcode/memory/`

```
~/.jcode/memory/
├── projects/
│   └── <project_hash>.json    # Per-directory memories
├── global.json                # User-wide memories
└── embeddings/
    └── <memory_id>.vec        # Embedding vectors
```

**Memory Entry Schema:**

```rust
struct MemoryEntry {
    id: String,
    content: String,
    category: MemoryCategory,      // fact, preference, entity, correction

    // Metadata for source tracking
    session_id: Option<String>,    // Where it came from
    message_range: Option<(u32, u32)>,  // Message indices for context
    file_paths: Vec<String>,       // Related files

    // Lifecycle tracking
    created_at: DateTime,
    updated_at: DateTime,
    access_count: u32,
    strength: u32,                 // Consolidation count

    // Trust & status
    trust_score: f32,              // 1.0 = user correction, 0.5 = agent inference
    active: bool,
    superseded_by: Option<String>, // If contradicted

    // Embedding
    embedding: Option<Vec<f32>>,
}
```

### 2. Embedding Process

Runs occasionally during main agent work (not continuously).

**Triggers:**
- Every N tokens of output
- On file type changes
- On error encounters
- On architecture/design discussions

**Process:**
1. Extract current context snippet (last few messages + current task)
2. Generate embedding (fast, local model or API)
3. Search memory store for similar vectors
4. If similarity > threshold, trigger memory sidecar

### 3. Memory Sidecar

A cheap, fast model (Haiku-class) that spins up on demand.

**Responsibilities:**
- Receive embedding hit notification
- Do deeper investigation via `session_search` using metadata
- Verify if memory is actually relevant to current context
- Decide whether to interrupt main agent
- Format relevant information for injection

**Decision criteria for interruption:**
- Relevance score > threshold
- Memory is actionable (not just trivia)
- Hasn't been surfaced recently (avoid repetition)

### 4. Main Agent Memory Tools

Direct tools available to the main agent:

```
memory { action: "remember", content: "...", category: "fact|preference|correction", scope: "project|global" }
memory { action: "recall" }           # Get relevant memories for current context
memory { action: "search", query: "..." }
memory { action: "list" }
memory { action: "forget", id: "..." }
```

### 5. End-of-Session Extraction

On session close, automatically extract learnings:

**Prompt to extraction model:**
```
Review this session and extract memories worth preserving:

1. Facts learned about this codebase (architecture, patterns, dependencies)
2. User preferences observed (coding style, conventions, tool preferences)
3. Corrections made by the user (things we got wrong)
4. Lessons learned from mistakes or debugging

For each memory, provide:
- Content (concise statement)
- Category
- Trust level (user_stated, observed, inferred)
- Related files if applicable
```

## Memory Lifecycle

### Decay

Memories decay over time based on category:

| Category    | Half-life | Rationale |
|-------------|-----------|-----------|
| Correction  | 365 days  | User corrections are high value |
| Preference  | 90 days   | Preferences may evolve |
| Fact        | 30 days   | Codebase facts can become stale |
| Observation | 7 days    | Low-confidence inferences |

**Scoring formula:**
```
score = base_relevance
      * e^(-age_days / half_life)
      * (1 + log(access_count + 1))
      * trust_weight
```

**Pruning:** When `score < 0.1`, memory becomes archive candidate.

### Consolidation

When storing a new memory, check for semantic duplicates:

```python
if similarity(new_memory, existing_memory) > 0.85:
    # Merge instead of creating new
    existing.strength += 1
    existing.sources.append(new_memory.session_id)
    existing.updated_at = now
    existing.content = merge_content(existing.content, new_memory.content)
```

Benefits:
- "User prefers tabs" x 50 occurrences = 1 memory with strength 50
- Reduces storage, improves signal-to-noise

### Contradiction Handling

When a new memory contradicts an existing one:

```python
def handle_contradiction(old, new):
    # User corrections always win
    if new.source_type == "user_correction":
        old.superseded_by = new.id
        old.active = False
        return new

    # Higher trust wins
    if new.trust_score > old.trust_score:
        old.superseded_by = new.id
        old.active = False
        return new

    # Same trust: more recent wins
    if new.trust_score == old.trust_score:
        old.superseded_by = new.id
        old.active = False
        return new

    # Otherwise keep both, flag conflict
    new.conflicts_with = old.id
    return new
```

**Key:** Superseded memories are not deleted, just marked inactive. Useful for:
- Debugging ("why did it think X?")
- Recovery ("actually the old way was right")

## Embedding Model Choice

**Decision:** Use a single standardized local model everywhere.

**Model:** `all-MiniLM-L6-v2`
- Size: 80MB
- RAM: ~200MB while running
- CPU latency: 20-40ms
- With NPU/GPU: ~10ms

**Rationale:**
- Small enough to run on any machine (even Raspberry Pi)
- Consistent embeddings across all devices (memories are portable)
- No API dependency (works offline, no cost)
- Hardware acceleration is a bonus, not a requirement
- Well-tested, stable model

**Why not per-machine tailored models:**
- Embeddings wouldn't be comparable across devices
- Can't share/sync memories between machines
- Testing nightmare (need all hardware variants)
- Model versioning issues

**Optional acceleration:**
- Intel NPU via OpenVINO (Lunar Lake, Meteor Lake chips)
- CUDA for NVIDIA GPUs
- Metal for Apple Silicon
- Falls back to CPU if unavailable

## Implementation Phases

### Phase 1: Basic Memory Tools
- [x] Memory store with file persistence (`src/memory.rs`)
- [x] Basic memory tool (`src/tool/memory.rs` - enabled)
- [ ] CLI commands (`jcode memory list`, `jcode memory forget`)
- [x] Re-enable and integrate with agent

**Status:** Implemented
- MemoryTool registered and active
- Added trust levels, consolidation strength, superseded tracking

### Phase 2: End-of-Session Extraction
- [ ] Hook into session close
- [ ] Extraction prompt to summarize learnings
- [ ] Automatic categorization

**Status:** Ready now (uses existing models)

### Phase 3: Embedding Search
- [ ] Integrate `all-MiniLM-L6-v2` via ONNX Runtime
- [ ] Vector storage alongside memory JSON
- [ ] Background embedding process
- [ ] Similarity search

**Status:** Ready now
- Local embeddings: ~30ms, free, consistent
- API embeddings (if needed): $0.00001/call (10x cheaper than threshold)

### Phase 4: Memory Sidecar
- [x] Sidecar agent infrastructure (`src/sidecar.rs` - HaikuSidecar)
- [ ] Session search integration
- [x] Relevance verification (`check_relevance()`)
- [ ] Main agent interruption protocol

**Status:** Implemented
- Claude Haiku 4.5: ~$0.003-0.005/call (context-dependent), <500ms latency
- `HaikuSidecar` provides: `complete()`, `check_relevance()`, `extract_memories()`
- Integrated with MemoryManager via `get_relevant_for_context()`, `extract_from_transcript()`

### Phase 5: Full Integration
- [ ] Decay/pruning background job
- [ ] Consolidation on write
- [ ] Contradiction detection
- [ ] User control UI/CLI

**Status:** After phases 1-4 stable

## Privacy & Security

### Do Not Remember
- API keys, secrets, credentials
- Passwords or tokens
- Personal identifying information
- File contents marked sensitive

### Filtering
Before storing any memory, scan for:
- Regex patterns for secrets (API keys, passwords)
- Files in `.gitignore` or `.secretsignore`
- Content from `.env` files

### User Control
- All memories stored in human-readable JSON
- CLI for viewing/editing/deleting
- Option to disable memory entirely
- Export/import for backup

## Open Questions

1. **Embedding model choice:** Local (llama.cpp) vs API (OpenAI, Voyage)?
2. **Sidecar communication:** How does sidecar interrupt main agent cleanly?
3. **Multi-machine sync:** Should memories sync across devices?
4. **Team sharing:** Should some memories be shareable across a team?

---

*Last updated: 2026-01-19*
