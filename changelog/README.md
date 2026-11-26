# Changelog entries

User-facing changelog entries for jcode releases. One JSON file per release,
written by the agent during `/cut-release`, reviewed in the release diff, and
consumed by jcode.sh and the GitHub release body.

## Files

- `v<version>.json` - one entry per release (e.g. `v0.34.0.json`).
- `index.json` - newest-first list of released versions with dates, so
  consumers can discover entries without directory listings.
- `discord/v<version>.json` - optional custom Discord announcement override.
  Omit it for normal single-message releases so Discord automatically posts the
  full GitHub release notes rendered from `v<version>.json`. Only add one when
  a custom (usually multi-part) Discord post is genuinely needed, and if one
  already exists when editing `v<version>.json`, update or remove it in the same
  commit so Discord never posts a stale override.

## Entry schema

```json
{
  "version": "0.34.0",
  "date": "2026-07-02",
  "title": "Optional short release name",
  "highlights": [
    "One-sentence, user-facing description of the most important change."
  ],
  "improvements": [
    "Smaller user-visible improvements."
  ],
  "fixes": [
    "Bug fixes described by their user-visible effect."
  ]
}
```

## index.json schema

```json
{
  "entries": [
    { "version": "0.34.0", "date": "2026-07-02" }
  ]
}
```

## Writing guidelines

- Write for users of jcode, not contributors. Describe the effect, not the
  implementation ("swarm agents no longer lose retried commands", not
  "close mutation-dedup races").
- Skip internal-only changes entirely: refactors, CI, test-only, code moves.
  If a release is purely internal, say so in a single `improvements` item like
  "Internal reliability and performance work."
- One sentence per item. No trailing periods needed, but be consistent within
  an entry.
- `highlights` is for the 1-3 changes a user would actually notice or care
  about. Everything else goes in `improvements` or `fixes`. Omit empty arrays.
- `title` is optional. Use it only when a release has an obvious theme.
- Keep the full commit log as the source of truth; the changelog is a
  user-facing layer over it, never a replacement.
