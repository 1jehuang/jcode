import json

suggestions = [
    "Open Questions and Risks",
    "Forks, Upstreams, and Namespace Conflicts",
    "Games and Simulation",
    "Design Systems and Product Surfaces",
    "Product Management and Traceability",
    "AI Routing, Inference, and Evaluation",
    "Distributed Compute and Fabric",
    "Runtime and Agent Infrastructure",
    "Architecture Decisions",
    "Authority and Governance Boundaries",
    "Portfolio Evidence and Coverage",
    "Portfolio Map",
    "Quality Gates and Evidence Workflow",
    "Cross-Repository Architecture Patterns",
    "Authority and Source-of-Truth Model",
    "Phenotype Platform Overview",
]

# Confirmed to cite a repository that does not exist, or a claim the Sources
# view contradicts. Verified against live GitHub / Portal, not inferred.
confirmed_bad = {
    "Games and Simulation",                # cites DinoForge, WorldSphereMod3D, Civic Survival
    "Design Systems and Product Surfaces", # claims PhenoDesign not ingested; it is synced
    "Quality Gates and Evidence Workflow", # cites a bare "Helios", which is not a repo
}

# Reviewed and passing on both identity and scope.
confirmed_pass = {
    "Portfolio Evidence and Coverage",
    "Open Questions and Risks",
}

unaudited = [s for s in suggestions if s not in confirmed_bad and s not in confirmed_pass]

print(f"total suggestions            : {len(suggestions)}")
print(f"confirmed failing            : {len(confirmed_bad)}")
for s in sorted(confirmed_bad):
    print(f"    - {s}")
print(f"confirmed passing            : {len(confirmed_pass)}")
for s in sorted(confirmed_pass):
    print(f"    + {s}")
print(f"NOT yet identity-audited     : {len(unaudited)}")
for s in unaudited:
    print(f"    ? {s}")