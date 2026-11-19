# Custom OpenAI-compatible provider profiles

Named profiles can supply HTTP headers for chat and model catalog requests.
Headers are scoped to the profile and retained during streaming retries.
Invalid header names or values fail during provider initialization.

Add a profile to `~/.jcode/config.toml`, using your endpoint and model IDs:

```toml
[providers.custom-gateway]
type = "openai-compatible"
base_url = "https://gateway.example.com/v1"
auth = "bearer"
api_key_env = "CUSTOM_GATEWAY_API_KEY"
default_model = "example-reasoning-model"
headers = { "User-Agent" = "my-coding-client/1.0" }

[[providers.custom-gateway.models]]
id = "example-reasoning-model"
reasoning = true
```

Use the header values required by your endpoint. Keep credentials in the named
environment variable or Jcode's private provider env file, outside the repository.

For an OpenAI-style reasoning model whose name Jcode does not recognize,
`reasoning = true` enables the effort selector and the `reasoning_effort` request
field. The initial effort follows `[provider].openai_reasoning_effort`; optionally
set `reasoning_effort = "high"` in the model entry for a model-specific default.
During a session, use `/effort high` or `/effort xhigh` to change it. Choose values
supported by your gateway and model.

The provider-level `supports_reasoning_effort = true` currently selects the
DeepSeek-style effort ladder. For OpenAI-style models that need values such as
`xhigh`, use the per-model `reasoning = true` declaration shown above.

Start a session:

```sh
jcode --provider-profile custom-gateway
```

For live validation, pass the model explicitly so `auth-test` uses the intended
model rather than its generic model-discovery path:

```sh
jcode --provider-profile custom-gateway --model example-reasoning-model auth-test
```

`auth-test` makes inference requests and exercises a Bash tool round trip.
