# Always-listening Jarvis (experimental)

Start a local interactive client explicitly:

```sh
jcode --jarvis-always-on
# Also opt in to local window metadata, without screenshots:
jcode --jarvis-always-on --jarvis-screen
```

Neither ordinary Jcode nor `--jarvis` opens an always-on microphone. Settings do
not enable listening by themselves. This variant is currently intended for a
local, unlocked Linux graphical session. It is not a background startup service.

## Bounded pipeline

1. Local PCM capture and a small RMS-based voice segmenter run on the client.
   This detector is a lightweight amplitude heuristic, not speaker recognition
   or a guarantee that sounds are human speech. Silence is not uploaded.
2. Bounded utterances, including a short pre-roll and silence tail, are
   transcribed through Nari. One job runs at a time with a bounded backlog.
3. A Jev decision can accept a streaming partial or a finalized transcript as
   a request for the assistant. Natural requests need not say a wake
   word. Ambient, quoted, hypothetical and uncertain speech should be rejected.
   Classifier failures fail closed, never forward directly to the agent.
4. The listening window shows a running local transcript of finalized phrases.
   A new `Heard` row starts gray. Once Jev scores the phrase, the row shows
   its confidence that the phrase was addressed to Jarvis, not the accuracy of
   the transcription. Green means the gate accepted it; an unscored phrase
   does not display a made-up percentage.
   A caption is not a chat message. When Jev accepts a phrase, the ordinary
   agent receives a voice turn containing the recent `background_transcript`
   and the exact `trigger`. Only the trigger is a request. Earlier speech may
   clarify references but cannot authorize an action.
5. Each accepted utterance has a local ID. A partial may start read-only work
   immediately. Its final text replaces a queued partial with the same ID or
   follows a partial already sent. A busy agent receives accepted requests
   through the normal interleave path instead of silently dropping a second
   request. The client bounds the queue and enforces the hourly turn limit.
   `JARVIS_DISPATCH` log rows record the session, utterance ID, and disposition
   (queued, merged, sent, suppressed, expired, or failed), without transcript
   text. A send is a transport submission, not proof the assistant replied.

Jev uses the existing voice provider routing and credentials. This path uses
`evaluate_once`, not interactive voice's speculative hedged requests. It sends
bounded recent speech and recent user prompts and Jarvis replies to classify
only the newest phrase. Tool calls, tool results, reasoning and screen snapshots
are not classifier context. Speech included with an accepted turn becomes part
of the ordinary conversation history; this is not an unlimited audio archive. A confidence threshold is a policy, not proof
that a request was intended, correctly transcribed, or spoken by the user.
Existing tool permissions and irreversible-action approvals still apply.

Capture is paused on client detach and checked against the local session's lock
and activity state. The logind LockedHint guard depends on the locker publishing
its state. Audio captured during spoken replies is suppressed, pending results
are invalidated, and a short cooldown avoids loudspeaker feedback. This is not
full acoustic echo cancellation or simultaneous barge-in. Use the normal stop
controls or pause the microphone before media playback or private conversations.

## What appears in the chat

By default, the TUI shows the spoken trigger or final request for each Jarvis user turn. A partial already sent and its later final remain two turns. Typed messages and old sessions without voice provenance are displayed as originally recorded, even if they resemble a voice envelope.

To expand each voice row into transcript, status, nonempty background (one utterance per line) and any injected instructions, set the following and restart the TUI:

```toml
[jarvis]
show_raw_requests = true
```

This multiline view is formatted for reading. For the original user payload instead, set `show_exact_request_payload = true` in the same section. It takes precedence over the other setting.

This is a display preference, not a privacy setting. The agent still receives the original JSON with its trigger, bounded background speech and optional `still_speaking` flag, or the final-transcript control message. That message is only one part of a provider call: system instructions, earlier conversation and tool results can also be in context. The toggle neither reveals that full context nor removes anything from saved history.

## Voice latency and recovery

Run `python3 scripts/jarvis_latency_report.py --session SESSION_ID` from the checkout.
You can also pass daily log files explicitly, including both days for a session
that crossed midnight. A green caption means *accepted by the gate*, not sent.
The report distinguishes confirmed dispositions from missing evidence. Early
injection can precede the speech endpoint. Player startup is a software
boundary, not evidence of the first audible sample at the speaker. Older logs
cannot reconstruct timings or drops they never recorded.

Local playback waits for a quiet microphone. New input can supersede a held
reply. In Handterm the default trigger cue says “On it” in the configured
Jarvis voice instead of playing a ding. A ready spoken reply takes precedence.
A custom `trigger_sound` still plays its file, and `trigger_sound = ""` disables
the cue. Network outages release the microphone for 2 seconds, then 8 seconds,
then 60 seconds between attempts until connectivity returns. Rate limits start
with a 15-second cooldown. An automatic retry respects pause and screen lock.
Repeat the utterance
after listening resumes: failed audio is not replayed. A known credential or
credit failure stops capture. Correct the issue, then use `/voice` to retry
without restarting the conversation or resetting the hourly allowance. On updated
clients, the Waybar `!` tooltip shows the failure and the next step, and clicking
it requests the same retry as `/voice`. An older client cannot retry from Waybar.

## Cost limits

Defaults are deliberately conservative:

```toml
[jarvis.listening]
max_agent_turns_per_hour = 0
action_probability = 0.97
```

Agent turns are uncapped by default. Set `max_agent_turns_per_hour` to 1 through
120 to cap newly dispatched voice requests in a rolling hour, or 0 to disable
this cap. Finishing a partial utterance does not consume another turn. This does
not establish a dollar limit or cap the tokens of an accepted coding task.
Provider billing, rounding and provider-side retention policies remain external
to Jcode.

Audio and classifier work require configured Nari and Jev credentials/access.
No model weights or voice clones are embedded in Jcode. The configured external
speech-output endpoint continues to supply spoken replies.

## Waybar capture indicator

`scripts/waybar/jarvis-voice-status.py` reads private, transcript-free runtime
status. It never opens a second microphone. The client holds an exclusive
microphone lease for this mode and publishes measured amplitude history, actual
capture state, and counters. Status expires after three seconds and vanishes
when the client exits. It is stored under `$XDG_RUNTIME_DIR/jcode-jarvis`, not
conversation storage. Synthetic test clients use their isolated JCODE_HOME.

Example module:

```json
"custom/jarvis-voice": {
  "exec": "python3 /path/to/jcode/scripts/waybar/jarvis-voice-status.py",
  "return-type": "json",
  "format": "{}",
  "tooltip": true,
  "on-click": "python3 /path/to/jcode/scripts/waybar/jarvis-voice-status.py --toggle"
}
```

Add `custom/jarvis-voice` to a module list. The waveform changes with measured
input, not a decorative animation. The tooltip distinguishes capture from
speech suppression. Click the module, use `/voice`, or press the voice key to
pause/resume. A pause closes the recorder, rather than merely hiding its meter.
A fatal provider or indicator error pauses processing and requires resolving
the error and restarting the mode, rather than an automatic retry loop.

## Native smooth waveform (patched Waybar)

For a graphical meter instead of text glyphs, the Jstack Waybar fork provides
`waveform/jarvis`, a GTK drawing area rendered with Cairo. The helper's `--native`
mode supplies bounded numeric amplitude history. Waybar renders seven rounded
vertical pills whose heights smoothly follow measured audio levels using its
frame clock, not a fake sine animation. It does
not open a microphone or send audio anywhere.

```json
"waveform/jarvis": {
  "exec": "python3 /path/to/jcode/scripts/waybar/jarvis-voice-status.py --native",
  "width": 86,
  "height": 18,
  "fps": 60,
  "on-click": "python3 /path/to/jcode/scripts/waybar/jarvis-voice-status.py --toggle"
}
```

Replace `custom/jarvis-voice` in the module list with `waveform/jarvis` after
installing the patched Waybar. Ordinary upstream Waybar can continue using the
text-module example above. The native helper is backward-compatible with older
5 Hz Jcode clients. New clients publish up to 20 measured samples per second,
retain at most 64 amplitudes, and publish only a 1 Hz heartbeat while paused.
No transcripts are included. A capture/state transition is published immediately.
The renderer stops animation when capture is off or its feed expires.

When Jev accepts a request, the module briefly enters the `triggered` state.
This confirms acceptance, not that an agent reply has finished. The words stay
out of Waybar. To make acceptance stand out even when the microphone is quiet:

```css
#waveform-jarvis.triggered { color: #1e1e2e; background-color: #a6e3a1; border-radius: 8px; }
```

## Screen context

`--jarvis-screen` is a separate explicit metadata-only opt-in. See
[JARVIS_SCREEN_CONTEXT.md](JARVIS_SCREEN_CONTEXT.md). Window changes are passive
observations, not user requests. They must not start model inference or speech.
Pixels, OCR, eye tracking and automatic focus changes are not enabled.
