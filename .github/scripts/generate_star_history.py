#!/usr/bin/env python3
"""Generate a self-hosted week-over-week GitHub stars chart for the README."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import math
import os
import urllib.request
from pathlib import Path


def fetch_stars(repository: str, token: str) -> list[dt.date]:
    url = f"https://api.github.com/repos/{repository}/stargazers?per_page=100"
    dates: list[dt.date] = []
    while url:
        request = urllib.request.Request(
            url,
            headers={
                "Accept": "application/vnd.github.star+json",
                "Authorization": f"Bearer {token}",
                "User-Agent": "jcode-star-history",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        with urllib.request.urlopen(request) as response:
            for star in json.load(response):
                dates.append(dt.datetime.fromisoformat(star["starred_at"].replace("Z", "+00:00")).date())
            links = response.headers.get("Link", "")
        url = ""
        for link in links.split(","):
            if 'rel="next"' in link:
                url = link[link.index("<") + 1 : link.index(">")]
                break
    return dates


def week_start(day: dt.date) -> dt.date:
    """Return the Monday containing ``day``."""
    return day - dt.timedelta(days=day.weekday())


def render_svg(repository: str, dates: list[dt.date], today: dt.date | None = None) -> str:
    if not dates:
        raise RuntimeError("GitHub returned no stargazers")
    today = today or dt.date.today()
    dates.sort()
    current_week = week_start(today)
    weeks = [current_week - dt.timedelta(weeks=week) for week in reversed(range(26))]
    counts = {week: 0 for week in weeks}
    for day in dates:
        if (week := week_start(day)) in counts:
            counts[week] += 1

    width, height = 800, 420
    left, right, top, bottom = 64, 24, 72, 54
    plot_w, plot_h = width - left - right, height - top - bottom

    values = [counts[week] for week in weeks]
    max_weekly = max(values)
    magnitude = 10 ** max(0, len(str(max_weekly)) - 1)
    step = max(magnitude, math.ceil(max_weekly / 4 / magnitude) * magnitude)
    grid_max = max(step * 4, 4)

    def y(value: int) -> float:
        return top + (1 - value / grid_max) * plot_h

    y_ticks = []
    for index in range(5):
        value = step * index
        yy = y(value)
        label = f"{value / 1000:g}k" if value >= 1000 else str(value)
        y_ticks.append(f'<line x1="{left}" y1="{yy:.1f}" x2="{width-right}" y2="{yy:.1f}" class="grid"/><text x="{left-12}" y="{yy+5:.1f}" text-anchor="end">{label}</text>')

    slot = plot_w / len(weeks)
    bar_width = max(slot - 5, 4)
    bars = []
    x_ticks = []
    for index, (week, value) in enumerate(zip(weeks, values)):
        xx = left + index * slot + (slot - bar_width) / 2
        yy = y(value)
        bar_height = top + plot_h - yy
        current = " current" if week == current_week else ""
        bars.append(
            f'<rect class="bar{current}" x="{xx:.1f}" y="{yy:.1f}" width="{bar_width:.1f}" '
            f'height="{bar_height:.1f}" rx="2"><title>{week:%b %-d}: +{value:,} stars</title></rect>'
        )
        if index % 4 == 0 or index == len(weeks) - 1:
            x_ticks.append(
                f'<text x="{xx + bar_width / 2:.1f}" y="{height-25}" text-anchor="middle">{week:%b %-d}</text>'
            )

    latest = values[-1]
    previous_week = weeks[-2]
    previous_cutoff = previous_week + dt.timedelta(days=today.weekday())
    previous_to_date = sum(previous_week <= day <= previous_cutoff for day in dates)
    change = latest - previous_to_date
    change_label = f"{change:+,} vs same point last week"
    period_total = sum(values)

    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" role="img" aria-labelledby="title desc">
<title id="title">{repository} weekly GitHub stars</title>
<desc id="desc">New GitHub stars per week for the last 26 weeks. This week: {latest:,}, {change_label}.</desc>
<style>
  :root {{ color-scheme: light dark; }}
  .bg {{ fill: #fff; }} text {{ fill: #57606a; font: 13px -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; }}
  .grid {{ stroke: #d8dee4; stroke-width: 1; }} .bar {{ fill: #2f81f7; }} .bar.current {{ fill: #bf8700; }}
  .heading {{ fill: #24292f; font-size: 17px; font-weight: 600; }}
  .metric {{ fill: #24292f; font-size: 15px; font-weight: 600; }}
  @media (prefers-color-scheme: dark) {{ .bg {{ fill: #0d1117; }} text {{ fill: #8b949e; }} .grid {{ stroke: #30363d; }} .bar {{ fill: #58a6ff; }} .bar.current {{ fill: #d29922; }} .heading,.metric {{ fill: #f0f6fc; }} }}
</style>
<rect class="bg" width="100%" height="100%" rx="6"/>
<text class="heading" x="{left}" y="27">Stars, week over week</text>
<text x="{left}" y="51">New stars per week · last 26 weeks</text>
<text class="metric" x="{width-right}" y="27" text-anchor="end">+{latest:,} this week so far</text>
<text x="{width-right}" y="51" text-anchor="end">{change_label} · {period_total:,} total</text>
{''.join(y_ticks)}{''.join(x_ticks)}
{''.join(bars)}
</svg>
'''


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default="1jehuang/jcode")
    parser.add_argument("--output", type=Path, default=Path("docs/images/star-history.svg"))
    args = parser.parse_args()
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if not token:
        raise SystemExit("GITHUB_TOKEN or GH_TOKEN is required")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(render_svg(args.repo, fetch_stars(args.repo, token)))


if __name__ == "__main__":
    main()
