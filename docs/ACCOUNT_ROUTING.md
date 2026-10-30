# Account routing priorities

Fresh sessions select a ready account from the highest available priority tier in
the automatic account pool. A saved default model is preferred on that account
when supported. It does not pin new sessions to the last-used login. Explicit
CLI/environment provider selections still take precedence, and existing sessions
keep their current account.

Lower numeric values have higher priority. The TUI displays stored `0` as `P1`,
`1` as `P2`, and so on. Accounts in the same tier keep their pool order. Failover
cycles within a tier before considering lower-priority accounts. Exhausted or
unavailable accounts are skipped while a ready account exists.

## Automatic defaults

The plan label is read from locally stored account metadata, without a network
request in the rendering or routing path:

- Claude: the saved `subscription_type`, populated by the existing profile fetch.
- OpenAI: `chatgpt_plan_type` in the saved ID token, falling back to the access token.

Known Claude Max and OpenAI Pro/ProMax plans default to P1. Standard subscriptions
and unknown plans default to P2. Known free plans and metered API keys default to
P3. These are routing heuristics, not price or remaining-credit estimates. Unknown
plan names are not treated as premium plans. A plan hint never authorizes billing.

Manual priority overrides take precedence over detected subscription metadata.
Resetting an account's priority to automatic removes only its override, not its
pool membership. Setting a priority never enrolls an account in automatic routing.

In `/account`, select a credential and press `[` for higher priority, `]` for
lower priority, or `Ctrl+R` to restore automatic plan-based priority. The picker
shows the plan when known and labels unknown plans explicitly. Priority badges
and normalized brightness make lower-priority identities subtly dimmer while
status dots and usage-limit colors keep their existing meaning.

Missing Claude plan metadata is backfilled asynchronously through the normal
profile endpoint at startup. At most two requests run concurrently. Each account
is attempted no more than once per fifteen minutes per process, including failed
attempts, and no network request blocks startup routing or rendering.

Preferences are stored in `~/.jcode/account-pool.json`. Its optional `priorities`
object maps account keys such as `openai:openai-fox` or `openai-api` to zero-based
priority values. Older files without this object continue to load.

API keys remain manual unless explicitly added to the pool. Antigravity is also
manual by default. Exclusion takes precedence over membership if a file contains
conflicting entries. Accounts remain available for explicit manual selection.
