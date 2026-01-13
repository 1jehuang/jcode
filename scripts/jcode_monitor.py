#!/usr/bin/env python3
"""
jcode Live Monitor - Real-time activity dashboard

Connects to jcode's debug socket and displays live streaming events.
Run jcode serve in one terminal, then this monitor in another.

Usage: ./jcode_monitor.py [--socket PATH]
"""

import json
import os
import socket
import sys
import time
from dataclasses import dataclass, field
from typing import Optional

# ANSI color codes
class Colors:
    RESET = "\033[0m"
    BOLD = "\033[1m"
    DIM = "\033[2m"

    RED = "\033[31m"
    GREEN = "\033[32m"
    YELLOW = "\033[33m"
    BLUE = "\033[34m"
    MAGENTA = "\033[35m"
    CYAN = "\033[36m"
    WHITE = "\033[37m"

    BG_BLUE = "\033[44m"

    # Clear screen and move cursor
    CLEAR = "\033[2J\033[H"
    CLEAR_LINE = "\033[2K"


@dataclass
class MonitorState:
    """Current state of the monitor"""
    connected: bool = False
    session_id: str = ""
    is_processing: bool = False
    input_tokens: int = 0
    output_tokens: int = 0
    current_text: str = ""
    active_tools: dict = field(default_factory=dict)  # id -> name
    tool_history: list = field(default_factory=list)  # recent tool completions
    events_received: int = 0
    last_event_time: float = 0
    errors: list = field(default_factory=list)


def get_socket_path() -> str:
    """Get the jcode debug socket path"""
    runtime_dir = os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")
    return os.path.join(runtime_dir, "jcode-debug.sock")


def connect_to_socket(path: str) -> Optional[socket.socket]:
    """Connect to the Unix socket"""
    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(path)
        sock.setblocking(False)
        return sock
    except Exception as e:
        return None


def send_request(sock: socket.socket, request: dict) -> bool:
    """Send a JSON request to the socket"""
    try:
        data = json.dumps(request) + "\n"
        sock.send(data.encode())
        return True
    except:
        return False


def read_events(sock: socket.socket) -> list:
    """Read available events from socket (non-blocking)"""
    events = []
    buffer = ""

    try:
        while True:
            data = sock.recv(4096)
            if not data:
                break
            buffer += data.decode()

            while "\n" in buffer:
                line, buffer = buffer.split("\n", 1)
                if line.strip():
                    try:
                        events.append(json.loads(line))
                    except json.JSONDecodeError:
                        pass
    except BlockingIOError:
        pass
    except:
        pass

    return events


def format_tokens(n: int) -> str:
    """Format token count with color based on size"""
    if n == 0:
        return f"{Colors.DIM}0{Colors.RESET}"
    elif n < 1000:
        return f"{Colors.GREEN}{n}{Colors.RESET}"
    elif n < 10000:
        return f"{Colors.YELLOW}{n:,}{Colors.RESET}"
    else:
        return f"{Colors.RED}{n:,}{Colors.RESET}"


def format_tool(name: str, status: str = "active") -> str:
    """Format a tool name with appropriate color"""
    if status == "active":
        return f"{Colors.CYAN}{Colors.BOLD}{name}{Colors.RESET}"
    elif status == "done":
        return f"{Colors.GREEN}{name}{Colors.RESET}"
    elif status == "error":
        return f"{Colors.RED}{name}{Colors.RESET}"
    return name


def truncate(s: str, max_len: int) -> str:
    """Truncate string with ellipsis"""
    if len(s) <= max_len:
        return s
    return s[:max_len-3] + "..."


def render_dashboard(state: MonitorState, width: int = 80):
    """Render the monitoring dashboard"""
    lines = []

    # Header
    header = f" JCODE MONITOR "
    padding = (width - len(header)) // 2
    lines.append(f"{Colors.BG_BLUE}{Colors.WHITE}{Colors.BOLD}{' ' * padding}{header}{' ' * padding}{Colors.RESET}")
    lines.append("")

    # Connection status
    if state.connected:
        status = f"{Colors.GREEN}CONNECTED{Colors.RESET}"
        session = f" | Session: {Colors.DIM}{state.session_id[:8]}...{Colors.RESET}" if state.session_id else ""
    else:
        status = f"{Colors.RED}DISCONNECTED{Colors.RESET}"
        session = ""
    lines.append(f"  Status: {status}{session}")

    # Processing state
    if state.is_processing:
        proc = f"{Colors.YELLOW}{Colors.BOLD}PROCESSING{Colors.RESET}"
    else:
        proc = f"{Colors.DIM}idle{Colors.RESET}"
    lines.append(f"  State:  {proc}")
    lines.append("")

    # Token usage
    lines.append(f"  {Colors.BOLD}Tokens{Colors.RESET}")
    lines.append(f"    Input:  {format_tokens(state.input_tokens)}")
    lines.append(f"    Output: {format_tokens(state.output_tokens)}")
    lines.append("")

    # Active tools
    lines.append(f"  {Colors.BOLD}Active Tools{Colors.RESET}")
    if state.active_tools:
        for tool_id, tool_name in state.active_tools.items():
            lines.append(f"    {Colors.CYAN}>{Colors.RESET} {format_tool(tool_name)}")
    else:
        lines.append(f"    {Colors.DIM}(none){Colors.RESET}")
    lines.append("")

    # Recent tool completions
    lines.append(f"  {Colors.BOLD}Recent Tools{Colors.RESET}")
    if state.tool_history:
        for item in state.tool_history[-5:]:
            name, success, output = item
            status = "done" if success else "error"
            output_preview = truncate(output.replace("\n", " "), 40)
            lines.append(f"    {format_tool(name, status)}: {Colors.DIM}{output_preview}{Colors.RESET}")
    else:
        lines.append(f"    {Colors.DIM}(none){Colors.RESET}")
    lines.append("")

    # Current streaming text
    lines.append(f"  {Colors.BOLD}Streaming Text{Colors.RESET}")
    if state.current_text:
        # Show last few lines of streaming text
        text_lines = state.current_text.split("\n")[-4:]
        for tl in text_lines:
            lines.append(f"    {Colors.WHITE}{truncate(tl, width-6)}{Colors.RESET}")
    else:
        lines.append(f"    {Colors.DIM}(waiting...){Colors.RESET}")
    lines.append("")

    # Stats
    lines.append(f"  {Colors.DIM}Events: {state.events_received} | Last: {time.time() - state.last_event_time:.1f}s ago{Colors.RESET}")

    # Errors
    if state.errors:
        lines.append("")
        lines.append(f"  {Colors.RED}{Colors.BOLD}Errors{Colors.RESET}")
        for err in state.errors[-3:]:
            lines.append(f"    {Colors.RED}{truncate(err, width-6)}{Colors.RESET}")

    # Print with clear
    print(Colors.CLEAR, end="")
    print("\n".join(lines))


def process_event(event: dict, state: MonitorState):
    """Process a single event and update state"""
    state.events_received += 1
    state.last_event_time = time.time()

    event_type = event.get("type", "")

    if event_type == "ack":
        state.is_processing = True

    elif event_type == "text_delta":
        state.current_text += event.get("text", "")
        # Keep last 2000 chars
        if len(state.current_text) > 2000:
            state.current_text = state.current_text[-2000:]

    elif event_type == "tool_start":
        tool_id = event.get("id", "")
        tool_name = event.get("name", "unknown")
        state.active_tools[tool_id] = tool_name

    elif event_type == "tool_exec":
        pass  # Tool is executing, still active

    elif event_type == "tool_done":
        tool_id = event.get("id", "")
        tool_name = event.get("name", "unknown")
        output = event.get("output", "")
        error = event.get("error")

        # Remove from active
        state.active_tools.pop(tool_id, None)

        # Add to history
        state.tool_history.append((tool_name, error is None, output[:100] if output else "(empty)"))
        # Keep last 20
        state.tool_history = state.tool_history[-20:]

    elif event_type == "tokens":
        state.input_tokens = event.get("input", 0)
        state.output_tokens = event.get("output", 0)

    elif event_type == "done":
        state.is_processing = False
        state.current_text = ""  # Clear for next turn

    elif event_type == "error":
        state.errors.append(event.get("message", "unknown error"))
        state.errors = state.errors[-10:]

    elif event_type == "pong":
        pass  # Health check response

    elif event_type == "state":
        state.session_id = event.get("session_id", "")
        state.is_processing = event.get("is_processing", False)

    elif event_type == "session":
        state.session_id = event.get("session_id", "")


def main():
    """Main monitor loop"""
    socket_path = sys.argv[1] if len(sys.argv) > 1 else get_socket_path()

    print(f"Connecting to {socket_path}...")

    state = MonitorState()
    sock = None
    request_id = 1
    last_ping = 0

    while True:
        try:
            # Connect if needed
            if sock is None:
                sock = connect_to_socket(socket_path)
                if sock:
                    state.connected = True
                    # Subscribe to events
                    send_request(sock, {"type": "subscribe", "id": request_id})
                    request_id += 1
                    # Get initial state
                    send_request(sock, {"type": "state", "id": request_id})
                    request_id += 1
                else:
                    state.connected = False

            # Read events
            if sock:
                events = read_events(sock)
                for event in events:
                    process_event(event, state)

                # Periodic ping
                if time.time() - last_ping > 5:
                    send_request(sock, {"type": "ping", "id": request_id})
                    request_id += 1
                    last_ping = time.time()

            # Render dashboard
            render_dashboard(state)

            # Small delay
            time.sleep(0.1)

        except KeyboardInterrupt:
            print(f"\n{Colors.DIM}Exiting...{Colors.RESET}")
            break
        except BrokenPipeError:
            state.connected = False
            sock = None
        except Exception as e:
            state.errors.append(str(e))
            time.sleep(1)

    if sock:
        sock.close()


if __name__ == "__main__":
    main()
