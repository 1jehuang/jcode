"""Build a small, committable session fixture for the CI probe.

The real reproduction uses a local recorded session, which never leaves the machine
it was recorded on. CI only needs *a* session to resume, so this keeps the recorded
session's own message shapes and type-controlled fields (ids, roles, timestamps,
display roles) and only replaces the text inside each message's content blocks. That
way every enum and field type stays valid: a hand-written message object drifts from
the schema the moment a variant is renamed.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

source_path, out_path = Path(sys.argv[1]), Path(sys.argv[2])
template = json.loads(source_path.read_text(encoding="utf-8"))

KEEP_MESSAGES = 2
TEXTS = [
    "Composer regression fixture: this session exists so a probe can resume it.",
    "Fixture loaded. The composer below is the surface under test.",
]


def with_text(message: dict, text: str) -> dict:
    """Keep the message shape, replace only the text inside its content blocks."""
    blocks = message.get("content")
    if isinstance(blocks, list):
        replaced = [dict(block, text=text) for block in blocks if block.get("type") == "text"]
        if replaced:
            message["content"] = replaced[:1]
            return message
    message["content"] = [{"type": "text", "text": text}]
    return message


messages = [
    with_text(dict(message), TEXTS[index % len(TEXTS)])
    for index, message in enumerate(template.get("messages", [])[:KEEP_MESSAGES])
]
if not messages:
    raise SystemExit(f"{source_path} has no messages to derive a fixture from")

# Copy only fields that carry structure, never conversation content. A recorded
# session also holds `compaction`, `memory_injections`, `system_prompt` and other
# fields that contain summaries and recalled private text; copying the whole object
# and patching a few keys would publish them.
SAFE_FIELDS = (
    "created_at",
    "updated_at",
    "last_active_at",
    "provider_key",
    "model",
    "reasoning_effort",
    "env_snapshots",
)
template = {key: template[key] for key in SAFE_FIELDS if key in template}

template["id"] = "session_probe_1735689600000_0123456789abcdef"
template["title"] = "probe fixture"
template["short_name"] = "probe"
template["parent_id"] = None
template["working_dir"] = None
template["is_debug"] = False
template["is_canary"] = False
template["saved"] = False
template["status"] = "Closed"
template["messages"] = messages

out_path.parent.mkdir(parents=True, exist_ok=True)
out_path.write_text(json.dumps(template, indent=2), encoding="utf-8")
print(f"{out_path}  {out_path.stat().st_size} bytes  {len(messages)} messages")
