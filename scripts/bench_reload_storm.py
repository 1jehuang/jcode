#!/usr/bin/env python3
"""Reload-storm benchmark: N clients resume N sessions concurrently.

Starts nothing; point it at a server socket. Each client sends a target-aware
Subscribe (exactly what a reconnecting TUI sends after a reload) and records
the time until the History event arrives.

    scripts/bench_reload_storm.py --socket /run/user/1000/jcode-bench.sock -n 40
"""
import argparse, asyncio, glob, json, os, statistics, time

def pick_sessions(n, home):
    files = sorted(glob.glob(os.path.join(home, "sessions", "session_*.json")),
                   key=os.path.getmtime, reverse=True)
    ids = []
    for f in files:
        base = os.path.basename(f)[:-5]
        if base.count("_") >= 3 and not base.endswith((".agent-models", ".meta")):
            ids.append(base)
        if len(ids) >= n:
            break
    return ids

async def one(sock, sid, cwd, chunk):
    t0 = time.perf_counter()
    r, w = await asyncio.open_unix_connection(sock, limit=1 << 30)
    req = {"type": "subscribe", "id": 1, "working_dir": cwd,
           "target_session_id": sid, "client_instance_id": f"bench_{sid}"}
    w.write((json.dumps(req) + "\n").encode()); await w.drain()
    first = None
    try:
        while True:
            line = await asyncio.wait_for(read_line(r, chunk), 60)
            if not line:
                return sid, None, "eof"
            if first is None:
                first = time.perf_counter() - t0
            if b'"type":"history"' in line[:200] or line.startswith(b'{"type":"history"'):
                return sid, time.perf_counter() - t0, len(line)
            if b'"type":"error"' in line[:100]:
                return sid, None, line[:200].decode(errors="replace")
    except asyncio.TimeoutError:
        return sid, None, "timeout"
    finally:
        w.close()

async def read_line(r, chunk):
    """Read one line using reads of at most `chunk` bytes, like a client
    with a fixed-size read buffer (yields to the loop between reads)."""
    if chunk <= 0:
        return await r.readline()
    buf = bytearray()
    while True:
        data = await r.read(chunk)
        if not data:
            return bytes(buf)
        buf += data
        i = buf.find(b"\n")
        if i >= 0:
            # push back the remainder
            r._buffer[0:0] = buf[i + 1:]
            return bytes(buf[: i + 1])
        await asyncio.sleep(0)

async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--socket", required=True)
    ap.add_argument("-n", type=int, default=40)
    ap.add_argument("--home", default=os.path.expanduser("~/.jcode"))
    ap.add_argument("--chunk", type=int, default=0, help="client read size in bytes (0 = unbounded)")
    a = ap.parse_args()
    ids = pick_sessions(a.n, a.home)
    t0 = time.perf_counter()
    res = await asyncio.gather(*(one(a.socket, s, os.getcwd(), a.chunk) for s in ids))
    wall = time.perf_counter() - t0
    ok = sorted(t for _, t, _ in res if t is not None)
    bad = [(s, e) for s, t, e in res if t is None]
    ms = lambda x: f"{x*1000:.0f}ms"
    if ok:
        q = lambda p: ok[min(len(ok)-1, int(len(ok)*p))]
        print(f"history n={len(ok)} p50={ms(q(.5))} p90={ms(q(.9))} max={ms(ok[-1])} wall={ms(wall)}")
    for s, e in bad[:5]:
        print("FAIL", s, e)

asyncio.run(main())
