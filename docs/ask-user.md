# Structured user questions

The `ask_user` tool asks one to four questions, with two to four options per
question. Each question supports single or multiple selection, free-form Other
text, optional notes, and optional option previews. Headers are limited to 12
terminal display cells. Text fields are bounded to 8192 Unicode code points.

The tool is registered when a form-capable client is attached, including after
the provider tool snapshot was locked. Explicit tool allowlists still apply.
Headless sessions do not advertise it. After detaching, the tool remains
registered to preserve the provider cache; an invocation without an attached
form client returns a message asking the agent to use its normal reply.

The TUI shows an inline form. Use arrows and Tab/Shift+Tab to navigate, digits
for options, Space to toggle multiple choices, and Enter to advance and submit.
Other accepts free text; notes and previews have dedicated panels and controls
shown in the form. Text editing supports Unicode graphemes and bracketed paste.

Forms use a bounded flat JSON schema and session-scoped broker. Responses are
validated before settlement; the first valid response wins. Reconnecting clients
receive pending forms followed by an explicit replay boundary. Drafts survive
transport loss, and an unacknowledged submission can be reviewed and resent.
Session changes and resolved forms clear stale input state.

