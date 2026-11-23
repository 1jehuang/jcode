# Non-interactive / agent-driven login

Every jcode provider can be logged in without a human at a terminal prompt.
Add `--json` to get exactly one JSON object on stdout
(`{"status":"authenticated"|"pending"|"cancelled", "provider", "credentials_path", ...}`).
Add `--no-validate` to skip the live provider check (offline/CI).

## API-key providers

`anthropic-api`, `openai-api`, `openrouter`, `gemini`, `cursor`, `bedrock`,
`azure`, and every OpenAI-compatible profile (`deepseek`, `groq`, `openai-compatible`, ...):

```sh
jcode login --provider openrouter --api-key-env OPENROUTER_API_KEY --json   # key from env, not argv
echo "$KEY" | jcode login --provider anthropic-api --api-key - --json        # key from stdin
jcode login --provider bedrock --api-key-env AWS_BEDROCK_KEY --region us-west-2
jcode login --provider azure --api-base https://res.openai.azure.com --model gpt-4.1 --api-key-env AZ_KEY
jcode login --provider azure --api-base https://res.openai.azure.com --model gpt-4.1   # Entra ID
jcode login --provider openai-compatible --api-base http://localhost:11434/v1 --api-key - --model llama3
```

`--provider gemini` with a key saves a Gemini Developer API key. OAuth-only
providers reject `--api-key` with a pointer to the flow below.

## Zero-command setup

Exporting the provider env var (for example `ANTHROPIC_API_KEY`) or writing
`~/.config/jcode/<provider>.env` is also enough. See
[AUTH_CREDENTIAL_SOURCES.md](AUTH_CREDENTIAL_SOURCES.md).

## OAuth / device providers (agent hands a URL to the human)

`claude`, `openai`, `gemini`, `antigravity`, `google`, `copilot`, `muse-subscription`:

```sh
jcode login --provider claude --print-auth-url --json --flow-id a1     # -> {"status":"pending","auth_url":...}
jcode login --provider claude --callback-url "$URL" --flow-id a1 --json  # or --auth-code CODE, or `-` for stdin
jcode login --provider copilot --complete --flow-id a1 --json            # device-code flows
jcode login --provider openai --device-code --json                       # OpenAI device code
jcode login --provider claude --cancel --flow-id a1
```

When stdin is not a terminal, OAuth providers switch to this flow automatically.
A running jcode server is notified of new credentials after every login.
