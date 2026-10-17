# Vendor question — Xirp custom harness registration contract
**Date:** 2026-10-17
**For:** Spotify Xirp / @chirp/squab team
**From:** Koosha Pari (Jcode maintainer)
**Status:** DRAFT (operator must paste into the channel of their choice)
**Full evidence:** `docs/XIRP_CUSTOM_HARNESS.md` in `KooshaPari/Jcode` master

---

## Short version (paste this)

Subject: Request for a custom coding-agent harness registration contract in Xirp

Hello,

I maintain Jcode, an open-source ACP-compliant coding agent. I would like to
register Jcode as a first-class custom harness in Xirp, alongside the five
vendors that ship with `@chirp/squab` 0.11.11 (`claude`, `codex`, `gemini`,
`pi`, `cursor`).

What I have found so far on the installed build (`xirp 0.49.0+app.db5402dc7`,
`/Applications/Xirp.app/Contents/Resources/app.asar`):

- `squab --available-harnesses` returns a fixed catalog of 5 harnesses. Jcode
  is absent.
- `xirp api send agents:list` returns the same 5 entries, all
  `"visibility": "public"`, all with hard-coded per-agent flags
  (`--launch-claude`, `--launch-codex`, etc.).
- The daemon message catalog (`xirp api list`) only exposes mutators
  (`harness:install|update|uninstall`) that act on harnesses already in the
  fixed catalog. There is no `harness:register` or `agents:create`.

What I would like to know:

1. **Is there a supported way to register a custom harness** (e.g., Jcode)
   without forking `@chirp/squab`?
2. If not, would Spotify accept a **generic ACP client adapter** in
   `@chirp/squab` so any ACP-compliant agent can be registered via a single
   config entry? ACP is a stable open spec and the Jcode side already
   implements it.
3. If neither is supported today, is a **vendor extension contract** (e.g.,
   an internal `@chirp/squab` plugin API) on the roadmap? If so, what
   timeline?

A full evidence dossier is in the Jcode repository at
`docs/XIRP_CUSTOM_HARNESS.md` (referenced from PR `KooshaPari/KCode#103`).
The dossier includes probe output, message-catalog enumeration, and an
honest record of one withdrawn sub-question (a session-list artifact that
turned out to be a project-registration issue, not a squab bug).

If the right channel for this is not direct email, I am happy to file it
wherever you prefer (issue tracker, customer support form, etc.). Please
let me know the channel and I will re-route.

Thank you for your time.

— Koosha Pari

---

## Full evidence dossier

The Jcode repository has a full evidence dossier at
`docs/XIRP_CUSTOM_HARNESS.md` in `KooshaPari/Jcode` master. The dossier
covers:

- XJ-01 (closed): no custom-harness registration contract exists at the
  current build. Evidence: 5 lines of cross-confirmation (squab
  --available-harnesses, xirp api send agents:list, daemon message catalog,
  AppBundle inspection, cross-checked against published docs).
- XJ-02 through XJ-07: blocked pending the answer to (1)-(3) above.
- Sub-question (g) (withdrawn): an earlier session-list artifact that
  appeared to be a squab bug turned out to be a project-registration issue
  on the Jcode side. Withdrawn rather than filed as a defect.

PR `KooshaPari/KCode#103` is a draft PR that captures the full
audit-correction arc; the body of that PR is the most recent summary of
what was learned.

---

## Operator action required

**I do not have a verified Spotify / Xirp contact channel from the agent
side.** xirp has no `feedback`/`support`/`contact` subcommand, the AppBundle
does not advertise a support URL, and the user-facing docs
(`https://backstage.spotify.com/docs/xirp`) do not list a contact email.
Past contact (if any) is not in agent knowledge.

The operator should:

1. Pick a channel (e.g., direct email, Spotify support form, GitHub
   issues on a public squab repo, etc.).
2. Paste the "Short version" above into that channel.
3. Send.

Or, if the operator has a prior contact at Spotify, paste the short
version into a reply to that thread.

---

## Channel options (2026-10-17 subagent investigation)

The `vendor-contact-search` subagent
(`session_mizaru_1792219502394_01b378b9830eb204`) was dispatched to find
a real contact channel and exhausted the following paths. **No public
contact channel exists for Xirp.** The product is Spotify-internal:

- `CFBundleIdentifier = "com.spotify.xirp"`, author = "Spotify"
- Internal codebase: `ghe.spotify.net` (Spotify's GitHub Enterprise;
  not externally accessible)
- Update CDN: `https://reckless-finch.spotifycdn.com/external/`
- `@chirp/*` packages (squab, daemon, etc.) are private (not on npm)
- No public GitHub repo for any `@chirp` package
- The Xirp CLI has no `feedback`/`support`/`contact` subcommand
- The Xirp AppBundle does not advertise a support URL
- `backstage.spotify.com/docs/xirp` does not list a contact email
- Web search for "Spotify Xirp developer support" returns no relevant hits

**Operator channel options, ranked by practicality:**

1. **Personal Slack/email to a Spotify contact** (Xirp team, Portal
   team, developer-relations, or any colleague with Xirp visibility).
   The operator is a `Portal Admins` member per STATE.md §B6, so
   internal channels are accessible. **This is the recommended path.**
2. **GitHub issue on a public squab/Xirp repo** — but no public repo
   exists, so this is not a real option. (Documented for completeness.)
3. **Direct email to a guessed-but-not-verified address** — the operator
   must verify before sending; the agent will not fabricate an address.
4. **Xirp app's built-in `update` flow** — the app has a built-in
   updater; the operator could in principle include a message with an
   update log. Not a real support channel, but documents the
   investigation exhaustively.

**Once a channel is chosen, paste the "Short version" verbatim from
the section above. Do not edit the questions — they were tuned against
the XJ-01 negative result and the 5-line proof in
`docs/XIRP_CUSTOM_HARNESS.md`.**
