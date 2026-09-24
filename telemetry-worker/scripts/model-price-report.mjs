// Coverage uses the raw reported token counters, not normalized billable tokens.
// Keep unresolved identities visible. An absent price is never evidence of $0.
export function buildModelPriceReport(observed, prices) {
  let reportedTokens = 0;
  let unpricedReportedTokens = 0;
  const missing = new Map();
  for (const row of observed) {
    const tokens = Number(row.tokens ?? 0);
    if (!Number.isFinite(tokens) || tokens < 0) {
      throw new Error(`Invalid reported token count for ${row.model}`);
    }
    reportedTokens += tokens;
    const price = prices.get(row.model);
    if (price && price.kind !== "unpriced" && price.input != null) continue;
    unpricedReportedTokens += tokens;
    const item = missing.get(row.model) ?? {
      model: row.model, reported_tokens: 0, sessions: 0, providers: new Set(),
    };
    item.reported_tokens += tokens;
    item.sessions += Number(row.sessions ?? 0);
    if (row.provider != null) item.providers.add(row.provider);
    missing.set(row.model, item);
  }
  return {
    token_basis: "raw reported counters, including potentially overlapping cache counts",
    reported_tokens: reportedTokens,
    unpriced_reported_tokens: unpricedReportedTokens,
    matched_token_pct: reportedTokens ? 100 * (1 - unpricedReportedTokens / reportedTokens) : null,
    unpriced: [...missing.values()]
      .map((item) => ({ ...item, providers: [...item.providers].sort() }))
      .sort((a, b) => b.reported_tokens - a.reported_tokens || String(a.model).localeCompare(String(b.model))),
  };
}
