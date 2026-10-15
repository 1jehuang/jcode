#!/usr/bin/env python3
"""Inspect CLI help only; never launch an agent session or read credentials."""
import json
import shutil
import subprocess

def inspect(name):
    executable = shutil.which(name)
    if executable is None:
        return {"available": False}
    try:
        result = subprocess.run(
            [executable, "--help"], capture_output=True, text=True,
            timeout=15, check=False,
        )
        return {
            "available": True,
            "exit_code": result.returncode,
            "help_stdout": result.stdout[:32768],
            "help_stderr": result.stderr[:8192],
        }
    except subprocess.TimeoutExpired:
        return {"available": True, "error": "help timed out after 15 seconds"}
    except OSError as error:
        return {"available": True, "error": type(error).__name__}

if __name__ == "__main__":
    print(json.dumps({
        "probe_version": 1,
        "scope": "CLI help only; output is local and is not uploaded",
        "xirp": inspect("xirp"),
        "jcode": inspect("jcode"),
    }, indent=2))
