# Sponsor attribution benchmark

`scripts/benchmark_attribution.py` scores, per sponsor, whether a signup or
install performed by an agent on the user's behalf is attributable to jcode.
It complements `scripts/benchmark_discovery.py`: that suite measures whether
Discovery triggers; this one measures whether the acquisition we drive is
actually credited to us.

## Why this exists

Catalog attribution today is mostly cookie-based referral links
(`?via=jcode-discovery`). That works when a human clicks the listing URL in a
browser. Most jcode-driven signups instead happen from an agent following the
select-phase `setup` instructions in a CLI, where no browser cookie is ever
set. Each sponsor's flow is different, so every sponsor gets its own
expectation entry and a 0-100 score.

## Checks and scoring

Each sponsor is scored over the applicable checks (skips are excluded):

| Check | Meaning |
|-------|---------|
| `catalog_entry_present` | The sponsor is in the live (or saved) catalog. |
| `listing_url_marked` | The browse URL carries the referral marker. |
| `setup_preserves_marker` | Setup never sends the user to an unmarked browser signup page on the vendor's own domain. Docs and API endpoints are exempt. |
| `cli_flow_attributable` | If setup is CLI-first, attribution must not depend solely on a browser cookie: either the setup routes account creation through the marked URL, or the sponsor declares a non-cookie mechanism. |
| `live_url_resolves` | (`--live-web`) The marked URL responds and redirects do not drop the marker. |

Score = 100 x passed / applicable. The run exits non-zero when any sponsor
scores below `--min-score` (default 100).

## Sponsor expectations

`scripts/attribution_benchmark_sponsors.json` declares, per sponsor, the
attribution `mechanism` and `marker`. Supported mechanisms:

- `referral-link`: cookie-based affiliate link (weakest for CLI flows);
- `signup-code`: a code the agent supplies during signup;
- `utm-forwarding`: the sponsor ingests query params server-side at signup;
- `api-partner-id`: a partner identifier embedded in the API/MCP setup.

A unit test enforces that every tool with a positive case in
`scripts/discovery_benchmark_cases.json` has an attribution expectation, so a
new sponsor cannot be onboarded without declaring how attribution works.

## Run it

```bash
# offline logic tests
python scripts/test_benchmark_attribution.py

# live against the discovery service, plus public-web URL verification
python scripts/benchmark_attribution.py --live --live-web

# one sponsor
python scripts/benchmark_attribution.py --live --sponsor greptile

# offline against a saved catalog with full entries (url/setup)
python scripts/benchmark_attribution.py --catalog-file catalog.json
```

Reports are written to `target/attribution-benchmark/latest.json`. Live
requests to the discovery service carry `x-jcode-discovery-benchmark: 1` so
they are excluded from sponsor reporting.

## Known gap (as of the first run)

All current sponsors score 80: their setups are CLI-first (MCP/API-key) but
their declared mechanism is cookie-based `referral-link`, so purely
agent-driven signups are not attributed. Closing this requires, per sponsor,
either a marked signup URL inside the setup instructions or a non-cookie
mechanism (signup code or partner ID), then updating the expectation file.
