#!/usr/bin/env python3
"""Analyze the monolithic root `jcode` crate to plan a bottom-up split.

For every top-level module under src/ (a `foo.rs` file or a `foo/` dir) it
computes:
  - loc: total lines of Rust (incl. submodules for dir modules)
  - facade: whether it is already just `pub use jcode_*::*;`
  - inbound: how many *other* top-level modules reference `crate::<mod>`
  - outbound: which other in-root (non-facade) modules it references

The goal is to find low-coupling, high-line-count leaves to extract next, and
to compute a topological-ish extraction order (extract modules whose in-root
outbound deps are already crates/facades first).

Pure static analysis: safe to run while builds are in progress.
"""
from __future__ import annotations
import os
import re
import sys
import json
from collections import defaultdict

SRC = os.path.join(os.path.dirname(__file__), "..", "src")
SRC = os.path.normpath(SRC)

CRATE_RE = re.compile(r"\bcrate::([a-z_][a-z0-9_]*)")
FACADE_RE = re.compile(r"pub use jcode_[a-z0-9_]+::")
# `use crate::...;` statement body (may span lines); group 1 is everything
# between `crate::` and the terminating `;`.
USE_CRATE_RE = re.compile(r"\buse\s+crate::([^;]*);", re.DOTALL)


def _split_top_level(body: str) -> list[str]:
    """Split a brace-group body on top-level commas (ignoring nested braces)."""
    parts: list[str] = []
    depth = 0
    cur = []
    for ch in body:
        if ch == "{":
            depth += 1
            cur.append(ch)
        elif ch == "}":
            depth -= 1
            cur.append(ch)
        elif ch == "," and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
    if cur:
        parts.append("".join(cur))
    return [p.strip() for p in parts if p.strip()]


def _module_imports(joined: str):
    """Parse `use crate::...;` statements in `joined` source text.

    Returns (group_edge_counts, alias_set):
      * group_edge_counts: target module -> number of grouped-import references
        that `CRATE_RE` cannot see (because `{` immediately follows `crate::`).
        Non-grouped `use crate::name...;` imports are intentionally left to
        `CRATE_RE` so we never double count.
      * alias_set: module names brought into local scope as a bare `name::`
        path prefix (so later bare `name::` usages can be attributed).
    """
    group_edges: dict[str, int] = defaultdict(int)
    aliases: set[str] = set()
    ident = re.compile(r"^[a-z_][a-z0-9_]*$")
    for m in USE_CRATE_RE.finditer(joined):
        rest = m.group(1).strip()
        if rest.startswith("{"):
            # Grouped import: `use crate::{a, b, c::Foo, d::{self, X}};`
            inner = rest[1 : rest.rfind("}")] if "}" in rest else rest[1:]
            for entry in _split_top_level(inner):
                if entry in ("self", "*"):
                    continue
                head = entry.split("::", 1)[0].split(" as ")[0].strip()
                if not ident.match(head):
                    continue
                group_edges[head] += 1
                # Bare `head` (no `::`) or `head::{self...}` brings `head` itself
                # into scope as a usable module path prefix.
                if "::" not in entry:
                    aliases.add(head)
                elif re.match(rf"^{re.escape(head)}::\{{\s*self\b", entry):
                    aliases.add(head)
        else:
            # Non-grouped: `use crate::name;`, `name::Item`, `name::{...}`,
            # `name as X`. CRATE_RE already counts the `crate::name` edge, so we
            # only track whether `name` becomes a bare path alias here.
            head = rest.split("::", 1)[0].split(" as ")[0].strip()
            if not ident.match(head):
                continue
            if "::" not in rest:
                aliases.add(head)
            elif re.match(rf"^{re.escape(head)}::\{{\s*self\b", rest):
                aliases.add(head)
    return dict(group_edges), aliases


def top_level_modules() -> dict[str, list[str]]:
    """Return {module_name: [file paths]} for each top-level module.

    A module `foo` may be backed by `src/foo.rs`, `src/foo/` (dir), or BOTH (a
    facade `foo.rs` that re-exports a crate plus a small local `foo/` submodule).
    We collect all backing files under one logical module and remember the
    canonical entry file (the `.rs` sibling, else `foo/mod.rs`).
    """
    mods: dict[str, list[str]] = {}
    entries: dict[str, str] = {}
    dir_entries: dict[str, str] = {}
    for entry in sorted(os.listdir(SRC)):
        path = os.path.join(SRC, entry)
        if entry.endswith(".rs") and os.path.isfile(path):
            name = entry[:-3]
            if name in ("lib", "main"):
                continue
            mods.setdefault(name, []).append(path)
            entries[name] = path  # foo.rs is always the canonical entry
        elif os.path.isdir(path):
            files = []
            for root, _dirs, fnames in os.walk(path):
                for fn in fnames:
                    if fn.endswith(".rs"):
                        files.append(os.path.join(root, fn))
            if files:
                mods.setdefault(entry, []).extend(sorted(files))
                modrs = os.path.join(path, "mod.rs")
                dir_entries[entry] = modrs if os.path.exists(modrs) else sorted(files)[0]
    # Canonical entry: prefer the `.rs` sibling; fall back to the dir entry.
    for name, dir_entry in dir_entries.items():
        entries.setdefault(name, dir_entry)
    # stash entries on the function for the caller
    top_level_modules.entries = entries  # type: ignore[attr-defined]
    return mods


def loc(files: list[str]) -> int:
    total = 0
    for f in files:
        try:
            with open(f, encoding="utf-8", errors="ignore") as fh:
                total += sum(1 for _ in fh)
        except OSError:
            pass
    return total


def facade_ratio(name: str) -> tuple[float, int, int]:
    """Return (re-export ratio, facade_lines, code_lines) for the entry file.

    A high ratio means the module's public surface mostly lives in a crate
    already; a low ratio means real local logic still lives in the root crate.
    """
    entry = getattr(top_level_modules, "entries", {}).get(name)
    if entry is None or not os.path.exists(entry):
        return (0.0, 0, 0)
    try:
        with open(entry, encoding="utf-8", errors="ignore") as fh:
            text = fh.read()
    except OSError:
        return (0.0, 0, 0)
    code_lines = [
        ln.strip()
        for ln in text.splitlines()
        if ln.strip()
        and not ln.strip().startswith("//")
        and not ln.strip().startswith("#![")
        and not ln.strip().startswith("#[")
    ]
    if not code_lines:
        return (0.0, 0, 0)
    facade_lines = [ln for ln in code_lines if FACADE_RE.search(ln)]
    return (len(facade_lines) / len(code_lines), len(facade_lines), len(code_lines))


def classify_facade(name: str, total_loc: int = 0) -> str:
    """fully | thick | none.

    fully: entry is essentially just re-exports (already a crate facade).
    thick: re-exports a crate but keeps a small residual of local API/logic.
    none:  no crate re-export, OR a large module whose bulk still lives locally
           (a re-export line in mod.rs does not make a 30K-line dir a facade).
    """
    ratio, facade_lines, code_lines = facade_ratio(name)
    if facade_lines == 0:
        return "none"
    # A large module still carries its weight in-root regardless of a convenience
    # re-export in its entry file; only small modules can be "extracted enough".
    if total_loc > 600:
        return "none"
    # Mostly re-exports, only a tiny tail of local helpers -> fully extracted.
    if ratio >= 0.5 or code_lines <= max(8, facade_lines + 4):
        return "fully"
    return "thick"


def is_facade(name: str, files: list[str]) -> bool:
    # "Extracted enough" to no longer count as a real in-root blocker for bulk.
    return classify_facade(name, loc(files)) == "fully"


def outbound_refs(files: list[str], self_name: str, exclude_tests: bool = True):
    """Return (ref_set, ref_counts) for `crate::<mod>` references.

    For crate-split planning we care about the *library* dependency graph, so by
    default we skip test-only files (`*_tests.rs`, `tests.rs`) and lines guarded
    by an immediately-preceding `#[cfg(test)]`. Test deps would become
    dev-dependencies and do not constrain how the lib is split into crates.

    ref_counts maps target module -> number of referencing lines, used as an edge
    weight: a cheap edge (few references) is easy to invert/cut to break a cycle.
    """
    counts: dict[str, int] = defaultdict(int)
    for f in files:
        base = os.path.basename(f)
        if exclude_tests and (base == "tests.rs" or base.endswith("_tests.rs")):
            continue
        try:
            with open(f, encoding="utf-8", errors="ignore") as fh:
                lines = fh.readlines()
        except OSError:
            continue
        in_test_block = False
        test_block_depth = 0
        depth = 0
        pending_cfg_test = False
        code_lines: list[str] = []
        for ln in lines:
            stripped = ln.strip()
            if exclude_tests and stripped.startswith("#[cfg(test)]"):
                pending_cfg_test = True
                continue
            if exclude_tests and pending_cfg_test and "{" in ln:
                in_test_block = True
                test_block_depth = depth
                pending_cfg_test = False
            if in_test_block:
                depth += ln.count("{") - ln.count("}")
                if depth <= test_block_depth:
                    in_test_block = False
                continue
            depth += ln.count("{") - ln.count("}")
            code_lines.append(ln)
            for m in CRATE_RE.finditer(ln):
                counts[m.group(1)] += 1
        # Grouped `use crate::{...}` edges (invisible to CRATE_RE) and bare
        # `alias::` usages from imported module aliases. Without this, any
        # module pulled in via a grouped import (e.g. `use crate::{id, tui};`)
        # and then referenced as `tui::App` would be entirely uncounted,
        # badly undercounting edge weights for crate-split planning.
        joined = "".join(code_lines)
        group_edges, aliases = _module_imports(joined)
        for name, c in group_edges.items():
            counts[name] += c
        aliases.discard(self_name)
        if aliases:
            alias_re = re.compile(
                r"(?<![\w:])(" + "|".join(re.escape(a) for a in sorted(aliases)) + r")::"
            )
            for ln in code_lines:
                if ln.lstrip().startswith("use "):
                    continue
                for m in alias_re.finditer(ln):
                    counts[m.group(1)] += 1
    counts.pop(self_name, None)
    return set(counts), dict(counts)


def strongly_connected_components(graph: dict[str, set[str]]) -> list[list[str]]:
    """Tarjan's SCC over the module dependency graph.

    A component with >1 node (or a self-loop) is a dependency cycle: those
    modules cannot be split into separate crates without first breaking the
    cycle (e.g. by extracting a shared trait/interface crate). Returned in
    reverse-topological order (leaves first).
    """
    index_counter = [0]
    stack: list[str] = []
    on_stack: dict[str, bool] = {}
    index: dict[str, int] = {}
    lowlink: dict[str, int] = {}
    result: list[list[str]] = []

    import sys as _sys

    _sys.setrecursionlimit(10000)

    def strongconnect(v: str) -> None:
        index[v] = index_counter[0]
        lowlink[v] = index_counter[0]
        index_counter[0] += 1
        stack.append(v)
        on_stack[v] = True
        for w in sorted(graph.get(v, ())):
            if w not in index:
                strongconnect(w)
                lowlink[v] = min(lowlink[v], lowlink[w])
            elif on_stack.get(w):
                lowlink[v] = min(lowlink[v], index[w])
        if lowlink[v] == index[v]:
            comp = []
            while True:
                w = stack.pop()
                on_stack[w] = False
                comp.append(w)
                if w == v:
                    break
            result.append(comp)

    for v in sorted(graph):
        if v not in index:
            strongconnect(v)
    return result


def main() -> int:
    mods = top_level_modules()
    names = set(mods)

    info = {}
    for name, files in mods.items():
        module_loc = loc(files)
        refs, ref_counts = outbound_refs(files, name)
        info[name] = {
            "loc": module_loc,
            "facade": classify_facade(name, module_loc) == "fully",
            "facade_class": classify_facade(name, module_loc),
            "outbound": refs,
            "ref_counts": ref_counts,
        }

    # inbound: count of other modules referencing crate::<name>
    inbound = defaultdict(set)
    for name, meta in info.items():
        for dep in meta["outbound"]:
            if dep in names and dep != name:
                inbound[dep].add(name)

    # "in-root blockers": outbound deps that are still real in-root modules with
    # substantive local logic. A `thick` facade (re-exports a crate + a tiny tail
    # of local helpers) is NOT a bulk blocker: its weight already moved to a crate.
    def is_blocker(dep: str) -> bool:
        return dep in names and info[dep]["facade_class"] == "none"

    for name, meta in info.items():
        blockers = {d for d in meta["outbound"] if is_blocker(d) and d != name}
        meta["in_root_blockers"] = blockers
        meta["inbound_count"] = len(inbound.get(name, ()))

    extractable_now = sorted(
        (
            n
            for n, m in info.items()
            if m["facade_class"] == "none"
            and not n.endswith("_tests")
            and not m["in_root_blockers"]
        ),
        key=lambda n: -info[n]["loc"],
    )

    if "--json" in sys.argv:
        out = {
            n: {
                "loc": m["loc"],
                "facade_class": m["facade_class"],
                "inbound": m["inbound_count"],
                "in_root_blockers": sorted(m["in_root_blockers"]),
            }
            for n, m in info.items()
        }
        print(json.dumps({"modules": out, "extractable_now": extractable_now}, indent=2))
        return 0

    total = sum(m["loc"] for m in info.values())
    nonfacade = sum(m["loc"] for m in info.values() if m["facade_class"] == "none")
    thick = sum(m["loc"] for m in info.values() if m["facade_class"] == "thick")
    print(f"root crate total loc (top-level modules): {total}")
    print(f"  fully in-root (no crate yet):           {nonfacade}")
    print(f"  thick facades (crate + residual local): {thick}")
    print(f"  fully-facade loc:                       {total - nonfacade - thick}")
    print()
    print(f"{'module':24} {'loc':>7} {'fac':>5} {'in':>4}  in-root blockers")
    print("-" * 90)
    for n in sorted(info, key=lambda n: -info[n]["loc"]):
        m = info[n]
        if m["facade_class"] == "fully" or n.endswith("_tests"):
            continue
        blk = ", ".join(sorted(m["in_root_blockers"])) or "-- (none: extractable now)"
        print(f"{n:24} {m['loc']:>7} {m['facade_class']:>5} {m['inbound_count']:>4}  {blk}")
    print()
    print("=== Extractable now (no in-root blockers), largest first ===")
    for n in extractable_now:
        print(f"  {n:24} {info[n]['loc']:>7} loc, inbound={info[n]['inbound_count']}")

    # SCC analysis over the in-root dependency graph (only real, non-fully-facade
    # modules count as nodes/edges). Multi-node components are dependency cycles
    # that must be broken before those modules can become independent crates.
    graph = {
        n: {d for d in m["outbound"] if d in info and info[d]["facade_class"] != "fully" and d != n}
        for n, m in info.items()
        if m["facade_class"] != "fully"
    }
    sccs = strongly_connected_components(graph)
    cycles = [c for c in sccs if len(c) > 1]
    cycles.sort(key=lambda c: -sum(info[n]["loc"] for n in c))
    print()
    print("=== Dependency cycles (SCCs > 1 node) — must break before clean split ===")
    if not cycles:
        print("  (none: the in-root module graph is already a DAG)")
    for c in cycles:
        cloc = sum(info[n]["loc"] for n in c)
        member_str = ", ".join(sorted(c, key=lambda n: -info[n]["loc"]))
        print(f"  [{len(c)} modules, {cloc} loc] {member_str}")

    # For the largest cycle, suggest the cheapest edges to cut/invert to make it
    # acyclic (a feedback arc set). We use Eades' greedy linear-arrangement
    # heuristic to get a vertex order, then report edges that go "backwards" in
    # that order, weighted by reference count (cheap edges = easy refactors).
    if cycles:
        big = max(cycles, key=lambda c: sum(info[n]["loc"] for n in c))
        sub = set(big)
        # Build weighted subgraph restricted to the cycle.
        out_edges = {
            n: {d: info[n]["ref_counts"].get(d, 1) for d in info[n]["outbound"] if d in sub and d != n}
            for n in big
        }
        order = eades_order(big, out_edges)
        pos = {n: i for i, n in enumerate(order)}
        back_edges = []
        for u in big:
            for v, w in out_edges[u].items():
                if pos[v] < pos[u]:  # edge points backwards => part of feedback set
                    back_edges.append((w, u, v))
        back_edges.sort()  # cheapest first
        total_back = sum(w for w, _u, _v in back_edges)
        print()
        print(
            f"=== Feedback arc set for the {len(big)}-module cycle "
            f"({len(back_edges)} back-edges, {total_back} refs to invert) ==="
        )
        print("    Invert/cut these edges (cheapest first) to make the cycle a DAG:")
        limit = 1000 if "--full" in sys.argv else 30
        for w, u, v in back_edges[:limit]:
            print(f"  {u} -> {v}   ({w} refs)")
        if len(back_edges) > limit:
            print(f"  ... and {len(back_edges) - limit} more (use --full to list all)")

        # Per-node eviction cost: a module only leaves the SCC once ALL of its
        # out-edges into the cycle are cut. Rank cycle members by how few/cheap
        # those out-edges are -- those are the cheapest modules to evict next
        # (turning the SCC strictly smaller, which is what shrinks the largest
        # compile unit). For each node, list its in-cycle out-edges.
        evict = []
        for n in big:
            edges = sorted(
                ((w, d) for d, w in out_edges[n].items()),
                key=lambda x: (x[0], x[1]),
            )
            n_edges = len(edges)
            total_w = sum(w for w, _ in edges)
            evict.append((n_edges, total_w, n, edges))
        evict.sort(key=lambda x: (x[0], x[1], -info[x[2]]["loc"]))
        print()
        print(
            "=== Cheapest modules to evict next from the cycle "
            "(fewest in-cycle out-edges first) ==="
        )
        print("    A module leaves the SCC once all these out-edges are inverted/cut:")
        ev_limit = 1000 if "--full" in sys.argv else 12
        for n_edges, total_w, n, edges in evict[:ev_limit]:
            tgt = ", ".join(f"{d}({w})" for w, d in edges) or "-- (none)"
            print(
                f"  {n:<20} {info[n]['loc']:>7} loc  "
                f"{n_edges} edges / {total_w} refs -> {tgt}"
            )
        if len(evict) > ev_limit:
            print(f"  ... and {len(evict) - ev_limit} more (use --full to list all)")
    return 0


def eades_order(nodes, out_edges):
    """Eades-Lin-Smyth greedy heuristic returning a vertex order that minimizes
    backward edges (an approximate minimum feedback arc set)."""
    remaining = set(nodes)
    in_w = {n: 0 for n in nodes}
    out_w = {n: 0 for n in nodes}
    for u in nodes:
        for v, w in out_edges[u].items():
            out_w[u] += w
            in_w[v] += w
    left = []
    right = []
    # Work on mutable copies of degrees.
    while remaining:
        # Remove sinks (no outgoing within remaining) to the right.
        changed = True
        while changed:
            changed = False
            for n in list(remaining):
                if all(v not in remaining for v in out_edges[n]):
                    right.insert(0, n)
                    remaining.discard(n)
                    changed = True
            for n in list(remaining):
                if all(u not in remaining for u in nodes if n in out_edges[u]):
                    # source (no incoming within remaining) to the left
                    left.append(n)
                    remaining.discard(n)
                    changed = True
        if not remaining:
            break
        # Pick the node maximizing (out_w - in_w) within remaining.
        def score(n):
            o = sum(w for v, w in out_edges[n].items() if v in remaining)
            i = sum(w for u in remaining for vv, w in out_edges[u].items() if vv == n)
            return o - i
        pick = max(remaining, key=score)
        left.append(pick)
        remaining.discard(pick)
    return left + right


if __name__ == "__main__":
    raise SystemExit(main())
