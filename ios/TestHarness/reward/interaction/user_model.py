"""user_model.py - assemble the weighted user-behavior ActionGraph.

Combines the two grounded inputs into one model of how a real jcode-mobile user
moves through the UI:

  - log_mining.mine_usage()  -> relative likelihoods of each high-level action
    (send_message, switch_session, scroll, soft_interrupt, ...), grounded in the
    user's actual TUI logs (falling back to literature defaults off-machine).
  - ui_map.build_ui_map()    -> the tappable-target geometry per screen, so each
    action references the real control it taps (drives Fitts movement time).

The result is an `ActionGraph` (see model.py): nodes are UI states the user can
be in, edges are concrete `Action`s with KLM/TLM operator sequences, a tapped
`target_id`, a `response_s` system wait, and a `weight` taken from the mined
usage profile. The engine then prices and walks this graph.

Why a graph and not a flat list: real usage is stateful. Switching sessions or
changing the model requires first being in the Settings sheet; the cost of those
flows depends on getting there and back. A Markov chain over states captures that
the composer (chat state) is where the user spends most time and returns to.
"""

from __future__ import annotations

from reward.interaction.log_mining import mine_usage
from reward.interaction.ui_map import build_ui_map
from reward.interaction.model import (
    Action,
    ActionGraph,
    Task,
    UITarget,
    UserState,
)

# States: the UI contexts a mobile user occupies.
_STATES = [
    UserState(id="chat", label="Chat / composer", screen="chat"),
    UserState(id="settings_sheet", label="Settings sheet", screen="settings_sheet"),
    UserState(id="pairing", label="Pairing", screen="pairing"),
    UserState(id="qr_scanner", label="QR scanner sheet", screen="qr_scanner"),
]


def _flatten_targets(ui_map: dict[str, list[UITarget]]) -> dict[str, UITarget]:
    """All targets keyed by id (ids are unique across screens in ui_map)."""
    out: dict[str, UITarget] = {}
    for targets in ui_map.values():
        for t in targets:
            out[t.id] = t
    return out


def build_user_model(
    *,
    days: int = 7,
    log_dir: str = "~/.jcode/logs",
    source_root: str | None = None,
) -> tuple[ActionGraph, dict[str, UITarget], dict]:
    """Return (graph, flat_targets, meta).

    meta carries the usage source + raw mined weights so callers can audit how
    the edge weights were grounded.
    """
    profile = mine_usage(log_dir=log_dir, days=days)
    w = profile.mobile_weights
    ui_map = build_ui_map(**({"source_root": source_root} if source_root else {}))
    targets = _flatten_targets(ui_map)

    def wt(key: str, default: float = 0.0) -> float:
        return float(w.get(key, default))

    # Edges. Each action: KLM/TLM operators + the target it acquires + any system
    # wait, weighted by mined usage. Operators follow KLM heuristics: an M (mental
    # decision) precedes a non-anticipated action; routine repeat taps drop the M.
    actions: list[Action] = [
        # --- from the chat screen (where the user lives) ---------------------
        Action(
            id="send_message", label="Type + send a message",
            src="chat", dst="chat", weight=wt("send_message", 0.45),
            target_id="send",
            # mental compose (M) + ~12 keystrokes typing + tap send. Typing cost
            # is intrinsic to messaging; engine prices K via operators.
            operators=["M"] + ["K"] * 12 + ["TAP"],
            response_s=0.0,
        ),
        Action(
            id="scroll", label="Scroll the transcript",
            src="chat", dst="chat", weight=wt("scroll", 0.25),
            target_id=None,
            operators=["TAP"],  # a swipe ~ one touch-drag; priced like a tap-move
            response_s=0.0,
        ),
        Action(
            id="read_idle", label="Read / dwell (no input)",
            src="chat", dst="chat", weight=wt("read_idle", 0.10),
            target_id=None,
            operators=["M"],  # a mental act, no motor cost
            response_s=0.0,
        ),
        Action(
            id="soft_interrupt", label="Queue a message mid-run",
            src="chat", dst="chat", weight=wt("soft_interrupt", 0.06),
            target_id="send",
            operators=["M"] + ["K"] * 8 + ["TAP"],
            response_s=0.0,
        ),
        Action(
            id="interrupt", label="Stop the running turn",
            src="chat", dst="chat", weight=wt("interrupt", 0.05),
            target_id="stop",
            operators=["M", "TAP"],
            response_s=0.0,
        ),
        Action(
            id="open_settings", label="Open the settings sheet",
            src="chat", dst="settings_sheet", weight=wt("open_settings", 0.02),
            target_id="settings",
            operators=["M", "TAP"],
            response_s=0.35,  # sheet present animation
        ),
        Action(
            id="expand_tool_card", label="Expand a tool call card",
            src="chat", dst="chat", weight=wt("expand_tool_card", 0.04),
            target_id="tool_card_header",
            operators=["M", "TAP"],
            response_s=0.15,  # expand animation
        ),
        Action(
            id="expand_reasoning", label="Expand the reasoning line",
            src="chat", dst="chat", weight=wt("expand_reasoning", 0.01),
            target_id="reasoning_row",
            operators=["M", "TAP"],
            response_s=0.15,
        ),
        Action(
            id="jump_to_latest", label="Jump back to the newest message",
            src="chat", dst="chat", weight=wt("jump_to_latest", 0.03),
            target_id="jump_to_latest",
            operators=["TAP"],  # anticipated after scrolling; no fresh M
            response_s=0.15,
        ),
        Action(
            id="dismiss_notice", label="Dismiss a notice/banner",
            src="chat", dst="chat", weight=wt("dismiss_notice", 0.02),
            target_id="notice_dismiss",
            operators=["M", "TAP"],
            response_s=0.0,
        ),
        # --- from the settings sheet ----------------------------------------
        Action(
            id="switch_session", label="Switch to another session",
            src="settings_sheet", dst="chat", weight=wt("switch_session", 0.05),
            target_id="session_row_1",
            operators=["M", "TAP"],
            response_s=0.30,  # dismiss + reconnect/resubscribe
        ),
        Action(
            id="change_model", label="Change the model",
            src="settings_sheet", dst="settings_sheet", weight=wt("change_model", 0.03),
            target_id="model_row_1",
            operators=["M", "TAP"],
            response_s=0.0,
        ),
        Action(
            id="rename_session", label="Rename the current session",
            src="settings_sheet", dst="settings_sheet", weight=wt("rename_session", 0.01),
            target_id="rename_session",
            # tap row -> alert -> type ~10 chars -> confirm
            operators=["M", "TAP"] + ["K"] * 10 + ["TAP"],
            response_s=0.35,  # alert present + dismiss
        ),
        Action(
            id="new_session", label="Start a new (cleared) session",
            src="settings_sheet", dst="chat", weight=wt("new_session", 0.01),
            target_id="new_session",
            operators=["M", "TAP"],
            response_s=0.30,  # dismiss + server clear
        ),
        Action(
            id="remove_server", label="Remove a paired server (swipe)",
            src="settings_sheet", dst="settings_sheet", weight=wt("remove_server", 0.002),
            target_id="server_row_0",
            # swipe (priced like a tap-move) + confirm tap
            operators=["M", "TAP", "TAP"],
            response_s=0.0,
        ),
        Action(
            id="close_settings", label="Dismiss settings back to chat",
            src="settings_sheet", dst="chat", weight=max(wt("open_settings", 0.02), 0.02),
            target_id="settings_done",
            operators=["TAP"],
            response_s=0.30,
        ),
        Action(
            id="pair_server", label="Pair a new server",
            src="settings_sheet", dst="pairing", weight=wt("pair_server", 0.01),
            target_id="pair_new_server",
            operators=["M", "TAP"],
            response_s=0.35,  # nested sheet present
        ),
        # --- from pairing ----------------------------------------------------
        Action(
            id="confirm_pair", label="Enter code + pair",
            src="pairing", dst="chat", weight=max(wt("pair_server", 0.01), 0.01),
            target_id="pair_button",
            operators=["M"] + ["K"] * 6 + ["TAP"],
            response_s=1.0,  # network pair round-trip
        ),
        Action(
            id="open_qr_scanner", label="Open the QR scanner",
            src="pairing", dst="qr_scanner", weight=max(wt("pair_server", 0.01), 0.01),
            target_id="scan_qr",
            operators=["M", "TAP"],
            response_s=0.5,  # sheet + camera spin-up
        ),
        # --- from the QR scanner ----------------------------------------------
        Action(
            id="scan_pair", label="Scan the QR code (auto-pairs)",
            src="qr_scanner", dst="chat", weight=1.0,
            target_id=None,
            # aim the camera (a homing act + a mental act); no typing at all
            operators=["M", "H"],
            response_s=1.5,  # detection + pair round-trip
        ),
    ]
    action_map = {a.id: a for a in actions}

    # Canonical tasks (goal-level), with frequency from the mined profile so the
    # task-time summary reflects how often each goal actually happens.
    tasks = [
        Task(id="t_send", label="Send a message",
             action_ids=["send_message"], frequency=wt("send_message", 0.45)),
        Task(id="t_switch", label="Switch session",
             action_ids=["open_settings", "switch_session"],
             frequency=wt("switch_session", 0.05)),
        Task(id="t_model", label="Change model",
             action_ids=["open_settings", "change_model", "close_settings"],
             frequency=wt("change_model", 0.03)),
        Task(id="t_interrupt", label="Interrupt a run",
             action_ids=["interrupt"], frequency=wt("interrupt", 0.05)),
        Task(id="t_pair", label="Pair a new server (manual code)",
             action_ids=["open_settings", "pair_server", "confirm_pair"],
             frequency=wt("pair_server", 0.01) * 0.5),
        Task(id="t_pair_qr", label="Pair a new server (QR scan)",
             action_ids=["open_settings", "pair_server", "open_qr_scanner", "scan_pair"],
             frequency=wt("pair_server", 0.01) * 0.5),
        Task(id="t_inspect_tool", label="Inspect a tool call",
             action_ids=["expand_tool_card"],
             frequency=wt("expand_tool_card", 0.04)),
        Task(id="t_catch_up", label="Scroll up, read, jump back to latest",
             action_ids=["scroll", "read_idle", "jump_to_latest"],
             frequency=wt("jump_to_latest", 0.03)),
        Task(id="t_rename", label="Rename the session",
             action_ids=["open_settings", "rename_session", "close_settings"],
             frequency=wt("rename_session", 0.01)),
        Task(id="t_new_session", label="Start a new session",
             action_ids=["open_settings", "new_session"],
             frequency=wt("new_session", 0.01)),
    ]

    graph = ActionGraph(
        states={s.id: s for s in _STATES},
        actions=action_map,
        tasks=tasks,
        start="chat",
    )
    meta = {
        "usage_source": profile.source,
        "days_seen": profile.days_seen,
        "lines_scanned": profile.lines_scanned,
        "mobile_weights": profile.mobile_weights,
        "notes": list(profile.notes),
    }
    return graph, targets, meta


if __name__ == "__main__":
    import json

    g, targets, meta = build_user_model()
    print("usage source:", meta["usage_source"], "lines:", meta["lines_scanned"])
    print("states:", list(g.states))
    print("actions:")
    for a in g.actions.values():
        print(f"  {a.id:16} {a.src}->{a.dst:14} w={a.weight:.3f} target={a.target_id}")
    print("tasks:")
    for t in g.tasks:
        print(f"  {t.id:12} freq={t.frequency:.3f} steps={t.action_ids}")
    print("mined weights:", json.dumps({k: round(v, 3) for k, v in meta["mobile_weights"].items()}))
