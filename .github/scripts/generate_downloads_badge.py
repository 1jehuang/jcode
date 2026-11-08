#!/usr/bin/env python3
"""Sum download counts across every GitHub release (all pages) and write a shields.io endpoint JSON."""
import json, os, urllib.request

repo = os.environ.get("GITHUB_REPOSITORY", "1jehuang/jcode")
token = os.environ.get("GITHUB_TOKEN")
total, page = 0, 1
while True:
    req = urllib.request.Request(f"https://api.github.com/repos/{repo}/releases?per_page=100&page={page}")
    req.add_header("Accept", "application/vnd.github+json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    releases = json.load(urllib.request.urlopen(req))
    if not releases:
        break
    total += sum(a["download_count"] for r in releases for a in r["assets"])
    page += 1

def human(n):
    for div, suf, prec in ((1_000_000, "M", 2), (1_000, "k", 1)):
        if n >= div:
            return f"{n / div:.{prec}f}".rstrip("0").rstrip(".") + suf
    return str(n)

with open("docs/badges/downloads.json", "w") as f:
    json.dump({"schemaVersion": 1, "label": "downloads", "message": human(total), "color": "brightgreen"}, f)
    f.write("\n")
print(total)
