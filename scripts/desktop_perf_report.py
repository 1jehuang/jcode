#!/usr/bin/env python3
"""Summarize Jcode Desktop persistent performance logs.

The desktop app writes JSONL events to ~/.cache/jcode/desktop/performance.log.
This helper groups events by launch_id when available, reports the latest run by
default, and ranks the largest no-paint gaps, frame stalls, event queue delays,
and background disk work.
"""

from __future__ import annotations

import argparse
import collections
import datetime as _dt
import json
import os
from pathlib import Path
from typing import Any


DEFAULT_TOP = 8


def default_log_path() -> Path:
    xdg = os.environ.get("XDG_CACHE_HOME")
    if xdg:
        return Path(xdg) / "jcode" / "desktop" / "performance.log"
    return Path.home() / ".cache" / "jcode" / "desktop" / "performance.log"


def ms(value: Any) -> float:
    if isinstance(value, (int, float)):
        return float(value)
    return 0.0


def event_ts(event: dict[str, Any]) -> int:
    value = event.get("timestamp_unix_ms")
    return int(value) if isinstance(value, int) else 0


def format_ts(timestamp_ms: int) -> str:
    if timestamp_ms <= 0:
        return "unknown-time"
    return _dt.datetime.fromtimestamp(timestamp_ms / 1000, tz=_dt.UTC).isoformat(timespec="seconds")


def load_events(path: Path) -> tuple[list[dict[str, Any]], int]:
    events: list[dict[str, Any]] = []
    malformed = 0
    if not path.exists():
        return events, malformed
    with path.open("r", encoding="utf-8", errors="replace") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                malformed += 1
                continue
            if isinstance(event, dict):
                events.append(event)
            else:
                malformed += 1
    return events, malformed


def latest_launch_id(events: list[dict[str, Any]]) -> str | None:
    latest: tuple[int, str] | None = None
    for event in events:
        launch_id = event.get("launch_id")
        if not isinstance(launch_id, str) or not launch_id:
            continue
        candidate = (event_ts(event), launch_id)
        if latest is None or candidate[0] >= latest[0]:
            latest = candidate
    return latest[1] if latest else None


def select_events(
    events: list[dict[str, Any]], *, launch_id: str | None, include_all: bool
) -> tuple[list[dict[str, Any]], str]:
    if include_all:
        return events, "all launches"
    if launch_id:
        return [event for event in events if event.get("launch_id") == launch_id], f"launch_id={launch_id}"
    latest = latest_launch_id(events)
    if latest:
        return [event for event in events if event.get("launch_id") == latest], f"latest launch_id={latest}"
    return events, "legacy events without launch_id"


def top_events(
    events: list[dict[str, Any]], event_name: str, key_path: tuple[str, ...], limit: int
) -> list[dict[str, Any]]:
    def key(event: dict[str, Any]) -> float:
        value: Any = event
        for key_part in key_path:
            if not isinstance(value, dict):
                return 0.0
            value = value.get(key_part)
        return ms(value)

    return sorted(
        (event for event in events if event.get("event") == event_name),
        key=key,
        reverse=True,
    )[:limit]


def stage_summary(payload: dict[str, Any]) -> tuple[str, float]:
    stages = payload.get("stages")
    if not isinstance(stages, list):
        return ("unknown", 0.0)
    best_name = "unknown"
    best_ms = 0.0
    for stage in stages:
        if not isinstance(stage, dict):
            continue
        stage_ms = ms(stage.get("ms"))
        if stage_ms > best_ms:
            best_ms = stage_ms
            best_name = str(stage.get("name", "unknown"))
    return best_name, best_ms


def summarize(events: list[dict[str, Any]], scope: str, malformed: int, top: int) -> dict[str, Any]:
    counts = collections.Counter(str(event.get("event", "unknown")) for event in events)
    timestamps = [event_ts(event) for event in events if event_ts(event) > 0]
    launch_ids = sorted({event.get("launch_id") for event in events if isinstance(event.get("launch_id"), str)})
    build_hashes = sorted({event.get("build_hash") for event in events if isinstance(event.get("build_hash"), str)})
    pids = sorted({event.get("pid") for event in events if isinstance(event.get("pid"), int)})

    no_paint = top_events(events, "jcode-desktop-no-paint-profile", ("payload", "gap_ms"), top)
    frames = top_events(events, "jcode-desktop-frame-profile", ("payload", "worst_wall_ms"), top)
    session_events = top_events(events, "jcode-desktop-session-event-profile", ("payload", "ui_queue_delay_ms"), top)
    card_loads = top_events(events, "jcode-desktop-session-cards-load-profile", ("payload", "loaded_in_ms"), top)
    single_card_loads = top_events(events, "jcode-desktop-session-card-refresh-profile", ("payload", "loaded_in_ms"), top)
    preference_saves = top_events(events, "jcode-desktop-preferences-save-profile", ("payload", "saved_in_ms"), top)
    restores = top_events(events, "jcode-desktop-crashed-sessions-restore-profile", ("payload", "elapsed_ms"), top)

    worst_frame_details = []
    for event in frames:
        payload = event.get("payload") if isinstance(event.get("payload"), dict) else {}
        stage_name, stage_ms = stage_summary(payload)
        worst_frame_details.append(
            {
                "timestamp_unix_ms": event_ts(event),
                "worst_wall_ms": ms(payload.get("worst_wall_ms")),
                "worst_cpu_ms": ms(payload.get("worst_cpu_ms")),
                "present_ms": ms(payload.get("present_ms")),
                "queue_submit_ms": ms(payload.get("queue_submit_ms")),
                "submit_present_ms": ms(payload.get("submit_present_ms")),
                "dominant_stage": stage_name,
                "dominant_stage_ms": stage_ms,
                "mode": payload.get("mode"),
            }
        )

    diagnoses: list[str] = []
    if no_paint:
        payload = no_paint[0].get("payload") if isinstance(no_paint[0].get("payload"), dict) else {}
        gap = ms(payload.get("gap_ms"))
        pending = payload.get("pending_interaction_kind")
        background = payload.get("has_background_work")
        diagnoses.append(
            f"Largest no-paint gap was {gap:.1f} ms; pending={pending!r}, background_work={background}."
        )
    if worst_frame_details:
        frame = worst_frame_details[0]
        diagnoses.append(
            f"Worst frame was {frame['worst_wall_ms']:.1f} ms; dominant stage {frame['dominant_stage']}={frame['dominant_stage_ms']:.1f} ms, present={frame['present_ms']:.1f} ms."
        )
        if frame["present_ms"] >= 33 or frame["dominant_stage"] in {"surface_acquire", "present", "queue_submit"}:
            diagnoses.append("Likely GPU/window-system stall when frame/present dominates rather than app CPU.")
    if session_events:
        payload = session_events[0].get("payload") if isinstance(session_events[0].get("payload"), dict) else {}
        queue_delay = ms(payload.get("ui_queue_delay_ms"))
        apply_ms = ms(payload.get("apply_ms"))
        if queue_delay >= 250:
            diagnoses.append(
                f"Session events waited {queue_delay:.1f} ms in the UI queue while apply cost was {apply_ms:.3f} ms, indicating event-loop starvation."
            )
    if card_loads or single_card_loads or preference_saves or restores:
        slow_disk = []
        if card_loads:
            slow_disk.append(f"cards={ms(card_loads[0].get('payload', {}).get('loaded_in_ms')):.1f} ms")
        if single_card_loads:
            slow_disk.append(f"single_card={ms(single_card_loads[0].get('payload', {}).get('loaded_in_ms')):.1f} ms")
        if preference_saves:
            slow_disk.append(f"prefs={ms(preference_saves[0].get('payload', {}).get('saved_in_ms')):.1f} ms")
        if restores:
            slow_disk.append(f"restore={ms(restores[0].get('payload', {}).get('elapsed_ms')):.1f} ms")
        diagnoses.append("Slow disk/process work was observed off the UI thread: " + ", ".join(slow_disk) + ".")

    return {
        "scope": scope,
        "events": len(events),
        "malformed_lines": malformed,
        "launch_ids": launch_ids,
        "build_hashes": build_hashes,
        "pids": pids,
        "start": min(timestamps) if timestamps else None,
        "end": max(timestamps) if timestamps else None,
        "event_counts": dict(counts.most_common()),
        "top_no_paint": [
            {
                "timestamp_unix_ms": event_ts(event),
                "gap_ms": ms(event.get("payload", {}).get("gap_ms")),
                "pending_interaction_kind": event.get("payload", {}).get("pending_interaction_kind"),
                "has_background_work": event.get("payload", {}).get("has_background_work"),
                "pending_backend_redraw": event.get("payload", {}).get("pending_backend_redraw"),
                "mode": event.get("payload", {}).get("mode"),
            }
            for event in no_paint
        ],
        "top_frames": worst_frame_details,
        "top_session_event_queue": [
            {
                "timestamp_unix_ms": event_ts(event),
                "ui_queue_delay_ms": ms(event.get("payload", {}).get("ui_queue_delay_ms")),
                "apply_ms": ms(event.get("payload", {}).get("apply_ms")),
                "forwarder_accumulated_ms": ms(event.get("payload", {}).get("forwarder_accumulated_ms")),
                "raw_events": event.get("payload", {}).get("raw_events"),
                "text_delta_bytes": event.get("payload", {}).get("text_delta_bytes"),
            }
            for event in session_events
        ],
        "top_background_disk": {
            "session_cards": [event.get("payload", {}) for event in card_loads],
            "single_card_refresh": [event.get("payload", {}) for event in single_card_loads],
            "preferences": [event.get("payload", {}) for event in preference_saves],
            "restore_crashed_sessions": [event.get("payload", {}) for event in restores],
        },
        "diagnosis": diagnoses,
    }


def print_text(summary: dict[str, Any], path: Path) -> None:
    print("Jcode Desktop performance report")
    print(f"log: {path}")
    print(f"scope: {summary['scope']}")
    print(f"events: {summary['events']} malformed_lines: {summary['malformed_lines']}")
    if summary["start"]:
        print(f"range: {format_ts(summary['start'])} -> {format_ts(summary['end'])}")
    if summary["launch_ids"]:
        print(f"launch_ids: {', '.join(summary['launch_ids'][-3:])}")
    if summary["build_hashes"]:
        print(f"build_hashes: {', '.join(summary['build_hashes'][-3:])}")
    if summary["pids"]:
        print(f"pids: {', '.join(str(pid) for pid in summary['pids'][-5:])}")

    print("\nevent counts:")
    for event, count in summary["event_counts"].items():
        print(f"  {event}: {count}")

    print("\ntop no-paint gaps:")
    if not summary["top_no_paint"]:
        print("  none")
    for item in summary["top_no_paint"]:
        print(
            "  {gap_ms:8.1f} ms  {ts}  pending={pending_interaction_kind!r} background={has_background_work} backend_redraw={pending_backend_redraw} mode={mode}".format(
                ts=format_ts(item["timestamp_unix_ms"]), **item
            )
        )

    print("\ntop frame stalls:")
    if not summary["top_frames"]:
        print("  none")
    for item in summary["top_frames"]:
        print(
            "  {worst_wall_ms:8.1f} ms wall  cpu={worst_cpu_ms:7.1f} present={present_ms:7.1f} submit={queue_submit_ms:7.1f} dominant={dominant_stage}:{dominant_stage_ms:.1f}  {ts}".format(
                ts=format_ts(item["timestamp_unix_ms"]), **item
            )
        )

    print("\ntop session event queue delays:")
    if not summary["top_session_event_queue"]:
        print("  none")
    for item in summary["top_session_event_queue"]:
        print(
            "  queue={ui_queue_delay_ms:8.1f} ms apply={apply_ms:7.3f} ms forwarder={forwarder_accumulated_ms:7.1f} ms raw={raw_events} bytes={text_delta_bytes} {ts}".format(
                ts=format_ts(item["timestamp_unix_ms"]), **item
            )
        )

    print("\nbackground disk/process work:")
    disk = summary["top_background_disk"]
    printed = False
    for label, items in disk.items():
        for payload in items[:3]:
            printed = True
            elapsed = payload.get("loaded_in_ms", payload.get("saved_in_ms", payload.get("elapsed_ms", 0.0)))
            print(f"  {label}: {ms(elapsed):8.1f} ms {payload}")
    if not printed:
        print("  none")

    print("\ndiagnosis:")
    if not summary["diagnosis"]:
        print("  No slow profile events in this scope.")
    for line in summary["diagnosis"]:
        print(f"  - {line}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log", nargs="?", type=Path, default=default_log_path())
    parser.add_argument("--launch-id", help="summarize only one launch_id")
    parser.add_argument("--all", action="store_true", help="summarize all launches instead of the latest launch")
    parser.add_argument("--top", type=int, default=DEFAULT_TOP, help="number of rows per top list")
    parser.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    args = parser.parse_args()

    events, malformed = load_events(args.log)
    selected, scope = select_events(events, launch_id=args.launch_id, include_all=args.all)
    summary = summarize(selected, scope, malformed, max(1, args.top))
    if args.json:
        print(json.dumps(summary, indent=2, sort_keys=True))
    else:
        print_text(summary, args.log)
    return 0 if summary["events"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
