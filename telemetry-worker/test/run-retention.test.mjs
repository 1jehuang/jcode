import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";

const runner = new URL("../scripts/run-retention.mjs", import.meta.url).href;
function run(apply, fail = false) {
  const source = `
    process.argv = ['node', 'run-retention.mjs', ...${JSON.stringify(apply ? ["--apply"] : [])}];
    globalThis.fetch = async (url, options) => {
      if (String(url).endsWith('/v1/health')) return Response.json({ok:true});
      if (!${apply}) throw new Error('Read-only mode attempted authenticated query');
      const {sql} = JSON.parse(options.body);
      if (${JSON.stringify(fail)} === "network" && /^DELETE FROM events/.test(sql.trim())) {
        throw new Error('simulated network failure');
      }
      if (${fail === true} && /^DELETE FROM events/.test(sql.trim())) {
        return Response.json({success:false,errors:[{code:7500,message:'simulated failure'}]});
      }
      if (/oauth_accounts|usage_report_ledger|INSERT|model_prices/i.test(sql)) {
        throw new Error('Hourly runner attempted unrelated nightly work: ' + sql);
      }
      return Response.json({success:true,result:[{success:true,results:[],meta:{changes:0,size_after:1000}}]});
    };
    await import(${JSON.stringify(runner)});
  `;
  return spawnSync(process.execPath, ["--input-type=module", "--eval", source], {
    encoding: "utf8",
    env: { ...process.env, CLOUDFLARE_API_TOKEN: "test-only-not-a-secret" },
    timeout: 20_000,
  });
}

test("retention operator defaults to read-only health without D1 access", () => {
  const result = run(false);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Read-only/);
  assert.doesNotMatch(result.stdout, /statements/);
});

test("explicit retention operator applies hourly policy without nightly work", () => {
  const result = run(true);
  assert.equal(result.status, 0, result.stderr);
  const summary = result.stdout.split("\n").filter(Boolean).map((line) => JSON.parse(line)).find((value) => "statements" in value);
  assert.ok(summary.statements > 0);
  assert.equal(summary.changed, 0);
  assert.equal(summary.failures, 0, result.stderr);
});

test("retention operator exits nonzero when scheduled cleanup catches D1 failures", () => {
  const result = run(true, true);
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stdout, /"failures":[1-9]/);
});

test("retention operator exits nonzero on caught network failures", () => {
  const result = run(true, "network");
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stdout, /"failures":[1-9]/);
});
