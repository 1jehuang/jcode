"""Minimal jcode gateway WebSocket client (stdlib only).

Used to validate cross-machine remote sessions: pair over HTTP, then drive a
session on another host over the gateway WebSocket.
"""
import base64, json, os, socket, struct, time, urllib.request


def pair(host, port, code, device_id, device_name):
    req = urllib.request.Request(
        f"http://{host}:{port}/pair",
        data=json.dumps({"code": code, "device_id": device_id,
                         "device_name": device_name}).encode(),
        headers={"Content-Type": "application/json"})
    return json.loads(urllib.request.urlopen(req, timeout=10).read().decode())


def health(host, port):
    return json.loads(urllib.request.urlopen(
        f"http://{host}:{port}/health", timeout=10).read().decode())


class Gateway:
    def __init__(self, host, port, token, timeout=30):
        self.host, self.port = host, port
        self.sock = socket.create_connection((host, port), timeout=timeout)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall((
            f"GET /ws HTTP/1.1\r\nHost: {host}:{port}\r\n"
            "Upgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n"
            f"Authorization: Bearer {token}\r\n\r\n").encode())
        buf = b""
        while b"\r\n\r\n" not in buf:
            buf += self.sock.recv(4096)
        status = buf.split(b"\r\n")[0]
        if b"101" not in status:
            raise RuntimeError(f"handshake failed: {status!r}")
        self.buf = buf.split(b"\r\n\r\n", 1)[1]
        self._id = 0

    def _frame(self, op, payload=b""):
        mask = os.urandom(4)
        masked = bytes(payload[i] ^ mask[i % 4] for i in range(len(payload)))
        h = bytearray([0x80 | op])
        n = len(payload)
        if n < 126:
            h.append(0x80 | n)
        elif n < 65536:
            h.append(0x80 | 126); h += struct.pack(">H", n)
        else:
            h.append(0x80 | 127); h += struct.pack(">Q", n)
        self.sock.sendall(bytes(h) + mask + masked)

    def send(self, **req):
        self._id += 1
        req.setdefault("id", self._id)
        self._frame(0x1, json.dumps(req).encode())
        return req["id"]

    def events(self, deadline):
        """Yield decoded protocol events until deadline, answering pings."""
        while time.time() < deadline:
            while True:
                fr = self._take_frame()
                if fr is None:
                    break
                op, data = fr
                if op == 0x9:
                    self._frame(0xA, data); continue
                if op == 0x8:
                    return
                if op != 0x1:
                    continue
                for line in data.decode(errors="replace").splitlines():
                    if line.strip():
                        yield json.loads(line)
            self.sock.settimeout(max(0.1, deadline - time.time()))
            try:
                chunk = self.sock.recv(65536)
            except socket.timeout:
                return
            if not chunk:
                return
            self.buf += chunk

    def _take_frame(self):
        b = self.buf
        if len(b) < 2:
            return None
        op = b[0] & 0x0F
        n = b[1] & 0x7F
        off = 2
        if n == 126:
            if len(b) < 4: return None
            n = struct.unpack(">H", b[2:4])[0]; off = 4
        elif n == 127:
            if len(b) < 10: return None
            n = struct.unpack(">Q", b[2:10])[0]; off = 10
        if len(b) < off + n:
            return None
        self.buf = b[off + n:]
        return op, b[off:off + n]

    def close(self):
        try: self._frame(0x8)
        except Exception: pass
        self.sock.close()
