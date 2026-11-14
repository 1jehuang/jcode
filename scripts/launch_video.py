#!/usr/bin/env python3
"""Compose reviewed real-terminal captures. Python stdlib + ffmpeg/ffprobe only."""
import argparse
import collections
import datetime
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

SECTIONS = ("swarm", "cloud", "performance")


def number(value, label, minimum=0):
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < minimum:
        raise ValueError(f"{label} must be a finite number >= {minimum}")
    return value


def run(args):
    subprocess.run([str(a) for a in args], check=True)


def probe(path):
    return json.loads(subprocess.check_output(["ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", str(path)]))


def font_path(value):
    if value:
        path = Path(value).expanduser().resolve()
    else:
        try:
            path = Path(subprocess.check_output(["fc-match", "sans", "-f", "%{file}"], text=True))
        except (OSError, subprocess.CalledProcessError):
            path = Path("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
    if not path.is_file():
        raise ValueError("A font file is required. Set manifest font to a TTF/OTF path")
    return path


def text_filter(work, text, size, y, index, color="white"):
    # Text travels through a file, never shell syntax or drawtext expressions.
    if not isinstance(text, str) or len(text) > 90 or "\n" in text:
        raise ValueError("Titles/captions must be single-line strings of at most 90 characters")
    size = min(size, max(18, int(1720 / max(1, len(text)) / 0.65)))
    name = f"text{index}.txt"
    (work / name).write_text(text, encoding="utf-8")
    return f"drawtext=fontfile=font.ttf:textfile={name}:expansion=none:fontsize={size}:fontcolor={color}:x=(w-tw)/2:y={y}"


def compose(args):
    manifest_path = Path(args.manifest).expanduser().resolve()
    data = json.loads(manifest_path.read_text())
    if data.get("version") != 1:
        raise ValueError("manifest version must be 1")
    clips = data.get("clips", [])
    if not clips or set(c.get("section") for c in clips) != set(SECTIONS):
        raise ValueError("clips must include swarm, cloud, and performance sections")
    order = [SECTIONS.index(c["section"]) for c in clips]
    if order != sorted(order):
        raise ValueError("clips must be ordered swarm, cloud, performance")
    cards = [data.get("intro", {}), data.get("outro", {})]
    durations = [number(c.get("duration", 4), "card duration", 1) for c in cards]
    total = sum(durations) + sum(number(c.get("duration"), "clip duration", 0.1) for c in clips)
    if not 45 <= total <= 70:
        raise ValueError(f"launch video must run 45-70 seconds, got {total:g}")
    for c in clips:
        if c.get("reviewed") is not True:
            raise ValueError("Every capture must have reviewed:true after a visual privacy review")
        number(c.get("start", 0), "start")
        number(c.get("speed", 1), "speed", 0.01)
    if not all(shutil.which(tool) for tool in ("ffmpeg", "ffprobe")):
        raise ValueError("ffmpeg and ffprobe must be installed")
    output = Path(args.output).expanduser().resolve()
    if output.exists() and not args.overwrite:
        raise ValueError(f"Output exists: {output}. Use --overwrite explicitly")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="launch-video-", dir=output.parent) as tmp:
        work = Path(tmp)
        shutil.copyfile(font_path(data.get("font")), work / "font.ttf")
        segments = []
        base = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-filter_threads", "1"]
        encoding = ["-an", "-c:v", "libx264", "-preset", args.preset, "-crf", "18", "-pix_fmt", "yuv420p", "-r", "30", "-threads", "2"]
        def render(inputs, filters, duration):
            target = work / f"segment{len(segments):03d}.mp4"
            subprocess.run(base + inputs + ["-vf", filters, "-t", str(duration)] + encoding + [str(target)], cwd=work, check=True)
            actual = float(probe(target)["format"]["duration"])
            if abs(actual - duration) > 0.1:
                raise ValueError(f"Capture too short: expected {duration}s, rendered {actual}s")
            segments.append(target)
        def card(spec, duration, index, default_title, default_subtitle):
            title = text_filter(work, spec.get("title", default_title), 76, 410, f"{index}a")
            subtitle = text_filter(work, spec.get("subtitle", default_subtitle), 32, 530, f"{index}b", "0xa8b5cb")
            render(["-f", "lavfi", "-i", "color=c=0x0b1020:s=1920x1080:r=30"], f"{title},{subtitle},fade=t=in:d=0.35,fade=t=out:st={duration-0.35}:d=0.35", duration)
        card(cards[0], durations[0], "intro", "Jcode", "One plan. Many agents. Real work.")
        for i, clip in enumerate(clips):
            path = Path(clip["path"]).expanduser()
            if not path.is_absolute():
                path = manifest_path.parent / path
            path = path.resolve()
            speed = clip.get("speed", 1)
            start = clip.get("start", 0)
            sequence = bool(re.search(r"%0?\d*d", path.name))
            if sequence:
                fps = number(clip.get("input_fps", 30), "input_fps", 1)
                first = clip.get("start_number", 0)
                if not isinstance(first, int) or first < 0:
                    raise ValueError("start_number must be a nonnegative integer")
                first_frame = Path(str(path) % first)
                dimensions = next(s for s in probe(first_frame)["streams"] if s.get("codec_type") == "video")
                inputs = ["-framerate", str(fps), "-start_number", str(first), "-i", str(path)]
            else:
                if not path.is_file():
                    raise ValueError(f"Missing clip: {path}")
                info = probe(path)
                if not any(s.get("codec_type") == "video" for s in info["streams"]):
                    raise ValueError(f"No video stream: {path}")
                dimensions = next(s for s in info["streams"] if s.get("codec_type") == "video")
                inputs = ["-noautorotate", "-i", str(path)]
            if dimensions["width"] > 1920 or dimensions["height"] > 960:
                raise ValueError(f"Capture exceeds native-pixel safe area 1920x960: {path}")
            if dimensions["width"] % 2 or dimensions["height"] % 2:
                raise ValueError("Capture dimensions must be even to preserve native pixels in yuv420p")
            caption = text_filter(work, clip.get("caption", ""), 26, 1028, i)
            label = text_filter(work, {"swarm":"01 / SWARM PLANS", "cloud":"02 / CLOUD", "performance":"03 / PERFORMANCE"}[clip["section"]], 24, 24, f"label{i}", "0x9badca")
            filters = (f"trim=start={start},setpts=(PTS-STARTPTS)/{speed},fps=30,"
                       "pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=0x0b1020,setsar=1,"
                       f"{label},{caption},fade=t=in:d=0.18,fade=t=out:st={clip['duration']-0.18}:d=0.18")
            render(inputs, filters, clip["duration"])
        card(cards[1], durations[1], "outro", "Build together.", "Jcode / github.com/1jehuang/jcode")
        (work / "concat.txt").write_text("".join(f"file '{p.name}'\n" for p in segments))
        final = work / "final.mp4"
        run(base + ["-f", "concat", "-safe", "0", "-i", work / "concat.txt", "-c", "copy", "-movflags", "+faststart", final])
        info = probe(final)
        stream = next(s for s in info["streams"] if s["codec_type"] == "video")
        if (stream["width"], stream["height"]) != (1920, 1080) or abs(float(info["format"]["duration"]) - total) > 0.2:
            raise ValueError("Final output failed dimensions/duration validation")
        if args.overwrite:
            os.replace(final, output)
        else:
            # Atomic no-clobber publication, even if another process created output.
            os.link(final, output)
        print(f"Created {output} ({total:g}s, 1920x1080, 30fps)")


def inspect_sources(args):
    root = Path(args.sessions).expanduser()
    if args.limit < 1:
        raise ValueError("limit must be positive")
    if not root.is_dir():
        raise ValueError(f"Sessions directory does not exist: {root}")
    candidates = []
    tool_candidates = []
    for path in sorted(root.glob("*.json")):
        try:
            data = json.loads(path.read_text())
            events = list(data.get("replay_events", []))
            messages = list(data.get("messages", []))
            journal = path.with_suffix(".journal.jsonl")
            if journal.exists():
                with journal.open() as recording:
                    for line in recording:
                        try:
                            entry = json.loads(line)
                        except ValueError:
                            continue  # An active journal may have an incomplete final line.
                        events.extend(entry.get("append_replay_events", []))
                        messages.extend(entry.get("append_messages", []))
            plans = []
            spawns = 0
            for index, message in enumerate(messages):
                for block in message.get("content", []):
                    if not isinstance(block, dict) or block.get("type") != "tool_use":
                        continue
                    value = block.get("input", {})
                    if not isinstance(value, dict):
                        continue
                    if block.get("name") == "swarm" and value.get("action") == "spawn":
                        spawns += 1
                    if block.get("name") == "todo":
                        lanes = [lane for phase in value.get("phases") or [] if isinstance(phase, dict)
                                 for lane in phase.get("lanes", []) if isinstance(lane, dict)]
                        if len(lanes) >= 3:
                            plans.append({"message_index": index, "timestamp": message.get("timestamp"),
                                          "lane_count": len(lanes), "suggested_duration": 8})
            if plans and spawns:
                tool_candidates.append({"session_id": data.get("id", path.stem), "title": data.get("title"),
                                        "source_kind": "actual_todo_tool_calls_and_results",
                                        "event_counts": {"multi_lane_plans": len(plans), "swarm_spawns": spawns},
                                        "suggested_cuts": plans[:5], "privacy_review": "required"})
            counts = collections.Counter(e.get("event") for e in events)
            if not counts["swarm_plan"] and not counts["swarm_status"]:
                continue
            cuts = [{"timestamp": e.get("timestamp"), "event": e["event"], "suggested_duration": 8}
                    for e in events if e.get("event") == "swarm_plan"][:5]
            candidates.append({"session_id": data.get("id", path.stem), "title": data.get("title"),
                               "event_counts": dict(counts), "suggested_cuts": cuts,
                               "privacy_review": "required, do not capture until reviewed"})
        except (OSError, ValueError, TypeError, AttributeError):
            continue
    candidates.sort(key=lambda c: (bool(c["event_counts"].get("swarm_plan") and c["event_counts"].get("swarm_status")), c["event_counts"].get("swarm_plan", 0)), reverse=True)
    tool_candidates.sort(key=lambda c: c["event_counts"]["multi_lane_plans"], reverse=True)
    result = {"version": 1, "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "note": "Metadata only. No transcripts copied. Candidate status is not privacy approval.",
              "candidates": candidates[:args.limit],
              "tool_plan_candidates": tool_candidates[:args.limit],
              "recording_limitation": "Tool-plan fallbacks are not recorded swarm_plan/swarm_status events. Preserve original tool results when replaying them."}
    output = Path(args.output).expanduser()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2) + "\n")
    output.chmod(0o600)
    print(f"Wrote {len(result['candidates'])} candidate(s) to {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    compose_parser = sub.add_parser("compose", help="Compose visually reviewed real-terminal captures")
    compose_parser.add_argument("--manifest", required=True)
    compose_parser.add_argument("--output", required=True)
    compose_parser.add_argument("--overwrite", action="store_true")
    compose_parser.add_argument("--preset", default="medium", choices=("ultrafast", "fast", "medium", "slow"))
    compose_parser.set_defaults(func=compose)
    inspect_parser = sub.add_parser("sources", help="Inventory persisted replay event metadata, never transcripts")
    inspect_parser.add_argument("--sessions", default="~/.jcode/sessions")
    inspect_parser.add_argument("--output", default="~/.jcode/scratch/launch-video/sources.json")
    inspect_parser.add_argument("--limit", type=int, default=20)
    inspect_parser.set_defaults(func=inspect_sources)
    args = parser.parse_args()
    try:
        args.func(args)
    except (ValueError, OSError, KeyError, TypeError, subprocess.CalledProcessError) as exc:
        parser.exit(1, f"launch-video: {exc}\n")


if __name__ == "__main__":
    main()
