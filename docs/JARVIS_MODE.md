# Experimental Jarvis mode

Start the interactive terminal client with `jcode --jarvis`, optionally with
`--resume <session>`. Ordinary Jcode sessions do not advertise the speech tool.
This first prototype supports local replies, not screen capture, wake words,
or always-on listening.

## Requirements

- Install `espeak-ng` or `espeak` on the client computer. This is the free,
  local placeholder voice. No ElevenLabs account or speech-output API key is
  required. If neither executable is available, startup reports an error before
  entering terminal raw mode.
- Voice input uses the existing Jcode `/voice` and voice-key workflow, including
  its existing Nari API-key and microphone-recorder requirements. Free local
  speech output does not make transcription free or offline.

## Data flow

Microphone -> existing transcription -> existing agent session -> `speak` tool
-> owner-scoped client callback -> bounded local speech queue -> speakers.

The CLI registers `speak` through the existing session-tool callback protocol,
not a new agent or global server tool. Speech synthesis and playback happen on
the local client even when the agent runs remotely. The current backend makes
no speech-output network requests. Transcription retains its existing network
behavior. The tool text is part of the conversation and tool result.

The agent is instructed to use concise conversational speech and keep code,
logs, and detailed explanations on screen. `speak({"text":"The tests passed.",
"intent":"Report results"})` acknowledges that speech was **queued**, not that
playback succeeded or the user heard it. The model does not wait for playback.

## Interruption and lifecycle

- Starting built-in voice input stops the active utterance and clears queued
  speech. The same happens when a companion recording event reaches the TUI.
- Speech callbacks are rejected during active microphone input to avoid feedback.
- Disconnecting, switching sessions, or closing the client cancels local audio.
- Session changes clear the old client-tool capability. Disconnect removes
  unrestricted temporary speech overlays, so a subsequent non-Jarvis client
  does not inherit a dead speech tool.
- Input and queue sizes are bounded. Playback children are killed and reaped
  on cancellation and shutdown. Text is passed without a shell.

## Current limits

The placeholder is deliberately a basic eSpeak voice, not a JARVIS imitation.
Speech is queued per tool call rather than streamed sentence by sentence.
Playback failure is not yet surfaced as a later client event. Companion
interruption depends on delivery of its recording status event. Full-duplex
echo cancellation, live screen awareness, and premium voice providers are future
work. Existing approval rules remain unchanged.
