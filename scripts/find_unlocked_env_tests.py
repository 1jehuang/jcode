#!/usr/bin/env python3
"""Find tests that mutate the process-global JCODE_HOME without the shared lock.

Rust runs tests in parallel threads of one process, so any test that changes
JCODE_HOME changes it for every concurrently running test. `storage::lock_test_env`
is the repo's convention for serializing those. A test that mutates the variable
without holding that lock can swap the home directory out from under an unrelated
test mid-assertion, which is the shape of jcode-tui's intermittent failures.

Reports each offending test function so they can be fixed rather than retried.
"""
import re
import subprocess
import sys

CRATE = sys.argv[1] if len(sys.argv) > 1 else "crates/jcode-tui/src"

files = subprocess.run(
    ["grep", "-rl", "JCODE_HOME", "--include=*.rs", CRATE],
    capture_output=True, text=True,
).stdout.split()

MUTATES = re.compile(r'(set_var\s*\(\s*"JCODE_HOME"|set_path\s*\(\s*"JCODE_HOME"|remove_var\s*\(\s*"JCODE_HOME")')
FN_START = re.compile(r"^\s*(?:async\s+)?fn\s+([A-Za-z0-9_]+)")
LOCK = re.compile(r"lock_test_env\s*\(\)")

offenders = []
for path in files:
    lines = open(path).read().split("\n")
    # Walk functions; a function "holds the lock" if it calls lock_test_env, or
    # if it delegates to a helper in the same file that does.
    helpers_with_lock = set()
    current, body = None, []
    def flush(name, body, record):
        if name is None:
            return
        text = "\n".join(body)
        if MUTATES.search(text) and not LOCK.search(text):
            record.append((path, name, text))
        if LOCK.search(text):
            helpers_with_lock.add(name)

    raw = []
    for line in lines:
        m = FN_START.match(line)
        if m:
            flush(current, body, raw)
            current, body = m.group(1), [line]
        else:
            body.append(line)
    flush(current, body, raw)

    for path_, name, text in raw:
        # A function calling a same-file helper that locks is fine.
        if any(re.search(rf"\b{re.escape(h)}\s*\(", text) for h in helpers_with_lock):
            continue
        offenders.append((path_, name))

for path, name in offenders:
    print(f"{path}: {name}")
print(f"\n{len(offenders)} function(s) mutate JCODE_HOME without the shared test-env lock")
