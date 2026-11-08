#!/usr/bin/env python3
"""Read-only GitHub signing readiness. Reports names, never secret values."""
import argparse
import json
import subprocess

REPO = "1jehuang/jcode"
ENVIRONMENT = "native-handoff-signing"
SIGNING_SECRETS = (
    "APPLE_CERTIFICATE_P12_BASE64", "APPLE_CERTIFICATE_PASSWORD", "APPLE_SIGNING_IDENTITY",
    "APPLE_NOTARY_KEY_P8_BASE64", "APPLE_NOTARY_KEY_ID", "APPLE_NOTARY_ISSUER",
)


def github(path):
    # All calls are explicit GETs. No secret-value, create/update, or dispatch API.
    try:
        result = subprocess.run(["gh", "api", "--method", "GET", "--paginate", "--slurp", path],
                                capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise LookupError("GitHub CLI unavailable or timed out") from error
    if result.returncode:
        status = "not found or not visible" if "404" in result.stderr else "not readable with current gh authorization"
        raise LookupError(status)
    try:
        pages = json.loads(result.stdout)
    except ValueError as error:
        raise LookupError("unexpected GitHub response") from error
    if not isinstance(pages, list) or not pages or not all(isinstance(page, dict) for page in pages):
        raise LookupError("unexpected GitHub response")
    return pages


def names(path):
    return {secret["name"] for page in github(path) for secret in page.get("secrets", [])}


def assess(environment, policies, secret_names):
    reviewers = any(rule.get("type") == "required_reviewers" and rule.get("reviewers")
                    for rule in environment.get("protection_rules", []))
    branch_policy = environment.get("deployment_branch_policy") or {}
    master_only = (branch_policy.get("custom_branch_policies") is True
                   and branch_policy.get("protected_branches") is False
                   and len(policies) == 1
                   and policies[0].get("name") == "master"
                   and policies[0].get("type") == "branch")
    missing = sorted(set(SIGNING_SECRETS) - set(secret_names))
    return {"environment": ENVIRONMENT, "required_reviewers": bool(reviewers),
            "prevent_self_review": bool(environment.get("protection_rules") and any(
                rule.get("type") == "required_reviewers" and rule.get("prevent_self_review")
                for rule in environment["protection_rules"])),
            "master_only_deployments": master_only, "missing_environment_secret_names": missing,
            "ready": bool(reviewers and master_only and not missing)}


def inspect():
    report = {"repository": REPO, "read_only": True,
              "note": "Secret names only. This cannot validate credential values, Apple access, or certificate validity."}
    try:
        report["hosted_arm_key_configured"] = "JUSTRUST_API_KEY" in names(f"repos/{REPO}/actions/secrets?per_page=100")
    except LookupError as error:
        report["hosted_arm_key_configured"] = None
        report["hosted_arm_check"] = str(error)
    try:
        environment = github(f"repos/{REPO}/environments/{ENVIRONMENT}")[0]
        policies = [policy for page in github(
            f"repos/{REPO}/environments/{ENVIRONMENT}/deployment-branch-policies?per_page=100")
                    for policy in page.get("branch_policies", [])]
        secret_names = names(f"repos/{REPO}/environments/{ENVIRONMENT}/secrets?per_page=100")
        report["signing"] = assess(environment, policies, secret_names)
    except LookupError as error:
        report["signing"] = {"environment": ENVIRONMENT, "ready": False, "check": str(error),
                             "required_environment_secret_names": list(SIGNING_SECRETS)}
    return report


def main():
    argparse.ArgumentParser(description=__doc__).parse_args()
    report = inspect()
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["signing"]["ready"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
