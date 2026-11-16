# Cloud directories info widget

SSH/cloud TUI sessions show a **Cloud Directories** card alongside **Cloud VM** when terminal margins have room. Local sessions do not show it.

- **Agents** counts live Jcode agent sessions, including idle sessions and swarm workers. Multiple agents sharing a daemon process count separately. Connected clients are not additional agents.
- **Busy** counts those agents currently inside a turn, including tool execution. Both agent turn loops retain their streaming presence guard for the entire turn.
- `>` marks the current project. Current projects sort first, followed by busy projects, other live projects and inactive projects. Up to six rows are shown with an overflow count.
- Paths use VM-home-relative notation where possible. Count columns stay aligned, paths truncate by terminal display cells, and rows never wrap.

## Directory inventory

Cloud transfers preserve absolute repository paths, so there is no universal `/workspace` root. The read-only VM probe combines persisted transfer lease repository roots, the current project, live sessions' working directories, immediate home directories, immediate project siblings, and projects immediately inside `~/workspaces`, `~/projects` and `~/repos`. It canonicalizes symlink aliases and resolves Git subdirectories to their repository root. Existing directories with no live agents remain visible. It does not recursively enumerate system or source directories, and excludes hidden sibling directories unless a lease or live session identifies them as projects.

The per-session active PID registry is shared across daemons using the same VM Jcode home. Live PID checks remove exited owners, migration leases exclude sessions moved elsewhere, and metadata excludes debug/canary sessions without excluding swarm workers. Custom daemons with separate `JCODE_HOME` registries are outside that registry's scope.

Fresh idle sessions may not have persisted metadata yet. The attached session uses its authoritative current-directory hint. Other live sessions without metadata appear in a `(directory pending)` row until their directory is saved, rather than making the inventory unavailable or guessing their directory.

The collector requires Python 3 on the remote host and uses only its standard library. Large session transcripts are not deserialized: current journal metadata is preferred, otherwise the snapshot is memory-mapped and top-level scalar metadata extracted. Nested tool/transcript fields cannot supply the working directory or debug flags.

## Refresh and failure behavior

Collection runs off the render thread through bounded native SSH commands, with the existing five-second pause between telemetry cycles. The inventory has its own stale flag: failed reads retain the last good snapshot and never replace counts with zeros. Initial failures show **Directories unavailable**. Reconnecting also marks displayed counts stale.

The probe bounds directory scans, metadata reads, runtime and SSH output. Incomplete active-session scans fail rather than publish partial counts. Directory-row truncation is explicitly marked.

## Verification

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p test_cloud_directories_probe.py -v
justrust test -p jcode-tui --lib cloud_ -- --nocapture
justrust test -p jcode-tui --test cloud_widget_api
```
