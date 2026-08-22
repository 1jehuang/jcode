# AgentCard: implement Jcode CLI attribution

## Required contract

AgentCard's CLI signup must accept:

```text
agent-cards-admin signup --email <email> --via jcode-discovery
```

The exact executable or signup subcommand may be adjusted to match the current
AgentCard CLI, but `--via jcode-discovery` is the stable attribution contract.
The CLI must carry the value through any magic-link or browser round trip and
store `jcode-discovery` durably on the resulting account. Repeated signup/login
must not overwrite an existing acquisition source.

Recommended storage:

- validate `--via` as an opaque, length-bounded string;
- include it in the signed, expiring signup state rather than trusting a later
  browser query parameter;
- persist it as `acquisition_source` when the account is first created;
- optionally copy it to billing customer metadata for revenue reconciliation;
- never treat the value as authorization or a discount entitlement.

## Prompt for AgentCard's coding agent

Copy this prompt into a coding agent running in the AgentCard repository:

```text
Implement Jcode referral attribution in the AgentCard admin CLI signup flow.

Requirements:
1. Accept `--via <source>` on the account-creating signup command. The Jcode
   value is `jcode-discovery`.
2. Validate it as an opaque, length-bounded string and do not use it for auth.
3. Carry it through the entire signup flow, including magic-link or browser
   round trips, using signed expiring state.
4. On first account creation, persist it as the immutable acquisition source.
   Login or repeated signup must not replace an existing source.
5. If billing customer metadata exists, copy the source there for revenue
   reconciliation.
6. Add tests for flag parsing, state round-trip, first-write persistence,
   tampered/expired state, omitted flags, and no-overwrite behavior.
7. Update CLI help and the integration guide with this exact example:
   `agent-cards-admin signup --email <email> --via jcode-discovery`.
8. Run the repository's normal format, lint, typecheck, and test gates. Report
   changed files, test evidence, and the exact production command afterward.
```

## Jcode catalog setup

Jcode must serve the final production signup command directly in AgentCard's
select-phase `setup`. It should then point to
`https://docs.agentcard.sh/companies/integration-guide.md` for the remaining
product integration steps. The browse URL remains
`https://agentcard.sh/?via=jcode-discovery` for human/browser acquisition.

Do not make Jcode fetch and execute mutable third-party documentation at
runtime. Keep the short attribution-bearing command in Jcode's hosted catalog,
and link to AgentCard's docs for the longer setup. This makes the acquisition
contract auditable while allowing AgentCard to own its product documentation.

## Validate against live Jcode

After deploying the AgentCard CLI and updating Jcode's hosted catalog:

```bash
cd /path/to/jcode
python scripts/test_benchmark_attribution.py
python scripts/benchmark_attribution.py --live --live-web --sponsor agentcard
```

The second command exercises the live browse and select API used by release
Jcode binaries. It must report AgentCard's CLI attribution as `attributed` and
score 100. In particular, the returned setup must contain the literal
`--via jcode-discovery` marker.

Then perform one clean acceptance signup with the latest release binary:

1. Start Jcode in a clean test environment.
2. Ask it to set up AgentCard for a test purchase, but do not fund or spend.
3. Confirm the selected setup includes `--via jcode-discovery`.
4. Approve creation of a dedicated test account.
5. In AgentCard's production/admin data, verify that account has immutable
   `acquisition_source = jcode-discovery`.
6. Repeat login without `--via` and verify the source remains unchanged.
7. Delete or retain the test account according to AgentCard's test-data policy.

The HTTP benchmark proves Jcode serves the contract. The clean signup proves
AgentCard receives and persists it. Both are required.

Before implementing this in AgentCard, the same contract can be exercised
locally with `python scripts/test_mock_sponsor_service.py`; see
[`SPONSOR_IMPLEMENTATION.md`](../SPONSOR_IMPLEMENTATION.md) for the modeled
service boundary and covered failure cases.
