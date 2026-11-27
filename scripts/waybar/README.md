# Waybar session status

Install `jcode-sessions-status` into `~/.config/waybar/` and make it executable.
Configure a streaming custom module (do not set an interval):

```json
"custom/jcode-sessions": {
  "exec": "~/.config/waybar/jcode-sessions-status",
  "return-type": "json",
  "restart-interval": 5,
  "format": "{}",
  "tooltip": true
}
```

The existing spinner, session, and swarm counters remain local. The cloud icon
adds **streaming/alive**, for example `☁ 2/7`. The tooltip labels both counts.
Cloud counts include root, detached, and internal/swarm sessions on the account's
managed machine and running ephemeral machines. Arbitrary third-party SSH hosts
are not included.

Cloud snapshots read the same `active_pids` and `streaming_pids` registries used
by Jcode's presence indicator, checking PID liveness and counting sessions rather
than unique daemon PIDs. No debug-control permissions or daemon restarts are
needed. Streaming means a model response is generating, not merely that an agent
has a running lifecycle status. Dead-owner markers are ignored.

Cloud polling runs independently of local socket events, every 30 seconds after
the previous probe completes. API or SSH failures show `?/?`, not misleading zero
counts. Machine inventory/status queries are read-only. SSH is attempted only
for machines reported as running, using Jcode's existing private SSH configs and
host-key verification. The managed SSH proxy may authorize the connection, and
an SSH connection can affect the host's idle timer. No wake command is issued,
although a host stopping between the status check and proxy connection can race
with the proxy's normal wake behavior.

Requirements: `~/.local/bin/jcode`, account login, prepared cloud SSH configs,
SSH, and Python 3.9+ locally and remotely. The probe sends this public script to
Python over SSH without installing files on the cloud host.

Run regression tests:

```sh
python3 -m unittest discover -s scripts/waybar -v
```
