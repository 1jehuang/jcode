import { test } from "node:test";
import assert from "node:assert/strict";
import { compareModelUsage, type ModelUsage } from "../dist/index.js";

test("route usage ranks tracked turns and prior selections separately", () => {
  const legacy: ModelUsage = { count: 0, selection_count: 10, last_selected_unix_secs: 99 };
  const frequent: ModelUsage = { count: 2, selection_count: 0, last_used_unix_secs: 10, tracking_started_unix_secs: 1 };
  const recent: ModelUsage = { count: 1, selection_count: 0, last_used_unix_secs: 20, tracking_started_unix_secs: 1 };
  const unused: ModelUsage = { count: 0, selection_count: 0, tracking_started_unix_secs: 1 };
  const rows = [undefined, unused, legacy, recent, frequent];
  // Array.sort always moves undefined elements last without calling compareFn.
  rows.sort(compareModelUsage);
  assert.deepEqual(rows, [frequent, recent, legacy, unused, undefined]);
  assert.equal(compareModelUsage(frequent, frequent), 0);
  assert.ok(compareModelUsage({ ...frequent, last_used_unix_secs: 11 }, frequent) < 0);
  assert.ok(compareModelUsage({ ...legacy, last_selected_unix_secs: 100 }, legacy) < 0);
  assert.equal(compareModelUsage(undefined, undefined), 0);
  assert.ok(compareModelUsage(undefined, unused) > 0);
});
