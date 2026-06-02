# Using the Z.AI Coding Plan with jcode

This guide covers how to log in to Z.AI (BigModel) and route a jcode session
through the Z.AI Coding Plan, including the most common gotcha:
**logging in alone is not enough — you also need to set Z.AI as the active
provider for new sessions.**

> If you only want to make Z.AI the *default* for a single command run,
> jump to [One-shot usage](#one-shot-usage) below.

---

## 1. Log in

```bash
# Interactive (opens stdin prompt for the API key):
jcode login --provider zai

# Or pipe the key in (recommended for scripts; avoids shell history):
printf '%s' "$ZAI_API_KEY" | jcode login --provider zai --no-validate
```

The API key is created at <https://z.ai/manage-apikey/apikey-list> after you
sign up for the Coding Plan. Copy it immediately — z.ai only shows the full
key once.

> **Mainland-China users:** the production endpoint
> `https://open.bigmodel.cn/api/paas/v4` is exposed under the `zhipu` /
> `bigmodel` profile and uses `ZHIPU_API_KEY` instead. See
> [Zhipu BigModel](#zhipu-bigmodel-china-endpoint) below.

After login completes you'll see something like:

```
✔  Logged in to z.ai. Stored credentials at ~/.jcode/auth.json.
```

You can verify the credential without spending a token:

```bash
jcode auth-test --provider zai
```

All four probes (`credential`, `refresh`, `provider_smoke`, `tool_smoke`)
should report `PASS`.

---

## 2. Make Z.AI the active provider

This is the step that closes [#63](https://github.com/quangdang46/jcode/issues/63):
the login UI used to leave the global default unchanged, so the next
`jcode` launch would still talk to your old provider (Claude, OpenAI, etc.).

### Option A — set Z.AI as the *global* default

```bash
jcode provider set-default zai
```

This writes `default_provider = "zai"` to `~/.jcode/config.toml` and applies
to all new sessions until you switch again.

### Option B — pin Z.AI to *this* run only

```bash
jcode --provider zai run "say hello"

# In the TUI:
jcode --provider zai
```

The `--provider` flag overrides the config-file default for the current
process tree. It does **not** persist.

### Option C — `/account` switcher inside the TUI

If you have multiple providers logged in, type `/account` (or its alias
`/accounts`) inside the TUI to bring up the picker. Pick `zai` and the
session continues against the new provider.

---

## 3. One-shot usage

For a single non-interactive command (e.g. inside a CI script) you usually
don't want to flip the global default. Use `--provider`:

```bash
jcode --provider zai --offline run 'Reply with the word PONG'
# → PONG
```

`--offline` disables the update check and telemetry, useful for quick
smoke tests.

---

## 4. Zhipu BigModel (China endpoint)

If you're inside mainland China or want to use the Zhipu-branded console,
the same Coding Plan is exposed under a different base URL:

```bash
jcode login --provider zhipu          # alias of bigmodel
# or set up via env:
export ZHIPU_API_KEY=...
jcode --provider zhipu run "你好"
```

Under the hood `zai` and `zhipu` resolve to the same backend; only the
account / billing surface differs.

---

## 5. Troubleshooting

### "Logged in but jcode still picks my old provider"

You skipped step 2 above. Run `jcode provider set-default zai` to make Z.AI
the default for new sessions, or pass `--provider zai` per invocation.

### Auth-test says credential is OK but `jcode run` returns a 401

Some Z.AI Coding Plan keys are scoped to specific model families (e.g. only
`glm-4.6`). Check which model the session resolves to:

```bash
jcode --provider zai doctor --json | jq '.providers[] | select(.provider=="zai")'
```

If your default model isn't in the plan's allowlist, switch with `/model`
inside the TUI or via the CLI:

```bash
jcode --provider zai --model glm-4.6 run 'hello'
```

### TLS / handshake failures from corporate networks

Z.AI's CDN occasionally serves TLS chains that older curl/openssl libraries
reject. Set `REQUESTS_CA_BUNDLE` to your corporate CA bundle or run
`jcode doctor` to see which TLS stack jcode is using.

---

## See also

- [`jcode auth-test`](../README.md#oauth-and-providers) — non-destructive
  credential health checks across all configured providers.
- [`jcode doctor`](../README.md#further-reading) — environment + provider
  + storage report (added in PR #210, expanded in PR #220).
- [Z.AI docs](https://docs.z.ai/) — official API reference.
