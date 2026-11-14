# Headless launch video from real sessions

`scripts/launch_video.py` uses Python's standard library and installed `ffmpeg` / `ffprobe`. It composes video clips rendered from recorded sessions through Jcode's production TUI. It does not draw, reconstruct, or invent terminal content. No visible terminal windows, desktop actions, or screenshots are required. Keep all source media and private manifests outside the repository.

## Render original sessions headlessly

The root `src/bin/replay_frames.rs` development binary exports production TUI frames and optional cell data without a terminal window or provider connection:

```bash
justrust build --profile selfdev -p jcode --features dev-bins --bin replay_frames
./target/selfdev/replay_frames \
  --session session_fox_1793673474425_cee7c7a5ef45bb8a \
  --timeline ~/.jcode/scratch/launch-video/fox-snippet.json \
  --output ~/.jcode/scratch/launch-video/fox-frames-new \
  --cols 120 --rows 30 --fps 24 --duration-ms 30000 --cells
```

The output directory must be new. The timeline must contain reviewed original session text and actual tool results. Record cuts and compressed timing in a scratch provenance file. Do not invent historical task completion, swarm membership, benchmark results, or terminal content. If recorded swarm events are unavailable, retain the actual todo tool call/result snapshots instead.

Feed the exported manifest to the direct Kitty-native glyph rasterizer. It streams RGB frames into ffmpeg and writes MP4 without visible windows or screenshot sequences:

```bash
python3 scripts/render_kitty_frames.py \
  ~/.jcode/scratch/launch-video/fox-frames-new/manifest.json \
  --output ~/.jcode/scratch/launch-video/fox.mp4 \
  --cell-width 14 --cell-height 32 --fps 24
```

The rasterizer loads Kitty's native extension using Kitty's embedded Python. Its configuration defaults to `~/.config/kitty/kitty.conf`. Repeat for each reviewed source timeline. Feed the resulting MP4s to the compositor below. The compositor's image-sequence input remains a general compatibility feature, not the prescribed production workflow.

The exporter uses the exact production TUI state/layout renderer, with historical todo snapshots and no current account/git data or automatic agent follow-ups. Kitty’s own VT parser, font shaping, glyph sprites, fallback fonts and decorations produce the raster headlessly. The compositor applies no terminal scaling. A saved real-terminal calibration at 14×32 cells measured 0.105/255 mean RGB error and a maximum difference of 3/255 after aligning terminal padding, across a 550×28 text crop. That is close visual agreement, not universal pixel identity: the check does not cover every emoji, desktop transparency or cursor. Use `--codec ffv1` with a `.mkv` output for an exact lossless copy of the generated RGB frames. H.264 exports and the shareable final composition are lossy. Review rendered video for faithful content and readable typography before publication.

## Privacy and output layout

Recommended source: **1680x960 pixels** for a 120-column, 30-row TUI. The compositor centers that rectangle on 1920x1080 without a scale filter. Review every rendered frame, including scrollback, expanded tool outputs, pinned todo names, account labels, paths, URLs, email addresses, tokens, and command arguments. `reviewed: true` is an explicit visual-review attestation, not an automatic secret scanner.

Keep captions short. Header and footer typography is outside the recommended source rectangle. Oversized sources fail instead of being silently resized. Audio is deliberately stripped. Intro/outro cards and short fades provide pacing without overlays obscuring the pinned plan.

## Source inventory

```bash
python3 scripts/launch_video.py sources \
  --output ~/.jcode/scratch/launch-video/sources.json
```

This scans snapshot `replay_events` and journal `append_replay_events`, counting actual `swarm_plan` / `swarm_status` records, not mentions of those names in tool output. It writes only IDs, titles, event counts, and suggested plan timestamps to a private scratch manifest. `tool_plan_candidates` separately lists genuine multi-lane todo calls paired with swarm spawns as a fallback. Replay those using their original tool results, not synthesized historical swarm events. A candidate still needs a visual privacy review. An empty event inventory means no suitable persisted event records were found, not permission to synthesize history.

## Tooling regression tests

```bash
python3 -m unittest discover -s scripts -p test_launch_video.py -v
```

The standard-library suite covers explicit privacy approval, missing and non-video paths, output preservation, invalid duration/start/speed, unsafe frame dimensions, literal caption handling, and metadata-only source inventory. It uses mocks and temporary fixtures, with no Rust build, ffmpeg process, provider request, or desktop interaction.

## Edit manifest, version 1

Save this outside the repository, for example `~/.jcode/scratch/launch-video/edit.json`. Paths are relative to the manifest or absolute. These example paths must be replaced with actual rendered clips. Set `reviewed` to true only after review.

```json
{
  "version": 1,
  "intro": {
    "title": "Jcode",
    "subtitle": "One plan. Many agents. Real work.",
    "duration": 4
  },
  "outro": {
    "title": "Build together.",
    "subtitle": "Jcode / github.com/1jehuang/jcode",
    "duration": 4
  },
  "clips": [
    {
      "path": "clips/swarm-plan.mp4",
      "section": "swarm",
      "caption": "A shared plan. Pinned where the work happens.",
      "start": 2,
      "duration": 18,
      "speed": 1,
      "reviewed": false
    },
    {
      "path": "clips/swarm-progress.mp4",
      "section": "swarm",
      "caption": "Parallel agents. One visible source of truth.",
      "duration": 12,
      "reviewed": false
    },
    {
      "path": "clips/cloud.mp4",
      "section": "cloud",
      "caption": "Your real session, wherever you work.",
      "duration": 10,
      "reviewed": false
    },
    {
      "path": "clips/performance.mp4",
      "section": "performance",
      "caption": "Stay in flow.",
      "duration": 10,
      "reviewed": false
    }
  ]
}
```

This edit is 58 seconds, with 30 seconds of swarm/plan footage. Sections must appear in swarm, cloud, performance order. `duration` is output seconds, `start` is source seconds, and `speed: 2` consumes two source seconds per output second. Frame sequences use ffmpeg printf patterns and their original `input_fps`. Missing frames or clips too short for the cut fail rather than silently freezing or inventing footage.

Titles/captions are literal text, not ffmpeg expressions, limited to one line and 90 characters. Keep titles much shorter for readable typography. Optional top-level `font` selects an installed TTF/OTF file. Otherwise the tool uses the system sans font via `fc-match`, with a DejaVu Sans fallback.

```bash
python3 scripts/launch_video.py compose \
  --manifest ~/.jcode/scratch/launch-video/edit.json \
  --output ~/.jcode/scratch/launch-video/jcode-launch.mp4
```

Output: 45-70 seconds, 1920x1080, 30fps, H.264/yuv420p, fast-start MP4. Existing outputs are protected unless `--overwrite` is explicitly supplied. Rendering uses temporary segments alongside the output and publishes only after dimensions and duration pass `ffprobe` checks. Use `--preset ultrafast` for a rough cut, then default `medium` for delivery. Watch the finished video at full resolution before publishing. Captions must describe only what the footage actually demonstrates. Do not imply a benchmark speedup without a recorded, defensible measurement.
