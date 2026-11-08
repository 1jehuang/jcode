# Research — what was investigated and what it established

## 1. Why the embedding cohort timed out

**Observation:** the embedding cohort exceeded a 600s budget.

**Investigation:** the 600s budget was measuring a **cold compile**, not the test itself.
`jcode-embedding` is never pre-built because the workspace has **no `default-members`**, so
`cargo test -p jcode-embedding` compiles the crate from scratch inside the job.

**Conclusion:** raise the cohort timeout `600s -> 1800s`. With the crate already compiled the test
completes in **26s**, so 1800s is headroom for the compile, not for a slow test.

## 2. Why ARM64 gating was incomplete

The ARM64 asset gating had been applied to **only 1 of 4** publish paths:

1. direct artifact upload
2. blanket `find`-based upload
3. expected-asset validation
4. `SHA256SUMS` generation

**Additional regression found:** two SDK consumers still demanded the gated ARM64 asset
**unconditionally**, so gating the publisher without fixing the consumers would have broken them.
Both were fixed in the same PR.

## 3. Why the gpt-5.6-sol test flaked

### The mechanism

`test_openai_supports_codex_models` populated the ACCOUNT catalog via `populate_account_models()`,
but its credentials are synthetic: `access_token = "test"`, empty `refresh_token`, no `id_token`.
That makes `is_chatgpt_mode` **false**, so `set_model` resolves the scope:

```
api-key::9f86d081884c7d65     # = sha256("test")[0:16]
```

The test therefore only passed **because that scope was EMPTY** — the lookup missed and fell back
to the static model list, which contains `gpt-5.6-sol`.

`test_set_model_clears_persistent_ws_state` uses the **same** `"test"` token, computes the **same**
scope, and calls `replace_scope_models` with a restricted catalog (`gpt-5.3-codex` only) into it.
`replace_scope_models` **overwrites**. Once that entry exists, the static fallback is skipped,
`gpt-5.6-sol` disappears, and `set_model` bails with `Unsupported OpenAI model`.

### Why it was intermittent and CI-only

Both tests hold `lock_test_env()`, so they **serialise** — but the **order of lock acquisition is
nondeterministic**. Whichever order leaves the restricted scope in place before the other reads it
determines pass/fail. That is a scheduling artifact, which is why it surfaced in CI and not locally.

### The fix

Populate the scope the credential **actually reads**, matching the sibling test's existing pattern:

```rust
populate_account_models_for_scope(&provider.catalog_scope(), ...)
```

### Honest limitation

No deterministic local reproduction of the **failing order** was achieved. The mechanism is proven
from code, and the scope fingerprint (`9f86d081884c7d65`) matches exactly between the two tests.

## 4. Workflow-injection research (SonarCloud S7630)

SonarCloud flagged `${{ inputs.jcode_release_tag }}` interpolated directly into a `run:` block.
GitHub Actions expands `${{ }}` into the shell script **before** the shell parses it, so a value
containing shell metacharacters is executed. The fix is to pass through `env:` and dereference as a
shell variable, which the shell treats as data, not code.

## 5. Desk host failure research

- `WSLExport` scheduled task ran `C:\Users\koosh\export-wsl.ps1`, which begins with
  `wsl --terminate FedoraLinux-44` then `wsl --shutdown`.
- Those **SIGKILL the runner listener** — shown in the journal as
  `Sending SIGKILL to runner listener`. The job then hangs until GitHub times it out and reports
  failure **with no steps executed**.
- While exporting, the distro reports state `Exporting` and refuses `CreateInstance` with
  `Wsl/Service/CreateInstance/E_ILLEGAL_STATE_CHANGE`
  ("An install, uninstall, or conversion is in progress"), so runners cannot start for the export's
  duration.
- Separately, `/home/kcode-run/actions-runner` and `/home/kcode-run2/actions-runner` were **deleted**
  (only `_diag` survived), leaving both services in an ENOENT re-launch loop.
- The distro rebooted at ~**02:13:42 local** on 2026-11-07. Windows did **not** reboot (up since
  2026-11-05) and no sleep/resume was recorded. `vmIdleTimeout=2147483647` and
  `instanceIdleTimeout=-1` rule out WSL idle timeouts.
