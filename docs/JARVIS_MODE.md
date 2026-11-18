# Experimental Jarvis mode

Start the interactive terminal client with `jcode --jarvis`, optionally with
`--resume <session>`. Ordinary sessions do not advertise the `speak` tool.
Screen capture and wake-word detection are not implemented. For the separate
opt-in continuous-listening variant and Waybar meter, see
[JARVIS_ALWAYS_ON.md](JARVIS_ALWAYS_ON.md). `--jarvis-screen` opts in to passive
local window metadata, without screenshots.

## Session prompting

Jarvis registers an opt-in, session-owned addition to the existing system prompt
alongside its `speak` tool. It does not replace base instructions or add a visible
reminder to every user turn or retry. Ordinary sessions keep their existing prompt.
See [screen context implementation](JARVIS_SCREEN_CONTEXT.md) for the bounded,
opt-in passive screen-awareness channel.

## External speech service

Jcode ships neither a speech model nor voice weights. Install and run your
chosen synthesis service separately, then configure `~/.jcode/config.toml`:

```toml
[jarvis.speech]
endpoint = "http://127.0.0.1:8765/v1/audio/speech"
model = "piper-jarvis"
voice = "jarvis"
# api_key_env = "MY_SPEECH_API_KEY"
```

The endpoint is the full URL, not a base URL. `JCODE_JARVIS_SPEECH_ENDPOINT`
overrides it. An explicitly empty or invalid endpoint is an error, not an
instruction to switch voices. If no endpoint is configured, Jcode uses an
installed `espeak-ng` or `espeak` executable as a free placeholder.

The client sends an OpenAI-compatible request:

```json
{"model":"piper-jarvis","voice":"jarvis","input":"The tests passed.","response_format":"wav"}
```

The service must respond with HTTP 200, `Content-Type: audio/wav`, and RIFF/WAVE
audio, at most 16 MiB. Redirects are disabled and requests time out after 45
seconds. A configured service does not silently fall back to another voice.
If `api_key_env` is set, that environment variable must contain a nonempty key.
Jcode sends it as a sensitive Bearer authorization header, never in the URL or
model context. Credentials require HTTPS except for localhost/loopback services.

For the selected community Piper JARVIS High voice, the ONNX model, JSON config,
Piper runtime, and local HTTP wrapper belong in a separate installation, such as
`~/.local/share/jcode/jarvis-service/`, not in this repository or the Jcode binary.
Community voice licensing and permission to use the underlying voice must be
assessed separately from the software license.

Install `pw-play` (PipeWire) or `aplay` (ALSA) on the client for HTTP audio
playback. Voice input still uses the existing `/voice` and voice-key workflow,
including its Nari API-key and microphone-recorder requirements. Free local
speech output does not make transcription free or offline.

## Data flow

Microphone -> existing transcription -> existing agent session -> `speak` tool
-> owner-scoped client callback -> bounded speech queue -> configured API
-> WAV bytes -> local speakers.

The CLI registers `speak` through the existing session-tool callback protocol,
not a second agent or a global daemon tool. Only the client calls the configured
speech endpoint, even when the agent runs remotely. A localhost endpoint keeps
speech synthesis on the client computer. A remote endpoint receives the spoken
text. The text is also retained in the conversation and tool result.

The agent is instructed to speak concise conversational replies and important
updates, keeping code, logs, and detailed explanations on screen.
`speak({"text":"The tests passed.","intent":"Report results"})` acknowledges
that speech was **queued**, not that playback succeeded or the user heard it.
Asynchronous API/playback errors appear locally in the TUI without exposing
URLs, credentials, or provider response bodies to the model.

## Interruption and lifecycle

- Starting built-in voice input stops active playback, cancels an in-flight
  speech request, and clears queued utterances. Companion recording events do
  the same when delivered to the TUI.
- Speech callbacks are rejected during active microphone input to avoid feedback.
- Disconnecting, switching sessions, or closing the client cancels local audio.
- Session changes clear the old client-tool capability. Disconnect removes
  unrestricted temporary speech overlays so a non-Jarvis client does not inherit
  a dead speech tool.
- Input and queue sizes are bounded. Playback children are killed and reaped
  on cancellation and shutdown. No shell is used for speech playback.

## Verification

Hosted regression tests cover the JSON contract, endpoint/credential validation,
response limits, rejected responses, cancellation, queue bounds, and fake audio
playback. A local-only acceptance test exercises a running Piper service and
real speaker output:

```bash
JCODE_TEST_SPEECH_ENDPOINT=http://127.0.0.1:8765/v1/audio/speech \
  justrust test --local="requires local Piper service and speakers" -p jcode-voice \
    --features desktop-test-support --lib real_local_piper_api_playback -- --ignored --nocapture
```

This check does not create an agent conversation or synthetic history.
Speech is generated per tool call, not yet streamed sentence by sentence.
Full-duplex echo cancellation and live screen awareness remain future work.
Existing approval rules are unchanged.
