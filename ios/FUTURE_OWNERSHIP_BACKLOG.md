# Future Mobile Ownership Backlog

This document tracks parts of the iOS/mobile stack that we **could potentially own later**, but are **not planning to own in v1**.

These are all areas where Apple does not fundamentally prevent us from taking control, but where the implementation cost, risk, or complexity is too high for the initial simulator-first architecture.

## Current direction

For now, we want to focus on:

- shared mobile core
- shared rendering architecture
- simulator-first automation and logging
- minimal platform shell dependencies

This file is the backlog of areas we may revisit after the base architecture is working.

---

## 1. Text input and editing

### 1.1 Full custom text editor internals
Could own later:
- cursor movement
- selection
- copy/paste handling
- composition behavior
- autocomplete UI
- rich prompt editing

Why not now:
- very hard
- IME and international input are painful
- many edge cases

### 1.2 Full custom keyboard interaction model
Could own later:
- keyboard avoidance behavior
- custom accessory bar behavior
- advanced editor shortcuts
- custom command palette tied to keyboard state

Why not now:
- too tied to platform quirks
- easier to bridge first

---

## 2. Scrolling and gestures

### 2.1 Fully custom scroll physics
Could own later:
- inertial scroll
- rubber banding
- transcript anchoring
- nested scroll coordination
- custom scrollbars

Why not now:
- lots of tuning
- not needed to prove architecture first

### 2.2 Full gesture recognition stack
Could own later:
- gesture arbitration
- drag routing
- swipe gestures
- custom edge gestures
- multi-touch interaction model

Why not now:
- easy to overbuild
- native or host-side bridging is enough early on

---

## 3. Rendering and layout

### 3.1 Complete text shaping and layout engine
Could own later:
- line breaking
- glyph shaping
- truncation
- syntax-aware layout
- markdown and code layout

Why not now:
- huge rabbit hole
- especially tricky cross-platform

### 3.2 Full animation engine
Could own later:
- spring system
- interruptible animations
- timeline-based choreography
- transition graph
- animation debugging tools

Why not now:
- basic animation support is enough first

### 3.3 Full custom compositor and effects stack
Could own later:
- blur pipelines
- layered compositing
- shadow system
- masking and clipping effects
- advanced transitions

Why not now:
- nice-to-have, not core first milestone

### 3.4 Full custom layout engine
Could own later:
- flex or grid equivalent
- intrinsic size resolution
- constraint-like behavior
- virtualized layout
- layout invalidation engine

Why not now:
- likely worth growing into incrementally, not all at once

---

## 4. Navigation and app shell

### 4.1 Complete custom in-app navigation system
Could own later:
- stack navigation
- modals and sheets
- tab system
- deep-link routing
- screen transition manager

Why not now:
- simpler shell and navigation bridge is safer initially

### 4.2 Complete custom modal and popup framework
Could own later:
- alerts
- menus
- action sheets
- overlays
- inspector panels

Why not now:
- native or simple host-driven versions are fine first

---

## 5. Accessibility

### 5.1 Rich accessibility mapping layer
Could own later:
- semantic-to-accessibility bridge
- focus order control
- live region support
- custom actions
- accessibility tree diffing

Why not now:
- important, but should not block initial simulator work
- bridging basics first is safer

### 5.2 Full accessibility-first custom renderer support
Could own later:
- VoiceOver mapping for custom-rendered surfaces
- semantic focus synchronization
- custom accessibility hit testing

Why not now:
- real work
- should be phased after rendering foundation exists

---

## 6. Media and device integrations

### 6.1 Custom camera capture UI and pipeline
Could own later:
- camera preview
- capture UI
- crop tools
- overlays
- multi-step media workflow

Why not now:
- default or native-backed flows are enough early on

### 6.2 Custom microphone and audio recording pipeline UI
Could own later:
- waveform visualizer
- recording states
- playback editor
- trimming UI
- audio session control UX

Why not now:
- not core to first simulator architecture

### 6.3 Custom photo and file picking experience
Could own later:
- custom picker shell
- media gallery UX
- attachment staging area

Why not now:
- not needed to validate the main chat and simulator loop

---

## 7. Input systems

### 7.1 Full custom focus system
Could own later:
- focus graph
- keyboard focus traversal
- responder ownership
- focus memory between screens

Why not now:
- can start with a simpler interaction model

### 7.2 Full custom hit-testing and input routing stack
Could own later:
- overlapping layers
- event capture and bubble model
- custom pointer and touch dispatch

Why not now:
- needed eventually for deep custom rendering
- too early now

---

## 8. Data and tooling

### 8.1 Full offline sync engine
Could own later:
- queued actions
- reconnect reconciliation
- optimistic UI
- conflict handling
- sync journal

Why not now:
- not needed for first simulator milestone

### 8.2 Full persistent app event journal
Could own later:
- durable action log
- replayable session state
- crash recovery from log
- cross-run state inspection

Why not now:
- we should log heavily, but full persistence and journaling can come later

### 8.3 Full fixture and replay scenario engine
Could own later:
- scenario authoring DSL
- deterministic playback
- fuzzing
- golden-state comparisons
- visual regression bundles

Why not now:
- we should design for it now, but full system can come after core exists

### 8.4 Full render and layout debug inspector
Could own later:
- live node explorer
- bounds overlays
- layout invalidation traces
- paint profiler
- interaction inspector

Why not now:
- valuable, but second-order tooling after the base simulator exists

---

## 9. Platform shell replacements

### 9.1 Replace more of the native shell
Could own later:
- more navigation chrome
- more window chrome
- more overlays
- more input UI surfaces
- more system-adjacent presentation

Why not now:
- we still want a thin host for sanity

### 9.2 More of the composer and input visuals
Could own later:
- fully custom composer
- richer prompt formatting UI
- inline token and status indicators
- custom editor overlays

Why not now:
- good future target, but we should not fight text input too early

---

## 10. Advanced visual and product surfaces

### 10.1 Advanced diff and code viewer engine
Could own later:
- syntax-aware layout
- inline comments
- folding
- side-by-side modes
- semantic diffs

Why not now:
- basic version first

### 10.2 Advanced transcript virtualization and rendering
Could own later:
- huge transcript virtualization
- partial rerender strategies
- render caching
- streaming-specific layout optimizations

Why not now:
- premature before baseline renderer exists

### 10.3 Advanced ambient dashboard visual systems
Could own later:
- charts
- timelines
- memory graphs
- live agent topology visualization
- swarm inspector UI

Why not now:
- not core to v1 simulator

---

## Short summary

### Things we probably should not own yet
- full text editing internals
- full keyboard or IME behavior
- full scroll physics
- full gesture and input routing stack
- full accessibility bridge
- full camera and audio stacks
- full custom navigation shell
- full offline sync and journaling system
- full render inspector and tooling suite

### Things we are likely to own sooner than others later
1. custom transcript rendering
2. tool cards
3. diff and code viewer
4. layout engine improvements
5. semantic tree and debug inspector
6. better animation system
7. better transcript scrolling
8. richer composer visuals

---

## Rule of thumb

### Own now or soon
- core state, reducer, and logging
- semantic tree
- main rendering architecture
- simulator automation and control

### Own later
- expensive OS-adjacent behavior
- high-complexity input systems
- polished advanced rendering infrastructure
