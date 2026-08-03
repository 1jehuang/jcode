import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { inheritCredentials, userJcodeHome } from "../dist/index.js";

/**
 * Cleanup must never follow a symlink out of the instance home.
 *
 * A launched instance links the user's real credential stores into its home so
 * token rotation stays coherent. That makes the obvious cleanup,
 * `rm -rf $home`, catastrophic: following those links deletes the user's
 * logins while "removing a temp directory". This is the single most damaging
 * thing the SDK could do, so it gets a test that actually builds the shape and
 * checks the target survives.
 */
test("removing an instance home never follows links to real credentials", async () => {
  const sandbox = fs.mkdtempSync(path.join(os.tmpdir(), "jcode-cleanup-test-"));
  const precious = path.join(sandbox, "real-home");
  fs.mkdirSync(precious, { recursive: true });
  fs.writeFileSync(path.join(precious, "auth.json"), '{"token":"keep me"}');

  const instance = path.join(sandbox, "instance");
  fs.mkdirSync(path.join(instance, "external"), { recursive: true });
  fs.symlinkSync(precious, path.join(instance, "external", "linked-home"));
  fs.symlinkSync(path.join(precious, "auth.json"), path.join(instance, "auth.json"));
  fs.writeFileSync(path.join(instance, "own-state.json"), "{}");

  // Exercise the same routine `close()` uses on an ephemeral instance.
  const { removeInstanceHomeForTest } = await import("../dist/launch.js");
  removeInstanceHomeForTest(instance);

  assert.ok(!fs.existsSync(instance), "the instance home should be gone");
  assert.ok(
    fs.existsSync(path.join(precious, "auth.json")),
    "the user's real credentials must survive instance cleanup",
  );
  assert.equal(
    fs.readFileSync(path.join(precious, "auth.json"), "utf8"),
    '{"token":"keep me"}',
    "the user's credentials must be untouched, not merely present",
  );

  fs.rmSync(sandbox, { recursive: true, force: true });
});

/** Inheriting must share rotating credentials, not copy them. */
test("rotating credentials are shared so token refresh stays coherent", () => {
  const sandbox = fs.mkdtempSync(path.join(os.tmpdir(), "jcode-inherit-test-"));
  const from = path.join(sandbox, "user");
  const to = path.join(sandbox, "instance");
  fs.mkdirSync(from, { recursive: true });
  fs.writeFileSync(path.join(from, "auth.json"), '{"refresh":"v1"}');
  fs.writeFileSync(path.join(from, "config.toml"), "[auth]\n");
  // Derived failure state must never be inherited: a stale rejection record
  // makes a fresh instance refuse to refresh credentials that work.
  fs.writeFileSync(path.join(from, "auth-refresh-state.json"), '{"claude":{"rejected":1}}');

  const inherited = inheritCredentials(from, to);

  assert.ok(
    fs.lstatSync(path.join(to, "auth.json")).isSymbolicLink(),
    "auth.json must be shared, since refresh tokens rotate and copies diverge",
  );
  assert.ok(
    !fs.lstatSync(path.join(to, "config.toml")).isSymbolicLink(),
    "config.toml must be copied so the instance can edit its own settings",
  );
  assert.ok(
    !fs.existsSync(path.join(to, "auth-refresh-state.json")),
    "derived auth failure state must not be inherited",
  );
  assert.ok(inherited.includes("auth.json"));

  // A rotation on either side must be visible to the other.
  fs.writeFileSync(path.join(from, "auth.json"), '{"refresh":"v2"}');
  assert.equal(fs.readFileSync(path.join(to, "auth.json"), "utf8"), '{"refresh":"v2"}');

  fs.rmSync(sandbox, { recursive: true, force: true });
});

test("the user's jcode home is resolved independently of an instance", () => {
  assert.ok(userJcodeHome().length > 0);
  assert.ok(path.isAbsolute(userJcodeHome()));
});
