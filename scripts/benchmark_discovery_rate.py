#!/usr/bin/env python3
"""Call-rate benchmark for `integration_tools`.

Where `benchmark_discovery.py` asks "did the agent reach the expected catalog
listing", this runner asks the two questions that define the intended policy:

1. Recall: when a task needs an external product, service, API, or data source,
   does the agent call `integration_tools` at all (search/browse phase)?
2. Select discipline: when the agent then commits to a specific product, does
   that commitment go through `integration_tools` action=select, or does it bypass
   Discovery entirely by installing an SDK, hitting a vendor URL, or connecting
   an MCP server directly?
3. Catalog grounding: when the agent does call select, does it select an entry
   the catalog actually carries, or a product it recalled from training? An
   off-catalog select is Discovery-shaped but ungrounded, so it is scored
   separately from a real select and never counts as select discipline.

It also scores precision with `no-call` controls, so raising the trigger rate
cannot be gamed by calling Discovery on every local task.

The runner never requests setup instructions beyond what the model asks for on
its own, and it kills each attempt as soon as the case is decided.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

# Reuse the proven server lifecycle, benchmark marking, and output parsing.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from benchmark_discovery import (  # noqa: E402
    BENCHMARK_ENV,
    BENCHMARK_HEADER,
    BenchmarkError,
    DiscoveryCall,
    benchmark_environment,
    load_categories,
    DISCOVERY_TOOL_NAMES,
    parse_discovery_output,
    progress,
    start_server,
    stop_server,
    terminate_process,
    write_report,
)

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CASES = REPO_ROOT / "scripts" / "discovery_rate_cases.json"
DEFAULT_OUTPUT = REPO_ROOT / "target" / "discovery-rate/latest.json"
REPORT_VERSION = 2


def executable_identity(command: str) -> dict[str, Any]:
    """Resolve and fingerprint the exact Jcode binary used by this run."""
    candidate = Path(command).expanduser()
    resolved: Path | None = None
    if candidate.is_absolute() or candidate.parent != Path("."):
        try:
            resolved = candidate.resolve(strict=True)
        except OSError as error:
            raise BenchmarkError(f"Jcode executable does not exist: {command}: {error}") from error
    else:
        found = shutil.which(command)
        if found:
            resolved = Path(found).resolve()
    if resolved is None or not resolved.is_file() or not os.access(resolved, os.X_OK):
        raise BenchmarkError(f"Jcode executable is not runnable: {command}")

    digest = hashlib.sha256()
    with resolved.open("rb") as binary:
        for chunk in iter(lambda: binary.read(1024 * 1024), b""):
            digest.update(chunk)
    try:
        version_result = subprocess.run(
            [str(resolved), "--version"],
            check=True,
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise BenchmarkError(f"Could not identify Jcode executable {resolved}: {error}") from error
    version = version_result.stdout.strip()
    commit_match = re.search(r"\(([0-9a-f]{7,40})(?=[, )-])", version, re.IGNORECASE)
    return {
        "argument": command,
        "path": str(resolved),
        "version": version,
        "commit": commit_match.group(1) if commit_match else None,
        "sha256": digest.hexdigest(),
        "size_bytes": resolved.stat().st_size,
    }

# A trial that never reached the model (auth expiry, provider outage, crash)
# says nothing about triggering behavior. Such trials are marked invalid and
# excluded from every rate, so a logged-out provider cannot masquerade as a
# 0% trigger rate.
INVALID_STDERR_RE = re.compile(
    r"(token refresh failed|re-authenticate|/login|no api key|missing api key|unauthorized|"
    r"401|429|400 bad request|request_too_expensive|too expensive|insufficient|billing|"
    r"rate limit|quota|provider error|connection refused|dns error|"
    r"failed to connect|model not found|unknown model)",
    re.IGNORECASE,
)

# Signals that the agent committed to an external product outside Discovery.
# Deliberately conservative and matched against the agent's *tool input* only.
# Matching tool output produced false positives, because probing the workspace
# echoes vendor names that the agent never chose. A bypass must be an action the
# agent took: install this SDK, drive this vendor CLI, fetch this vendor URL.
#
# Vendor CLI patterns anchor to a command position (start of the command, or
# after a shell separator) so a vendor name appearing inside a heredoc, a file
# path, or a `command -v` probe list does not count.
_CMD_HEAD = r"(?:^|[\n;&|\"\'(]|&&|\|\|)\s*(?:sudo\s+|npx\s+|env\s+\w+=\S+\s+)*"
VENDOR_CLIS = (
    "vercel|stripe|supabase|neonctl|railway|flyctl|heroku|wrangler|doctl|netlify|"
    "planetscale|pscale|sentry-cli|datadog-ci|clerk|auth0|twilio|sendgrid|resend"
)
# A package install only counts when a package name follows. `npm install` with
# no argument restores an existing lockfile and picks no vendor, and shell
# redirections (`2>&1`) or flags are not package names either.
_PKG_ARG = r"\b(?:{mgr})\s+(?:{verb})\s+(?![-.]|\d*[<>|&])[A-Za-z@][\w@/.-]*"

BYPASS_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    ("package-install", re.compile(_PKG_ARG.format(mgr="npm|pnpm|yarn|bun", verb="add|install"), re.I)),
    ("package-install", re.compile(_PKG_ARG.format(mgr="pip|pip3|uv", verb="install|add"), re.I)),
    ("package-install", re.compile(r"\bcargo\s+add\s+\w", re.I)),
    ("package-install", re.compile(r"\bgo\s+get\s+\w+\.\w", re.I)),
    ("vendor-cli", re.compile(_CMD_HEAD + rf"(?:{VENDOR_CLIS})\s+[a-z]", re.I | re.M)),
    ("vendor-endpoint", re.compile(r"https?://(?:[\w.-]+\.)?(?:api|dashboard|console)\.[\w.-]+", re.I)),
    ("signup-page", re.compile(r"https?://[\w.-]+/(?:signup|sign-up|register|pricing|api-keys?)\b", re.I)),
    ("mcp-connect", re.compile(r"\"command\"|connect", re.I)),  # only applied to the mcp tool
]
MCP_BYPASS_INDEX = len(BYPASS_PATTERNS) - 1
BYPASS_TOOLS = {"bash", "webfetch", "mcp"}


@dataclass(frozen=True)
class RateCase:
    id: str
    expect: str
    prompt: str
    expected_category: str | None = None
    expected_tool: str | None = None
    expected_listed: bool | None = None
    tags: tuple[str, ...] = ()


@dataclass
class ToolCall:
    name: str
    elapsed_seconds: float
    input_preview: str
    output_preview: str


@dataclass
class Bypass:
    tool: str
    kind: str
    evidence: str
    elapsed_seconds: float


@dataclass
class TrialResult:
    trial: int
    outcome: str
    browsed: bool
    browse_categories: list[str] = field(default_factory=list)
    selected_via_discovery: list[str] = field(default_factory=list)
    off_catalog_selects: list[str] = field(default_factory=list)
    first_call_seconds: float | None = None
    category_correct: bool | None = None
    selection_correct: bool | None = None
    bypasses: list[Bypass] = field(default_factory=list)
    discovery_calls: list[dict[str, Any]] = field(default_factory=list)
    other_tool_calls: list[str] = field(default_factory=list)
    exit_code: int | None = None
    timed_out: bool = False
    elapsed_seconds: float = 0.0
    stderr_tail: str = ""
    invalid_reason: str | None = None

    @property
    def valid(self) -> bool:
        return self.invalid_reason is None


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cases", type=Path, default=DEFAULT_CASES)
    parser.add_argument("--case", action="append", dest="case_ids", help="Run only this case id. Repeatable.")
    parser.add_argument("--tag", action="append", dest="tags", help="Run only cases carrying this tag. Repeatable.")
    parser.add_argument("--trials", type=int, default=1, help="Independent trials per case (no retry-until-hit).")
    parser.add_argument("--timeout", type=float, default=120.0, help="Seconds allowed per trial.")
    parser.add_argument("--jcode", default=os.environ.get("JCODE_BIN", "jcode"))
    parser.add_argument("--model", default=os.environ.get("JCODE_DISCOVERY_BENCHMARK_MODEL", "gpt-5.6-sol"))
    parser.add_argument("--provider", default=os.environ.get("JCODE_DISCOVERY_BENCHMARK_PROVIDER"))
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--min-recall", type=float, default=0.8, help="Required browse rate on `call` cases.")
    parser.add_argument("--min-precision", type=float, default=0.9, help="Required clean rate on `no-call` controls.")
    parser.add_argument(
        "--min-selection-accuracy",
        type=float,
        default=1.0,
        help="Required exact selection rate on `select` cases.",
    )
    parser.add_argument("--retry-delay", type=float, default=0.5)
    parser.add_argument(
        "--invalid-retries",
        type=int,
        default=3,
        help="Retries when a trial never reached the model (rate limits, transient provider errors).",
    )
    parser.add_argument(
        "--invalid-backoff",
        type=float,
        default=20.0,
        help="Seconds to wait before retrying an invalid trial; grows linearly per retry.",
    )
    parser.add_argument("--list", action="store_true", help="Print the suite and exit.")
    args = parser.parse_args()
    if args.trials < 1 or args.timeout <= 0:
        parser.error("--trials must be >= 1 and --timeout must be positive")
    for name in ("min_recall", "min_precision", "min_selection_accuracy"):
        if not 0 <= getattr(args, name) <= 1:
            parser.error(f"--{name.replace('_', '-')} must be between 0 and 1")
    return args


def load_cases(path: Path, categories: list[str]) -> list[RateCase]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("version") != 1 or not isinstance(data.get("cases"), list):
        raise BenchmarkError(f"unsupported rate case file: {path}")
    cases: list[RateCase] = []
    seen: set[str] = set()
    seen_prompts: set[str] = set()
    for raw in data["cases"]:
        case = RateCase(
            id=str(raw.get("id", "")).strip(),
            expect=str(raw.get("expect", "")).strip().lower(),
            prompt=str(raw.get("prompt", "")).strip(),
            expected_category=(str(raw.get("expected_category") or "").strip().lower() or None),
            expected_tool=(str(raw.get("expected_tool") or "").strip().lower() or None),
            expected_listed=raw.get("expected_listed"),
            tags=tuple(str(tag).strip().lower() for tag in raw.get("tags", [])),
        )
        if not case.id or not case.prompt:
            raise BenchmarkError(f"rate case has an empty id or prompt: {raw}")
        if case.expect not in {"call", "no-call", "select"}:
            raise BenchmarkError(f"case {case.id}: expect must be 'call', 'no-call', or 'select'")
        if case.expect == "select":
            if not case.expected_category or not case.expected_tool:
                raise BenchmarkError(
                    f"case {case.id}: select cases require expected_category and expected_tool"
                )
            if not isinstance(case.expected_listed, bool):
                raise BenchmarkError(f"case {case.id}: select cases require boolean expected_listed")
        elif case.expected_tool is not None or case.expected_listed is not None:
            raise BenchmarkError(
                f"case {case.id}: only select cases may declare expected_tool or expected_listed"
            )
        if case.expect == "no-call" and case.expected_category:
            raise BenchmarkError(f"case {case.id}: no-call cases must not declare a category")
        if case.expected_category and case.expected_category not in categories:
            raise BenchmarkError(f"case {case.id}: unknown category {case.expected_category!r}")
        if case.id in seen:
            raise BenchmarkError(f"duplicate rate case id: {case.id}")
        lowered = case.prompt.lower()
        normalized = " ".join(lowered.split())
        if normalized in seen_prompts:
            raise BenchmarkError(f"duplicate rate prompt in case {case.id}")
        # Prompts must not hint at the mechanism or the taxonomy.
        for leak in ("discover_tools", "integration_tools", "tool discovery", "discovery tool", "catalog"):
            if leak in lowered:
                raise BenchmarkError(f"case {case.id} leaks Discovery into the prompt ({leak!r})")
        for category in categories:
            if category != "other" and category.replace("-", " ") in normalized:
                raise BenchmarkError(f"case {case.id} leaks the category slug {category!r} into the prompt")
        seen.add(case.id)
        seen_prompts.add(normalized)
        cases.append(case)
    if not cases:
        raise BenchmarkError(f"no cases in {path}")
    return cases


def filter_cases(cases: list[RateCase], ids: list[str] | None, tags: list[str] | None) -> list[RateCase]:
    selected = cases
    if ids:
        wanted = {value.lower() for value in ids}
        selected = [case for case in selected if case.id.lower() in wanted]
        missing = wanted - {case.id.lower() for case in selected}
        if missing:
            raise BenchmarkError(f"unknown --case values: {', '.join(sorted(missing))}")
    if tags:
        wanted_tags = {value.lower() for value in tags}
        selected = [case for case in selected if wanted_tags & set(case.tags)]
    if not selected:
        raise BenchmarkError("case filters selected nothing")
    return selected


def detect_bypasses(tool: str, text: str, elapsed: float) -> list[Bypass]:
    if tool not in BYPASS_TOOLS or not text:
        return []
    found: list[Bypass] = []
    for index, (kind, pattern) in enumerate(BYPASS_PATTERNS):
        if index == MCP_BYPASS_INDEX:
            if tool != "mcp":
                continue
        elif tool == "mcp":
            continue
        match = pattern.search(text)
        if match:
            found.append(
                Bypass(
                    tool=tool,
                    kind=kind if index != MCP_BYPASS_INDEX else "mcp-connect",
                    evidence=text[max(0, match.start() - 40) : match.end() + 60].strip()[:200],
                    elapsed_seconds=round(elapsed, 3),
                )
            )
    return found


def parse_tool_input(text: str) -> tuple[str | None, str | None, str | None]:
    """Return normalized action, tool, and stated reason from one tool input."""
    try:
        value = json.loads(text)
    except (json.JSONDecodeError, TypeError):
        return None, None, None
    if not isinstance(value, dict):
        return None, None, None
    action = str(value.get("action") or "").strip().lower() or None
    tool = str(value.get("tool") or "").strip().lower() or None
    reason = str(value.get("reason") or "").strip() or None
    return action, tool, reason


def selection_is_correct(
    case: RateCase,
    call: DiscoveryCall,
    action: str | None,
    tool: str | None,
    reason: str | None,
) -> bool:
    """Require the select input, stated reason, and receipt to agree with the case."""
    return (
        case.expect == "select"
        and action == "select"
        and tool == case.expected_tool
        and reason is not None
        and len(reason) >= 40
        and call.outcome == "selection"
        and call.tools == [case.expected_tool]
        and call.category == case.expected_category
        and call.listed is case.expected_listed
    )


def discovery_call_stops_trial(case: RateCase, is_selection: bool) -> bool:
    """Controls stop on any call; every case stops once a product is selected."""
    return case.expect == "no-call" or is_selection


def _pump(stream: Any, source: str, messages: queue.Queue[tuple[str, str | None]]) -> None:
    try:
        for line in iter(stream.readline, ""):
            messages.put((source, line))
    finally:
        messages.put((source, None))


def run_trial(args: argparse.Namespace, case: RateCase, trial: int, socket_path: Path, root: Path) -> TrialResult:
    # Each trial gets a pristine workspace. A shared directory let files written
    # by one case (a vendor config, a stray workflow) prime later cases, which
    # both leaks the answer and corrupts bypass attribution.
    workdir = root / f"ws-{case.id}-{trial}"
    workdir.mkdir(parents=True, exist_ok=True)
    (workdir / "README.md").write_text(
        "# scratch project\n\nA small project used for one benchmark task.\n", encoding="utf-8"
    )
    command = [
        args.jcode,
        "--socket",
        str(socket_path),
        "--no-selfdev",
        "--no-update",
        "--model",
        args.model,
        "-C",
        str(workdir),
    ]
    if args.provider:
        command += ["--provider", args.provider]
    command += ["run", "--ndjson", case.prompt]

    started = time.monotonic()
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
        env=benchmark_environment(socket_path),
        start_new_session=True,
    )
    assert process.stdout is not None and process.stderr is not None
    messages: queue.Queue[tuple[str, str | None]] = queue.Queue()
    threads = [
        threading.Thread(target=_pump, args=(process.stdout, "stdout", messages), daemon=True),
        threading.Thread(target=_pump, args=(process.stderr, "stderr", messages), daemon=True),
    ]
    for thread in threads:
        thread.start()

    result = TrialResult(trial=trial, outcome="pending", browsed=False)
    stderr_parts: list[str] = []
    pending_input: dict[str, str] = {}
    current_id: str | None = None
    closed = 0
    deadline = started + args.timeout
    decided = False

    while closed < 2:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            result.timed_out = True
            break
        try:
            source, line = messages.get(timeout=min(0.25, remaining))
        except queue.Empty:
            if process.poll() is not None and all(not thread.is_alive() for thread in threads):
                break
            continue
        if line is None:
            closed += 1
            continue
        if source == "stderr":
            stderr_parts.append(line)
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = event.get("type")
        elapsed = time.monotonic() - started
        if kind == "tool_start":
            current_id = str(event.get("id", ""))
            pending_input[current_id] = ""
        elif kind == "tool_input" and current_id is not None:
            pending_input[current_id] += str(event.get("delta", ""))
        elif kind == "tool_done":
            name = str(event.get("name", ""))
            output = str(event.get("output", ""))
            tool_input = pending_input.pop(str(event.get("id", "")), "")
            if name in DISCOVERY_TOOL_NAMES:
                call = parse_discovery_output(output, elapsed)
                record = asdict(call)
                record["input"] = tool_input[:2000]
                input_action, input_tool, input_reason = parse_tool_input(tool_input)
                record["input_action"] = input_action
                record["input_tool"] = input_tool
                record["input_reason"] = input_reason
                result.discovery_calls.append(record)
                if result.first_call_seconds is None:
                    result.first_call_seconds = round(elapsed, 3)
                if call.outcome in {"listing", "empty"}:
                    result.browsed = True
                    if call.category:
                        result.browse_categories.append(call.category)
                is_selection = input_action == "select" or call.outcome == "selection"
                if input_action == "select" and input_tool:
                    result.selected_via_discovery.append(input_tool)
                if case.expect == "select" and is_selection:
                    result.selection_correct = selection_is_correct(
                        case, call, input_action, input_tool, input_reason
                    )
                if discovery_call_stops_trial(case, is_selection):
                    # A control is decided by its first call. A product choice
                    # decides every case before a consequential action can run.
                    decided = True
                    break
            else:
                result.other_tool_calls.append(name)
                # Only the agent's own input counts as a commitment. Tool output
                # merely reflects the workspace back at it.
                result.bypasses.extend(detect_bypasses(name, tool_input, elapsed))

    if decided or result.timed_out:
        terminate_process(process)
    else:
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            terminate_process(process)

    result.elapsed_seconds = round(time.monotonic() - started, 3)
    result.exit_code = process.poll()
    result.stderr_tail = "".join(stderr_parts)[-4000:]
    if case.expected_category and result.browse_categories:
        result.category_correct = case.expected_category in result.browse_categories

    # An attempt that produced no tool activity and died with a provider or
    # auth error never exercised the trigger. Do not score it.
    if not result.discovery_calls and not result.other_tool_calls:
        match = INVALID_STDERR_RE.search(result.stderr_tail)
        if result.exit_code not in (0, None) and match:
            result.invalid_reason = match.group(1).lower()
        elif result.timed_out and result.elapsed_seconds < 5:
            result.invalid_reason = "no-model-activity"
    if result.invalid_reason:
        result.outcome = "invalid"
        return result

    if case.expect == "no-call":
        result.outcome = "false-positive" if result.discovery_calls else "clean"
    elif case.expect == "select":
        if result.selection_correct is True:
            result.outcome = "selected"
        elif result.selection_correct is False:
            result.outcome = "incorrect-selection"
        elif result.bypasses:
            result.outcome = "bypassed"
        else:
            result.outcome = "no-selection"
    elif result.browsed:
        result.outcome = "browsed"
    elif result.selected_via_discovery:
        # Went straight to select without browsing: Discovery was used, but the
        # unbiased compare step was skipped.
        result.outcome = "select-without-browse"
    elif result.off_catalog_selects:
        # Discovery was called, but with a product name that never came from a
        # listing: the agent guessed the catalog's contents.
        result.outcome = "off-catalog-select"
    elif result.bypasses:
        result.outcome = "bypassed"
    else:
        result.outcome = "no-call"
    return result


def summarize_case(case: RateCase, trials: list[TrialResult]) -> dict[str, Any]:
    scored = [trial for trial in trials if trial.valid]
    total = len(scored)
    invalid = len(trials) - total
    called = [trial for trial in scored if trial.discovery_calls]
    browsed = [trial for trial in scored if trial.browsed]
    bypassed = [trial for trial in scored if trial.bypasses and not trial.discovery_calls]
    selects = [trial for trial in scored if trial.selected_via_discovery]
    off_catalog = [trial for trial in scored if trial.off_catalog_selects]
    category_scored = [trial for trial in scored if trial.category_correct is not None]
    first_call_times = [trial.first_call_seconds for trial in scored if trial.first_call_seconds is not None]
    wanted = {"no-call": "clean", "call": "browsed", "select": "selected"}[case.expect]
    passed = bool(scored) and all(trial.outcome == wanted for trial in scored)

    def rate(subset: list[TrialResult]) -> float | None:
        return len(subset) / total if total else None

    return {
        "case": {**asdict(case), "tags": list(case.tags)},
        "trial_count": len(trials),
        "scored_trial_count": total,
        "invalid_trial_count": invalid,
        "passed": passed,
        "call_rate": rate(called),
        "browse_rate": rate(browsed),
        "bypass_rate": rate(bypassed),
        "select_rate": rate(selects),
        "selection_accuracy": (
            sum(trial.selection_correct is True for trial in scored) / total
            if case.expect == "select" and total
            else None
        ),
        "category_accuracy": (
            sum(1 for trial in category_scored if trial.category_correct) / len(category_scored)
            if category_scored
            else None
        ),
        "median_first_call_seconds": (
            round(statistics.median(first_call_times), 3) if first_call_times else None
        ),
        "outcomes": {
            outcome: sum(1 for trial in trials if trial.outcome == outcome)
            for outcome in sorted({trial.outcome for trial in trials})
        },
        "bypass_kinds": sorted({bypass.kind for trial in trials for bypass in trial.bypasses}),
        "trials": [asdict(trial) for trial in trials],
    }


def aggregate(results: list[dict[str, Any]]) -> dict[str, Any]:
    call_cases = [result for result in results if result["case"]["expect"] == "call"]
    control_cases = [result for result in results if result["case"]["expect"] == "no-call"]
    select_cases = [result for result in results if result["case"]["expect"] == "select"]

    def mean(values: list[float]) -> float | None:
        values = [value for value in values if value is not None]
        return round(statistics.mean(values), 4) if values else None

    category_scores = [
        result["category_accuracy"] for result in call_cases if result["category_accuracy"] is not None
    ]
    action_cases = call_cases + select_cases
    return {
        "call_case_count": len(call_cases),
        "control_case_count": len(control_cases),
        "select_case_count": len(select_cases),
        "invalid_trial_count": sum(result["invalid_trial_count"] for result in results),
        "scored_trial_count": sum(result["scored_trial_count"] for result in results),
        "recall_browse_rate": mean([result["browse_rate"] for result in call_cases]),
        "recall_any_call_rate": mean([result["call_rate"] for result in call_cases]),
        "bypass_rate": mean([result["bypass_rate"] for result in call_cases]),
        "select_rate": mean([result["select_rate"] for result in action_cases]),
        "selection_accuracy": mean(
            [result["selection_accuracy"] for result in select_cases]
        ),
        "category_accuracy": mean(category_scores),
        "control_clean_rate": mean(
            [1.0 - result["call_rate"] for result in control_cases if result["call_rate"] is not None]
        ),
        "worst_call_cases": [
            result["case"]["id"]
            for result in sorted(
                [case for case in call_cases if case["browse_rate"] is not None],
                key=lambda r: r["browse_rate"],
            )[:5]
        ],
        "failing_controls": [
            result["case"]["id"] for result in control_cases if (result["call_rate"] or 0) > 0
        ],
    }


def passes_gates(
    summary: dict[str, Any],
    min_recall: float,
    min_precision: float,
    min_selection_accuracy: float,
) -> bool:
    """Apply only gates for case families represented by scored trials."""
    if summary["scored_trial_count"] <= 0:
        return False
    return all(
        value is None or value >= minimum
        for value, minimum in (
            (summary["recall_browse_rate"], min_recall),
            (summary["control_clean_rate"], min_precision),
            (summary["selection_accuracy"], min_selection_accuracy),
        )
    )


def main() -> int:
    args = parse_args()
    started_at = datetime.now(timezone.utc)
    categories = load_categories()
    cases = filter_cases(load_cases(args.cases, categories), args.case_ids, args.tags)

    if args.list:
        for case in cases:
            marker = {"call": "call", "no-call": "CTRL", "select": "SEL"}[case.expect]
            target = case.expected_tool or "-"
            print(
                f"{marker:5} {case.id:38} {case.expected_category or '-':24} "
                f"{target:16} {case.prompt[:70]}"
            )
        print(f"\n{len(cases)} cases")
        return 0

    executable = executable_identity(args.jcode)
    # Pin the resolved binary path so a symlink update during the run cannot
    # make the recorded identity differ from later trials.
    args.jcode = executable["path"]

    results: list[dict[str, Any]] = []
    with tempfile.TemporaryDirectory(prefix="jcode-discovery-rate-") as temp_dir:
        root = Path(temp_dir)
        socket_path = root / "jcode.sock"
        server = start_server(args, socket_path)
        try:
            total = len(cases) * args.trials
            done = 0
            for case in cases:
                trials: list[TrialResult] = []
                for trial_index in range(1, args.trials + 1):
                    print(f"[{case.id}] trial {trial_index}/{args.trials} ({case.expect})", flush=True)
                    # A trial that never reached the model measures nothing.
                    # Retry it with backoff so a transient rate limit does not
                    # silently shrink the sample.
                    trial = run_trial(args, case, trial_index, socket_path, root)
                    for retry in range(1, args.invalid_retries + 1):
                        if trial.valid:
                            break
                        wait = args.invalid_backoff * retry
                        print(
                            f"[{case.id}] trial {trial_index} invalid "
                            f"({trial.invalid_reason}); retry {retry}/{args.invalid_retries} "
                            f"in {wait:.0f}s",
                            flush=True,
                        )
                        time.sleep(wait)
                        trial = run_trial(args, case, trial_index, socket_path, root)
                    trials.append(trial)
                    done += 1
                    progress(done, total, "trials", f"{case.id} trial {trial_index}: {trials[-1].outcome}")
                    if args.retry_delay:
                        time.sleep(args.retry_delay)
                results.append(summarize_case(case, trials))
        finally:
            stop_server(args, socket_path, server)

    summary = aggregate(results)
    recall = summary["recall_browse_rate"]
    precision = summary["control_clean_rate"]
    selection_accuracy = summary["selection_accuracy"]
    passed = passes_gates(
        summary,
        args.min_recall,
        args.min_precision,
        args.min_selection_accuracy,
    )
    report = {
        "benchmark": "discovery-call-rate",
        "version": REPORT_VERSION,
        "started_at": started_at.isoformat(),
        "finished_at": datetime.now(timezone.utc).isoformat(),
        "benchmark_marker": {
            "environment": f"{BENCHMARK_ENV}=1",
            "request_header": f"{BENCHMARK_HEADER}: 1",
            "telemetry_field": "benchmark_run=true",
        },
        "config": {
            "executable": executable,
            "model": args.model,
            "provider": args.provider,
            "trials": args.trials,
            "timeout_seconds": args.timeout,
            "cases_file": str(args.cases),
            "min_recall": args.min_recall,
            "min_precision": args.min_precision,
            "min_selection_accuracy": args.min_selection_accuracy,
        },
        "summary": summary,
        "results": results,
        "passed": passed,
    }
    write_report(args.output, report)

    print("\nDiscovery call-rate summary")
    print(
        f"  Scored trials: {summary['scored_trial_count']}"
        f" (invalid, excluded: {summary['invalid_trial_count']})"
    )
    print(f"  Browse recall on capability-gap cases: {_pct(recall)} (gate {args.min_recall:.0%})")
    print(f"  Any Discovery call on those cases:     {_pct(summary['recall_any_call_rate'])}")
    print(f"  Bypassed Discovery entirely:           {_pct(summary['bypass_rate'])}")
    print(f"  Reached action=select:                 {_pct(summary['select_rate'])}")
    print(
        f"  Exact selection accuracy:              {_pct(selection_accuracy)} "
        f"(gate {args.min_selection_accuracy:.0%})"
    )
    print(f"  Correct category when browsing:        {_pct(summary['category_accuracy'])}")
    print(f"  Controls left clean:                   {_pct(precision)} (gate {args.min_precision:.0%})")
    if summary["failing_controls"]:
        print(f"  False positives: {', '.join(summary['failing_controls'])}")
    print("\n  Per case:")
    for result in results:
        case = result["case"]
        print(
            f"    {case['id']:38} {case['expect']:8} browse={_pct(result['browse_rate'])} "
            f"bypass={_pct(result['bypass_rate'])} select={_pct(result['select_rate'])} "
            f"accuracy={_pct(result['selection_accuracy'])} "
            f"{'invalid=' + str(result['invalid_trial_count']) + ' ' if result['invalid_trial_count'] else ''}"
            f"{'' if result['passed'] else 'FAIL'}"
        )
    print(f"\n  Report: {args.output}")
    return 0 if passed else 1


def _pct(value: float | None) -> str:
    return "n/a" if value is None else f"{value:.0%}"


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BenchmarkError as error:
        print(f"discovery rate benchmark error: {error}", file=sys.stderr)
        raise SystemExit(2)
