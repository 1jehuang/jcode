#!/usr/bin/env python3
"""Generate a self-hosted GitHub star-history chart for the README."""

from __future__ import annotations

import argparse
import datetime as dt
import json
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


def render_svg(repository: str, dates: list[dt.date]) -> str:
    if not dates:
        raise RuntimeError("GitHub returned no stargazers")
    dates.sort()
    start, end = dates[0], max(dates[-1], dt.date.today())
    span = max((end - start).days, 1)
    width, height = 800, 420
    left, right, top, bottom = 72, 24, 38, 58
    plot_w, plot_h = width - left - right, height - top - bottom

    points: list[tuple[dt.date, int]] = []
    for index, day in enumerate(dates, 1):
        if index == len(dates) or dates[index] != day:
            points.append((day, index))
    if points[-1][0] < end:
        points.append((end, len(dates)))

    def x(day: dt.date) -> float:
        return left + (day - start).days / span * plot_w

    max_stars = len(dates)
    grid_max = ((max_stars + 4999) // 5000) * 5000 or 1

    def y(value: int) -> float:
        return top + (1 - value / grid_max) * plot_h

    path = " ".join(
        ("M" if index == 0 else "L") + f" {x(day):.1f} {y(count):.1f}"
        for index, (day, count) in enumerate(points)
    )
    area = f"{path} L {x(end):.1f} {top + plot_h:.1f} L {x(start):.1f} {top + plot_h:.1f} Z"

    y_ticks = []
    for index in range(5):
        value = round(grid_max * index / 4)
        yy = y(value)
        label = f"{value / 1000:g}k" if value >= 1000 else str(value)
        y_ticks.append(f'<line x1="{left}" y1="{yy:.1f}" x2="{width-right}" y2="{yy:.1f}" class="grid"/><text x="{left-12}" y="{yy+5:.1f}" text-anchor="end">{label}</text>')

    x_ticks = []
    years = range(start.year, end.year + 1)
    for year in years:
        day = max(start, dt.date(year, 1, 1))
        if day > end:
            continue
        xx = x(day)
        x_ticks.append(f'<line x1="{xx:.1f}" y1="{top}" x2="{xx:.1f}" y2="{top+plot_h}" class="grid"/><text x="{xx:.1f}" y="{height-25}" text-anchor="middle">{day.year}</text>')

    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" role="img" aria-labelledby="title desc">
<title id="title">{repository} star history</title>
<desc id="desc">GitHub stars over time, currently {max_stars:,}</desc>
<style>
  :root {{ color-scheme: light dark; }}
  .bg {{ fill: #fff; }} text {{ fill: #57606a; font: 13px -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif; }}
  .grid {{ stroke: #d8dee4; stroke-width: 1; }} .area {{ fill: #0969da; opacity: .12; }} .line {{ fill: none; stroke: #0969da; stroke-width: 3; }}
  .heading {{ fill: #24292f; font-size: 17px; font-weight: 600; }}
  @media (prefers-color-scheme: dark) {{ .bg {{ fill: #0d1117; }} text {{ fill: #8b949e; }} .grid {{ stroke: #30363d; }} .area {{ fill: #58a6ff; }} .line {{ stroke: #58a6ff; }} .heading {{ fill: #f0f6fc; }} }}
</style>
<rect class="bg" width="100%" height="100%" rx="6"/>
<text class="heading" x="{left}" y="25">GitHub stars over time</text>
{''.join(y_ticks)}{''.join(x_ticks)}
<path class="area" d="{area}"/><path class="line" d="{path}"/>
<circle cx="{x(points[-1][0]):.1f}" cy="{y(max_stars):.1f}" r="4" class="line"/>
<text x="{width-right}" y="25" text-anchor="end">{max_stars:,} stars</text>
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
