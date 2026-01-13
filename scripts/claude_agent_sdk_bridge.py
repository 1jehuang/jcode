#!/usr/bin/env python3
"""Bridge for J-Code to call the Claude Agent SDK.

Reads a single JSON request from stdin, runs a Claude Agent SDK query, and
streams JSON messages to stdout (one per line).
"""

from __future__ import annotations

import json
import sys
from typing import Any, AsyncIterator, Dict, Iterable, List, Optional

import anyio
from claude_agent_sdk import ClaudeAgentOptions, query
from claude_agent_sdk.types import (
    AssistantMessage,
    ResultMessage,
    StreamEvent,
    SystemMessage,
    UserMessage,
)


def _to_cli_content_blocks(blocks: Iterable[Dict[str, Any]]) -> List[Dict[str, Any]]:
    converted: List[Dict[str, Any]] = []
    for block in blocks:
        block_type = block.get("type")
        if block_type == "text":
            converted.append({"type": "text", "text": block.get("text", "")})
        elif block_type == "tool_use":
            converted.append(
                {
                    "type": "tool_use",
                    "id": block.get("id"),
                    "name": block.get("name"),
                    "input": block.get("input", {}),
                }
            )
        elif block_type == "tool_result":
            converted.append(
                {
                    "type": "tool_result",
                    "tool_use_id": block.get("tool_use_id"),
                    "content": block.get("content"),
                    "is_error": block.get("is_error"),
                }
            )
    return converted


def _to_cli_message(message: Dict[str, Any]) -> Dict[str, Any]:
    role = message.get("role")
    content = message.get("content", [])

    if isinstance(content, str):
        content_value: Any = content
    else:
        converted = _to_cli_content_blocks(content)
        if converted and all(block.get("type") == "text" for block in converted):
            text_parts = [block.get("text", "") for block in converted]
            content_value = "\n\n".join(text_parts)
        else:
            content_value = converted

    return {
        "type": role,  # Use role as type: "user" or "assistant"
        "message": {"role": role, "content": content_value},
    }


def _serialize_assistant_message(message: AssistantMessage) -> Dict[str, Any]:
    blocks: List[Dict[str, Any]] = []
    for block in message.content:
        # SDK block objects use class names like TextBlock, ToolUseBlock, ThinkingBlock
        # Check class name to determine type
        class_name = type(block).__name__

        if class_name == "TextBlock" or hasattr(block, "text") and not hasattr(block, "thinking"):
            blocks.append({"type": "text", "text": block.text})
        elif class_name == "ToolUseBlock":
            blocks.append(
                {
                    "type": "tool_use",
                    "id": block.id,
                    "name": block.name,
                    "input": block.input,
                }
            )
        elif class_name == "ToolResultBlock":
            blocks.append(
                {
                    "type": "tool_result",
                    "tool_use_id": block.tool_use_id,
                    "content": block.content,
                    "is_error": block.is_error,
                }
            )
        elif class_name == "ThinkingBlock":
            # Thinking blocks are internal reasoning - skip them
            pass
        # Also handle legacy type attribute format
        elif hasattr(block, "type"):
            block_type = block.type
            if block_type == "text":
                blocks.append({"type": "text", "text": block.text})
            elif block_type == "tool_use":
                blocks.append(
                    {
                        "type": "tool_use",
                        "id": block.id,
                        "name": block.name,
                        "input": block.input,
                    }
                )
            elif block_type == "tool_result":
                blocks.append(
                    {
                        "type": "tool_result",
                        "tool_use_id": block.tool_use_id,
                        "content": block.content,
                        "is_error": block.is_error,
                    }
                )
    return {"type": "assistant_message", "content": blocks, "model": message.model}


def _serialize_result_message(message: ResultMessage) -> Dict[str, Any]:
    return {
        "type": "result",
        "is_error": message.is_error,
        "usage": message.usage,
        "result": message.result,
        "structured_output": message.structured_output,
        "session_id": message.session_id,  # Include session_id for resume support
    }


def _serialize_stream_event(message: StreamEvent) -> Dict[str, Any]:
    return {"type": "stream_event", "event": message.event}


async def _stream_messages(messages: List[Dict[str, Any]]) -> AsyncIterator[Dict[str, Any]]:
    for msg in messages:
        yield _to_cli_message(msg)


async def _run() -> None:
    request = json.load(sys.stdin)

    messages = request.get("messages", [])
    system_prompt = request.get("system", "") or ""
    tools = request.get("tools", [])
    options = request.get("options", {}) or {}

    permission_mode = options.get("permission_mode")
    model = options.get("model")
    cli_path = options.get("cli_path")
    cwd = options.get("cwd")
    include_partial_messages = options.get("include_partial_messages", True)
    extra_args = options.get("extra_args") or {}
    resume_session_id = options.get("resume")  # Session ID to resume
    max_thinking_tokens = options.get("max_thinking_tokens")  # Extended thinking budget

    if permission_mode == "bypassPermissions":
        extra_args = dict(extra_args)
        extra_args.setdefault("allow-dangerously-skip-permissions", None)

    if system_prompt.strip():
        system_value: Optional[Dict[str, Any]] = {
            "type": "preset",
            "preset": "claude_code",
            "append": system_prompt,
        }
    else:
        system_value = {"type": "preset", "preset": "claude_code"}

    claude_options = ClaudeAgentOptions(
        tools=tools if tools else None,
        allowed_tools=tools if tools else [],
        system_prompt=system_value,
        permission_mode=permission_mode,
        model=model,
        cli_path=cli_path,
        cwd=cwd,
        include_partial_messages=include_partial_messages,
        extra_args=extra_args,
        resume=resume_session_id,  # Resume previous session if provided
        max_thinking_tokens=max_thinking_tokens,  # Extended thinking for Opus models
    )

    # When resuming, only send the last user message as a simple string
    # When starting fresh, send all messages in streaming format
    if resume_session_id and messages:
        # Find the last user message for the prompt
        last_user_msg = None
        for msg in reversed(messages):
            if msg.get("role") == "user":
                content = msg.get("content", [])
                if isinstance(content, str):
                    last_user_msg = content
                elif content:
                    # Extract text from content blocks
                    texts = [b.get("text", "") for b in content if b.get("type") == "text"]
                    last_user_msg = "\n\n".join(texts)
                break
        prompt_value: Any = last_user_msg or ""
    else:
        prompt_value = _stream_messages(messages)

    async for message in query(prompt=prompt_value, options=claude_options):
        payload: Optional[Dict[str, Any]] = None
        if isinstance(message, StreamEvent):
            payload = _serialize_stream_event(message)
        elif isinstance(message, AssistantMessage):
            payload = _serialize_assistant_message(message)
        elif isinstance(message, ResultMessage):
            payload = _serialize_result_message(message)
        elif isinstance(message, (SystemMessage, UserMessage)):
            payload = None

        if payload is not None:
            sys.stdout.write(json.dumps(payload) + "\n")
            sys.stdout.flush()


def main() -> None:
    try:
        anyio.run(_run)
    except Exception as exc:  # pragma: no cover - surfaced to Rust caller
        error_payload = {"type": "error", "message": str(exc), "kind": exc.__class__.__name__}
        sys.stdout.write(json.dumps(error_payload) + "\n")
        sys.stdout.flush()
        raise


if __name__ == "__main__":
    main()
