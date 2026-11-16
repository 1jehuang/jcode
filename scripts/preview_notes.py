"""Generate preview changelogs from published release baselines and exact commits."""
import json
import re
from urllib.parse import quote


def change_notes(repository, sha, tag, gh):
    pages = json.loads(gh("api", "--paginate", "--slurp",
                          f"repos/{repository}/releases?per_page=100"))
    releases = [release for page in pages for release in page
                if not release.get("draft") and release.get("tag_name") != tag]
    number = int(tag.rsplit(".", 1)[1])
    previews = [release for release in releases
                if re.fullmatch(r"v1\.0\.0-preview\.[1-9][0-9]*", release.get("tag_name", ""))
                and int(release["tag_name"].rsplit(".", 1)[1]) < number]
    if previews:
        baseline = max(previews, key=lambda release: int(release["tag_name"].rsplit(".", 1)[1]))
    else:
        stable = [release for release in releases if not release.get("prerelease")]
        if not stable:
            raise ValueError("No published release baseline for preview changelog")
        baseline = max(stable, key=lambda release: release.get("published_at") or "")
    base = baseline["tag_name"]
    comparison = f"{quote(base, safe='')}...{sha}"
    pages = json.loads(gh("api", "--paginate", "--slurp",
                          f"repos/{repository}/compare/{comparison}?per_page=100"))
    commits = []
    seen = set()
    for page in pages:
        for commit in page["commits"]:
            commit_sha = commit["sha"]
            if not re.fullmatch(r"[0-9a-f]{40}", commit_sha):
                raise ValueError("Invalid commit SHA in preview comparison")
            if commit_sha not in seen:
                seen.add(commit_sha)
                commits.append((commit_sha, commit["commit"]["message"].splitlines()[0]))
    total = pages[0]["total_commits"]
    if len(commits) != total:
        raise ValueError("Incomplete preview comparison, refusing misleading changelog")
    features = sum(bool(re.match(r"(?:feat(?:\([^)]*\))?!?:|add\b|implement\b)", subject, re.I))
                   for _, subject in commits)
    fixes = sum(bool(re.match(r"(?:fix(?:\([^)]*\))?!?:|fix\b|repair\b|resolve\b)", subject, re.I))
                for _, subject in commits)
    url = f"https://github.com/{repository}/compare/{comparison}"
    lines = ["## What changed", "",
             f"{total} commits since **{base}**: {features} feature/addition commits, "
             f"{fixes} fixes, and {total - features - fixes} other changes "
             "(classified from commit titles).", "",
             f"[Full comparison]({url})", ""]
    highlights = []
    for pattern in (r"(?:feat(?:\([^)]*\))?!?:|add\b|implement\b)",
                    r"(?:fix(?:\([^)]*\))?!?:|fix\b|repair\b|resolve\b)"):
        # Two recent titles per category keep the summary grounded and brief.
        highlights.extend([subject for _, subject in reversed(commits)
                           if re.match(pattern, subject, re.I)][:2])
    if highlights:
        lines.extend(["**Highlights from commit titles**", ""])
        for subject in highlights[:4]:
            literal = ''.join('\\' + char if char in '\\`*_{}[]()<>#' else char for char in subject)
            lines.append(f"- {literal}")
        lines.append("")
    lines.extend(["## Commits", ""])
    # Newest commits first, so announcements lead with the latest changes.
    for index, (commit_sha, subject) in enumerate(reversed(commits)):
        # Keep arbitrary commit titles literal, with no injected HTML/Markdown.
        subject = ''.join('\\' + char if char in '\\`*_{}[]()<>#' else char for char in subject)
        line = f"- [{commit_sha[:8]}](https://github.com/{repository}/commit/{commit_sha}) {subject}"
        if sum(len(item) + 1 for item in lines) + len(line) > 55000:
            lines.append(f"\n{total - index} more commits in the [full comparison]({url}).")
            break
        lines.append(line)
    if not commits:
        lines.append("No source changes since the previous published release.")
    return "\n".join(lines) + "\n\n"
