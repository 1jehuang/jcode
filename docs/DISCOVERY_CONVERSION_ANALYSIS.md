# Discovery conversion analysis (2026-07-25)

Data sources:

- `jcode-telemetry` D1 (`discovery_details` joined to `events`) for client-side attempts,
  including attempts that never reached the endpoint.
- `jcode-subscriptions` D1 (`discovery_request_events`, `discovery_events`,
  `discovery_suggestions`) for server-side funnel, raw `query`/`reason` text, and
  provenance classification.

Traffic is filtered to `benchmark_run = 0` and, where noted, to
`provenance_class IN ('likely-user','unverified')` so self-dev, internal, and benchmark
runs do not inflate demand.

## 1. Headline numbers

| metric | value |
|---|---|
| distinct users with any session, last 7d | 41,838 |
| distinct users who invoked `discover_tools`, last 7d | 444 (1.1%) |
| real browse units (session x category, likely-user/unverified) | 460 |
| units reaching `select` (setup fetch) | 8 (1.7%) |
| units ending in `suggest` (agent says the catalog has no fit) | 265 (58%) |
| units that just stop with no select and no suggest | 187 (41%) |
| distinct users blocked by `sponsors.enabled = false` | 168 |

Two separate problems: the tool is called by ~1% of users, and when it is called it almost
never converts because the browse returns a product that does not match the request.

## 2. Blocker: 168 users have discovery hard-disabled

`discovery_details` records 168 failures with `failure_reason = 'disabled'`, one per distinct
user, i.e. 27% of the 625 users who ever tried discovery. Client versions are mostly
post-flip (0.54.4: 93, 0.58.0: 35), so these are persisted `[sponsors] enabled = false`
entries in `~/.jcode/config.toml`, not the current default.

Root cause: discovery shipped opt-in in `203a3f3d3` (v0.36.x) with `enabled = false`, and
`Config::save()` serializes the entire struct with `toml::to_string_pretty`. Any config write
during that window froze `enabled = false` into the user's file. The opt-out flip in
`e226b84c4` only changed the in-code default, so those files still disable the tool forever
and the agent sees no `discover_tools` at all (`tool/mod.rs:293` skips registration).

## 3. Funnel by category (real traffic, session x category units)

| category | units | select | suggest | silent drop |
|---|---|---|---|---|
| ai-models | 66 | 0 | 37 | 29 |
| other | 60 | 1 | 28 | 31 |
| integration-platforms | 58 | 0 | 28 | 30 |
| deployment | 44 | 0 | 37 | 7 |
| browser-automation | 30 | 1 | 13 | 16 |
| web-data | 27 | 2 | 9 | 16 |
| web-search | 26 | 0 | 11 | 15 |
| cloud-infrastructure | 24 | 0 | 20 | 4 |
| email-messaging | 20 | 0 | 12 | 8 |
| code-review | 18 | 3 | 9 | 6 |
| databases | 17 | 0 | 14 | 3 |
| payments | 15 | 1 | 10 | 4 |

13 of 18 categories were empty in the catalog when this data was collected, so 60%+ of all
browses returned zero results. Even the five stocked categories convert in the single digits.
A `financial-data` category was added after this analysis (it is stocked server-side as
empty), which does not change the ratio: the deployed catalog now has 5 stocked of 19.

## 4. Why the three stocked categories do not convert

Bucketed from the raw `query` text of likely-user/unverified browses (85 browses in these
three categories; 18 of them returned zero results because they predate the listing).

### payments (15 units, 1 select)

The catalog has exactly one entry: Agentcard, prepaid virtual Visa cards for agents.
Actual demand:

- 9 queries want merchant billing, not agent spending: hosted checkout, recurring
  subscriptions, customer portal, signed webhooks, payment links, Stripe live-mode product
  administration, marketplace escrow with split payouts.
- 4 queries want regional or platform rails: Razorpay lookups, WeChat Pay v3, Toss Payments
  merchant onboarding, Google Play Billing / RevenueCat subscription testing.
- 1 query wants provider billing introspection ("read my API credits/charges").
- 1 query is a genuine virtual-card match, and it converted.

Named-product gap suggestions in payments: Stripe Billing, Stripe Connect, Stripe,
Razorpay, Google Play Billing, Toss Payments.

### code-review (18 units, 3 selects)

Catalog: Greptile, repository-aware PR review, which requires Node 22, a global npm install,
an interactive `greptile login`, and a `greptile onboard` wizard the agent cannot complete.

- 9 queries are actually Git host authentication, not review: push to a denied upstream,
  fork and open a PR, private Gitea push, GitLab MR discussions and inline comments,
  GitHub issue access.
- 7 queries want an independent local reviewer for uncommitted diffs, usually because the
  swarm reviewer failed to spawn, or want SonarQube/Ponytail/OpenCode Orcal specifically.
  Greptile cannot review an uncommitted local diff without onboarding a repo.
- 3 selects came from the queries whose wording matched "repository-aware PR review".

The dominant pattern here is a failure of a jcode feature (swarm reviewer unavailable,
recursive spawning disabled) leaking into discovery as a workaround attempt.

### web-data (27 units, 2 selects)

Catalog: context.dev, scraping and structured extraction. Demand:

- 12 queries want a specific data source, not a generic scraper: YouTube/Bilibili
  transcripts, Danish land registry, Google Merchant Center, ACM full-text by DOI, Polygon.io
  equities, Figma files, Mobbin screens, reverse phone lookup, ETF history, stock video APIs.
- 4 queries want a search-engine API because `websearch` was blocked by anti-bot pages.
- 2 queries want GitHub MCP repo access.
- 11 are miscellaneous one-off enrichment or extraction asks.
- Both selects came from queries phrased as generic scraping/extraction.

## 5. Most requested missing products (all categories, named suggestions)

railway 6, playwright 5, vercel 5, supabase 4, github 3, gitlab 3, coolify 3,
hitl-notary MCP 3, notion 3, litellm 2, LM Studio 2, cloudflare 2 (+workers 2, R2 2,
api 2), github MCP server 2, slack MCP 2, linear 2, figma MCP 2, polygon.io 2, agentmail 2.

The agent is asking for infrastructure and integration MCPs, not for sponsored products.

## 6. Actionable conclusions

1. Repair the 168 stuck configs. Treat a persisted `sponsors.enabled = false` written before
   the opt-out flip as unset, or migrate it once on load. Also stop `Config::save()` from
   writing default-valued sections back to disk, which is what froze the flag.
2. Fill the empty categories or shrink the category list. 14 empty categories mean most
   browses are guaranteed misses, and the agent then spends a second call on `suggest`.
   Adding a category without a listing makes the miss rate worse, not better: every new
   empty category is another guaranteed zero-result browse plus a follow-up `suggest`.
3. Broaden payments beyond agent-issued cards. Merchant billing is 60% of payments demand
   and currently has no listing at all.
4. Split code-review from Git host access. Most code-review browses are auth/access asks;
   a GitHub/GitLab listing would serve them, and Greptile's interactive onboarding is a
   hard blocker for agent-completed setup.
5. Fix the upstream feature failures that generate discovery traffic: swarm reviewer spawn
   failures and blocked `websearch` account for a large share of code-review and web-data
   browses.
6. Instrument setup completion. `discovery_usage` is empty, so nothing after `select` is
   observable. Without it, "select" is the only conversion proxy we have.
