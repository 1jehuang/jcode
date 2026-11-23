# Always-listening Jev gate evaluation

Run `justrust test -p jcode-voice --features desktop-test-support --lib continuous::gate_eval` from the repository root. The regular tests are offline: no model, credentials, microphone, or recorded utterances. All fixture dialogue was invented for this suite.

`crates/jcode-voice/src/continuous/gate_eval_cases.json` holds the expected decision for the **candidate**, not for earlier speech. In each of five contrasts the candidate stays identical while the evidence changes. The stale pair also keeps the words of the evidence identical and changes only its age. An additional partial/final pair tests a clear provisional request revised into a quotation. Its provisional positive label means “worth listening to”, not permission to take action. Other negatives contain an imperative in earlier speech or instructions addressed to the classifier. The ASR pair uses an imprecise transcription without requiring an audio service. The manual case enters history via the real manual claim, not a model-assigned positive label.

Offline checks verify distinguishable serialized inputs, exact candidate text, role and microphone history placement, expiry, bounds, ordering and isolation between recording owners. They cannot tell whether the provider noticed the difference, predict natural speech prevalence, measure speaker attribution, or establish safe end-to-end dispatch. The current wire format exposes a single recent-acceptance flag, not whether it was manual or automatic. Both paths are exercised. Partial/final annotations identify test cases rather than provider input fields. Labels are never sent to Jev.

## Optional configured-provider replay

Only after approving use of the configured account and its quota, run locally so credentials stay on the machine:

```sh
JCODE_RUN_JEV_EVAL=1 justrust test --local -p jcode-voice --features desktop-test-support --lib continuous::gate_eval::provider_replay -- --ignored --exact --nocapture
```

Replay sends one real semantic-gate evaluation per case (15 synthetic inputs) sequentially with the runtime threshold. It reports the case ID, TP/TN/FP/FN and score, then totals false positives and false negatives. Unscored cases are errors, not correct rejections. Failures are accumulated so a first wrong decision does not hide later errors. No model accuracy claim follows from an offline pass.
