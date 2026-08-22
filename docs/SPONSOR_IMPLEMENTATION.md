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
