# Replace a cloud checkout without moving a conversation

Run `jcode cloud replace --dry-run` to preview, then `jcode cloud replace --yes`
from inside your local Git repository to apply. Explicit `--yes` is required
before the cloud checkout can be changed. Dry-run validates the incoming snapshot
and checks ownership without replacing cloud files or creating a backup. Use
`--host <ssh-alias>` to choose a host, or use the same configured/managed destination as
`jcode cloud move`. `--remote-binary`, `--transport`, and `--json` are supported.
Both machines need a version of jcode supporting this command.

Inside a local Jcode session, use `/cloud replace` to see usage,
`/cloud replace --dry-run` to preview, and `/cloud replace --yes` to apply.
Append `--host <ssh-alias>` for a specific destination. The command waits for
the current turn to finish and uses the session's local working directory.
It does not move the conversation, close Jcode, or discard drafts.

```sh
jcode cloud replace --dry-run
jcode cloud replace --yes
```

The repository is restored at its local absolute root path on the cloud host.
An absent checkout is created. An existing checkout is first moved, in its
entirety, to a sibling named `<repo>.jcode-backup-<unique-id>`. The command prints
this recoverable backup path, or returns it in the JSON `backup_path` field. Backups
are never automatically deleted. This preserves cloud-only commits, ignored
files, untracked files and Git metadata. Restore a backup by stopping sessions
using the checkout, moving the replacement elsewhere, then renaming the backup
to the original path.

The replacement includes committed history and the current tracked/untracked
non-ignored working tree. Uncommitted content remains uncommitted, although the
staged/unstaged split is not retained. As with cloud move, a repository must have
an initial commit. Ignored files (including ignored `.env` files), authentication
stores, the origin URL, conversation transcripts, and session ownership are not
transferred. Secrets already committed or present in non-ignored repository files
are repository content and will be copied. Audit those files before transfer.

The receiver rejects a checkout owned by any locally-owned cloud migration
lease, including a different session, or used by an active session. Return moved
sessions and stop other sessions using the repository first. Ownership is checked
again just before replacement, and replacement operations are serialized. Do not
start another session during replacement: daemon session startup does not share
the replacement lock. An SSH failure can leave a completed replacement whose
response was lost. Inspect the checkout and its sibling backups before retrying.
Symlink destinations, linked Git worktrees, home directories, Jcode state
directories and their ancestors are rejected to protect unrelated state.
