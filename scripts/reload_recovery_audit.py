#!/usr/bin/env python3
"""
Audit self-dev reload recovery handoffs.

This is a read-only diagnostic helper for bugs where `selfdev reload` restarts the
server but not every interrupted session continues. It correlates three sources:

  1. ~/.jcode/reload-recovery/*.json durable recovery intent records
  2. ~/.jcode/logs/jcode-*.log recovery lifecycle messages
  3. ~/.jcode/sessions/<session>.json coarse transcript markers

The important invariant this helps check is:

  A recovery intent should not become effectively terminal unless the client
  queued/dispatched the continuation or the server accepted the continuation.

Older builds could mark the server-side intent "Delivered" while merely building
a History payload, before the TUI processed that payload. Current builds keep the
intent pending until the replacement server accepts the exact hidden
continuation. This script flags old-style claimed/delivered records that have no
client queue or server acceptance evidence.

Examples:

  scripts/reload_recovery_audit.py
  scripts/reload_recovery_audit.py --reload-id reload_1779786108758_15766248331403251908
  scripts/reload_recovery_audit.py --max-log-files 5 --show-lines
  scripts/reload_recovery_audit.py --json
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import os
import pathlib
import re
import sys
from collections import defaultdict
from typing import Any


PERSISTED_RE = re.compile(
    r"reload recovery store: persisted intent reload_id=(reload_[^ ]+) session=([^ ]+) role=([^ ]+)"
)
CLAIMED_RE = re.compile(
    r"reload recovery store: claimed intent reload_id=(reload_[^ ]+) session=([^ ]+) role=([^ ]+)"
)
ATTACHED_RE = re.compile(
    r"reload recovery store: attached pending intent reload_id=(reload_[^ ]+) session=([^ ]+) role=([^ ]+)"
)
DELIVERED_RE = re.compile(
    r"reload recovery store: delivered intent reload_id=(reload_[^ ]+) session=([^ ]+) role=([^ ]+) accepted_by=([^ ]+)"
)
NON_PENDING_RE = re.compile(
    r"reload recovery store: skipping non-pending intent session=([^ ]+) reload_id=(reload_[^ ]+) status=([^ ]+)"
)
HISTORY_QUEUE_RE = re.compile(
    r"History payload requested reload recovery continuation: session=([^ ]+) was_interrupted=([^ ]+)"
)
TUI_FLOW_RE = re.compile(
    r"reload recovery flow=([^ ]+) session_id=([^ ]+) outcome=([^ ]+) detail=(.*)"
)
HISTORY_SNAPSHOT_RE = re.compile(
    r"history_reload_recovery_snapshot: (?:using|attaching) server-owned recovery intent for session=([^ ]+)"
)
SEND_HISTORY_RE = re.compile(r"\[TIMING\] send_history write: session=([^, ]+)")
HIDDEN_SEND_RE = re.compile(r"Sending hidden continuation reminder \((\d+) chars\)")
MESSAGE_RUNNING_RE = re.compile(
    r"new_status=running old_status=[^ ]+ phase=member_status_updated .*session_id=([^ ]+)"
)
RELOAD_ID_RE = re.compile(r"reload_\d+_\d+")


@dataclasses.dataclass
class Event:
    kind: str
    line: int
    text: str
    file: str
    reload_id: str | None = None
    session_id: str | None = None
    role: str | None = None
    detail: str | None = None

    def compact(self) -> dict[str, Any]:
        return {
            "kind": self.kind,
            "line": self.line,
            "file": self.file,
            "reload_id": self.reload_id,
            "session_id": self.session_id,
            "role": self.role,
            "detail": self.detail,
            "text": self.text.strip(),
        }


@dataclasses.dataclass
class IntentRecord:
    reload_id: str
    session_id: str
    role: str | None = None
    persisted: Event | None = None
    claimed: Event | None = None
    attached: list[Event] = dataclasses.field(default_factory=list)
    delivered: list[Event] = dataclasses.field(default_factory=list)
    delivery_mismatch: list[Event] = dataclasses.field(default_factory=list)
    non_pending: list[Event] = dataclasses.field(default_factory=list)
    history_snapshot: list[Event] = dataclasses.field(default_factory=list)
    history_queued: list[Event] = dataclasses.field(default_factory=list)
    tui_flows: list[Event] = dataclasses.field(default_factory=list)
    send_history: list[Event] = dataclasses.field(default_factory=list)
    hidden_sends_nearby: list[Event] = dataclasses.field(default_factory=list)
    member_running: list[Event] = dataclasses.field(default_factory=list)
    durable_status: str | None = None
    durable_delivered_at: str | None = None
    transcript_markers: dict[str, int] = dataclasses.field(default_factory=dict)

    def has_client_queue_evidence(self) -> bool:
        return bool(self.history_queued) or any(
            event.detail
            and (
                "queued" in event.detail.lower()
                or "outcome=resumed" in event.detail.lower()
            )
            for event in self.tui_flows
        )

    def has_server_acceptance_evidence(self) -> bool:
        return bool(self.delivered) or any(
            event.detail
            and (
                "accepted_by=client_message_accepted" in event.detail
                or "accepted_by=server_startup_headless" in event.detail
            )
            for event in self.tui_flows
        )

    def verdict(self) -> str:
        if self.has_client_queue_evidence():
            return "ok-client-queued"
        if self.has_server_acceptance_evidence():
            return "ok-server-accepted"
        if self.durable_status == "pending" or self.durable_status == "Pending":
            return "pending"
        if self.claimed or (self.durable_status or "").lower() == "delivered":
            return "suspect-delivered-without-acceptance-log"
        if self.attached:
            return "attached-not-accepted"
        if self.persisted:
            return "persisted-not-attached"
        return "observed-without-persist-log"

    def compact(self, show_lines: bool = False) -> dict[str, Any]:
        data: dict[str, Any] = {
            "reload_id": self.reload_id,
            "session_id": self.session_id,
            "role": self.role,
            "durable_status": self.durable_status,
            "durable_delivered_at": self.durable_delivered_at,
            "persisted": bool(self.persisted),
            "claimed": bool(self.claimed),
            "attached_count": len(self.attached),
            "delivered_count": len(self.delivered),
            "delivery_mismatch_count": len(self.delivery_mismatch),
            "history_snapshot_count": len(self.history_snapshot),
            "history_queued_count": len(self.history_queued),
            "tui_flow_count": len(self.tui_flows),
            "send_history_count": len(self.send_history),
            "member_running_after_reload_count": len(self.member_running),
            "transcript_markers": self.transcript_markers,
            "verdict": self.verdict(),
        }
        if show_lines:
            data["events"] = [
                *(event.compact() for event in [self.persisted, self.claimed] if event),
                *(event.compact() for event in self.attached),
                *(event.compact() for event in self.delivered),
                *(event.compact() for event in self.delivery_mismatch),
                *(event.compact() for event in self.history_snapshot),
                *(event.compact() for event in self.history_queued),
                *(event.compact() for event in self.tui_flows),
                *(event.compact() for event in self.send_history),
                *(event.compact() for event in self.member_running),
            ]
        return data


def jcode_home() -> pathlib.Path:
    return pathlib.Path(os.environ.get("JCODE_HOME", pathlib.Path.home() / ".jcode"))


def log_files(log_dir: pathlib.Path, max_log_files: int) -> list[pathlib.Path]:
    files = sorted(log_dir.glob("jcode-*.log"), key=lambda p: p.stat().st_mtime, reverse=True)
    return list(reversed(files[:max_log_files]))


def normalize_reload_id(value: str | None) -> str | None:
    if not value:
        return None
    return value if value.startswith("reload_") else f"reload_{value}"


def add_event(records: dict[tuple[str, str], IntentRecord], event: Event) -> None:
    if not event.reload_id or not event.session_id:
        return
    key = (event.reload_id, event.session_id)
    record = records.setdefault(
        key,
        IntentRecord(
            reload_id=event.reload_id,
            session_id=event.session_id,
            role=event.role,
        ),
    )
    if event.role and not record.role:
        record.role = event.role
    if event.kind == "persisted":
        record.persisted = event
    elif event.kind == "claimed":
        record.claimed = event
    elif event.kind == "attached":
        record.attached.append(event)
    elif event.kind == "delivered":
        record.delivered.append(event)
    elif event.kind == "delivery_mismatch":
        record.delivery_mismatch.append(event)
    elif event.kind == "non_pending":
        record.non_pending.append(event)
    elif event.kind == "history_snapshot":
        record.history_snapshot.append(event)
    elif event.kind == "history_queued":
        record.history_queued.append(event)
    elif event.kind == "tui_flow":
        record.tui_flows.append(event)
    elif event.kind == "send_history":
        record.send_history.append(event)
    elif event.kind == "member_running":
        record.member_running.append(event)


def parse_logs(files: list[pathlib.Path]) -> tuple[dict[tuple[str, str], IntentRecord], list[Event]]:
    records: dict[tuple[str, str], IntentRecord] = {}
    session_latest_reload: dict[str, str] = {}
    global_hidden_sends: list[Event] = []

    for path in files:
        try:
            handle = path.open(errors="ignore")
        except OSError:
            continue
        with handle:
            for line_no, line in enumerate(handle, 1):
                text = line.rstrip("\n")

                if match := PERSISTED_RE.search(text):
                    reload_id, session_id, role = match.groups()
                    session_latest_reload[session_id] = reload_id
                    add_event(
                        records,
                        Event(
                            "persisted",
                            line_no,
                            text,
                            path.name,
                            reload_id,
                            session_id,
                            role,
                        ),
                    )
                    continue

                if match := CLAIMED_RE.search(text):
                    reload_id, session_id, role = match.groups()
                    session_latest_reload[session_id] = reload_id
                    add_event(
                        records,
                        Event("claimed", line_no, text, path.name, reload_id, session_id, role),
                    )
                    continue

                if match := ATTACHED_RE.search(text):
                    reload_id, session_id, role = match.groups()
                    session_latest_reload[session_id] = reload_id
                    add_event(
                        records,
                        Event("attached", line_no, text, path.name, reload_id, session_id, role),
                    )
                    continue

                if match := DELIVERED_RE.search(text):
                    reload_id, session_id, role, accepted_by = match.groups()
                    session_latest_reload[session_id] = reload_id
                    add_event(
                        records,
                        Event(
                            "delivered",
                            line_no,
                            text,
                            path.name,
                            reload_id,
                            session_id,
                            role,
                            detail=f"accepted_by={accepted_by}",
                        ),
                    )
                    continue

                if match := NON_PENDING_RE.search(text):
                    session_id, reload_id, status = match.groups()
                    session_latest_reload[session_id] = reload_id
                    add_event(
                        records,
                        Event(
                            "non_pending",
                            line_no,
                            text,
                            path.name,
                            reload_id,
                            session_id,
                            detail=status,
                        ),
                    )
                    continue

                if match := HISTORY_QUEUE_RE.search(text):
                    session_id, was_interrupted = match.groups()
                    reload_id = session_latest_reload.get(session_id)
                    if reload_id:
                        add_event(
                            records,
                            Event(
                                "history_queued",
                                line_no,
                                text,
                                path.name,
                                reload_id,
                                session_id,
                                detail=f"was_interrupted={was_interrupted}",
                            ),
                        )
                    continue

                if match := TUI_FLOW_RE.search(text):
                    flow, session_id, outcome, detail = match.groups()
                    reload_id = session_latest_reload.get(session_id)
                    if reload_id:
                        add_event(
                            records,
                            Event(
                                "tui_flow",
                                line_no,
                                text,
                                path.name,
                                reload_id,
                                session_id,
                                detail=f"flow={flow} outcome={outcome} detail={detail}",
                            ),
                        )
                    continue

                if match := HISTORY_SNAPSHOT_RE.search(text):
                    session_id = match.group(1)
                    reload_id = session_latest_reload.get(session_id)
                    if reload_id:
                        add_event(
                            records,
                            Event(
                                "history_snapshot",
                                line_no,
                                text,
                                path.name,
                                reload_id,
                                session_id,
                            ),
                        )
                    continue

                if match := SEND_HISTORY_RE.search(text):
                    session_id = match.group(1)
                    reload_id = session_latest_reload.get(session_id)
                    if reload_id:
                        add_event(
                            records,
                            Event("send_history", line_no, text, path.name, reload_id, session_id),
                        )
                    continue

                if match := MESSAGE_RUNNING_RE.search(text):
                    session_id = match.group(1)
                    reload_id = session_latest_reload.get(session_id)
                    if reload_id:
                        add_event(
                            records,
                            Event("member_running", line_no, text, path.name, reload_id, session_id),
                        )
                    continue

                if match := HIDDEN_SEND_RE.search(text):
                    global_hidden_sends.append(
                        Event(
                            "hidden_send",
                            line_no,
                            text,
                            path.name,
                            detail=f"chars={match.group(1)}",
                        )
                    )

    return records, global_hidden_sends


def parse_trace_files(records: dict[tuple[str, str], IntentRecord], trace_dir: pathlib.Path) -> None:
    """Merge structured ~/.jcode/reload-traces/*.jsonl lifecycle events."""
    phase_to_kind = {
        "intent_attached_to_history": "attached",
        "intent_delivered": "delivered",
        "intent_delivery_mismatch": "delivery_mismatch",
    }
    for path in sorted(trace_dir.glob("*.jsonl")):
        try:
            handle = path.open(errors="ignore")
        except OSError:
            continue
        with handle:
            for line_no, line in enumerate(handle, 1):
                text = line.rstrip("\n")
                try:
                    data = json.loads(text)
                except json.JSONDecodeError:
                    continue
                phase = data.get("phase")
                kind = phase_to_kind.get(phase)
                if not kind:
                    continue
                reload_id = data.get("reload_id") or path.stem
                session_id = data.get("session_id")
                if not reload_id or not session_id:
                    continue
                detail_parts = []
                if accepted_by := data.get("accepted_by"):
                    detail_parts.append(f"accepted_by={accepted_by}")
                if status := data.get("status"):
                    detail_parts.append(f"status={status}")
                if phase:
                    detail_parts.append(f"phase={phase}")
                add_event(
                    records,
                    Event(
                        kind,
                        line_no,
                        text,
                        path.name,
                        reload_id,
                        session_id,
                        data.get("role"),
                        detail=" ".join(detail_parts) or None,
                    ),
                )


def merge_durable_records(records: dict[tuple[str, str], IntentRecord], recovery_dir: pathlib.Path) -> None:
    for path in sorted(recovery_dir.glob("*.json")):
        try:
            data = json.loads(path.read_text())
        except Exception:
            continue
        reload_id = data.get("reload_id")
        session_id = data.get("session_id")
        if not reload_id or not session_id:
            continue
        key = (reload_id, session_id)
        record = records.setdefault(
            key,
            IntentRecord(
                reload_id=reload_id,
                session_id=session_id,
                role=data.get("role"),
            ),
        )
        record.role = record.role or data.get("role")
        record.durable_status = data.get("status")
        record.durable_delivered_at = data.get("delivered_at")


def add_transcript_markers(records: list[IntentRecord], sessions_dir: pathlib.Path) -> None:
    markers = [
        "Reload succeeded",
        "interrupted by a server reload",
        "Continue immediately from where you left off",
        "Reload complete",
    ]
    session_to_records: dict[str, list[IntentRecord]] = defaultdict(list)
    for record in records:
        session_to_records[record.session_id].append(record)

    for session_id, session_records in session_to_records.items():
        path = sessions_dir / f"{session_id}.json"
        if not path.exists():
            continue
        try:
            text = path.read_text(errors="ignore")
        except OSError:
            continue
        counts = {marker: text.count(marker) for marker in markers}
        for record in session_records:
            record.transcript_markers = counts


def short_session(session_id: str) -> str:
    if len(session_id) <= 34:
        return session_id
    return session_id[:31] + "..."


def render_table(records: list[IntentRecord], hidden_sends: list[Event], show_lines: bool) -> str:
    if not records:
        return "No reload recovery records found for the selected filters."

    rows = []
    for record in records:
        client = "yes" if record.has_client_queue_evidence() else "no"
        accepted = "yes" if record.has_server_acceptance_evidence() else "no"
        claimed = "yes" if record.claimed else "no"
        persisted = "yes" if record.persisted else "no"
        transcript = sum(record.transcript_markers.values()) if record.transcript_markers else 0
        rows.append(
            [
                record.reload_id,
                short_session(record.session_id),
                record.role or "?",
                record.durable_status or "?",
                persisted,
                claimed,
                str(len(record.attached)),
                accepted,
                client,
                str(record.send_history and len(record.send_history) or 0),
                str(record.member_running and len(record.member_running) or 0),
                str(transcript),
                record.verdict(),
            ]
        )

    headers = [
        "reload_id",
        "session",
        "role",
        "store",
        "persist",
        "old_claim",
        "attach",
        "accepted",
        "client_queue",
        "hist_write",
        "ran_after",
        "markers",
        "verdict",
    ]
    widths = [len(h) for h in headers]
    for row in rows:
        for idx, value in enumerate(row):
            widths[idx] = max(widths[idx], len(value))

    def fmt(row: list[str]) -> str:
        return "  ".join(value.ljust(widths[idx]) for idx, value in enumerate(row))

    out = [fmt(headers), fmt(["-" * width for width in widths])]
    out.extend(fmt(row) for row in rows)
    suspects = [record for record in records if record.verdict().startswith("suspect")]
    out.append("")
    out.append(f"Global hidden continuation send log lines scanned: {len(hidden_sends)}")
    out.append(f"Suspect delivered-without-acceptance records: {len(suspects)}")

    if suspects:
        out.append("")
        out.append("Suspects:")
        for record in suspects:
            claim = f"{record.claimed.file}:{record.claimed.line}" if record.claimed else "durable-only"
            out.append(
                f"  - {record.session_id} ({record.role}) {record.reload_id} terminal={claim}"
            )

    if show_lines:
        out.append("")
        out.append("Event lines:")
        for record in records:
            out.append(f"\n[{record.reload_id} {record.session_id}]")
            for event in record.compact(show_lines=True).get("events", []):
                out.append(f"  {event['file']}:{event['line']} {event['kind']}: {event['text']}")

    return "\n".join(out)


def main() -> int:
    parser = argparse.ArgumentParser(description="Audit self-dev reload recovery handoffs")
    parser.add_argument("--home", type=pathlib.Path, default=jcode_home(), help="JCODE_HOME path")
    parser.add_argument("--reload-id", type=str, help="Only show one reload id")
    parser.add_argument("--session", type=str, help="Only show one session id")
    parser.add_argument("--max-log-files", type=int, default=3, help="Newest jcode logs to scan")
    parser.add_argument("--show-lines", action="store_true", help="Show matching source log lines")
    parser.add_argument("--json", action="store_true", help="Emit JSON")
    args = parser.parse_args()

    home = args.home
    records, hidden_sends = parse_logs(log_files(home / "logs", args.max_log_files))
    parse_trace_files(records, home / "reload-traces")
    merge_durable_records(records, home / "reload-recovery")

    reload_filter = normalize_reload_id(args.reload_id)
    selected = list(records.values())
    if reload_filter:
        selected = [record for record in selected if record.reload_id == reload_filter]
    if args.session:
        selected = [record for record in selected if record.session_id == args.session]

    selected.sort(key=lambda record: (record.reload_id, record.session_id))
    add_transcript_markers(selected, home / "sessions")

    if args.json:
        print(
            json.dumps(
                {
                    "home": str(home),
                    "records": [record.compact(show_lines=args.show_lines) for record in selected],
                    "global_hidden_sends": [event.compact() for event in hidden_sends],
                },
                indent=2,
                sort_keys=True,
            )
        )
    else:
        print(render_table(selected, hidden_sends, args.show_lines))

    return 0


if __name__ == "__main__":
    sys.exit(main())
