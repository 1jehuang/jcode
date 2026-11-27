# Kagi web search

Use Kagi's Search API as the built-in `websearch` backend in `~/.jcode/config.toml`:

```toml
[websearch]
engine = "kagi"
prefer_native = false
fallback_engines = []
kagi_api_key_env = "KAGI_API_KEY"
```

Set `KAGI_API_KEY` in the environment of the Jcode server process to your Kagi API
key, without an authentication prefix. You can change `kagi_api_key_env` to name
another variable. Keep the key out of the config file and chat. An already-running
server needs to be restarted from an environment containing the key.

Kagi API searches incur charges. Selecting Kagi is opt-in and doesn't change the
existing defaults. `prefer_native` still defaults to true, so set it to false to
prevent supported model providers from replacing the local backend with their own
hosted search. `JCODE_WEBSEARCH_ENGINE=kagi` also selects Kagi, but doesn't disable
native search by itself.

Errors and empty results try `fallback_engines` in order. The default fallback is
Bing, so use `[]` if queries must stay with Kagi. Kagi can also be explicitly listed
as a fallback, where each attempted Kagi search may incur a charge.

## API behavior

Jcode sends `POST https://kagi.com/api/v1/search` with a sensitive
`Authorization: Bearer` header and a JSON body containing `query`,
`workflow = "search"`, and `limit`. It reads web results from `data.search`,
returning each result's title, URL, and snippet in the usual search output.
Missing titles fall back to the URL, missing snippets become empty, and rows
without URLs are skipped. `num_results` defaults to 8 and is capped at 20.
Requesting zero results doesn't call Kagi.

Requests time out after 30 seconds, don't follow redirects, and aren't
automatically retried. Missing or invalid credentials fail before network access.
HTTP failures show status codes without response bodies or credentials. Invalid
JSON and missing result arrays produce errors rather than looking like an empty
search. These errors use the same configured fallback behavior as other backends.

The implementation follows Kagi's current v1 OpenAPI contract, not its deprecated
v0 search format. Reference: `https://kagi.com/api/docs/openapi/search`, with the
schema at `https://kagi.com/api/docs/_spec/openapi.yaml`.
