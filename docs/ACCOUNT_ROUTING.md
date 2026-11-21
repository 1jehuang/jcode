# Account routing priorities

Fresh sessions select a ready account from the highest available priority tier in
the automatic account pool. A saved default model is preferred on that account
when supported. It does not pin new sessions to the last-used login. Explicit
CLI/environment provider selections still take precedence, and existing sessions
keep their current account.

Lower numeric values have higher priority. Account rows communicate priority
through dimming only, without numeric badges. Accounts in the same tier keep their pool order. Failover
cycles within a tier before considering lower-priority accounts. Exhausted or
unavailable accounts are skipped while a ready account exists.

## Automatic defaults

### Manual account hot-swapping

In chat, press `Ctrl+Alt+J` for the next saved OAuth account or `Ctrl+Alt+K`
for the previous one. Cycling wraps at either end, traverses Claude accounts
then OpenAI accounts, and preserves your input draft. These shortcuts switch
immediately using the normal account-switch path. They skip API keys and
management actions such as add, replace, and remove. Remote sessions use only
the daemon-reported account inventory, never local credentials.

The header's active-account marker follows the daemon's confirmed session
credential, not the global preferred login or an unacknowledged request. A
same-provider account switch preserves the selected model. If a response is
still running, the switch waits until the session is idle. Repeated presses
continue from the latest requested account while that switch is pending.

### Plan-based priorities

The plan label is read from locally stored account metadata, without a network
request in the rendering or routing path:

- Claude: the saved `subscription_type`, populated by the profile fetch from
  `organization.organization_type`. Known Max rate-limit tiers distinguish 5x
  and 20x. An explicit `organization.subscription_type`, if provided, wins.
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
shows the plan when known and labels unknown plans explicitly. Normalized
brightness makes lower-priority identities subtly dimmer, never brighter than
their original colors, while
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

## KCode account pool

### Per-product pool files and enrolment

The account pool file is per product. A `kcode` binary never reads jcode's
pool, and jcode never reads kcode's pool. Keep each product's pool file under
its own product home directory.

OAuth accounts auto-enrol. Claude Max and OpenAI Pro/ProMax default to priority
1; standard or unknown plans default to priority 2; known free plans and
metered API keys default to priority 3. Manual overrides in kcode's own
`account-pool.json` win over detection. API keys stay manual until explicitly
added to the pool. Multiple keys for one provider are separate pool members and
rotate independently.

To enrol API keys from a dotenv-style `KEY=value` file, use the helper:

```text
python scripts/enroll_kcode_pool_keys.py <keys-file> --provider <provider> --out <outside-repo-output.json>
```

Use a descriptive, non-secret file name of your choosing for the input. The
helper reads that file, emits one tier-3 member per distinct value with
`enrolled=true`, and writes only to the caller-supplied output path. It refuses
an output path inside the repository working tree, never prints key values, and
stores only a short SHA-256 digest in each member id, never the key material.
Keep both the input and output files outside the repository.

### Routing and failover semantics

`resolve_account` selects the highest available priority tier for a fresh
session; it never pins a new session to the last login. Explicit CLI/environment
provider selections still take precedence. Failover cycles through members in
the current tier before considering a lower tier. An exhausted route is marked
unavailable with `mark_unavailable`. A 429 rotates to the next member in the
same tier by marking the exhausted id unavailable and calling `resolve_account`
again. If no member remains available, `resolve_account` returns no selection.
