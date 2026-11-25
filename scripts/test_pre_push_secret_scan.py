#!/usr/bin/env python3
"""Exercise the real push hook with a generated dummy key in a disposable repo."""
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile


hook = Path(__file__).resolve().parent / "git-hooks/pre-push"
assert shutil.which("gitleaks"), "Gitleaks is required"
identity = {
    key: subprocess.check_output(["git", "config", "--get", key], text=True).strip()
    for key in ["user.name", "user.email"]
}
with tempfile.TemporaryDirectory(prefix="jcode-push-hook-") as directory:
    root = Path(directory)

    def git(*args):
        return subprocess.check_output(["git", *args], cwd=root, stderr=subprocess.PIPE, text=True).strip()

    def push(local, remote="0" * 40):
        return subprocess.run(
            ["sh", str(hook), "origin"], cwd=root, text=True, capture_output=True,
            input=f"refs/heads/master {local} refs/heads/master {remote}\n",
        ).returncode

    git("init", "-q")
    for key, value in identity.items():
        git("config", key, value)
    (root / "README").write_text("Safe initial commit\n")
    git("add", "README")
    git("commit", "-qm", "Safe fixture")
    safe = git("rev-parse", "HEAD")
    assert push(safe) == 0, "Safe new branch must pass"
    (root / ".env").write_text("OPENAI_API_KEY=sk-" + secrets.token_hex(24) + "\n")
    git("add", ".env")
    git("commit", "-qm", "Dummy credential fixture")
    unsafe = git("rev-parse", "HEAD")
    assert push(unsafe) != 0, "Dummy secret on a new branch must block the push"
    assert push(unsafe, safe) != 0, "Dummy secret on an existing branch must block the push"
    assert push("0" * 40, unsafe) == 0, "Ref deletion must pass"
print("Push hook passed safe-commit, new-branch secret, existing-branch secret, and deletion checks.")
