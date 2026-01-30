#!/usr/bin/env python3
"""
Compare token usage between jcode and Claude Code CLI.

This script runs the same prompts through both tools and compares their token usage.
The goal is to verify that jcode's token consumption is within expected bounds
compared to the official Claude Code CLI.

NOTE: jcode typically uses FEWER tokens than Claude CLI because:
1. jcode has a smaller/simpler system prompt
2. jcode registers fewer tools (Claude CLI has many built-in tools)
3. Different prompt caching behavior

The test PASSES if jcode uses fewer tokens OR at most 50% more tokens.
Using more tokens would indicate a problem with the system prompt or tool registration.

Usage:
    python scripts/compare_token_usage.py [--verbose] [--runs N]

Requirements:
    - jcode built and in PATH or at target/release/jcode
    - claude CLI installed and authenticated
    - Both should use the same model (claude-opus-4-5-20251101 by default)
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Optional


@dataclass
class TokenUsage:
    """Token usage from a single run."""
    input_tokens: int
    output_tokens: int
    cache_read_tokens: int
    cache_creation_tokens: int
    total_cost_usd: Optional[float] = None
    duration_ms: Optional[int] = None

    @property
    def total_input(self) -> int:
        """Total input tokens including cache."""
        return self.input_tokens + self.cache_read_tokens + self.cache_creation_tokens

    @property
    def total(self) -> int:
        """Total tokens (input + output)."""
        return self.total_input + self.output_tokens


@dataclass
class RunResult:
    """Result of a single tool run."""
    tool: str
    prompt: str
    usage: TokenUsage
    success: bool
    output: str
    error: Optional[str] = None


def find_jcode_binary() -> str:
    """Find the jcode binary."""
    # Check target/release first
    repo_root = Path(__file__).parent.parent
    release_binary = repo_root / "target" / "release" / "jcode"
    if release_binary.exists():
        return str(release_binary)

    # Check PATH
    result = subprocess.run(["which", "jcode"], capture_output=True, text=True)
    if result.returncode == 0:
        return result.stdout.strip()

    raise FileNotFoundError("jcode binary not found. Run 'cargo build --release' first.")


def run_claude_cli(prompt: str, workdir: str, model: str = "opus") -> RunResult:
    """Run the Claude Code CLI and capture token usage."""
    try:
        result = subprocess.run(
            [
                "claude",
                "-p",
                "--output-format", "json",
                "--dangerously-skip-permissions",
                "--model", model,
                prompt,
            ],
            capture_output=True,
            text=True,
            cwd=workdir,
            timeout=120,
        )

        if result.returncode != 0 and not result.stdout:
            return RunResult(
                tool="claude",
                prompt=prompt,
                usage=TokenUsage(0, 0, 0, 0),
                success=False,
                output="",
                error=result.stderr or f"Exit code {result.returncode}",
            )

        # Parse JSON output
        data = json.loads(result.stdout)
        usage = data.get("usage", {})

        token_usage = TokenUsage(
            input_tokens=usage.get("input_tokens", 0),
            output_tokens=usage.get("output_tokens", 0),
            cache_read_tokens=usage.get("cache_read_input_tokens", 0),
            cache_creation_tokens=usage.get("cache_creation_input_tokens", 0),
            total_cost_usd=data.get("total_cost_usd"),
            duration_ms=data.get("duration_ms"),
        )

        return RunResult(
            tool="claude",
            prompt=prompt,
            usage=token_usage,
            success=not data.get("is_error", False),
            output=data.get("result", ""),
        )

    except subprocess.TimeoutExpired:
        return RunResult(
            tool="claude",
            prompt=prompt,
            usage=TokenUsage(0, 0, 0, 0),
            success=False,
            output="",
            error="Timeout after 120s",
        )
    except json.JSONDecodeError as e:
        return RunResult(
            tool="claude",
            prompt=prompt,
            usage=TokenUsage(0, 0, 0, 0),
            success=False,
            output=result.stdout if 'result' in dir() else "",
            error=f"JSON parse error: {e}",
        )
    except Exception as e:
        return RunResult(
            tool="claude",
            prompt=prompt,
            usage=TokenUsage(0, 0, 0, 0),
            success=False,
            output="",
            error=str(e),
        )


def run_jcode(prompt: str, workdir: str, jcode_binary: str, model: str = "claude-opus-4-5-20251101") -> RunResult:
    """Run jcode and capture token usage from trace output."""
    try:
        # Create a temporary JCODE_HOME to avoid polluting user's sessions
        with tempfile.TemporaryDirectory() as tmpdir:
            env = os.environ.copy()
            env["JCODE_HOME"] = tmpdir
            env["JCODE_TRACE"] = "1"

            result = subprocess.run(
                [
                    jcode_binary,
                    "run",
                    "--no-update",
                    "--model", model,
                    prompt,
                ],
                capture_output=True,
                text=True,
                cwd=workdir,
                timeout=120,
                env=env,
            )

            # Parse token usage from trace output in stderr
            # Format: [trace] token_usage input=X output=Y cache_read=Z cache_write=W
            input_tokens = 0
            output_tokens = 0
            cache_read = 0
            cache_write = 0

            for line in result.stderr.split("\n"):
                if "[trace] token_usage" in line:
                    parts = line.split()
                    for part in parts:
                        if part.startswith("input="):
                            input_tokens = int(part.split("=")[1])
                        elif part.startswith("output="):
                            output_tokens = int(part.split("=")[1])
                        elif part.startswith("cache_read="):
                            cache_read = int(part.split("=")[1])
                        elif part.startswith("cache_write="):
                            cache_write = int(part.split("=")[1])

            token_usage = TokenUsage(
                input_tokens=input_tokens,
                output_tokens=output_tokens,
                cache_read_tokens=cache_read,
                cache_creation_tokens=cache_write,
            )

            return RunResult(
                tool="jcode",
                prompt=prompt,
                usage=token_usage,
                success=result.returncode == 0,
                output=result.stdout,
                error=None if result.returncode == 0 else result.stderr,
            )

    except subprocess.TimeoutExpired:
        return RunResult(
            tool="jcode",
            prompt=prompt,
            usage=TokenUsage(0, 0, 0, 0),
            success=False,
            output="",
            error="Timeout after 120s",
        )
    except Exception as e:
        return RunResult(
            tool="jcode",
            prompt=prompt,
            usage=TokenUsage(0, 0, 0, 0),
            success=False,
            output="",
            error=str(e),
        )


def compare_usage(claude_result: RunResult, jcode_result: RunResult, verbose: bool = False) -> dict:
    """Compare token usage between Claude CLI and jcode."""
    c = claude_result.usage
    j = jcode_result.usage

    # Calculate differences
    input_diff = j.input_tokens - c.input_tokens
    output_diff = j.output_tokens - c.output_tokens
    cache_read_diff = j.cache_read_tokens - c.cache_read_tokens
    cache_write_diff = j.cache_creation_tokens - c.cache_creation_tokens
    total_diff = j.total - c.total

    # Calculate percentages (avoid division by zero)
    def pct_diff(a: int, b: int) -> float:
        if b == 0:
            return 0.0 if a == 0 else float('inf')
        return ((a - b) / b) * 100

    input_pct = pct_diff(j.input_tokens, c.input_tokens)
    output_pct = pct_diff(j.output_tokens, c.output_tokens)
    total_pct = pct_diff(j.total, c.total)

    return {
        "claude": {
            "input": c.input_tokens,
            "output": c.output_tokens,
            "cache_read": c.cache_read_tokens,
            "cache_write": c.cache_creation_tokens,
            "total": c.total,
            "cost_usd": c.total_cost_usd,
            "duration_ms": c.duration_ms,
        },
        "jcode": {
            "input": j.input_tokens,
            "output": j.output_tokens,
            "cache_read": j.cache_read_tokens,
            "cache_write": j.cache_creation_tokens,
            "total": j.total,
        },
        "diff": {
            "input": input_diff,
            "output": output_diff,
            "cache_read": cache_read_diff,
            "cache_write": cache_write_diff,
            "total": total_diff,
        },
        "pct_diff": {
            "input": input_pct,
            "output": output_pct,
            "total": total_pct,
        },
    }


def print_comparison(comparison: dict, prompt: str, verbose: bool = False):
    """Print a formatted comparison."""
    print(f"\n{'='*60}")
    print(f"Prompt: {prompt[:50]}..." if len(prompt) > 50 else f"Prompt: {prompt}")
    print(f"{'='*60}")

    c = comparison["claude"]
    j = comparison["jcode"]
    d = comparison["diff"]
    p = comparison["pct_diff"]

    print(f"\n{'Metric':<20} {'Claude':<15} {'jcode':<15} {'Diff':<15} {'% Diff':<10}")
    print("-" * 75)
    print(f"{'Input tokens':<20} {c['input']:<15} {j['input']:<15} {d['input']:+<15} {p['input']:+.1f}%")
    print(f"{'Output tokens':<20} {c['output']:<15} {j['output']:<15} {d['output']:+<15} {p['output']:+.1f}%")
    print(f"{'Cache read':<20} {c['cache_read']:<15} {j['cache_read']:<15} {d['cache_read']:+<15}")
    print(f"{'Cache write':<20} {c['cache_write']:<15} {j['cache_write']:<15} {d['cache_write']:+<15}")
    print("-" * 75)
    print(f"{'TOTAL':<20} {c['total']:<15} {j['total']:<15} {d['total']:+<15} {p['total']:+.1f}%")

    if c.get("cost_usd"):
        print(f"\nClaude CLI cost: ${c['cost_usd']:.6f}")
    if c.get("duration_ms"):
        print(f"Claude CLI duration: {c['duration_ms']}ms")


def run_test_suite(verbose: bool = False, runs: int = 1) -> list:
    """Run the full test suite."""
    # Test prompts - simple ones that don't require tools
    prompts = [
        "Reply with a single word: test",
        "What is 2 + 2? Reply with just the number.",
        "List three primary colors, one per line.",
    ]

    jcode_binary = find_jcode_binary()
    print(f"Using jcode binary: {jcode_binary}")
    print(f"Running {len(prompts)} prompts, {runs} run(s) each\n")

    results = []

    with tempfile.TemporaryDirectory() as workdir:
        for prompt in prompts:
            for run_num in range(runs):
                print(f"\n[Run {run_num + 1}/{runs}] Testing: {prompt[:40]}...")

                # Run both tools
                print("  Running Claude CLI...", end=" ", flush=True)
                claude_result = run_claude_cli(prompt, workdir)
                if claude_result.success:
                    print(f"OK ({claude_result.usage.total} tokens)")
                else:
                    print(f"FAILED: {claude_result.error}")
                    if verbose:
                        print(f"    Output: {claude_result.output[:200]}")

                # Small delay to avoid rate limiting
                time.sleep(1)

                print("  Running jcode...", end=" ", flush=True)
                jcode_result = run_jcode(prompt, workdir, jcode_binary)
                if jcode_result.success:
                    print(f"OK ({jcode_result.usage.total} tokens)")
                else:
                    print(f"FAILED: {jcode_result.error}")
                    if verbose:
                        print(f"    Output: {jcode_result.output[:200]}")

                if claude_result.success and jcode_result.success:
                    comparison = compare_usage(claude_result, jcode_result, verbose)
                    results.append({
                        "prompt": prompt,
                        "run": run_num + 1,
                        "comparison": comparison,
                    })

                    if verbose:
                        print_comparison(comparison, prompt, verbose)

                # Delay between prompts
                time.sleep(2)

    return results


def summarize_results(results: list) -> bool:
    """Print summary of all results. Returns True if test passed."""
    if not results:
        print("\nNo successful results to summarize.")
        return False

    print(f"\n{'='*60}")
    print("SUMMARY")
    print(f"{'='*60}")

    total_claude = sum(r["comparison"]["claude"]["total"] for r in results)
    total_jcode = sum(r["comparison"]["jcode"]["total"] for r in results)
    total_diff = total_jcode - total_claude

    # Also compare just input+output (excluding cache)
    total_claude_io = sum(
        r["comparison"]["claude"]["input"] + r["comparison"]["claude"]["output"]
        for r in results
    )
    total_jcode_io = sum(
        r["comparison"]["jcode"]["input"] + r["comparison"]["jcode"]["output"]
        for r in results
    )

    if total_claude > 0:
        pct_diff = ((total_jcode - total_claude) / total_claude) * 100
    else:
        pct_diff = 0

    print(f"\nTotal runs: {len(results)}")
    print(f"\n--- Total Tokens (including cache) ---")
    print(f"Claude CLI: {total_claude}")
    print(f"jcode:      {total_jcode}")
    print(f"Difference: {total_diff:+} ({pct_diff:+.1f}%)")

    print(f"\n--- Input + Output only (excluding cache) ---")
    print(f"Claude CLI: {total_claude_io}")
    print(f"jcode:      {total_jcode_io}")
    if total_claude_io > 0:
        io_pct_diff = ((total_jcode_io - total_claude_io) / total_claude_io) * 100
        print(f"Difference: {total_jcode_io - total_claude_io:+} ({io_pct_diff:+.1f}%)")

    # Check if within acceptable bounds
    # jcode using fewer tokens is always good (negative diff)
    # jcode using more tokens is acceptable up to MAX_OVERHEAD_PCT
    MAX_OVERHEAD_PCT = 50  # Allow up to 50% more tokens (for different system prompts)

    passed = True
    if pct_diff <= 0:
        print(f"\n✅ PASS: jcode uses {abs(pct_diff):.1f}% fewer tokens than Claude CLI")
    elif pct_diff <= MAX_OVERHEAD_PCT:
        print(f"\n✅ PASS: jcode uses {pct_diff:.1f}% more tokens (within {MAX_OVERHEAD_PCT}% threshold)")
    else:
        print(f"\n❌ FAIL: jcode uses {pct_diff:.1f}% more tokens (exceeds {MAX_OVERHEAD_PCT}% threshold)")
        passed = False

    # Per-prompt breakdown
    print("\nPer-prompt breakdown:")
    print(f"{'Prompt':<40} {'Claude':<10} {'jcode':<10} {'Diff':<10}")
    print("-" * 70)

    for r in results:
        prompt = r["prompt"][:37] + "..." if len(r["prompt"]) > 40 else r["prompt"]
        c_total = r["comparison"]["claude"]["total"]
        j_total = r["comparison"]["jcode"]["total"]
        diff = j_total - c_total
        print(f"{prompt:<40} {c_total:<10} {j_total:<10} {diff:+<10}")

    return passed


def main():
    parser = argparse.ArgumentParser(
        description="Compare token usage between jcode and Claude Code CLI"
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Show detailed output for each run",
    )
    parser.add_argument(
        "--runs", "-n",
        type=int,
        default=1,
        help="Number of runs per prompt (default: 1)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Output results as JSON",
    )

    args = parser.parse_args()

    print("Token Usage Comparison: jcode vs Claude Code CLI")
    print("=" * 50)

    try:
        results = run_test_suite(verbose=args.verbose, runs=args.runs)

        if args.json:
            print(json.dumps(results, indent=2))
            # For JSON mode, check if all runs succeeded
            sys.exit(0 if results else 1)
        else:
            passed = summarize_results(results)
            sys.exit(0 if passed else 1)

    except FileNotFoundError as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("\nInterrupted by user")
        sys.exit(130)


if __name__ == "__main__":
    main()
