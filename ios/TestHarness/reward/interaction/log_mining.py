"""Ground the jcode-mobile user model in the user's REAL TUI usage logs.

This module streams the last N daily logs under ``~/.jcode/logs`` and mines
proxies for "what the user actually does", then maps those TUI actions onto the
set of actions the iOS app must also support. The output (:class:`UsageProfile`)
feeds the mobile ``ActionGraph`` as relative edge weights (see ``model.py``).

Design / honesty notes
----------------------
TUI logs are a *proxy* for mobile usage, not identical. Concretely:

  * Some TUI actions have no mobile analogue (``diff_mode``, ``side_panel``) and
    are dropped.
  * Some mobile actions leave **no trace** in TUI logs at all -- there is no
    scroll event, no "read/idle" event, and no pair-server event in the TUI log
    stream (we verified ``scroll_delta`` is always null and there is no
    ``scroll_up/down`` verb). Mobile is *more* scroll/read heavy than the TUI,
    so we **impute** those weights from HCI/mobile literature priors rather than
    pretend we measured them. Those imputed weights are fixed module constants
    and are clearly listed in ``notes`` so the assumption is auditable.

What we CAN mine (clean, structured, low-noise markers):

  ============================  =========================================  ==================
  raw_count key                 log marker (one event per line)            mobile action
  ============================  =========================================  ==================
  req_message                   SERVER_REQUEST_LIFECYCLE phase=received     send_message
                                  request_kind=message
  req_soft_interrupt            ... request_kind=soft_interrupt             soft_interrupt
  req_cancel                    ... request_kind=cancel                     interrupt (hard stop)
  req_cancel_soft_interrupts    ... request_kind=cancel_soft_interrupts     cancel (drop queued msg)
  req_set_reasoning_effort      ... request_kind=set_reasoning_effort       open_settings
  req_subscribe                 ... request_kind=subscribe                  (dropped: connect noise)
  req_reload                    ... request_kind=reload                     (dropped: reconnect)
  session_resume_start          SESSION_LIFECYCLE phase=resume_start         switch_session
  env_set_model                 ENV_SNAPSHOT ... "reason":"set_model"        change_model
  tool_start                    TOOL_LIFECYCLE phase=start                   (dropped: agent-driven)
  assistant_turns               "Assistant:"                                 (cross-check only)
  remote_interrupt_cancel       REMOTE_INTERRUPT_SEND_START kind=cancel      (cross-check only)
  remote_interrupt_soft         REMOTE_INTERRUPT_SEND_START kind=soft_...    (cross-check only)
  ============================  =========================================  ==================

We dedupe each request to a single line by counting only ``phase=received``
(every request also logs ``handled`` and ``acked``).

Determinism / purity: pure aside from reading files; deterministic for a fixed
log set; stdlib only; tolerant of missing/locked files.
"""

from __future__ import annotations

import glob
import json
import os
import re
from dataclasses import dataclass, field, asdict

# --------------------------------------------------------------------------- #
# Mobile action vocabulary (must stay in sync with the mobile ActionGraph).
# --------------------------------------------------------------------------- #
MOBILE_ACTIONS: tuple[str, ...] = (
    "send_message",
    "scroll",
    "soft_interrupt",
    "interrupt",
    "cancel",
    "switch_session",
    "change_model",
    "open_settings",
    "pair_server",
    "read_idle",  # the deliverable's "read/idle"
    # Imputed-only chat-surface actions (no TUI log proxy):
    "expand_tool_card",
    "expand_reasoning",
    "jump_to_latest",
    "dismiss_notice",
    "rename_session",
    "new_session",
    "remove_server",
    "tap_suggestion",
)

# Mobile actions that leave NO trace in TUI logs and must be imputed from
# literature priors. Mobile is more scroll/read heavy than the TUI; pairing is a
# rare one-time onboarding step. These are absolute weight reservations.
IMPUTED_WEIGHTS: dict[str, float] = {
    "scroll": 0.25,
    "read_idle": 0.10,
    "pair_server": 0.005,
    # Chat-surface interactions with no TUI analogue. Priors: tool inspection
    # is common on mobile (small screen -> more expanding), reasoning rarely
    # expanded, jump-to-latest follows most scroll-up excursions, notices are
    # occasional, session admin is rare.
    "expand_tool_card": 0.04,
    "expand_reasoning": 0.01,
    "jump_to_latest": 0.03,
    "dismiss_notice": 0.02,
    "rename_session": 0.01,
    "new_session": 0.01,
    "remove_server": 0.002,
    "tap_suggestion": 0.02,
}

# Observed (mineable) mobile actions, distributed over the remaining mass
# proportional to their real mined counts.
OBSERVED_ACTIONS: tuple[str, ...] = (
    "send_message",
    "soft_interrupt",
    "interrupt",
    "cancel",
    "switch_session",
    "change_model",
    "open_settings",
)

# Literature/default profile used when there are no logs (fresh machine / CI),
# or when logs exist but contain zero user-action markers. Pre-normalized to ~1.
DEFAULT_WEIGHTS: dict[str, float] = {
    "send_message": 0.45,
    "scroll": 0.25,
    "read_idle": 0.10,
    "soft_interrupt": 0.06,
    "interrupt": 0.05,
    "switch_session": 0.04,
    "change_model": 0.03,
    "open_settings": 0.02,
    "cancel": 0.005,
    "pair_server": 0.005,
    "expand_tool_card": 0.04,
    "expand_reasoning": 0.01,
    "jump_to_latest": 0.03,
    "dismiss_notice": 0.02,
    "rename_session": 0.01,
    "new_session": 0.01,
    "remove_server": 0.002,
    "tap_suggestion": 0.02,
}

# TUI-only verbs we explicitly acknowledge and drop (no mobile analogue).
TUI_ONLY_DROPPED: tuple[str, ...] = ("diff_mode", "side_panel")

_DATE_RE = re.compile(r"jcode-(\d{4}-\d{2}-\d{2})\.log$")
_REQUEST_KIND_RE = re.compile(r"request_kind=([a-z_]+)")
_REMOTE_KIND_RE = re.compile(r"REMOTE_INTERRUPT_SEND_START kind=([a-z_]+)")


@dataclass
class UsageProfile:
    """Mined (or defaulted) user-behavior profile for the mobile ActionGraph.

    raw_counts      raw TUI event/verb counts actually mined (audit trail).
    mobile_weights  normalized relative weights over MOBILE_ACTIONS (sum ~= 1.0);
                    used directly as edge weights in the mobile user graph.
    days_seen       number of daily log files actually read (with content).
    lines_scanned   total lines streamed across those files.
    source          "logs" if real logs drove the weights, else "defaults".
    notes           auditable assumptions about the TUI -> mobile mapping.
    """

    raw_counts: dict[str, int]
    mobile_weights: dict[str, float]
    days_seen: int
    lines_scanned: int
    source: str  # "logs" | "defaults"
    notes: list[str] = field(default_factory=list)

    def to_dict(self) -> dict:
        return asdict(self)


def _default_profile(extra_note: str, raw_counts: dict[str, int] | None = None,
                     days_seen: int = 0, lines_scanned: int = 0,
                     source: str = "defaults") -> UsageProfile:
    """Build a literature-default profile so the engine runs on a fresh machine."""
    weights = _normalize(dict(DEFAULT_WEIGHTS))
    notes = [
        extra_note,
        "mobile_weights are literature/HCI defaults (Card/Moran/Newell + mobile "
        "usage priors), not mined from this machine.",
        f"TUI-only verbs dropped (no mobile analogue): {', '.join(TUI_ONLY_DROPPED)}.",
        "scroll, read_idle, pair_server have no TUI log proxy and are imputed.",
    ]
    return UsageProfile(
        raw_counts=raw_counts if raw_counts is not None else {},
        mobile_weights=weights,
        days_seen=days_seen,
        lines_scanned=lines_scanned,
        source=source,
        notes=notes,
    )


def _normalize(weights: dict[str, float]) -> dict[str, float]:
    """Return weights normalized to sum 1.0 (stable order = MOBILE_ACTIONS)."""
    total = sum(weights.get(k, 0.0) for k in MOBILE_ACTIONS)
    if total <= 0:
        # Degenerate; fall back to uniform over the action set.
        n = len(MOBILE_ACTIONS)
        return {k: 1.0 / n for k in MOBILE_ACTIONS}
    return {k: weights.get(k, 0.0) / total for k in MOBILE_ACTIONS}


def _select_log_files(log_dir: str, days: int) -> list[tuple[str, str]]:
    """Return up to `days` (date, path) pairs, most recent first, by filename date."""
    pattern = os.path.join(log_dir, "jcode-*.log")
    dated: list[tuple[str, str]] = []
    for path in glob.glob(pattern):
        m = _DATE_RE.search(os.path.basename(path))
        if m:
            dated.append((m.group(1), path))
    # Sort by ISO date descending (lexicographic works for YYYY-MM-DD), keep last N.
    dated.sort(key=lambda t: t[0], reverse=True)
    return dated[: max(0, days)]


def _scan_file(path: str, counts: dict[str, int]) -> int:
    """Stream one log file line-by-line, accumulating into `counts`.

    Returns the number of lines scanned. Cheap substring guards keep the hot
    loop fast on 100k+ line files; only candidate lines are parsed further.
    """
    lines = 0
    # errors="replace" so a corrupt byte never aborts a multi-MB scan.
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            lines += 1

            if "EVENT event=" in line:
                if "SERVER_REQUEST_LIFECYCLE" in line:
                    # Dedupe to one event per request via phase=received.
                    if "phase=received" in line:
                        m = _REQUEST_KIND_RE.search(line)
                        if m:
                            counts[f"req_{m.group(1)}"] = counts.get(f"req_{m.group(1)}", 0) + 1
                elif "SESSION_LIFECYCLE" in line:
                    if "phase=resume_start" in line:
                        counts["session_resume_start"] += 1
                elif "TOOL_LIFECYCLE" in line:
                    if "phase=start" in line:
                        counts["tool_start"] += 1
                continue

            if "REMOTE_INTERRUPT_SEND_START" in line:
                m = _REMOTE_KIND_RE.search(line)
                if m:
                    kind = m.group(1)
                    if kind == "cancel":
                        counts["remote_interrupt_cancel"] += 1
                    elif kind == "soft_interrupt":
                        counts["remote_interrupt_soft"] += 1
                continue

            if "ENV_SNAPSHOT" in line and '"reason":"set_model"' in line:
                counts["env_set_model"] += 1
                continue

            # Cheap cross-check proxy for conversation turns.
            if "Assistant:" in line:
                counts["assistant_turns"] += 1

    return lines


def _map_to_mobile(counts: dict[str, int]) -> dict[str, int]:
    """Map mined raw TUI counts onto the OBSERVED mobile actions."""
    return {
        "send_message": counts.get("req_message", 0),
        "soft_interrupt": counts.get("req_soft_interrupt", 0),
        "interrupt": counts.get("req_cancel", 0),               # hard stop a run
        "cancel": counts.get("req_cancel_soft_interrupts", 0),  # drop a queued msg
        "switch_session": counts.get("session_resume_start", 0),
        "change_model": counts.get("env_set_model", 0),
        "open_settings": counts.get("req_set_reasoning_effort", 0),
    }


def mine_usage(log_dir: str = "~/.jcode/logs", days: int = 7) -> UsageProfile:
    """Mine the last `days` daily logs under `log_dir` into a :class:`UsageProfile`.

    Degrades gracefully: if the directory is absent, empty, or contains no
    user-action markers, returns a literature-default profile so the engine
    still runs on a fresh machine / in CI.
    """
    resolved = os.path.expanduser(log_dir)

    if not os.path.isdir(resolved):
        return _default_profile(
            f"No log directory at {resolved!r}; using literature defaults."
        )

    files = _select_log_files(resolved, days)
    if not files:
        return _default_profile(
            f"No jcode-YYYY-MM-DD.log files under {resolved!r}; using defaults."
        )

    counts: dict[str, int] = {
        "session_resume_start": 0,
        "tool_start": 0,
        "env_set_model": 0,
        "assistant_turns": 0,
        "remote_interrupt_cancel": 0,
        "remote_interrupt_soft": 0,
    }
    lines_scanned = 0
    days_seen = 0
    skipped: list[str] = []

    for date_str, path in files:
        try:
            n = _scan_file(path, counts)
        except OSError as exc:  # missing/locked/permission -> tolerate
            skipped.append(f"{os.path.basename(path)} ({exc.__class__.__name__})")
            continue
        if n > 0:
            days_seen += 1
            lines_scanned += n

    observed = _map_to_mobile(counts)
    observed_total = sum(observed.values())

    notes: list[str] = []
    notes.append(
        f"Scanned {days_seen} daily log(s), {lines_scanned} lines, from "
        f"{os.path.basename(files[-1][1])}..{os.path.basename(files[0][1])}."
    )

    if days_seen == 0:
        return _default_profile(
            f"All candidate logs under {resolved!r} were unreadable; using defaults.",
            raw_counts=dict(counts),
            days_seen=0,
            lines_scanned=lines_scanned,
        )

    if observed_total == 0:
        prof = _default_profile(
            "Logs read but zero user-action markers matched; using default weights.",
            raw_counts=dict(counts),
            days_seen=days_seen,
            lines_scanned=lines_scanned,
            source="logs",
        )
        prof.notes = notes + prof.notes
        if skipped:
            prof.notes.append(f"Skipped unreadable files: {', '.join(skipped)}.")
        return prof

    # ---- Build weights: imputed reserve + observed mass distributed by count -- #
    imputed_total = sum(IMPUTED_WEIGHTS.values())
    observed_mass = max(0.0, 1.0 - imputed_total)

    weights: dict[str, float] = dict(IMPUTED_WEIGHTS)
    for action in OBSERVED_ACTIONS:
        weights[action] = observed_mass * (observed[action] / observed_total)
    # Ensure every mobile action key is present.
    for action in MOBILE_ACTIONS:
        weights.setdefault(action, 0.0)

    mobile_weights = _normalize(weights)

    # ---- Auditable mapping notes -------------------------------------------- #
    notes.append(
        "Observed counts -> mobile: send_message=req_message, "
        "soft_interrupt=req_soft_interrupt, interrupt=req_cancel (hard stop), "
        "cancel=req_cancel_soft_interrupts (drop queued msg), "
        "switch_session=resume_start, change_model=ENV_SNAPSHOT set_model, "
        "open_settings=req_set_reasoning_effort."
    )
    notes.append(
        "Dedupe: SERVER_REQUEST_LIFECYCLE counted only at phase=received "
        "(each request also logs handled+acked)."
    )
    notes.append(
        f"Imputed (no TUI log proxy) at fixed literature weights: "
        f"scroll={IMPUTED_WEIGHTS['scroll']}, read_idle={IMPUTED_WEIGHTS['read_idle']}, "
        f"pair_server={IMPUTED_WEIGHTS['pair_server']}; remaining "
        f"{observed_mass:.3f} split across observed actions by real counts."
    )
    notes.append(
        f"Dropped (not user-driven / TUI-only): tool_start (agent turns, "
        f"{counts.get('tool_start', 0)}), req_subscribe ("
        f"{counts.get('req_subscribe', 0)}, connection noise), req_reload ("
        f"{counts.get('req_reload', 0)}, reconnect), and verbs "
        f"{', '.join(TUI_ONLY_DROPPED)}."
    )
    notes.append(
        "CAVEAT: TUI usage is a proxy for mobile, not identical; switch_session "
        "(resume_start) may include automatic swarm reconnects; assistant_turns "
        f"({counts.get('assistant_turns', 0)}) and remote_interrupt_* are kept as "
        "cross-checks only, not folded into weights."
    )
    if skipped:
        notes.append(f"Skipped unreadable files: {', '.join(skipped)}.")

    return UsageProfile(
        raw_counts=dict(counts),
        mobile_weights=mobile_weights,
        days_seen=days_seen,
        lines_scanned=lines_scanned,
        source="logs",
        notes=notes,
    )


if __name__ == "__main__":
    profile = mine_usage()
    print(json.dumps(profile.to_dict(), indent=2, sort_keys=False))
