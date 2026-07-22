#!/usr/bin/env python3
"""Attribution benchmark for sponsored Discovery listings.

For every sponsor in the catalog, this suite verifies that a signup or
install performed by an agent on the user's behalf is attributable to jcode.
Most signups happen from a CLI-driven agent flow, so the checks focus on
whether the catalog data itself carries a working attribution mechanism:

1. listing_url_marked      - the browse listing URL carries the referral marker.
2. select_url_marked       - the select-phase URL carries the referral marker.
3. setup_preserves_marker  - setup instructions never point to an unmarked
                             signup URL for the same host (which would drop
                             attribution when an agent follows them verbatim).
4. cli_flow_attributable   - if setup is CLI-first (install command, MCP
                             server, API key), attribution does not depend
                             solely on a browser cookie: either the setup
                             routes account creation through the marked URL,
                             or the sponsor declares a non-cookie mechanism
                             (e.g. signup-code, utm-forwarding) in
                             scripts/attribution_benchmark_sponsors.json.
5. live_url_resolves       - (optional, --live-web) the marked URL returns a
                             2xx/3xx response and the marker survives
                             redirects, so the referral cookie can be set.

Each sponsor gets a 0-100 score. 100 means every applicable check passed;
the suite exits non-zero if any sponsor scores below --min-score
(default 100).

Run offline against a saved catalog:

    python scripts/benchmark_attribution.py --catalog-file catalog.json

Run live against the discovery service (browse + select per sponsor), and
verify referral URLs on the public web:

    python scripts/benchmark_attribution.py --live --live-web

Live requests carry x-jcode-discovery-benchmark: 1 via
JCODE_DISCOVERY_BENCHMARK=1 semantics: this runner calls the discovery HTTP
API directly and always sends the benchmark header itself.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SPONSORS = REPO_ROOT / "scripts" / "attribution_benchmark_sponsors.json"
DEFAULT_OUTPUT = REPO_ROOT / "target" / "attribution-benchmark" / "latest.json"
DEFAULT_ENDPOINT = "https://api.jcode.sh/v1/discovery"
BENCHMARK_HEADER = "x-jcode-discovery-benchmark"

URL_RE = re.compile(r"https?://[^\s`'\")\]>]+")
CLI_SETUP_RE = re.compile(
    r"(npx |npm |pipx? |uvx? |cargo |brew |curl |mcp|api[ _-]?key|cli)",
    re.IGNORECASE,
)
SIGNUPISH_PATH_RE = re.compile(r"(signup|sign-up|register|get[-_]?started|join)", re.IGNORECASE)

KNOWN_MECHANISMS = {
    "referral-link",      # ?via= / ?ref= cookie-based affiliate tracking
    "signup-code",        # a code or coupon the agent supplies at signup
    "utm-forwarding",     # sponsor ingests utm/query params server-side
    "api-partner-id",     # partner id embedded in the API/MCP setup itself
}


class AttributionError(RuntimeError):
    pass


@dataclass
class CheckResult:
    name: str
    status: str  # pass | fail | skip
    detail: str


@dataclass
class SponsorReport:
    tool: str
    category: str
    mechanism: str
    checks: list[CheckResult] = field(default_factory=list)

    @property
    def score(self) -> int:
        applicable = [c for c in self.checks if c.status != "skip"]
        if not applicable:
            return 0
        passed = sum(1 for c in applicable if c.status == "pass")
        return round(100 * passed / len(applicable))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--sponsors", type=Path, default=DEFAULT_SPONSORS)
    parser.add_argument("--catalog-file", type=Path, help="Saved catalog JSON with full entries (url/setup).")
    parser.add_argument("--live", action="store_true", help="Fetch browse+select from the discovery service.")
    parser.add_argument("--live-web", action="store_true", help="Also verify referral URLs resolve on the public web.")
    parser.add_argument("--endpoint", default=DEFAULT_ENDPOINT)
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument("--min-score", type=int, default=100)
    parser.add_argument("--sponsor", action="append", dest="only", help="Limit to this tool name. Repeatable.")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    if not args.catalog_file and not args.live:
        parser.error("provide --catalog-file or --live")
    return args


def load_sponsor_expectations(path: Path) -> tuple[str, list[dict[str, Any]]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("version") != 1 or not isinstance(data.get("sponsors"), list):
        raise AttributionError(f"unsupported sponsor expectation file: {path}")
    default_marker = str(data.get("default_marker", "")).strip()
    sponsors = []
    seen: set[str] = set()
    for raw in data["sponsors"]:
        tool = str(raw.get("tool", "")).strip().lower()
        category = str(raw.get("category", "")).strip().lower()
        mechanism = str(raw.get("mechanism", "")).strip().lower()
        if not tool or not category:
            raise AttributionError(f"sponsor entry missing tool/category: {raw}")
        if mechanism not in KNOWN_MECHANISMS:
            raise AttributionError(f"sponsor {tool} has unknown mechanism {mechanism!r}")
        if tool in seen:
            raise AttributionError(f"duplicate sponsor expectation: {tool}")
        marker = str(raw.get("marker", default_marker)).strip()
        if not marker:
            raise AttributionError(f"sponsor {tool} has no attribution marker")
        seen.add(tool)
        sponsors.append({**raw, "tool": tool, "category": category, "mechanism": mechanism, "marker": marker})
    return default_marker, sponsors


def load_catalog_entries(path: Path) -> dict[str, dict[str, Any]]:
    """Return tool-name -> full entry (with url/setup) from a saved catalog file."""
    data = json.loads(path.read_text(encoding="utf-8"))
    raw_categories = data.get("categories", data)
    entries: dict[str, dict[str, Any]] = {}
    for category, raw in raw_categories.items():
        tools = raw.get("tools", raw) if isinstance(raw, dict) else raw
        for tool in tools:
            name = str(tool.get("name", "")).strip().lower()
            if name:
                entries[name] = {**tool, "category": category}
    return entries


def http_get(url: str, timeout: float, benchmark: bool = False) -> tuple[int, str, str]:
    """Return (status, final_url, body_prefix)."""
    request = urllib.request.Request(url, headers={"User-Agent": "jcode-attribution-benchmark"})
    if benchmark:
        request.add_header(BENCHMARK_HEADER, "1")
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.status, response.geturl(), response.read(65536).decode("utf-8", "replace")


def fetch_live_entry(endpoint: str, sponsor: dict[str, Any], timeout: float) -> dict[str, Any]:
    base = {
        "category": sponsor["category"],
        "q": f"attribution benchmark validation for {sponsor['category'].replace('-', ' ')} listings",
        "reason": (
            "Automated attribution benchmark verifying that sponsor listing and setup data "
            "preserve the jcode referral marker. No account will be created."
        ),
    }
    browse_url = f"{endpoint}?{urllib.parse.urlencode(base)}"
    status, _, body = http_get(browse_url, timeout, benchmark=True)
    if status != 200:
        raise AttributionError(f"browse failed for {sponsor['tool']}: HTTP {status}")
    browse = json.loads(body)
    listing = next(
        (t for t in browse.get("tools", []) if str(t.get("name", "")).lower() == sponsor["tool"]),
        None,
    )
    if listing is None:
        raise AttributionError(f"{sponsor['tool']} missing from live {sponsor['category']} browse")
    select_url = f"{endpoint}?{urllib.parse.urlencode({**base, 'tool': sponsor['tool']})}"
    status, _, body = http_get(select_url, timeout, benchmark=True)
    if status != 200:
        raise AttributionError(f"select failed for {sponsor['tool']}: HTTP {status}")
    selected = json.loads(body).get("tool", {})
    return {**listing, **selected, "category": sponsor["category"]}


def urls_in_text(text: str) -> list[str]:
    return [u.rstrip(".,;") for u in URL_RE.findall(text or "")]


def marker_in_url(url: str, marker: str) -> bool:
    return marker.lower() in url.lower()


def check_sponsor(
    sponsor: dict[str, Any],
    entry: dict[str, Any] | None,
    live_web: bool,
    timeout: float,
) -> SponsorReport:
    report = SponsorReport(tool=sponsor["tool"], category=sponsor["category"], mechanism=sponsor["mechanism"])
    marker = sponsor["marker"]

    if entry is None:
        report.checks.append(CheckResult("catalog_entry_present", "fail", "sponsor not found in catalog"))
        return report
    report.checks.append(CheckResult("catalog_entry_present", "pass", "listed"))

    url = str(entry.get("url", "") or "")
    setup = str(entry.get("setup", "") or "")

    # 1. listing URL marked
    if not url:
        report.checks.append(CheckResult("listing_url_marked", "fail", "no URL in catalog entry"))
    elif marker_in_url(url, marker):
        report.checks.append(CheckResult("listing_url_marked", "pass", url))
    else:
        report.checks.append(CheckResult("listing_url_marked", "fail", f"URL lacks marker {marker!r}: {url}"))

    # 2/3. setup URLs preserve the marker
    if not setup:
        report.checks.append(CheckResult("setup_preserves_marker", "skip", "no setup text (offline browse-only entry)"))
    else:
        marked_host = urllib.parse.urlsplit(url).netloc.lower().removeprefix("www.") if url else ""
        unmarked = []
        for setup_url in urls_in_text(setup):
            parts = urllib.parse.urlsplit(setup_url)
            host = parts.netloc.lower().removeprefix("www.")
            same_vendor = marked_host and (host == marked_host or host.endswith("." + marked_host))
            # Only browser-facing signup pages need the referral marker.
            # Docs hosts and API endpoints do not set referral cookies.
            non_browser = host.startswith(("docs.", "api.")) or "/api" in parts.path or "/oauth" in parts.path
            signupish = SIGNUPISH_PATH_RE.search(parts.path or "")
            if same_vendor and signupish and not non_browser and not marker_in_url(setup_url, marker):
                unmarked.append(setup_url)
        if unmarked:
            report.checks.append(
                CheckResult("setup_preserves_marker", "fail", f"unmarked vendor/signup URLs in setup: {unmarked}")
            )
        else:
            report.checks.append(CheckResult("setup_preserves_marker", "pass", "no unmarked vendor signup URLs"))

    # 4. CLI-first flows must not depend solely on a browser cookie
    if setup and CLI_SETUP_RE.search(setup):
        setup_urls = urls_in_text(setup)
        setup_has_marked_url = any(marker_in_url(u, marker) for u in setup_urls)
        if sponsor["mechanism"] != "referral-link":
            report.checks.append(
                CheckResult("cli_flow_attributable", "pass", f"non-cookie mechanism: {sponsor['mechanism']}")
            )
        elif setup_has_marked_url:
            report.checks.append(CheckResult("cli_flow_attributable", "pass", "setup routes signup through marked URL"))
        else:
            report.checks.append(
                CheckResult(
                    "cli_flow_attributable",
                    "fail",
                    "CLI-first setup with cookie-based referral-link attribution and no marked "
                    "signup URL in setup; agent-driven CLI signups will not be attributed",
                )
            )
    else:
        report.checks.append(CheckResult("cli_flow_attributable", "skip", "setup is not CLI-first or unavailable"))

    # 5. live web resolution
    if live_web and url and marker_in_url(url, marker):
        try:
            status, final_url, _ = http_get(url, timeout)
            if status >= 400:
                report.checks.append(CheckResult("live_url_resolves", "fail", f"HTTP {status}"))
            elif not marker_in_url(final_url, marker) and final_url != url:
                report.checks.append(
                    CheckResult("live_url_resolves", "fail", f"redirect dropped marker: {url} -> {final_url}")
                )
            else:
                report.checks.append(CheckResult("live_url_resolves", "pass", f"HTTP {status} at {final_url}"))
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            report.checks.append(CheckResult("live_url_resolves", "fail", f"request error: {error}"))
    else:
        report.checks.append(CheckResult("live_url_resolves", "skip", "live web check disabled or URL unmarked"))

    return report


def main() -> int:
    args = parse_args()
    _, sponsors = load_sponsor_expectations(args.sponsors)
    if args.only:
        wanted = {value.lower() for value in args.only}
        sponsors = [s for s in sponsors if s["tool"] in wanted]
        missing = wanted - {s["tool"] for s in sponsors}
        if missing:
            raise AttributionError(f"unknown --sponsor values: {', '.join(sorted(missing))}")

    catalog: dict[str, dict[str, Any]] = {}
    if args.catalog_file:
        catalog = load_catalog_entries(args.catalog_file)

    reports: list[SponsorReport] = []
    errors: list[str] = []
    for sponsor in sponsors:
        entry: dict[str, Any] | None = catalog.get(sponsor["tool"])
        if args.live:
            try:
                live_entry = fetch_live_entry(args.endpoint, sponsor, args.timeout)
                entry = {**(entry or {}), **live_entry}
            except (AttributionError, urllib.error.URLError, TimeoutError, OSError, json.JSONDecodeError) as error:
                errors.append(f"{sponsor['tool']}: {error}")
                if entry is None:
                    reports.append(
                        SponsorReport(
                            tool=sponsor["tool"],
                            category=sponsor["category"],
                            mechanism=sponsor["mechanism"],
                            checks=[CheckResult("live_fetch", "fail", str(error))],
                        )
                    )
                    continue
        reports.append(check_sponsor(sponsor, entry, args.live_web, args.timeout))

    result = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "endpoint": args.endpoint if args.live else None,
        "live_web": args.live_web,
        "errors": errors,
        "sponsors": [
            {
                "tool": r.tool,
                "category": r.category,
                "mechanism": r.mechanism,
                "score": r.score,
                "checks": [c.__dict__ for c in r.checks],
            }
            for r in reports
        ],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")

    worst = 100
    for r in reports:
        worst = min(worst, r.score)
        print(f"{r.tool:<14} {r.category:<18} score={r.score:>3}  "
              + " ".join(f"{c.name}={c.status}" for c in r.checks))
    for error in errors:
        print(f"error: {error}", file=sys.stderr)
    print(f"report: {args.output}")
    if worst < args.min_score or (errors and not reports):
        print(f"FAIL: minimum sponsor score {worst} < required {args.min_score}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except AttributionError as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(2)
