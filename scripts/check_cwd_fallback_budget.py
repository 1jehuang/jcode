#!/usr/bin/env python3
"""Enforce the "never trust the daemon cwd" invariant as a ratchet.

One `jcode` daemon serves sessions for many projects, and its process cwd is
whichever project happened to start it. Session-scoped code therefore must never
resolve a project-relative resource by falling back to the process cwd. The
forbidden shapes are:

- `x.unwrap_or(Path::new("."))` and friends
- `x.unwrap_or_else(|| std::env::current_dir())` and friends

Policy:
- New production fallbacks are rejected outright. There is no acceptable count
  to stay under: a fallback here is not a style issue, it is a way for one
  project's session to read another project's files.
- Pre-existing sites are tracked individually in `cwd_fallback.json`. They are
  grandfathered, not blessed, and each carries a reason.
- A site is identified by its *normalized statement text* and `kind`, not by its
  line number. An earlier version keyed on the line, which meant any edit above a
  recorded site reported a violation that did not exist, and `--update` (which may
  only remove) had no way to recover one that had merely moved. The line is kept in
  the file as a hint for humans and refreshed on `--update`, but it is not load
  bearing. Content keying still pins the specific code: a *different* fallback in
  an already-grandfathered file is a new site and stays reported.
- `--update` is one-directional: it may only *remove* sites, so the budget is
  monotone and this script can never be the thing that makes it worse. Adding a
  site is a hand edit to `cwd_fallback.json` carrying a reason that names why the
  process cwd is correct there. An automated path cannot honestly supply that
  justification, and offering one was the bug: `--update` refused a new *file*
  with no reason but silently absorbed a new site in an already-grandfathered
  file, handing it that file's unrelated justification. (Found by planting one
  and running the guard, not by reading it.)

Scope note: this is a text scan, like the other ratchets here. It cannot tell
session-scoped code from a legitimately foreground-only path, which is why the
foreground binary's legitimate uses are not special-cased -- they simply do not
use a fallback.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "scripts"))

from check_panic_budget import (  # noqa: E402
    CFG_TEST_RE,
    ITEM_START_RE,
    brace_delta,
    production_rust_files,
)

# This guard scans ITS OWN repo root, not the imported module's. `production_rust_files`
# defaults to that module's `SCAN_ROOTS`, and because Python caches imports under a
# bare module name the default is whichever copy was imported first -- which in a
# test suite is some earlier test's throwaway scratch tree, now deleted. Passing
# the roots explicitly is what makes each run scan the tree it was asked to scan.
SCAN_ROOTS = (REPO_ROOT / "src", REPO_ROOT / "crates")

BASELINE_FILE = REPO_ROOT / "scripts" / "cwd_fallback.json"

# What is forbidden is *silently substituting* the daemon's process cwd for a
# project directory. There are three ways to write that, and all three are
# matched:
#
#   x.unwrap_or(Path::new("."))                   an explicit "."
#   x.unwrap_or_else(|| std::env::current_dir())  the cwd as the fallback value
#   env::current_dir().unwrap_or_else(|_| ...)    the cwd, its error swallowed
#
# Two near-misses are deliberately NOT patterns, and both were found by running
# this guard over the tree rather than by reasoning:
#
#   std::env::current_dir()?        propagates the error instead of hiding it,
#                                   and is how the foreground CLI and several
#                                   load paths are meant to work (AGENTS.md keeps
#                                   the process-cwd fallback only in the CLI's own
#                                   load paths). Flagging it produced 40-odd false
#                                   positives on first run.
#   String::unwrap_or_default()     a generic combinator used hundreds of times
#                                   on strings, lengths and counts. That
#                                   `PathBuf::default()` is "" only bites when the
#                                   result is a path, and a text scan cannot
#                                   honestly infer the type. It is matched only
#                                   when it sits next to a `current_dir()`, where
#                                   the cwd is unambiguous.
#   unwrap_or_else(named_fn)         a known limitation, not an oversight. The
#                                   fallback is behind a name, so nothing on the
#                                   line says "." or `current_dir()`; matching every
#                                   `unwrap_or_else(` would flag hundreds of
#                                   legitimate combinators. Caught this way it is
#                                   a hole: a caller can hide the substitution here
#                                   and the guard stays green. `test_cwd_
#                                   fallback_ratchet.py` pins that it stays a hole
#                                   so the day someone widens the pattern they have
#                                   to close this test on purpose.
#
# The composite cases need a window, because rustfmt wraps them:
#
#     std::env::current_dir()
#         .unwrap_or_else(|_| PathBuf::from("."))
#
# Each statement is joined into one string before matching, so a wrapped chain is
# seen whole. An early version matched a window of neighbouring lines instead and
# reported whole blocks: four lines for one call, two of them blank.

# The optional closure a lazy fallback takes, and the whitespace after it.
# Deliberately loose: `||`, `|_|`, `|err|` all mean "there is a fallback body
# here", and none of their contents bear on whether the fallback is a path.
CLOSURE = r"(?:\|+\s*|\|[A-Za-z_][A-Za-z0-9_]*\|\s*)?"

# Self-contained: an explicit "." as a fallback value.
#
# The closure has to be skipped explicitly. `unwrap_or(Path::new("."))` matches
# a short pattern, but `unwrap_or_else(|| Path::new("."))` puts the closure
# between the paren and the constructor, so a pattern that only allows
# whitespace and an optional `||` misses it -- and a planted `unwrap_or_else`
# probe was reported as clean. That is the same shape AGENTS.md forbids by
# name, so an allowlisted ratchet may not be blinder than the rule it enforces.
DOT_RE = re.compile(
    r'unwrap_or(?:_else)?\s*\(' + CLOSURE + r'(?:std::)?(?:path::)?Path(?:Buf)?::'
    r'(?:new|from)\s*\(\s*"\."'
)

# The bare cwd call, and the two ways its error gets swallowed. These are only
# meaningful together: a lone `current_dir()` is fine, and a lone swallow is
# ordinary combinator code.
CWD_CALL_RE = re.compile(r'(?:std::)?env::current_dir\s*\(\s*\)')
CWD_AS_VALUE_RE = re.compile(
    r'unwrap_or(?:_else)?\s*\(' + CLOSURE + r'(?:\|\|\s*)?(?:std::)?env::current_dir\s*\(\s*\)'
)

# The bare cwd call, and the two ways its error gets swallowed. These are only
# meaningful together: a lone `current_dir()` is fine, and a lone swallow is
# ordinary combinator code.
CWD_CALL_RE = re.compile(r'(?:std::)?env::current_dir\s*\(\s*\)')
CWD_AS_VALUE_RE = re.compile(
    r'unwrap_or(?:_else)?\s*\(\s*(?:\|\|\s*)?(?:std::)?env::current_dir\s*\(\s*\)'
)

# The first method call applied to a `current_dir()` result, and whether it is
# itself a fallback, decide the verdict. A `.map(..)` in first place means the
# value was turned into something that is not a path, and whether the fallback
# that follows then matters is a judgement a text scan cannot make.
FIRST_METHOD_RE = re.compile(r'\.\s*[A-Za-z_]')
# A fallback applied directly to the `current_dir()` call: `.unwrap_or_else(|_|`
# or `.unwrap_or_default()`.
SWALLOW_DIRECT_RE = re.compile(
    r'\.\s*(?:unwrap_or(?:_else)?\s*\(|unwrap_or_default\s*\()'
)


def classify(statement: str) -> str | None:
    """Name the forbidden shape a statement contains, or None if it is legitimate.

    The subtle case is a swallowed `current_dir()` error. Which of those is a
    project-isolation bug depends entirely on what the fallback produces:

        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            -- a path. The OS will resolve it against the daemon. FORBIDDEN.
        std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
            -- a string shown in a status line. FORBIDDEN to look for, because
               the value never reaches the filesystem.

    A text scan cannot know the type, so the rule is structural instead: the
    fallback must apply *directly* to the `current_dir()` call. A `.map(...)` or
    any other method between them means the result has been turned into
    something else, which is left alone. Running the guard over the tree found
    exactly the two cases above, and the shape test separates them.
    """
    if DOT_RE.search(statement):
        return "dot"

    if CWD_AS_VALUE_RE.search(statement):
        # `current_dir()` as the value inside an `unwrap_or*`.
        return "cwd-as-value"

    # A direct swallow: `current_dir()` immediately followed by `.unwrap_or*`,
    # with nothing mapped in between. The check is "is the *first* method call
    # after `current_dir()` a swallow", which is what distinguishes the path case
    # from the display-string case. (Testing it the other way round, by asking
    # whether *some* method comes first, made the swallow itself look like a
    # transform and every true positive disappear.)
    #
    # `search`, not `match`: `statement_at` joins a wrapped chain with a space,
    # so a multi-line `current_dir()\n    .unwrap_or_else(..)` arrives with its
    # first method preceded by that joining space. Anchoring at index 0 skipped
    # every wrapped chain, which the planted-shape proof caught (exit 0 on a
    # violation rustfmt would actually produce). Searching also keeps the
    # semantics honest: the first `.` after the call *is* the first method.
    for match in CWD_CALL_RE.finditer(statement):
        tail = statement[match.end():]
        method = FIRST_METHOD_RE.search(tail)
        if method is None:
            continue
        if SWALLOW_DIRECT_RE.match(tail[method.start():]):
            return "cwd-error-swallowed"
    return None



# A site may be grandfathered only by naming why the process cwd is the right
# answer there. Kept deliberately short so "it was already there" cannot pass.
REASON_MIN_WORDS = 4


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--update",
        action="store_true",
        help="drop grandfathered sites that were cleaned up (refuses to add any)",
    )
    parser.add_argument(
        "--explain",
        metavar="FILE",
        help="print the grandfathered site in FILE, if any",
    )
    return parser.parse_args()


def production_line_numbers(path: Path) -> list[int]:
    """1-based line numbers of `path` that are outside every cfg(test) block.

    Same brace-depth walk as the panic budget, but returning line numbers so a
    violation can be pointed at precisely.
    """
    lines = path.read_text(encoding="utf-8", errors="ignore").splitlines()
    skip_stack: list[int] = []
    keep: list[int] = []
    pending_cfg_test = False

    for index, line in enumerate(lines):
        if sum(skip_stack) == 0:
            if pending_cfg_test and ITEM_START_RE.match(line):
                delta = brace_delta(line)
                if delta > 0:
                    skip_stack.append(delta)
                pending_cfg_test = False
                continue
            if pending_cfg_test and line.strip() and not line.strip().startswith("#"):
                pending_cfg_test = False
            if CFG_TEST_RE.match(line):
                pending_cfg_test = True
                continue
            keep.append(index + 1)
        else:
            skip_stack[-1] += brace_delta(line)
            if skip_stack[-1] <= 0:
                skip_stack.pop()
    return keep


def _span(line: str) -> tuple[int, int]:
    """Character range of a line's code, ignoring its indentation."""
    stripped = line.strip()
    return len(line) - len(stripped), len(line)


def _continuation(lines: list[str], index: int) -> int:
    """How many further lines belong to the statement starting at `index`.

    A chain that rustfmt wrapped keeps its `.method(` or `?` shape on the next
    line with deeper indentation. A statement ends at the first line that is not
    more indented than the one that started it, which is what rustfmt produces
    for every chain in this tree.
    """
    base = len(lines[index]) - len(lines[index].lstrip())
    count = 1
    for k in range(index + 1, min(index + 8, len(lines))):
        line = lines[k]
        if not line.strip():
            break
        indent = len(line) - len(line.lstrip())
        if indent <= base:
            break
        count += 1
    return count


def _strip_comment(line: str) -> str:
    """Drop a trailing `//` comment, respecting string literals.

    A doc comment describing the bug is not the bug. Guarding against
    `unwrap_or_else(|| Path::new("."))` means the fix's own explanation of that
    shape has to be quotable in a comment, so comment text must not be scanned.
    Anything past `//` is dropped unless it sits inside a string, and a bare
    `//` at the start of the line removes the whole line.
    """
    in_string = False
    escaped = False
    for index, char in enumerate(line):
        if escaped:
            escaped = False
            continue
        if char == "\\" and in_string:
            escaped = True
            continue
        if char == '"':
            in_string = not in_string
            continue
        if not in_string and char == "/" and line[index + 1: index + 2] == "/":
            return line[:index]
    return line


# Known limitation: block comments are not stripped. A `/* ... */` that quotes
# one of the forbidden shapes is therefore still reported. That is deliberate.
# Over-clearing costs one manual allowlist entry; under-clearing costs a real
# violation going unnoticed, and the tradeoff is not close. Probed: line
# comments are handled correctly, including one that follows code on the same
# line and a `//` inside a string literal.


def statement_at(lines: list[str], index: int) -> str:
    """The whole statement beginning at `index`, joined into one line.

    Comments are stripped from every line first. A guard that quotes the shape
    it forbids inside a doc comment would otherwise flag its own explanation,
    which is how the first version of this proof reported a violation in
    `agentgrep/args.rs` that existed only in prose.
    """
    parts = [_strip_comment(lines[index]).strip()]
    for offset in range(1, _continuation(lines, index)):
        parts.append(_strip_comment(lines[index + offset]).strip())
    return " ".join(p for p in parts if p)


def current_sites() -> dict[str, list[dict[str, Any]]]:
    """Every production statement that silently substitutes the process cwd.

    Statements, not lines: `current_dir()` and the fallback that swallows its
    error are routinely split across two lines by rustfmt, and matching a window
    of neighbouring lines flagged whole blocks instead of sites (an early version
    reported four lines for one call, two of them blank). Joining each
    statement first and then matching puts one site on one line, and a statement
    is only reported once.
    """
    sites: dict[str, list[dict[str, Any]]] = {}
    for path in production_rust_files(SCAN_ROOTS):
        rel = path.relative_to(REPO_ROOT).as_posix()
        lines = path.read_text(encoding="utf-8", errors="ignore").splitlines()
        keep = set(production_line_numbers(path))
        consumed: set[int] = set()
        for number in sorted(keep):
            if number in consumed:
                continue
            if not _strip_comment(lines[number - 1]).strip():
                # Blank or comment-only: not a statement, so it cannot host a
                # site and must not anchor a continuation either.
                continue
            statement = statement_at(lines, number - 1)
            span = _continuation(lines, number - 1)
            kind = classify(statement)
            if kind is not None:
                sites.setdefault(rel, []).append(
                    {"line": number, "kind": kind, "text": statement[:160]}
                )
                for offset in range(span):
                    consumed.add(number + offset)
    return sites


def _key(kind: str, text: str) -> str:
    """The identity of a site: its shape plus its normalized statement.

    Matching on this rather than on the line is what lets an edit above a
    grandfathered site move it without the guard reporting a violation that does
    not exist.

    Whitespace OUTSIDE a string literal is dropped, not merely collapsed. Joining a
    statement that rustfmt split leaves a space before the continuation --
    `p.parent()` versus `p .parent()` -- so collapsing alone would still report a
    phantom violation the next time rustfmt chose a different layout for a line
    nobody edited. Whitespace inside a string is kept, because `Path::new(" ")` and
    `Path::new("")` are different code and only one of them is a bare cwd.
    """
    return kind + "\x00" + _strip_ws_outside_strings(text)


def _strip_ws_outside_strings(text: str) -> str:
    out: list[str] = []
    in_string = False
    escaped = False
    for char in text:
        if escaped:
            out.append(char)
            escaped = False
            continue
        if char == "\\" and in_string:
            out.append(char)
            escaped = True
            continue
        if char == '"':
            in_string = not in_string
            out.append(char)
            continue
        if not in_string and char.isspace():
            continue
        out.append(char)
    return "".join(out)


def _site_key(site: dict[str, Any]) -> str:
    return _key(site["kind"], site.get("text", ""))


def _hit_key(hit: dict[str, Any]) -> str:
    return _key(hit["kind"], hit["text"])


def load_baseline() -> dict[str, Any]:
    if not BASELINE_FILE.exists():
        return {"version": 1, "grandfathered": {}}
    data = json.loads(BASELINE_FILE.read_text(encoding="utf-8"))
    if not isinstance(data, dict) or not isinstance(data.get("grandfathered"), dict):
        raise SystemExit(f"error: invalid baseline file format: {BASELINE_FILE}")
    for path, entry in data["grandfathered"].items():
        if not isinstance(entry, dict) or not isinstance(entry.get("reason"), str):
            raise SystemExit(f"error: grandfathered site {path} has no reason")
        sites = entry.get("sites", [])
        if not isinstance(sites, list):
            raise SystemExit(f"error: grandfathered site {path} has a malformed site list")
        for site in sites:
            if not isinstance(site, dict) or not isinstance(site.get("line"), int):
                # Caught here rather than as a KeyError in the comparison below,
                # which would report a crash instead of a bad entry.
                raise SystemExit(
                    f"error: grandfathered site {path} has a site with no line number"
                )
            if not isinstance(site.get("kind"), str):
                raise SystemExit(
                    f"error: grandfathered site {path} has a site with no kind"
                )
            if "text" not in site:
                # Sites are matched on their statement text now, so a site without
                # one can never match and every one of them would be reported as a
                # new violation. Name the real cause instead of letting that read
                # as a genuine finding.
                raise SystemExit(
                    f"error: grandfathered site {path} line {site['line']} has no text. "
                    "This baseline predates content keying, where sites were matched "
                    "by line number and any edit above one reported a violation that "
                    "did not exist. Regenerate it: for each file, run "
                    "check_cwd_fallback_budget.py --update to drop stale entries, "
                    "then re-add the still-present sites with the statement text."
                )
    return data


def write_baseline(sites: dict[str, list[dict[str, Any]]], reasons: dict[str, str]) -> None:
    """Write back only sites that were already grandfathered before this run."""
    payload = {}
    for path, hits in sites.items():
        # One entry per file; the reason covers every site listed there.
        payload[path] = {
            "reason": reasons[path],
            "sites": [
                {"line": h["line"], "kind": h["kind"], "text": h["text"]} for h in hits
            ],
        }
    BASELINE_FILE.write_text(
        json.dumps({"version": 1, "grandfathered": payload}, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def main() -> int:
    args = parse_args()
    sites = current_sites()
    baseline = load_baseline()
    tracked: dict[str, Any] = baseline["grandfathered"]

    if args.explain:
        entry = tracked.get(args.explain)
        if entry is None:
            print(f"{args.explain}: not grandfathered")
            return 0
        print(f"{args.explain}: {entry['reason']}")
        for site in entry.get("sites", []):
            # The text is the site's real identity; the line is only a hint and
            # may be out of date if the file has been edited above it.
            print(f"  line {site['line']}: {site['kind']}  {site.get('text', '(no text)')}")
        return 0

    if args.update:
        reasons = {path: entry.get("reason", "") for path, entry in tracked.items()}
        # Only sites this file already grandfathered may survive. Anything else is
        # a site the author would be newly accepting, and that is a hand edit.
        known: dict[str, set[str]] = {
            path: {_site_key(site) for site in entry.get("sites", [])}
            for path, entry in tracked.items()
        }
        additions = [
            (path, hit)
            for path, hits in sorted(sites.items())
            for hit in hits
            if _hit_key(hit) not in known.get(path, set())
        ]
        if additions:
            print(
                "--update can only drop grandfathered sites; it will not add one. "
                "Fix the code, or accept the site deliberately by adding it to "
                f"{BASELINE_FILE.relative_to(REPO_ROOT)} by hand with a reason saying "
                "why the process cwd is the right answer there:",
                file=sys.stderr,
            )
            for path, hit in additions:
                print(f"  {path}:{hit['line']}  {hit['text']}", file=sys.stderr)
            print(
                f"\nRefusing to update: the budget would grow from "
                f"{sum(len(v) for v in tracked.values())} to "
                f"{sum(len(v) for v in sites.values())} site(s).",
                file=sys.stderr,
            )
            return 1
        write_baseline(sites, reasons)
        print(
            "Updated process-cwd fallback baseline: "
            f"files={len(tracked)} -> {len(sites)}"
        )
        return 0

    violations: list[str] = []
    stale: list[str] = []

    for path, hits in sorted(sites.items()):
        entry = tracked.get(path)
        if entry is None:
            for hit in hits:
                violations.append(
                    f"new process-cwd fallback: {path}:{hit['line']}  {hit['text']}"
                )
            continue
        tracked_keys = {_site_key(site) for site in entry.get("sites", [])}
        for hit in hits:
            if _hit_key(hit) not in tracked_keys:
                violations.append(
                    f"process-cwd fallback at unrecorded site: {path}:{hit['line']}  {hit['text']}"
                )

    for path, entry in sorted(tracked.items()):
        if path not in sites:
            stale.append(f"grandfathered site cleaned up: {path} ({entry['reason']})")

    if violations:
        print("Process-cwd fallback budget exceeded:", file=sys.stderr)
        for entry in violations:
            print(f"  - {entry}", file=sys.stderr)
        print(
            "\nA session must resolve project resources from its own working dir, "
            "never the daemon's. If this path is genuinely foreground-only, say why "
            "in a comment and record it with --update.",
            file=sys.stderr,
        )
        return 1

    if stale:
        for entry in stale:
            print(f"note: {entry}")
    print(
        "Process-cwd fallback budget OK: "
        f"{sum(len(v) for v in sites.values())} grandfathered site(s), no new ones."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
