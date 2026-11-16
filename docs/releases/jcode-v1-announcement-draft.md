# Discord announcement draft

## Message 1: introduction and swarm rework

# jcode v1.0 🎉

**jcode v1 brings a reworked swarm system, the official launch of jcode Cloud, and a lot less background work on your laptop.**

## 🐝 A swarm that works from a plan

Plans now have **phases, parallel lanes, and tasks**, with workers starting automatically as their dependencies finish. Independent parts of a job run together, the coordinator receives progress and results automatically, and you can follow the whole team in one pinned view.

We’re using this to build jcode itself. **In one bug-fix batch, four worker lanes delivered fixes for five GitHub issues in about six minutes.** Another four-lane batch handled lint, safety checks, the session picker, and runtime fixes simultaneously.

Plans can wait for background jobs or other workers and wake when those finish. Long-running tools yield to the background after **3 seconds**, so the agent can keep doing useful work.

**Clearer todo semantics keep work moving:** a retryable problem is not a reason to stop. Tasks can pause while tracked background work finishes, or when they genuinely need a login, 2FA, or credentials the agent cannot access. Work that truly cannot be completed is recorded at the goal level, instead of repeatedly poking the same impossible task.

## Message 2: official cloud launch

## ☁️ jcode Cloud is officially launching

**Start locally. Move the work to the cloud. Come back when you’re ready.**

Use **`/cloud`** to hand a session or coordinated swarm to a VM, and **`/local`** to bring it back. Your conversation and repository move with the work, so you can continue without starting over.

- **Keep working after you disconnect.** Remote execution and follow-up loops live on the server, and active agent work keeps the host awake.
- **Move the whole team.** Coordinated handoff brings selected swarm members over together, with a visible transfer checklist and recovery if something goes wrong.
- **Find everything in `/resume`.** Local and cloud sessions appear together, with a Cloud filter, live previews, and running-agent indicators.
- **Reconnect without replaying everything.** Recent history loads first, with older messages available on demand. A 7,000-message session’s initial history payload dropped from **14.3 MB to 261 KB**, about **98% smaller**.
- **Update without losing your place.** Managed cloud runtimes support in-place reloads while sessions stay attached.
- **Keep control over your workspace.** Improved SSH commands, environment handoff, recoverable repository replacement, and protection for unreturned cloud edits make moving between machines much less fragile.

Cloud is part of the same workflow, not a separate conversation you have to rebuild from scratch.

## Message 3: performance, CPU and battery efficiency

## ⚡ Less CPU. Fewer wakeups. A lighter terminal.

Running more agents shouldn’t mean keeping your laptop busy with redraws, polling, and copies of the same data.

**Some measured improvements:**
- **25% lower CPU usage in a profiled release client:** about **8% → 6% of one core**.
- **44% fewer idle client wakeups:** **2.68 → 1.51 per second** after slowing terminal-theme queries when there’s no input.
- **Sleeping spinner thread:** status ticks no longer depend on a runtime timer, and the thread parks when its ticks aren’t needed.
- **3.5× faster spinner-frame work** and **1.8× faster animation-frame work**.
- **8.3× faster append-frame work** on a 4,000-message transcript: **39 ms → 4.7 ms**.
- **28% less rendered-span memory** in a large conversation: **20.4 MB → 14.6 MB**.

**Less background work throughout:** terminal-theme timers sleep until a real deadline, server memory heartbeats stop reopening thousands of finished task files, and unchanged todo snapshots no longer trigger repaints. Unfocused windows poll todos every **30 seconds instead of 2**, even during a turn, and account usage every **5 minutes instead of 30 seconds**, with prompt refreshes when you return.

Git status reacts to filesystem changes, build status uses persistent watchers, MCP reconnects back off, and inherited child processes get cleaned up. Background focus survives reloads, so hidden windows don’t quietly start animating as if you were watching them.

More agents doing useful work. Less of your laptop spent maintaining the UI around them.

## Message 4: search, retries, codemode, and settings

## 🔎 Better search and everyday reliability

- **Search full conversations in `/resume`**, with typo tolerance and ranking that favors your prompts. Warm cached searches return their first match in **~2–3 ms instead of ~152–153 ms**.
- **Live session previews** use the real chat renderer. Cached 80-turn preview revisits dropped from **48.8 ms to 0.19 ms**.
- **Retries that handle real network blips.** Claude transport failures get up to six attempts with about 31 seconds of backoff, bounded to 60 seconds from the first failure. HTTP/2 stream faults trigger network recovery rather than an unhelpful model-switch offer.
- **Safer failover:** bounded cross-route retries, stronger cancellation handling, validated switch acknowledgements, and protection against stale manual fallback resends. Account switches show one clear notice with the model and masked account.
- **See what codemode is doing.** Bounded JavaScript previews stay visible for live and completed calls, through status changes and transcript compaction.
- **Account priority tiers and hot-swapping** with **Ctrl+Alt+J/K**, refreshed **`/usage`** cards, and usage refreshed immediately after authentication.
- **Per-session context limits**, better compaction, broader live model discovery, and live voice input with a waveform and streaming draft transcript.
- **Terminal recovery fixes**, including tiny tmux panes and recovered draw panics.

### A few settings changes
- Codemode is enabled by default for tool orchestration.
- Default Claude reasoning effort is low, with other levels available explicitly.
- Bash commands are non-interactive.
- Integration discovery is opt-in. Background MCP auto-connect no longer opens OAuth browser windows.

Thanks to everyone testing previews, filing issues, and contributing ❤️

# Twitter launch copy

## Main post

jcode v1 is here. I wanted coding agents that could work as a team without making me manage the team. This release is a big step toward that: a plan-driven swarm, a cloud handoff that keeps your place, and less background work on your laptop.

## Optional reply 1: swarms

The swarm works from phases, parallel lanes, and tasks. Workers start as dependencies finish, and progress comes back to the coordinator. We used four worker lanes to fix five GitHub issues in about six minutes while building jcode itself.

## Optional reply 2: official cloud launch

jcode Cloud is officially launching. Use /cloud to move a session or coordinated swarm to a VM, then /local to bring it back. Your conversation and repository move with the work. Disconnect, return, and pick up where you left off.

## Optional reply 3: CPU performance

We also cut the work around the work: idle client wakeups fell from 2.68 to 1.51/s in one measurement, and a profiled release client went from about 8% to 6% of one CPU core. Fewer polls, fewer redraws, more time spent on the actual task.
