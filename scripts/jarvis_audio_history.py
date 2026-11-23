#!/usr/bin/env python3
"""Control local Jarvis audio history. Never receives audio or transcripts."""
import argparse
import json
import os
from pathlib import Path
import socket
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['status', 'on', 'off', 'clear', 'replay', 'previous'])
    parser.add_argument('index', nargs='?', choices=['1', '2', '3'])
    parser.add_argument('--pid', type=int, help='select one Jarvis client explicitly')
    args = parser.parse_args()
    if args.index and args.command != 'replay':
        parser.error('only replay accepts a clip index')
    root = Path(os.environ['XDG_RUNTIME_DIR']) / 'jcode-jarvis'
    active = []
    for path in root.glob('*.json'):
        try:
            data = json.loads(path.read_text())
            pid = data['pid']
            if args.pid is not None and pid != args.pid:
                continue
            info = data['counters']['audio_history']
            name = info['socket']
            if not name or Path(name).name != name or path.stem != str(pid):
                continue
            peer = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
            peer.connect(str(root / name))
            active.append((peer, path, info))
        except (TypeError, KeyError, ValueError, OSError):
            continue
    if len(active) != 1:
        raise SystemExit('Expected one live Jarvis history endpoint; use --pid if multiple are active.')
    peer, path, info = active[0]
    if args.command != 'status':
        command = ' '.join(filter(None, [args.command, args.index]))
        old = info
        peer.send(command.encode('ascii'))
        deadline = time.monotonic() + 4
        while time.monotonic() < deadline:
            info = json.loads(path.read_text())['counters']['audio_history']
            if info['socket'] != old['socket']:
                raise SystemExit('Jarvis session changed during command')
            if info['commands'] != old['commands']:
                break
            time.sleep(.05)
        else:
            raise SystemExit('Command sent; no acknowledgment. Check status before retrying.')
    print(json.dumps(info))
    if args.command != 'status' and info['last'] not in {'history on', 'history off', 'history cleared', 'playing', 'played'}:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
