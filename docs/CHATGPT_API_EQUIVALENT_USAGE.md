# ChatGPT OAuth API-equivalent usage

The Accounts views show locally recorded ChatGPT OAuth token usage and its estimated equivalent API cost. These estimates are **not charges to the ChatGPT subscription**, invoices, or an account balance.

## Time periods and scope

- **Today** uses the machine's local calendar date, starting at local midnight, rather than a rolling 24-hour or UTC window.
- **Lifetime** accumulates usage since Jcode began recording it, with the first-recorded timestamp shown when available.
- Only usage observed by this Jcode installation is counted. ChatGPT website activity and other clients are not included.
- Historical session token counts are not backfilled because they do not reliably identify the credential and model for each response.
- Usage is separated by saved OAuth account label. The record is stored locally in `openai_oauth_usage.json` under the Jcode data directory, independently of actual API-key spend.

## Cost calculation

The tracker uses Jcode's curated OpenAI API price table for the response's model and service tier. OpenAI input token totals include cached input, so the estimate charges uncached input at the normal input rate, cached input at the cached-input rate, and output at the output rate. Reasoning tokens already included in output are not added again.

Unknown model prices and incomplete usage reports are shown explicitly, not silently priced as free. A mixed total reports the known estimate plus unknown cost. No recorded usage is distinct from a measured zero-dollar result.

## Surfaces

The account details in the terminal and desktop expose the same today and lifetime summaries. The CLI's `jcode usage --json` includes these details in the matching provider report's `extra_info`, so clients do not need to read the ledger directly or compute subscription billing.
