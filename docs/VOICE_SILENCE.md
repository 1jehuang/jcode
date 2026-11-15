# Silence-safe Nari transcription

## Root cause

The shared `jcode-voice` client configured `turn_detection: null`, sending silence
through manual commits and duration boundaries as recognition input. Its name
context prominently biases recognition toward Jev. The existing text-only prompt
echo filters cannot distinguish a short hallucinated `Jev.` from a real address,
and should not blacklist that legitimate utterance.

The account-authenticated cloud relay also rejected every non-null
`turn_detection`, preventing clients from enabling provider speech detection.

## Fix

Both direct and relayed Nari sessions request `server_vad` with threshold 0.5,
500 ms silence duration and 300 ms prefix padding. PCM continues streaming during
pauses, preserving the provider's speech boundaries and word onsets. The final
manual commit and pending-item acknowledgement drain remain in place so releasing
push-to-talk during speech does not drop its final words.

The client requires an `input_audio_buffer.speech_started` event before publishing
any transcript. It validates and drains unsolicited silent partial/final results
but discards their text, including from the final aggregate. A recording with no
detected speech succeeds with an empty string. Genuine spoken `Jev.` is retained.
This is not a guarantee against all ASR errors or noise classified as speech.

## Validation and rollout

Run `cargo test -p jcode-voice --no-default-features --lib` for local WebSocket
protocol regressions, including silent hallucinated partials/finals and real short
addresses. These mocks exercise client behavior, not the provider's acoustic model.
A live quiet-room microphone check is still necessary for acoustic acceptance.

Deploy the backend relay VAD validator before distributing updated mobile clients.
Older relay deployments reject VAD settings. Existing clients using omitted/null
manual configuration remain compatible with the updated relay. Desktop's direct
Nari connection does not require the relay deployment.

Provider contract checked November 15, 2026:
<https://docs.narilabs.com/transcripts-and-turn-detection>.
