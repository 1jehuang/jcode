#!/usr/bin/env python3
"""Bridge for J-Code to call the Claude Agent SDK.

Reads a single JSON request from stdin, runs a Claude Agent SDK query, and
streams JSON messages to stdout (one per line).
"""

from __future__ import annotations

import json
import signal
import sys
import time
from typing import Any, AsyncIterator, Dict, Iterable, List, Optional


def _write_output(payload: dict) -> bool:
    """Write JSON to stdout, returning False if pipe is broken."""
    try:
        sys.stdout.write(json.dumps(payload) + "\n")
        sys.stdout.flush()
        return True
    except BrokenPipeError:
        return False

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


def _serialize_assistant_message(message: AssistantMessage) -> tuple[Dict[str, Any], bool, bool]:
    """Serialize an AssistantMessage.

    Returns: (payload, has_thinking, has_non_thinking)
    """
    blocks: List[Dict[str, Any]] = []
    has_thinking = False
    has_non_thinking = False

    for block in message.content:
        # SDK block objects use class names like TextBlock, ToolUseBlock, ThinkingBlock
        # Check class name to determine type
        class_name = type(block).__name__

        if class_name == "TextBlock" or hasattr(block, "text") and not hasattr(block, "thinking"):
            blocks.append({"type": "text", "text": block.text})
            has_non_thinking = True
        elif class_name == "ToolUseBlock":
            blocks.append(
                {
                    "type": "tool_use",
                    "id": block.id,
                    "name": block.name,
                    "input": block.input,
                }
            )
            has_non_thinking = True
        elif class_name == "ToolResultBlock":
            blocks.append(
                {
                    "type": "tool_result",
                    "tool_use_id": block.tool_use_id,
                    "content": block.content,
                    "is_error": block.is_error,
                }
            )
            has_non_thinking = True
        elif class_name == "ThinkingBlock":
            # Thinking blocks are internal reasoning - skip content but track timing
            has_thinking = True
        # Also handle legacy type attribute format
        elif hasattr(block, "type"):
            block_type = block.type
            if block_type == "text":
                blocks.append({"type": "text", "text": block.text})
                has_non_thinking = True
            elif block_type == "tool_use":
                blocks.append(
                    {
                        "type": "tool_use",
                        "id": block.id,
                        "name": block.name,
                        "input": block.input,
                    }
                )
                has_non_thinking = True
            elif block_type == "tool_result":
                blocks.append(
                    {
                        "type": "tool_result",
                        "tool_use_id": block.tool_use_id,
                        "content": block.content,
                        "is_error": block.is_error,
                    }
                )
                has_non_thinking = True
    return {"type": "assistant_message", "content": blocks, "model": message.model}, has_thinking, has_non_thinking


def _serialize_result_message(message: ResultMessage) -> Dict[str, Any]:
    return {
        "type": "result",
        "is_error": message.is_error,
        "usage": message.usage,
        "result": message.result,
        "structured_output": message.structured_output,
        "session_id": message.session_id,  # Include session_id for resume support
    }


def _serialize_user_message(message: UserMessage) -> Optional[Dict[str, Any]]:
    """Serialize a UserMessage - mainly for tool_result blocks."""
    blocks: List[Dict[str, Any]] = []

    for block in message.content:
        class_name = type(block).__name__

        if class_name == "ToolResultBlock":
            blocks.append({
                "type": "tool_result",
                "tool_use_id": block.tool_use_id,
                "content": block.content,
                "is_error": block.is_error,
            })
        elif hasattr(block, "type") and block.type == "tool_result":
            blocks.append({
                "type": "tool_result",
                "tool_use_id": block.tool_use_id,
                "content": block.content,
                "is_error": getattr(block, "is_error", False),
            })

    if blocks:
        return {"type": "user_message", "content": blocks}
    return None


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

    # Always use our own system prompt as a plain string (never use Claude Code preset)
    # The SDK accepts either a string or SystemPromptPreset dict, we use string
    system_value: Optional[str] = system_prompt.strip() if system_prompt.strip() else "You are an AI coding assistant."

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
    # When starting fresh with history (e.g., after reload), format as context
    # When starting fresh without history, stream messages normally
    has_assistant_messages = any(msg.get("role") == "assistant" for msg in messages)

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
    elif has_assistant_messages:
        # Can't send assistant messages to SDK without resume - format as context
        # This happens after reload when we have conversation history but no valid session
        history_parts = []
        last_user_msg = ""
        for msg in messages:
            role = msg.get("role", "")
            content = msg.get("content", [])
            if isinstance(content, str):
                text = content
            elif content:
                texts = [b.get("text", "") for b in content if b.get("type") == "text"]
                text = "\n\n".join(texts)
            else:
                text = ""

            if role == "user":
                last_user_msg = text
                history_parts.append(f"User: {text}")
            elif role == "assistant":
                history_parts.append(f"Assistant: {text}")

        # Format: provide history as context, then the actual request
        if len(history_parts) > 1:
            # Has actual history - format as context
            history_context = "\n\n".join(history_parts[:-1])  # All but last
            prompt_value = f"<conversation_history>\n{history_context}\n</conversation_history>\n\n{last_user_msg}"
        else:
            prompt_value = last_user_msg
    else:
        prompt_value = _stream_messages(messages)

    thinking_start: Optional[float] = None
    in_thinking_block: bool = False
    thinking_done_emitted: bool = False

    # Track query start time - thinking happens during the API call
    query_start = time.time()
    saw_thinking = False

    async for message in query(prompt=prompt_value, options=claude_options):
        payload: Optional[Dict[str, Any]] = None
        if isinstance(message, StreamEvent):
            event = message.event
            event_type = event.get("type", "")

            # Track thinking timing from stream events
            if event_type == "content_block_start":
                block = event.get("content_block", {})
                block_type = block.get("type")
                if block_type == "thinking":
                    thinking_start = time.time()
                    in_thinking_block = True
                    saw_thinking = True
                elif block_type == "text" and thinking_start is not None and not thinking_done_emitted:
                    # Text block started - emit thinking duration
                    elapsed = time.time() - thinking_start
                    thinking_payload = {"type": "thinking_done", "duration_secs": elapsed}
                    if not _write_output(thinking_payload):
                        return  # Pipe closed, exit cleanly
                    thinking_done_emitted = True
            elif event_type == "content_block_stop" and in_thinking_block:
                in_thinking_block = False

            payload = _serialize_stream_event(message)
        elif isinstance(message, AssistantMessage):
            payload, has_thinking, has_non_thinking = _serialize_assistant_message(message)
            # Track thinking from AssistantMessage
            if has_thinking:
                saw_thinking = True
            # Emit thinking duration when we see non-thinking content after thinking
            # Use time from query start since thinking happens during API call
            if has_non_thinking and saw_thinking and not thinking_done_emitted:
                elapsed = time.time() - query_start
                thinking_done_emitted = True
                # Emit thinking duration event
                thinking_payload = {"type": "thinking_done", "duration_secs": elapsed}
                if not _write_output(thinking_payload):
                    return  # Pipe closed, exit cleanly
        elif isinstance(message, ResultMessage):
            payload = _serialize_result_message(message)
        elif isinstance(message, SystemMessage):
            # Check for compaction boundary
            if hasattr(message, 'subtype') and message.subtype == 'compact_boundary':
                compact_meta = getattr(message, 'compact_metadata', {}) or {}
                payload = {
                    "type": "compaction",
                    "trigger": compact_meta.get("trigger", "unknown"),
                    "pre_tokens": compact_meta.get("pre_tokens"),
                }
            else:
                payload = None
        elif isinstance(message, UserMessage):
            # UserMessage contains tool_result blocks when SDK executes tools
            payload = _serialize_user_message(message)

        if payload is not None:
            if not _write_output(payload):
                return  # Pipe closed, exit cleanly


def main() -> None:
    # Exit cleanly on broken pipe instead of raising exception
    signal.signal(signal.SIGPIPE, signal.SIG_DFL)
    try:
        anyio.run(_run)
    except BrokenPipeError:
        # Parent closed the pipe - exit silently
        sys.exit(0)
    except Exception as exc:  # pragma: no cover - surfaced to Rust caller
        error_payload = {"type": "error", "message": str(exc), "kind": exc.__class__.__name__}

        # Extract rate limit info if available
        # Anthropic SDK errors may have response headers with retry-after
        if hasattr(exc, 'response'):
            response = exc.response
            if hasattr(response, 'headers'):
                retry_after = response.headers.get('retry-after')
                if retry_after:
                    try:
                        error_payload["retry_after_secs"] = int(retry_after)
                    except (ValueError, TypeError):
                        pass
            if hasattr(response, 'status_code'):
                error_payload["status_code"] = response.status_code

        # Also check for rate_limit_error in the error body
        if hasattr(exc, 'body') and isinstance(exc.body, dict):
            error_type = exc.body.get('error', {}).get('type', '')
            if error_type:
                error_payload["error_type"] = error_type

        _write_output(error_payload)
        raise


if __name__ == "__main__":
    main()
