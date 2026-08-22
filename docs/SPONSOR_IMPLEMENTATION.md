# Sponsor implementation

This is the sponsor-facing entry point for implementing and validating Jcode
Discovery attribution. Each sponsor gets a tailored page containing the exact
contract its product must support, a coding-agent prompt, and live validation.

## Responsibility split

- The sponsor owns signup behavior and durable storage of the attribution.
- Jcode owns the select-phase `setup` text that agents follow.
- The sponsor's docs remain the authority for installing and using the product.
- Jcode's setup must include the attribution-bearing signup command directly.
  Linking to sponsor docs alone is insufficient because docs can change and an
  agent may follow the CLI path without opening a referral URL.
- A sponsor may also document the attributed command on its own site. That is
  useful defense in depth, but it does not replace the catalog setup marker.

## Tailored implementations

- [AgentCard CLI attribution](sponsors/agentcard.md)

## Acceptance gate

Before a campaign is considered attributable:

1. The sponsor implements and tests its attribution contract.
2. The live Jcode select response includes the exact configured marker.
3. `python scripts/benchmark_attribution.py --live --live-web --sponsor TOOL`
   reports `CLI attribution: attributed` and a score of 100.
4. A clean end-to-end signup through a release Jcode binary creates a sponsor
   record with the expected acquisition source.

The internal catalog and rollout runbook remains
[Sponsored discovery sponsor onboarding](SPONSORED_DISCOVERY_SPONSOR_ONBOARDING.md).

## Exercise the reference implementation

`scripts/mock_sponsor_service.py` is a runnable reference sponsor boundary. It
exposes a real local HTTP signup API and account API plus CLI commands for
signup, magic-link confirmation, and account inspection. Attribution is carried
inside signed, expiring, one-use state and persisted in SQLite on first account
creation.

Run the end-to-end acceptance suite:

```bash
python scripts/test_mock_sponsor_service.py
```

The suite invokes the public CLI in subprocesses and crosses the HTTP boundary.
It verifies that `--via jcode-discovery` survives confirmation, the acquisition
source is immutable, an omitted flag stays unattributed, tampered and expired
state is rejected, and a magic link cannot be reused. This reference proves the
proposed contract is implementable. Sponsor production acceptance still
requires the live catalog benchmark and a test account in the sponsor's system.
It also exposes a discovery-compatible browse/select endpoint and runs the real
`benchmark_attribution.py` entry point against it, including marked public-web
URL resolution. The integrated reference must score 100 with CLI attribution
reported as `attributed`.
The end-to-end suite also fetches the select response, extracts the exact setup
command Jcode would hand to an agent, executes that command through the public
CLI, confirms its magic link, and observes `acquisition_source =
jcode-discovery` through the public account API. This prevents the catalog and
sponsor-flow tests from passing independently while disagreeing at their shared
command boundary.
