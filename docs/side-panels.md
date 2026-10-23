# Side panels

Side panels belong to a session, not to the selected model. Switching models or refreshing the model catalog keeps the current document, selection, and viewport. Content-only updates don't reopen a pane you've hidden.

## Open and navigate

Use the `panel` tool to spawn a Markdown or PDF panel. Clicking a repository-relative Markdown link also registers that document with the current session. The existing repository-path checks still apply, and an SSH client doesn't read a remote path from the local filesystem.

`Alt+M` cycles split view, fullscreen, and hidden. The header's close button hides the pane without deleting its documents. While the pane has keyboard focus, `Tab` and `Shift+Tab` cycle open documents. `Esc` returns keyboard focus to chat.

Use `/panels` to search recent documents in the current session. Entries show their source, state, and last opened/updated times. Selecting an open document restores its local view. Selecting an archived document reopens its saved body. On SSH, archived-document actions run on the attached server and are rejected if the session changes before the request arrives.

## Saved copies and freshness

Linked documents retain their last successfully loaded content. If the source disappears, becomes unreadable, or fails PDF validation, the panel shows the saved copy with a source-unavailable warning. A successful refresh clears that warning.

An update time records when content was successfully loaded. It doesn't claim that the document's assertions are still correct. A reconnect or recovery label explains why a saved copy may need checking. Legacy documents without a real update timestamp aren't assigned a fabricated age.

PDF safety limits remain 20 MiB per document and 32 MiB of decoded PDF payload per session snapshot. Recent-history listings contain metadata, not archived Markdown bodies or PDF payloads.

## Close, reopen, and delete

The `panel` tool's `close` action archives a document. `reopen` restores it, `history` lists archived metadata, and `purge` permanently removes the retained entry. Purging never deletes an external linked source file. UI dismissal remains a visibility change rather than an archive operation.

Archived history is bounded to 50 entries and 128 MiB of retained archive bodies per session. Old archived entries are evicted when either limit is reached. This isn't a revision timeline for every streamed update to one document.

Client reload preserves session-specific visibility, selection, fullscreen state, and per-document viewport without putting document bodies in the reload handoff. Switching sessions restores each session's own view. Ctrl+R recovery carries retained documents into the replacement session with recovery provenance. `/clear` starts a genuinely new, empty session; the old session's documents remain associated with that old session.
