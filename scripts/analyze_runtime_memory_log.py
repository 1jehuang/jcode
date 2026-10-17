#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import statistics
from collections import Counter
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

DEFAULT_LOG_GLOB = "*runtime-memory-*.jsonl"
DEFAULT_TOP_N = 8


@dataclass
class Sample:
    path: Path
    line_no: int
    raw: dict[str, Any]
    timestamp_ms: int
    kind: str
    target: str
    instance_id: str
    source: str
    trigger_category: str
    trigger_reason: str
    sessions: dict[str, Any] | None
    totals: dict[str, Any] | None

    @property
    def pss_bytes(self) -> int | None:
        os_info = self.raw.get("process", {}).get("os") or {}
        value = os_info.get("pss_bytes")
        return int(value) if isinstance(value, int | float) else None

    @property
    def rss_bytes(self) -> int | None:
        value = self.raw.get("process", {}).get("rss_bytes")
        return int(value) if isinstance(value, int | float) else None

    @property
    def allocator_allocated_bytes(self) -> int | None:
        value = (((self.raw.get("process") or {}).get("allocator") or {}).get("stats") or {}).get(
            "allocated_bytes"
        )
        return int(value) if isinstance(value, int | float) else None

    @property
    def allocator_resident_bytes(self) -> int | None:
        value = (((self.raw.get("process") or {}).get("allocator") or {}).get("stats") or {}).get(
            "resident_bytes"
        )
        return int(value) if isinstance(value, int | float) else None

    @property
    def allocator_retained_bytes(self) -> int | None:
        value = (((self.raw.get("process") or {}).get("allocator") or {}).get("stats") or {}).get(
            "retained_bytes"
        )
        return int(value) if isinstance(value, int | float) else None

    @property
    def os_info(self) -> dict[str, Any]:
        value = (self.raw.get("process") or {}).get("os")
        return value if isinstance(value, dict) else {}

    @property
    def process_info(self) -> dict[str, Any]:
        value = self.raw.get("process")
        return value if isinstance(value, dict) else {}

    @property
    def process_diagnostics(self) -> dict[str, Any]:
        value = self.raw.get("process_diagnostics")
        return value if isinstance(value, dict) else {}


def first_int(mapping: dict[str, Any], *keys: str) -> int | None:
    """Return the first present integer value among candidate key names."""
    for key in keys:
        value = mapping.get(key)
        if isinstance(value, int | float):
            return int(value)
    return None


@dataclass
class Spike:
    start: Sample
    end: Sample
    delta_pss_bytes: int


@dataclass
class AttributionDelta:
    start: Sample
    end: Sample
    delta_total_json_bytes: int
    delta_payload_text_bytes: int
    delta_provider_cache_json_bytes: int
    delta_tool_result_bytes: int
    delta_large_blob_bytes: int
    delta_live_count: int
    delta_memory_enabled_session_count: int

    @property
    def magnitude_bytes(self) -> int:
        return max(
            abs(self.delta_total_json_bytes),
            abs(self.delta_provider_cache_json_bytes),
            abs(self.delta_tool_result_bytes),
            abs(self.delta_large_blob_bytes),
            abs(self.delta_payload_text_bytes),
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Analyze jcode runtime memory JSONL logs for growth, spikes, attribution, and optimization hints"
    )
    parser.add_argument("paths", nargs="*", help="Specific JSONL files or directories to analyze")
    parser.add_argument(
        "--log-dir",
        help="Directory containing runtime memory JSONL logs (default: ~/.kcode/logs/memory or $KCODE_HOME/logs/memory)",
    )
    parser.add_argument("--days", type=int, default=None, help="Only include files from the last N daily logs")
    parser.add_argument("--top", type=int, default=DEFAULT_TOP_N, help="How many spikes/sessions/deltas to show")
    parser.add_argument("--json", action="store_true", help="Emit machine-readable JSON summary")
    instance_mode = parser.add_mutually_exclusive_group()
    instance_mode.add_argument(
        "--all-instances",
        action="store_true",
        help="Analyze all process instances in the selected files instead of the latest server/client instance",
    )
    instance_mode.add_argument(
        "--instance",
        help="Analyze one exact server or client process instance ID",
    )
    instance_mode.add_argument(
        "--list-instances",
        action="store_true",
        help="List available process instance IDs and their time ranges, then exit",
    )
    parser.add_argument(
        "--min-spike-mb",
        type=float,
        default=8.0,
        help="Minimum absolute PSS delta in MB to include in spike lists",
    )
    return parser.parse_args()


def default_log_dir() -> Path:
    jcode_home = os.environ.get("KCODE_HOME")
    if jcode_home:
        return Path(jcode_home).expanduser() / "logs" / "memory"
    return Path.home() / ".kcode" / "logs" / "memory"


def resolve_paths(args: argparse.Namespace) -> list[Path]:
    raw_paths = [Path(value).expanduser() for value in args.paths]
    if args.log_dir:
        raw_paths.append(Path(args.log_dir).expanduser())
    if not raw_paths:
        raw_paths.append(default_log_dir())

    files: list[Path] = []
    for raw in raw_paths:
        if raw.is_file():
            files.append(raw)
            continue
        if raw.is_dir():
            files.extend(sorted(raw.glob(DEFAULT_LOG_GLOB)))
    files = sorted(dict.fromkeys(path.resolve() for path in files))
    if args.days is not None and args.days > 0:
        selected_dates = []
        for path in reversed(files):
            date = extract_log_date(path)
            if date is None or date in selected_dates:
                continue
            selected_dates.append(date)
            if len(selected_dates) >= args.days:
                break
        files = [path for path in files if extract_log_date(path) in selected_dates]
    return files


def extract_log_date(path: Path) -> str | None:
    name = path.name
    if not name.endswith('.jsonl'):
        return None
    stem = name[:-len('.jsonl')]
    if '-' not in stem:
        return None
    return stem.rsplit('-', 3)[-3] + '-' + stem.rsplit('-', 3)[-2] + '-' + stem.rsplit('-', 3)[-1]


def load_samples(paths: Iterable[Path]) -> list[Sample]:
    samples: list[Sample] = []
    for path in paths:
        try:
            lines = path.read_text().splitlines()
        except FileNotFoundError:
            continue
        for idx, line in enumerate(lines, start=1):
            line = line.strip()
            if not line:
                continue
            try:
                raw = json.loads(line)
            except json.JSONDecodeError:
                continue
            trigger = raw.get("trigger") or {}
            source = str(raw.get("source") or "")
            kind = infer_kind(raw, source)
            target = infer_target(raw, path)
            instance_id = infer_instance_id(raw, target)
            trigger_category, trigger_reason = infer_trigger(raw, kind, source, trigger)
            samples.append(
                Sample(
                    path=path,
                    line_no=idx,
                    raw=raw,
                    timestamp_ms=int(raw.get("timestamp_ms") or 0),
                    kind=kind,
                    target=target,
                    instance_id=instance_id,
                    source=source,
                    trigger_category=trigger_category,
                    trigger_reason=trigger_reason,
                    sessions=raw.get("sessions") if isinstance(raw.get("sessions"), dict) else None,
                    totals=raw.get("totals") if isinstance(raw.get("totals"), dict) else None,
                )
            )
    samples.sort(key=lambda sample: (sample.timestamp_ms, str(sample.path), sample.line_no))
    return samples


def infer_instance_id(raw: dict[str, Any], target: str) -> str:
    if target == "server":
        server = raw.get("server") or {}
        value = server.get("id")
    else:
        client = raw.get("client") or {}
        value = client.get("client_instance_id")
    return str(value) if value else "legacy"


def select_latest_instances(samples: list[Sample]) -> list[Sample]:
    """Keep one coherent process lifetime per target.

    Daily JSONL files contain multiple server reloads and client processes. PSS
    deltas across process boundaries are meaningless and previously produced
    multi-gigabyte false spikes. The default report now follows the latest
    instance for each target; --all-instances preserves the old forensic view.
    """
    latest_by_target: dict[str, tuple[str, int]] = {}
    for sample in samples:
        current = latest_by_target.get(sample.target)
        if current is None or sample.timestamp_ms > current[1]:
            latest_by_target[sample.target] = (sample.instance_id, sample.timestamp_ms)
    selected = [
        sample
        for sample in samples
        if latest_by_target.get(sample.target, (None, 0))[0] == sample.instance_id
    ]
    selected.sort(key=lambda sample: (sample.timestamp_ms, str(sample.path), sample.line_no))
    return selected


def select_instance(samples: list[Sample], instance_id: str) -> list[Sample]:
    selected = [sample for sample in samples if sample.instance_id == instance_id]
    selected.sort(key=lambda sample: (sample.timestamp_ms, str(sample.path), sample.line_no))
    return selected


def instance_inventory(samples: list[Sample]) -> list[dict[str, Any]]:
    grouped: dict[tuple[str, str], list[Sample]] = {}
    for sample in samples:
        grouped.setdefault((sample.target, sample.instance_id), []).append(sample)

    inventory: list[dict[str, Any]] = []
    for (target, instance_id), group in grouped.items():
        ordered = sorted(group, key=lambda sample: (sample.timestamp_ms, str(sample.path), sample.line_no))
        inventory.append(
            {
                "target": target,
                "instance_id": instance_id,
                "sample_count": len(ordered),
                "first_timestamp_ms": ordered[0].timestamp_ms,
                "last_timestamp_ms": ordered[-1].timestamp_ms,
                "duration_ms": max(0, ordered[-1].timestamp_ms - ordered[0].timestamp_ms),
                "kinds": dict(Counter(sample.kind for sample in ordered)),
            }
        )
    inventory.sort(key=lambda item: (item["last_timestamp_ms"], item["target"]), reverse=True)
    return inventory


def infer_kind(raw: dict[str, Any], source: str) -> str:
    kind = raw.get("kind")
    if isinstance(kind, str) and kind:
        return kind
    if isinstance(raw.get("sessions"), dict):
        return "attribution"
    if source.startswith("process:"):
        return "process"
    if source.startswith("attribution:"):
        return "attribution"
    return "legacy"


def infer_target(raw: dict[str, Any], path: Path) -> str:
    if isinstance(raw.get("client"), dict):
        return "client"
    if isinstance(raw.get("server"), dict):
        return "server"
    name = path.name
    if name.startswith("client-runtime-memory-"):
        return "client"
    return "server"


def infer_trigger(
    raw: dict[str, Any], kind: str, source: str, trigger: dict[str, Any]
) -> tuple[str, str]:
    category = str(trigger.get("category") or "")
    reason = str(trigger.get("reason") or "")
    if category and reason:
        return category, reason
    if source == "startup" or source.endswith(":startup"):
        return category or "startup", reason or "server_start"
    if source == "interval" or source.startswith("process:heartbeat"):
        return category or "process_heartbeat", reason or "periodic"
    if source.startswith("attribution:heartbeat"):
        return category or "attribution_heartbeat", reason or "periodic"
    if source.startswith("attribution:event:") or source.startswith("process:event:"):
        suffix = source.split(":event:", 1)[1]
        return category or suffix, reason or "event"
    if source.startswith("server:runtime-log:"):
        suffix = source.rsplit(":", 1)[-1]
        return category or suffix, reason or ("periodic" if suffix == "interval" else kind)
    return category or kind, reason or "legacy"


def bytes_to_mb(value: int | None) -> float | None:
    if value is None:
        return None
    return round(value / (1024.0 * 1024.0), 1)


def fmt_mb(value: int | None) -> str:
    if value is None:
        return "n/a"
    return f"{value / (1024.0 * 1024.0):.1f} MB"


def fmt_signed_mb(value: int | None) -> str:
    if value is None:
        return "n/a"
    sign = "+" if value >= 0 else "-"
    return f"{sign}{abs(value) / (1024.0 * 1024.0):.1f} MB"


def fmt_duration_ms(ms: int) -> str:
    seconds = ms / 1000.0
    if seconds < 60:
        return f"{seconds:.1f}s"
    minutes = seconds / 60.0
    if minutes < 60:
        return f"{minutes:.1f}m"
    hours = minutes / 60.0
    return f"{hours:.1f}h"


def fmt_ts(timestamp_ms: int) -> str:
    dt = datetime.fromtimestamp(timestamp_ms / 1000.0, tz=timezone.utc)
    return dt.isoformat().replace("+00:00", "Z")


def attributed_total_bytes(sample: Sample) -> int | None:
    if sample.sessions:
        value = sample.sessions.get("total_json_bytes")
        return int(value) if isinstance(value, int | float) else None
    if sample.totals:
        value = sample.totals.get("total_attributed_bytes")
        return int(value) if isinstance(value, int | float) else None
    return None


def compute_spikes(samples: list[Sample], min_spike_bytes: int) -> list[Spike]:
    process_samples = [sample for sample in samples if sample.pss_bytes is not None]
    spikes: list[Spike] = []
    for prev, curr in zip(process_samples, process_samples[1:]):
        if prev.pss_bytes is None or curr.pss_bytes is None:
            continue
        delta = curr.pss_bytes - prev.pss_bytes
        if abs(delta) >= min_spike_bytes:
            spikes.append(Spike(start=prev, end=curr, delta_pss_bytes=delta))
    spikes.sort(key=lambda spike: abs(spike.delta_pss_bytes), reverse=True)
    return spikes


def compute_attribution_deltas(samples: list[Sample]) -> list[AttributionDelta]:
    attribution = [sample for sample in samples if sample.sessions]
    deltas: list[AttributionDelta] = []
    for prev, curr in zip(attribution, attribution[1:]):
        prev_sessions = prev.sessions or {}
        curr_sessions = curr.sessions or {}
        deltas.append(
            AttributionDelta(
                start=prev,
                end=curr,
                delta_total_json_bytes=int(curr_sessions.get("total_json_bytes", 0))
                - int(prev_sessions.get("total_json_bytes", 0)),
                delta_payload_text_bytes=int(curr_sessions.get("total_payload_text_bytes", 0))
                - int(prev_sessions.get("total_payload_text_bytes", 0)),
                delta_provider_cache_json_bytes=int(curr_sessions.get("total_provider_cache_json_bytes", 0))
                - int(prev_sessions.get("total_provider_cache_json_bytes", 0)),
                delta_tool_result_bytes=int(curr_sessions.get("total_tool_result_bytes", 0))
                - int(prev_sessions.get("total_tool_result_bytes", 0)),
                delta_large_blob_bytes=int(curr_sessions.get("total_large_blob_bytes", 0))
                - int(prev_sessions.get("total_large_blob_bytes", 0)),
                delta_live_count=int(curr_sessions.get("live_count", 0))
                - int(prev_sessions.get("live_count", 0)),
                delta_memory_enabled_session_count=int(curr_sessions.get("memory_enabled_session_count", 0))
                - int(prev_sessions.get("memory_enabled_session_count", 0)),
            )
        )
    deltas.sort(key=lambda delta: delta.magnitude_bytes, reverse=True)
    return deltas


def collect_session_peaks(samples: list[Sample]) -> list[dict[str, Any]]:
    session_stats: dict[str, dict[str, Any]] = {}
    for sample in samples:
        sessions = sample.sessions or {}
        top = sessions.get("top_by_json_bytes") or []
        if not isinstance(top, list):
            continue
        for entry in top:
            if not isinstance(entry, dict):
                continue
            session_id = str(entry.get("session_id") or "")
            if not session_id:
                continue
            json_bytes = int(entry.get("json_bytes") or 0)
            current = session_stats.get(session_id)
            if current is None or json_bytes > current["peak_json_bytes"]:
                session_stats[session_id] = {
                    "session_id": session_id,
                    "provider": entry.get("provider"),
                    "model": entry.get("model"),
                    "memory_enabled": bool(entry.get("memory_enabled")),
                    "peak_json_bytes": json_bytes,
                    "peak_payload_text_bytes": int(entry.get("payload_text_bytes") or 0),
                    "peak_provider_cache_json_bytes": int(entry.get("provider_cache_json_bytes") or 0),
                    "peak_tool_result_bytes": int(entry.get("tool_result_bytes") or 0),
                    "peak_large_blob_bytes": int(entry.get("large_blob_bytes") or 0),
                    "message_count": int(entry.get("message_count") or 0),
                    "last_seen_timestamp_ms": sample.timestamp_ms,
                }
    return sorted(session_stats.values(), key=lambda item: item["peak_json_bytes"], reverse=True)


def last_attribution_sample(samples: list[Sample]) -> Sample | None:
    for sample in reversed(samples):
        if sample.sessions or sample.totals:
            return sample
    return None


def build_coverage_report(sample: Sample) -> dict[str, Any]:
    """Decompose PSS into attributed live, unattributed live heap, allocator
    retention, file-backed, and stack buckets.

    Uses allocator stats (mallinfo2/jemalloc) plus the newer smaps_rollup and
    process_diagnostics fields when present; older logs degrade gracefully to
    whatever fields exist. The key outputs are two coverage ratios:
    - coverage_ratio_pss: attributed / PSS (the historical, misleading one; the
      denominator includes allocator retention and file maps).
    - coverage_ratio_live_heap: attributed / allocator live bytes (estimator
      quality against the memory the app actually holds).

    Explained PSS follows the in-binary summary definition:
    total_attributed + allocator_retained_resident_estimate + pss_file +
    thread_stack_estimate.
    """
    pss = sample.pss_bytes
    attributed = attributed_total_bytes(sample)
    allocator_live = sample.allocator_allocated_bytes
    allocator_retained = sample.allocator_retained_bytes

    process_info = sample.process_info
    os_info = sample.os_info
    pss_file = first_int(os_info, "pss_file_bytes")
    pss_anon = first_int(os_info, "pss_anon_bytes")
    pss_shmem = first_int(os_info, "pss_shmem_bytes")
    anon_huge_pages = first_int(os_info, "anon_huge_pages_bytes")
    rss_file = first_int(os_info, "rss_file_bytes")
    stack_bytes = first_int(process_info, "main_stack_bytes") or first_int(
        os_info, "main_stack_bytes", "stack_bytes", "vm_stk_bytes"
    )
    thread_count = first_int(process_info, "thread_count") or first_int(
        os_info, "thread_count", "threads"
    )

    diagnostics = sample.process_diagnostics
    retained_resident = first_int(
        diagnostics,
        "allocator_retained_resident_estimate_bytes",
        "allocator_retained_bytes",
    )
    if retained_resident is None:
        retained_resident = allocator_retained
    thread_stack_estimate = first_int(diagnostics, "thread_stack_estimate_bytes")
    if thread_stack_estimate is None:
        thread_stack_estimate = stack_bytes

    report: dict[str, Any] = {
        "timestamp_ms": sample.timestamp_ms,
        "pss_bytes": pss,
        "pss_anon_bytes": pss_anon,
        "pss_file_bytes": pss_file,
        "pss_shmem_bytes": pss_shmem,
        "anon_huge_pages_bytes": anon_huge_pages,
        "rss_file_bytes": rss_file,
        "main_stack_bytes": stack_bytes,
        "thread_stack_estimate_bytes": thread_stack_estimate,
        "thread_count": thread_count,
        "attributed_live_bytes": attributed,
        "allocator_live_bytes": allocator_live,
        "allocator_retained_bytes": allocator_retained,
        "allocator_retained_resident_estimate_bytes": retained_resident,
    }

    if attributed is not None and allocator_live is not None:
        report["unattributed_live_heap_bytes"] = max(0, allocator_live - attributed)
    if pss is not None and attributed is not None:
        report["coverage_ratio_pss"] = round(attributed / pss, 4) if pss else 0.0
    if allocator_live is not None and attributed is not None:
        report["coverage_ratio_live_heap"] = (
            round(attributed / allocator_live, 4) if allocator_live else 0.0
        )

    # Explained PSS matches the in-binary summary: attributed live + allocator
    # retention (resident estimate) + file-backed PSS + thread stacks. The
    # remainder is what the buckets still miss (unattributed live heap, shared
    # anon, allocator metadata).
    if pss is not None:
        explained = 0
        for key in (
            "attributed_live_bytes",
            "allocator_retained_resident_estimate_bytes",
            "pss_file_bytes",
            "thread_stack_estimate_bytes",
        ):
            value = report.get(key)
            if isinstance(value, int):
                explained += value
        report["explained_pss_bytes"] = explained
        report["unexplained_pss_bytes"] = max(0, pss - explained)
        report["explained_ratio"] = round(explained / pss, 4) if pss else 0.0
    return report


def count_event_categories(samples: list[Sample]) -> Counter[str]:
    counter: Counter[str] = Counter()
    for sample in samples:
        category = sample.trigger_category or sample.kind
        counter[category] += 1
    return counter


def process_summary(samples: list[Sample]) -> dict[str, Any]:
    process_samples = [sample for sample in samples if sample.pss_bytes is not None]
    if not process_samples:
        return {}
    first = process_samples[0]
    last = process_samples[-1]
    peak = max(process_samples, key=lambda sample: sample.pss_bytes or -1)
    pss_values = [sample.pss_bytes for sample in process_samples if sample.pss_bytes is not None]
    median_pss = int(statistics.median(pss_values)) if pss_values else None
    return {
        "sample_count": len(process_samples),
        "first_timestamp_ms": first.timestamp_ms,
        "last_timestamp_ms": last.timestamp_ms,
        "duration_ms": max(0, last.timestamp_ms - first.timestamp_ms),
        "baseline_pss_bytes": first.pss_bytes,
        "final_pss_bytes": last.pss_bytes,
        "net_pss_growth_bytes": (last.pss_bytes or 0) - (first.pss_bytes or 0),
        "peak_pss_bytes": peak.pss_bytes,
        "peak_growth_vs_baseline_bytes": (peak.pss_bytes or 0) - (first.pss_bytes or 0),
        "median_pss_bytes": median_pss,
        "peak_timestamp_ms": peak.timestamp_ms,
        "peak_trigger_category": peak.trigger_category,
        "peak_trigger_reason": peak.trigger_reason,
        "allocator_allocated_bytes": last.allocator_allocated_bytes,
        "allocator_resident_bytes": last.allocator_resident_bytes,
        "allocator_retained_bytes": last.allocator_retained_bytes,
    }


def session_population_summary(samples: list[Sample]) -> dict[str, Any]:
    attribution = [
        sample
        for sample in samples
        if sample.sessions and isinstance(sample.sessions.get("live_count"), int | float)
    ]
    if not attribution:
        return {}
    first = attribution[0]
    last = attribution[-1]
    peak = max(attribution, key=lambda sample: int((sample.sessions or {}).get("live_count") or 0))
    baseline_live = int((first.sessions or {}).get("live_count") or 0)
    final_live = int((last.sessions or {}).get("live_count") or 0)
    peak_live = int((peak.sessions or {}).get("live_count") or 0)
    baseline_allocated = first.allocator_allocated_bytes
    final_allocated = last.allocator_allocated_bytes
    net_allocated_growth = (
        final_allocated - baseline_allocated
        if final_allocated is not None and baseline_allocated is not None
        else None
    )
    added_sessions = final_live - baseline_live
    connected_count = first_int(last.raw.get("clients") or {}, "connected_count", "connected_clients")
    return {
        "baseline_timestamp_ms": first.timestamp_ms,
        "final_timestamp_ms": last.timestamp_ms,
        "baseline_live_sessions": baseline_live,
        "final_live_sessions": final_live,
        "peak_live_sessions": peak_live,
        "peak_timestamp_ms": peak.timestamp_ms,
        "net_live_session_growth": added_sessions,
        "final_connected_clients": connected_count,
        "detached_or_headless_live_sessions": (
            max(0, final_live - connected_count) if connected_count is not None else None
        ),
        "baseline_allocator_live_bytes": baseline_allocated,
        "final_allocator_live_bytes": final_allocated,
        "net_allocator_live_growth_bytes": net_allocated_growth,
        "allocator_growth_per_added_session_bytes": (
            max(0, net_allocated_growth) // added_sessions
            if net_allocated_growth is not None and added_sessions > 0
            else None
        ),
    }


def build_incident_assessment(
    samples: list[Sample],
    process: dict[str, Any],
    coverage: dict[str, Any] | None,
    population: dict[str, Any],
) -> dict[str, Any]:
    if not samples:
        return {}
    final_pss = int(process.get("final_pss_bytes") or 0)
    growth = int(process.get("net_pss_growth_bytes") or 0)
    final_live = int(population.get("final_live_sessions") or 0)
    live_growth = int(population.get("net_live_session_growth") or 0)
    connected = population.get("final_connected_clients")
    detached = population.get("detached_or_headless_live_sessions")
    allocator_live = int((coverage or {}).get("allocator_live_bytes") or 0)
    attributed = int((coverage or {}).get("attributed_live_bytes") or 0)
    retained_resident = int(
        (coverage or {}).get("allocator_retained_resident_estimate_bytes") or 0
    )
    coverage_live = (coverage or {}).get("coverage_ratio_live_heap")

    runaway_population = final_live >= 128 and (
        live_growth >= 64
        or (isinstance(detached, int) and detached >= 128)
        or (isinstance(connected, int) and final_live >= max(128, connected * 16))
    )
    retention_dominates = (
        retained_resident >= 256 * 1024 * 1024
        and final_pss > 0
        and retained_resident * 4 >= final_pss
    )
    attributed_state_dominates = (
        allocator_live > 0 and attributed >= 512 * 1024 * 1024 and attributed * 2 >= allocator_live
    )

    if runaway_population:
        cause = "runaway_live_session_population"
        confidence = "high"
        summary = (
            f"Live Agent population grew from {population.get('baseline_live_sessions')} to "
            f"{final_live} while allocator live memory grew by "
            f"{fmt_mb(population.get('net_allocator_live_growth_bytes'))}."
        )
        evidence = [
            f"{final_live} live sessions vs {connected if connected is not None else 'unknown'} connected clients",
            f"{live_growth:+d} live sessions during this process lifetime",
            f"{fmt_mb(population.get('allocator_growth_per_added_session_bytes'))} allocator growth per added session",
            f"allocator live {fmt_mb(allocator_live)} vs attributed session JSON {fmt_mb(attributed)}",
        ]
        actions = [
            {
                "priority": 1,
                "action": "Pause or cap the workload creating headless sessions.",
                "why": "These are live allocations; allocator purge is not the first response.",
                "commands": ["jcode debug 'server:memory-incident'", "jcode debug 'swarm:list'"],
            },
            {
                "priority": 2,
                "action": "Inspect the largest live swarm and clean only workers confirmed disposable by its owning coordinator.",
                "commands": ["Use `swarm list` and `swarm cleanup` from the owning coordinator"],
            },
            {
                "priority": 3,
                "action": "Re-measure and require live_sessions, allocator live, and PSS to fall together.",
                "commands": ["python scripts/analyze_runtime_memory_log.py --days 1"],
            },
            {
                "priority": 4,
                "action": "Purge only after session cleanup if freed-but-held memory remains high.",
                "commands": ["jcode debug 'allocator:purge'"],
            },
        ]
    elif retention_dominates:
        cause = "allocator_retention"
        confidence = "high"
        summary = "Freed-but-held allocator pages are a material share of current PSS."
        evidence = [
            f"retained resident estimate {fmt_mb(retained_resident)}",
            f"allocator live {fmt_mb(allocator_live)} vs PSS {fmt_mb(final_pss)}",
        ]
        actions = [
            {
                "priority": 1,
                "action": "Capture a before/after allocator purge and compare PSS.",
                "commands": ["jcode debug 'allocator:purge'", "jcode debug 'server:memory-incident'"],
            },
            {
                "priority": 2,
                "action": "If retained pages repeatedly regrow, inspect allocation churn and allocator decay.",
                "commands": ["jcode debug 'allocator'", "jcode debug 'allocator:decay:1000'"],
            },
        ]
    elif attributed_state_dominates:
        cause = "session_payload_growth"
        confidence = "high"
        summary = "Tracked transcript, provider-cache, tool-result, or blob state explains most live heap."
        evidence = [
            f"attributed live {fmt_mb(attributed)}",
            f"live-heap attribution coverage {coverage_live:.1%}"
            if isinstance(coverage_live, int | float)
            else "live-heap attribution coverage unavailable",
        ]
        actions = [
            {
                "priority": 1,
                "action": "Start with the heaviest sessions and dominant payload category in this report.",
                "commands": ["jcode debug 'server:memory'"],
            }
        ]
    elif allocator_live >= 1024 * 1024 * 1024:
        cause = "unattributed_live_heap"
        confidence = "medium"
        summary = "Live allocator bytes are high, but current application attribution is incomplete."
        evidence = [
            f"allocator live {fmt_mb(allocator_live)}",
            f"attributed live {fmt_mb(attributed)}",
            f"live-heap attribution coverage {coverage_live:.1%}"
            if isinstance(coverage_live, int | float)
            else "live-heap attribution coverage unavailable",
        ]
        actions = [
            {
                "priority": 1,
                "action": "Capture full server attribution and add counters for the missing owner.",
                "commands": ["jcode debug 'server:memory'"],
            },
            {
                "priority": 2,
                "action": "Use a jemalloc-prof build and heap dump if coverage remains below 50%.",
                "commands": [
                    "jcode debug 'allocator:profile:on'",
                    "jcode debug 'allocator:profile:dump /tmp/jcode-server.heap'",
                ],
            },
        ]
    elif final_pss >= 1024 * 1024 * 1024:
        cause = "non_heap_or_mapping_growth"
        confidence = "medium"
        summary = "PSS is high without matching allocator live bytes; inspect mappings and threads."
        evidence = [f"PSS {fmt_mb(final_pss)}", f"allocator live {fmt_mb(allocator_live)}"]
        actions = [
            {
                "priority": 1,
                "action": "Inspect smaps, pmap, thread count, and shared mappings.",
                "commands": ["cat /proc/<pid>/smaps_rollup", "pmap -x <pid> | sort -k3 -nr | head"],
            }
        ]
    else:
        cause = "within_normal_operating_range"
        confidence = "high"
        summary = "No server memory incident threshold is exceeded in this process lifetime."
        evidence = [f"final PSS {fmt_mb(final_pss)}", f"net PSS growth {fmt_signed_mb(growth)}"]
        actions = [
            {
                "priority": 1,
                "action": "Continue normal monitoring and preserve this process-lifetime baseline.",
                "commands": ["python scripts/analyze_runtime_memory_log.py --days 1"],
            }
        ]

    critical = final_pss >= 2 * 1024 * 1024 * 1024 or growth >= 1024 * 1024 * 1024 or final_live >= 512
    warning = final_pss >= 1024 * 1024 * 1024 or growth >= 256 * 1024 * 1024 or final_live >= 128
    severity = "critical" if critical else "warning" if warning else "healthy"
    return {
        "severity": severity,
        "primary_cause": cause,
        "confidence": confidence,
        "summary": summary,
        "evidence": evidence,
        "recommended_actions": actions,
        "runbook": "docs/MEMORY_INCIDENT_RUNBOOK.md",
    }


def build_server_hints(samples: list[Sample], session_peaks: list[dict[str, Any]]) -> list[str]:
    hints: list[str] = []
    last_attr = last_attribution_sample(samples)
    if not last_attr or not last_attr.sessions:
        return ["Need at least one attribution sample before generating optimization hints."]

    sessions = last_attr.sessions
    total_json = int(sessions.get("total_json_bytes") or 0)
    provider_cache_json = int(sessions.get("total_provider_cache_json_bytes") or 0)
    tool_result_bytes = int(sessions.get("total_tool_result_bytes") or 0)
    large_blob_bytes = int(sessions.get("total_large_blob_bytes") or 0)
    payload_text_bytes = int(sessions.get("total_payload_text_bytes") or 0)
    population = session_population_summary(samples)

    if int(population.get("final_live_sessions") or 0) >= 128:
        hints.append(
            f"Live session population is high ({population['final_live_sessions']} sessions, "
            f"{population.get('final_connected_clients') or 0} connected clients). Reduce or cap session population before optimizing individual transcripts."
        )

    if total_json > 0 and provider_cache_json / total_json >= 0.35:
        hints.append(
            f"Provider cache is a large share of attributed state ({provider_cache_json / total_json:.0%} of total JSON). Prioritize cache compaction, cache invalidation discipline, and avoiding redundant provider-message mirrors."
        )
    if total_json > 0 and tool_result_bytes / total_json >= 0.25:
        hints.append(
            f"Tool results are heavy ({tool_result_bytes / total_json:.0%} of total JSON). Consider truncating stored tool output, summarizing verbose results, or storing large artifacts out-of-line."
        )
    if total_json > 0 and large_blob_bytes / total_json >= 0.15:
        hints.append(
            f"Large blobs are materially retained ({large_blob_bytes / total_json:.0%} of total JSON). Focus on blob thresholds, attachment retention, and aggressive post-use slimming."
        )
    if payload_text_bytes > 0 and total_json > 0 and payload_text_bytes / total_json >= 0.45:
        hints.append(
            f"Transcript payload text dominates attributed state ({payload_text_bytes / total_json:.0%} of total JSON). Compaction and transcript summarization will likely pay off."
        )

    last_process = samples[-1] if samples else None
    process_diag = (last_process.raw.get("process_diagnostics") or {}) if last_process else {}
    resident_minus_active = process_diag.get("allocator_resident_minus_active_bytes")
    pss_minus_allocated = process_diag.get("pss_minus_allocator_allocated_bytes")
    if isinstance(resident_minus_active, int) and resident_minus_active >= 64 * 1024 * 1024:
        hints.append(
            f"Allocator resident slack is high ({fmt_mb(resident_minus_active)} above active). Some memory pressure may be allocator retention rather than live app state."
        )
    if isinstance(pss_minus_allocated, int) and pss_minus_allocated >= 64 * 1024 * 1024:
        hints.append(
            f"PSS is materially above allocator allocated ({fmt_mb(pss_minus_allocated)} delta), suggesting shared mappings, allocator overhead, or retained pages are worth checking alongside app-owned structures."
        )

    embedding_events = [s for s in samples if s.trigger_category in {"embedding_loaded", "embedding_unloaded"}]
    if embedding_events:
        hints.append(
            f"Embedding lifecycle events were observed ({len(embedding_events)} samples). Compare memory before/after those windows to decide whether local embeddings should unload more aggressively."
        )

    if session_peaks:
        heaviest = session_peaks[0]
        hints.append(
            f"Heaviest observed session was {heaviest['session_id']} at {fmt_mb(heaviest['peak_json_bytes'])} attributed JSON. Start optimization work with that session’s transcript and tool-result profile."
        )

    if not hints:
        hints.append("No single dominant culprit stood out yet. Collect more runtime history and compare multiple attribution samples after heavier real usage.")
    return hints


def collect_client_peaks(samples: list[Sample]) -> list[dict[str, Any]]:
    client_stats: dict[str, dict[str, Any]] = {}
    for sample in samples:
        if not sample.totals:
            continue
        client = sample.raw.get("client") or {}
        session_id = str(client.get("session_id") or "")
        if not session_id:
            continue
        total = int(sample.totals.get("total_attributed_bytes") or 0)
        current = client_stats.get(session_id)
        if current is None or total > current["peak_total_attributed_bytes"]:
            client_stats[session_id] = {
                "session_id": session_id,
                "client_instance_id": client.get("client_instance_id"),
                "provider": client.get("provider"),
                "model": client.get("model"),
                "is_remote": bool(client.get("is_remote")),
                "peak_total_attributed_bytes": total,
                "peak_display_messages_estimate_bytes": int(sample.totals.get("display_messages_estimate_bytes") or 0),
                "peak_provider_messages_json_bytes": int(sample.totals.get("provider_messages_json_bytes") or 0),
                "peak_side_panel_estimate_bytes": int(sample.totals.get("side_panel_estimate_bytes") or 0),
                "peak_remote_state_bytes": int(sample.totals.get("remote_state_bytes") or 0),
                "last_seen_timestamp_ms": sample.timestamp_ms,
            }
    return sorted(client_stats.values(), key=lambda item: item["peak_total_attributed_bytes"], reverse=True)


def build_client_hints(samples: list[Sample], client_peaks: list[dict[str, Any]]) -> list[str]:
    hints: list[str] = []
    last_attr = last_attribution_sample(samples)
    if not last_attr or not last_attr.totals:
        return ["Need at least one client attribution sample before generating optimization hints."]

    totals = last_attr.totals
    total = int(totals.get("total_attributed_bytes") or 0)
    display = int(totals.get("display_messages_estimate_bytes") or 0)
    provider_messages = int(totals.get("provider_messages_json_bytes") or 0)
    side_panel = int(totals.get("side_panel_estimate_bytes") or 0)
    remote_state = int(totals.get("remote_state_bytes") or 0)

    if total > 0 and display / total >= 0.45:
        hints.append(
            f"Display-message state is a large share of attributed client memory ({display / total:.0%}). Tighten UI duplication and display-history retention first."
        )
    if total > 0 and provider_messages / total >= 0.20:
        hints.append(
            f"Resident provider-message copies are a meaningful share of client memory ({provider_messages / total:.0%}). Prefer borrowing or lazy hydration where possible."
        )
    if total > 0 and side_panel / total >= 0.15:
        hints.append(
            f"Side-panel state is materially retained ({side_panel / total:.0%} of attributed client memory). Focus on page content retention and render-cache discipline."
        )
    if total > 0 and remote_state / total >= 0.10:
        hints.append(
            f"Remote session metadata is non-trivial ({remote_state / total:.0%} of attributed client memory). Review retained model/session lists and remote bootstrap state."
        )

    coverage = build_coverage_report(last_attr)
    pss = coverage.get("pss_bytes")
    retained = coverage.get("allocator_retained_resident_estimate_bytes")
    if isinstance(pss, int) and isinstance(retained, int) and pss > 0 and retained / pss >= 0.30:
        hints.append(
            f"Allocator retention dominates PSS ({retained / pss:.0%}, {fmt_mb(retained)}). This is freed-but-held heap, not live app state; malloc_trim/purge cadence matters more than estimator coverage here."
        )
    unattributed_live = coverage.get("unattributed_live_heap_bytes")
    live = coverage.get("allocator_live_bytes")
    if (
        isinstance(unattributed_live, int)
        and isinstance(live, int)
        and live > 0
        and unattributed_live / live >= 0.40
    ):
        hints.append(
            f"Estimators miss {unattributed_live / live:.0%} of live heap ({fmt_mb(unattributed_live)}). The real estimator gap is tokio/render/runtime structures, not allocator slack."
        )

    if client_peaks:
        heaviest = client_peaks[0]
        hints.append(
            f"Heaviest observed client session was {heaviest['session_id']} at {fmt_mb(heaviest['peak_total_attributed_bytes'])} attributed client memory. Start with that session’s display and provider-message layers."
        )

    if not hints:
        hints.append("No single dominant client-side culprit stood out yet. Collect more client runtime history during heavier UI usage.")
    return hints


def summarize_target(samples: list[Sample], top_n: int, min_spike_bytes: int) -> dict[str, Any]:
    spikes = compute_spikes(samples, min_spike_bytes=min_spike_bytes)
    target = samples[0].target if samples else "unknown"
    deltas = compute_attribution_deltas(samples) if target == "server" else []
    session_peaks = collect_session_peaks(samples) if target == "server" else []
    client_peaks = collect_client_peaks(samples) if target == "client" else []
    event_counts = count_event_categories(samples)
    proc = process_summary(samples)
    last_attr = last_attribution_sample(samples)
    coverage = build_coverage_report(last_attr) if last_attr else None
    population = session_population_summary(samples) if target == "server" else {}
    incident = (
        build_incident_assessment(samples, proc, coverage, population)
        if target == "server"
        else {}
    )
    summary = {
        "target": target,
        "instance_id": samples[-1].instance_id if samples else None,
        "sample_count": len(samples),
        "first_timestamp_ms": samples[0].timestamp_ms if samples else None,
        "last_timestamp_ms": samples[-1].timestamp_ms if samples else None,
        "kinds": Counter(sample.kind for sample in samples),
        "process": proc,
        "coverage": coverage,
        "session_population": population,
        "incident": incident,
        "last_attribution": {
            "timestamp_ms": last_attr.timestamp_ms,
            "sessions": last_attr.sessions,
            "totals": last_attr.totals,
            "client": last_attr.raw.get("client") if target == "client" else None,
            "trigger_category": last_attr.trigger_category,
            "trigger_reason": last_attr.trigger_reason,
        }
        if last_attr
        else None,
        "top_spikes": [
            {
                "from": spike.start.timestamp_ms,
                "to": spike.end.timestamp_ms,
                "delta_pss_bytes": spike.delta_pss_bytes,
                "from_source": spike.start.source,
                "to_source": spike.end.source,
                "to_trigger_category": spike.end.trigger_category,
                "to_trigger_reason": spike.end.trigger_reason,
            }
            for spike in spikes[:top_n]
        ],
        "top_attribution_deltas": [
            {
                "from": delta.start.timestamp_ms,
                "to": delta.end.timestamp_ms,
                "to_trigger_category": delta.end.trigger_category,
                "to_trigger_reason": delta.end.trigger_reason,
                "delta_total_json_bytes": delta.delta_total_json_bytes,
                "delta_payload_text_bytes": delta.delta_payload_text_bytes,
                "delta_provider_cache_json_bytes": delta.delta_provider_cache_json_bytes,
                "delta_tool_result_bytes": delta.delta_tool_result_bytes,
                "delta_large_blob_bytes": delta.delta_large_blob_bytes,
                "delta_live_count": delta.delta_live_count,
                "delta_memory_enabled_session_count": delta.delta_memory_enabled_session_count,
            }
            for delta in deltas[:top_n]
        ],
        "top_sessions": session_peaks[:top_n],
        "top_clients": client_peaks[:top_n],
        "event_counts": dict(event_counts.most_common()),
        "hints": build_server_hints(samples, session_peaks[:top_n])
        if target == "server"
        else build_client_hints(samples, client_peaks[:top_n]),
    }
    return summary


def summarize(samples: list[Sample], top_n: int, min_spike_bytes: int) -> dict[str, Any]:
    targets = sorted({sample.target for sample in samples})
    if len(targets) <= 1:
        return summarize_target(samples, top_n=top_n, min_spike_bytes=min_spike_bytes)
    return {
        "targets": {
            target: summarize_target(
                [sample for sample in samples if sample.target == target],
                top_n=top_n,
                min_spike_bytes=min_spike_bytes,
            )
            for target in targets
        }
    }


def print_human(summary: dict[str, Any], paths: list[Path]) -> None:
    if "targets" in summary:
        print("Runtime Memory Log Analysis")
        print("===========================")
        if paths:
            print(f"files: {len(paths)}")
            for path in paths:
                print(f"  - {path}")
        for target, target_summary in summary["targets"].items():
            print(f"\n[{target}]")
            print_human(target_summary, [])
        return

    print("Runtime Memory Log Analysis")
    print("===========================")
    if paths:
        print(f"files: {len(paths)}")
        for path in paths:
            print(f"  - {path}")
    print(f"samples: {summary['sample_count']}")
    if summary.get("instance_id"):
        print(f"instance: {summary['instance_id']}")
    if summary.get("first_timestamp_ms") is not None:
        print(f"window: {fmt_ts(summary['first_timestamp_ms'])} -> {fmt_ts(summary['last_timestamp_ms'])}")
        print(
            f"duration: {fmt_duration_ms(summary['last_timestamp_ms'] - summary['first_timestamp_ms'])}"
        )

    proc = summary.get("process") or {}
    if proc:
        print("\nProcess memory")
        print("--------------")
        print(f"baseline PSS: {fmt_mb(proc.get('baseline_pss_bytes'))}")
        print(f"final PSS:    {fmt_mb(proc.get('final_pss_bytes'))} ({fmt_signed_mb(proc.get('net_pss_growth_bytes'))})")
        print(f"peak PSS:     {fmt_mb(proc.get('peak_pss_bytes'))} ({fmt_signed_mb(proc.get('peak_growth_vs_baseline_bytes'))} vs baseline)")
        print(f"median PSS:   {fmt_mb(proc.get('median_pss_bytes'))}")
        peak_ts = proc.get("peak_timestamp_ms")
        if peak_ts is not None:
            print(
                f"peak trigger: {fmt_ts(peak_ts)} | {proc.get('peak_trigger_category') or 'unknown'} / {proc.get('peak_trigger_reason') or 'unknown'}"
            )
        print(
            f"allocator: allocated {fmt_mb(proc.get('allocator_allocated_bytes'))} | resident {fmt_mb(proc.get('allocator_resident_bytes'))} | retained {fmt_mb(proc.get('allocator_retained_bytes'))}"
        )

    population = summary.get("session_population") or {}
    if population:
        print("\nSession population")
        print("------------------")
        print(
            f"live sessions:     {population.get('baseline_live_sessions')} -> {population.get('final_live_sessions')} "
            f"({population.get('net_live_session_growth'):+d}) | peak {population.get('peak_live_sessions')}"
        )
        print(
            f"connected clients: {population.get('final_connected_clients') if population.get('final_connected_clients') is not None else 'n/a'} | "
            f"detached/headless {population.get('detached_or_headless_live_sessions') if population.get('detached_or_headless_live_sessions') is not None else 'n/a'}"
        )
        print(
            f"allocator growth:  {fmt_signed_mb(population.get('net_allocator_live_growth_bytes'))} | "
            f"{fmt_mb(population.get('allocator_growth_per_added_session_bytes'))} per added session"
        )

    incident = summary.get("incident") or {}
    if incident:
        print("\nIncident assessment")
        print("-------------------")
        print(
            f"severity: {incident.get('severity')} | cause: {incident.get('primary_cause')} | confidence: {incident.get('confidence')}"
        )
        print(incident.get("summary") or "")
        for item in incident.get("evidence") or []:
            print(f"- evidence: {item}")
        print("next actions:")
        for action in incident.get("recommended_actions") or []:
            print(f"  {action.get('priority')}. {action.get('action')}")
            if action.get("why"):
                print(f"     why: {action['why']}")
            for command in action.get("commands") or []:
                print(f"     $ {command}")

    coverage = summary.get("coverage") or {}
    if coverage:
        print("\nAttribution coverage (last attribution sample)")
        print("----------------------------------------------")
        print(f"PSS:                {fmt_mb(coverage.get('pss_bytes'))}")
        if coverage.get("pss_anon_bytes") is not None or coverage.get("pss_file_bytes") is not None:
            print(
                f"PSS split:          anon {fmt_mb(coverage.get('pss_anon_bytes'))} | file {fmt_mb(coverage.get('pss_file_bytes'))} | shmem {fmt_mb(coverage.get('pss_shmem_bytes'))}"
            )
        print(f"attributed live:    {fmt_mb(coverage.get('attributed_live_bytes'))}")
        print(f"allocator live:     {fmt_mb(coverage.get('allocator_live_bytes'))}")
        if coverage.get("unattributed_live_heap_bytes") is not None:
            print(f"unattributed live:  {fmt_mb(coverage.get('unattributed_live_heap_bytes'))}")
        print(
            f"allocator retained: {fmt_mb(coverage.get('allocator_retained_resident_estimate_bytes'))}"
        )
        if coverage.get("main_stack_bytes") is not None or coverage.get("thread_count") is not None:
            threads = coverage.get("thread_count")
            print(
                f"stacks/threads:     main stack {fmt_mb(coverage.get('main_stack_bytes'))} | stack estimate {fmt_mb(coverage.get('thread_stack_estimate_bytes'))} | threads {threads if threads is not None else 'n/a'}"
            )
        ratio_pss = coverage.get("coverage_ratio_pss")
        ratio_live = coverage.get("coverage_ratio_live_heap")
        if ratio_pss is not None or ratio_live is not None:
            pss_text = f"{ratio_pss:.1%}" if isinstance(ratio_pss, int | float) else "n/a"
            live_text = f"{ratio_live:.1%}" if isinstance(ratio_live, int | float) else "n/a"
            print(f"coverage:           vs PSS {pss_text} | vs live heap {live_text}")
        if coverage.get("explained_pss_bytes") is not None:
            explained_ratio = coverage.get("explained_ratio")
            ratio_text = (
                f"{explained_ratio:.1%}" if isinstance(explained_ratio, int | float) else "n/a"
            )
            print(
                f"explained PSS:      {fmt_mb(coverage.get('explained_pss_bytes'))} ({ratio_text}) | unexplained {fmt_mb(coverage.get('unexplained_pss_bytes'))}"
            )

    print("\nEvent counts")
    print("------------")
    for category, count in list((summary.get("event_counts") or {}).items())[:12]:
        print(f"{category}: {count}")

    print("\nTop PSS spikes")
    print("-------------")
    spikes = summary.get("top_spikes") or []
    if not spikes:
        print("No spikes above threshold.")
    for spike in spikes:
        print(
            f"{fmt_ts(spike['from'])} -> {fmt_ts(spike['to'])} | {fmt_signed_mb(spike['delta_pss_bytes'])} | {spike['to_trigger_category'] or 'unknown'} / {spike['to_trigger_reason'] or 'unknown'}"
        )

    print("\nTop attribution deltas")
    print("----------------------")
    deltas = summary.get("top_attribution_deltas") or []
    if not deltas:
        print("Need at least two attribution samples.")
    for delta in deltas:
        print(
            f"{fmt_ts(delta['from'])} -> {fmt_ts(delta['to'])} | total {fmt_signed_mb(delta['delta_total_json_bytes'])} | cache {fmt_signed_mb(delta['delta_provider_cache_json_bytes'])} | tool {fmt_signed_mb(delta['delta_tool_result_bytes'])} | blob {fmt_signed_mb(delta['delta_large_blob_bytes'])} | text {fmt_signed_mb(delta['delta_payload_text_bytes'])} | {delta['to_trigger_category'] or 'unknown'}"
        )

    target = summary.get("target") or "server"
    section_title = "Heaviest sessions" if target == "server" else "Heaviest clients"
    print(f"\n{section_title}")
    print("-" * len(section_title))
    sessions = summary.get("top_sessions") or []
    clients = summary.get("top_clients") or []
    if not sessions:
        if target == "server":
            print("No per-session attribution data yet.")
    for item in sessions:
        print(
            f"{item['session_id']} | peak json {fmt_mb(item['peak_json_bytes'])} | provider cache {fmt_mb(item['peak_provider_cache_json_bytes'])} | tool results {fmt_mb(item['peak_tool_result_bytes'])} | large blobs {fmt_mb(item['peak_large_blob_bytes'])} | provider={item.get('provider') or 'unknown'} model={item.get('model') or 'unknown'}"
        )
    if target == "client":
        if not clients:
            print("No per-client attribution data yet.")
        for item in clients:
            print(
                f"{item['session_id']} | peak attributed {fmt_mb(item['peak_total_attributed_bytes'])} | display {fmt_mb(item['peak_display_messages_estimate_bytes'])} | provider view {fmt_mb(item['peak_provider_messages_json_bytes'])} | side panel {fmt_mb(item['peak_side_panel_estimate_bytes'])} | provider={item.get('provider') or 'unknown'} model={item.get('model') or 'unknown'}"
            )

    print("\nOptimization hints")
    print("------------------")
    for hint in summary.get("hints") or []:
        print(f"- {hint}")


def to_jsonable(value: Any) -> Any:
    if isinstance(value, Counter):
        return dict(value)
    if isinstance(value, dict):
        return {key: to_jsonable(inner) for key, inner in value.items()}
    if isinstance(value, list):
        return [to_jsonable(item) for item in value]
    return value


def print_instance_inventory(inventory: list[dict[str, Any]], paths: list[Path]) -> None:
    print("Runtime Memory Process Instances")
    print("================================")
    print(f"files: {len(paths)}")
    for item in inventory:
        print(
            f"{item['instance_id']} | target={item['target']} | samples={item['sample_count']} | "
            f"{fmt_ts(item['first_timestamp_ms'])} -> {fmt_ts(item['last_timestamp_ms'])} | "
            f"duration={fmt_duration_ms(item['duration_ms'])}"
        )


def main() -> int:
    args = parse_args()
    paths = resolve_paths(args)
    if not paths:
        raise SystemExit("No runtime memory log files found.")
    samples = load_samples(paths)
    if not samples:
        raise SystemExit("No runtime memory samples found in selected files.")
    inventory = instance_inventory(samples)
    if args.list_instances:
        if args.json:
            print(
                json.dumps(
                    {
                        "files": [str(path) for path in paths],
                        "instances": inventory,
                    },
                    indent=2,
                )
            )
        else:
            print_instance_inventory(inventory, paths)
        return 0
    if args.instance:
        samples = select_instance(samples, args.instance)
        if not samples:
            available = ", ".join(item["instance_id"] for item in inventory)
            raise SystemExit(
                f"Process instance {args.instance!r} was not found. Available instances: {available}"
            )
    elif not args.all_instances:
        samples = select_latest_instances(samples)
    summary = summarize(samples, top_n=args.top, min_spike_bytes=int(args.min_spike_mb * 1024 * 1024))
    if args.json:
        payload = to_jsonable(summary)
        payload["files"] = [str(path) for path in paths]
        print(json.dumps(payload, indent=2))
    else:
        print_human(summary, paths)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
