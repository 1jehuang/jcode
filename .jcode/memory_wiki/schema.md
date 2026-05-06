# Jcode Living Memory Schema

This memory backend follows the LLM Wiki pattern: immutable raw sources plus a maintained Markdown wiki.

## Rules

1. Raw sources under `raw/` are append-only source material. Read them, cite them, but do not rewrite them unless the user explicitly requests maintenance/redaction.
2. Wiki pages under `wiki/` are working memory. Update them when durable user preferences, project facts, decisions, procedures, conventions, entities, concepts, or recurring issues are learned.
3. Every important claim should cite provenance using `sources` frontmatter and/or inline references to `raw/...` paths.
4. Use YAML frontmatter for all wiki pages: `title`, `kind`, `scope`, `created_at`, `updated_at`, `sources`, `confidence`, and `status`.
5. Use wikilinks like `[[user/preferences]]` and `[[projects/<project_slug>/overview]]` for related pages.
6. Preserve conflicts. Do not silently overwrite contradictory information. Add notes to `open_questions.md` or a page section named `Conflicts`.
7. Keep `index.md` useful for navigation and `log.md` useful for audit.
8. Never store secrets, tokens, passwords, `.env` contents, private keys, credentials, or sensitive personal data.
9. Prefer concise, stable summaries over chat transcript dumps. Raw sources are for transcript-level detail.
10. Offline operation is required. Do not depend on network access to read or update the wiki.
