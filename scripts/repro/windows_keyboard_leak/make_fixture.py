"""Build a small, committable session fixture for the CI probe.

The real reproduction uses a local recorded session, which never leaves the machine
it was recorded on. CI only needs *a* session to resume, so this writes a minimal
one from the same schema: same keys, two short messages, no user content.
"""

from __future__ import annotations

import json
import sys
from datetime import datetime, timezone
from pathlib import Path

TEMPLATE = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
OUT = Path(sys.argv[2])


def stamp(offset: int) -> str:
    return datetime(2026, 1, 1, 0, 0, offset, tzinfo=timezone.utc).isoformat().replace("+00:00", "Z")


template = TEMPLATE
template["id"] = "session_probe_1735689600000_0123456789abcdef"
template["title"] = "probe fixture"
template["short_name"] = "probe"
template["created_at"] = stamp(0)
template["updated_at"] = stamp(2)
template["last_active_at"] = stamp(2)
template["working_dir"] = None
template["is_debug"] = False
template["is_canary"] = False
template["saved"] = False
template["status"] = "Closed"
template["parent_id"] = None
template["last_pid"] = None
template["model_usage_turn_id"] = None
template["env_snapshots"] = []
template["messages"] = [
    {
        "id": "msg_probe_0001",
        "role": "user",
        "content": "Composer regression fixture: this session exists so a probe can resume it.",
        "display_role": "user",
        "timestamp": stamp(0),
    },
    {
        "id": "msg_probe_0002",
        "role": "assistant",
        "content": "Fixture loaded. The composer below is the surface under test.",
        "display_role": "assistant",
        "timestamp": stamp(1),
    },
]

OUT.parent.mkdir(parents=True, exist_ok=True)
OUT.write_text(json.dumps(template, indent=2), encoding="utf-8")
print(f"{OUT}  {OUT.stat().st_size} bytes")
