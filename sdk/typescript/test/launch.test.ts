import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { inheritCredentials, userJcodeHome } from "../dist/index.js";

test("platform runtime packages map to npm platform conventions", async () => {
  const { platformBinaryPackage } = await import("../dist/index.js");
  assert.equal(platformBinaryPackage("linux", "x64"), "@1jehuang/jcode-linux-x64");
  assert.equal(platformBinaryPackage("darwin", "arm64"), "@1jehuang/jcode-darwin-arm64");
  assert.equal(platformBinaryPackage("win32", "x64"), "@1jehuang/jcode-win32-x64");
  assert.equal(platformBinaryPackage("freebsd", "x64"), undefined);
});

/**
 * Cleanup must never follow a symlink out of the instance home.
 *
 * Current inheritance links files only, but cleanup must also be safe against a
 * legacy, corrupted, or attacker-injected directory link. Following that link
 * would delete the user's logins while "removing a temp directory", so this
 * test actually builds the hostile shape and checks the target survives.
 */
test("removing an instance home never follows links to real credentials", async () => {
  const instance = fs.mkdtempSync(path.join(os.tmpdir(), "jcode-sdk-instance-"));
  const precious = fs.mkdtempSync(path.join(os.tmpdir(), "jcode-precious-test-"));
  fs.mkdirSync(precious, { recursive: true });
  fs.writeFileSync(path.join(precious, "auth.json"), '{"token":"keep me"}');

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

  fs.rmSync(precious, { recursive: true, force: true });
});

test("cleanup refuses arbitrary directories even when asked directly", async () => {
  const arbitrary = fs.mkdtempSync(path.join(os.tmpdir(), "not-a-jcode-instance-"));
  fs.writeFileSync(path.join(arbitrary, "keep"), "safe");

  const { removeInstanceHomeForTest } = await import("../dist/launch.js");
  removeInstanceHomeForTest(arbitrary);

  assert.equal(fs.readFileSync(path.join(arbitrary, "keep"), "utf8"), "safe");
  fs.rmSync(arbitrary, { recursive: true, force: true });
});

test("daemon shutdown lookup waits for a delayed registry entry", async () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "jcode-daemon-race-test-"));
  const runtimeDir = path.join(home, "run");
  fs.mkdirSync(runtimeDir);
  const { waitForDaemonPidForTest } = await import("../dist/launch.js");

  const lookup = waitForDaemonPidForTest(home, runtimeDir, 1000);
  setTimeout(() => {
    fs.writeFileSync(
      path.join(home, "servers.json"),
      JSON.stringify({ instance: { socket: path.join(runtimeDir, "jcode.sock"), pid: 4242 } }),
    );
  }, 100);

  assert.equal(await lookup, 4242);
  fs.rmSync(home, { recursive: true, force: true });
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

test("credential inheritance refuses to use the user's home as the instance home", () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "jcode-same-home-test-"));
  const auth = path.join(home, "auth.json");
  fs.writeFileSync(auth, '{"token":"must survive"}');

  assert.throws(
    () => inheritCredentials(home, home),
    (error: { code?: string }) => error.code === "invalid_instance_home",
  );
  assert.equal(fs.readFileSync(auth, "utf8"), '{"token":"must survive"}');
  fs.rmSync(home, { recursive: true, force: true });
});

test("login inheritance creates file links, never credential directory links", () => {
  const sandbox = fs.mkdtempSync(path.join(os.tmpdir(), "jcode-inherit-shape-test-"));
  const oldHome = process.env.HOME;
  const oldXdg = process.env.XDG_CONFIG_HOME;
  const home = path.join(sandbox, "home");
  const config = path.join(sandbox, "config");
  const from = path.join(home, ".jcode");
  const to = path.join(sandbox, "instance");
  fs.mkdirSync(path.join(home, ".claude"), { recursive: true });
  fs.mkdirSync(path.join(config, "jcode"), { recursive: true });
  fs.mkdirSync(from, { recursive: true });
  fs.writeFileSync(path.join(home, ".claude", ".credentials.json"), "{}");
  fs.writeFileSync(path.join(config, "jcode", "n.env"), "TOKEN=test");
  fs.writeFileSync(path.join(config, "jcode", "usage.json"), "{}");
  const legacyTarget = path.join(sandbox, "legacy-linked-config");
  fs.mkdirSync(legacyTarget);
  fs.writeFileSync(path.join(legacyTarget, "precious"), "untouched");
  fs.mkdirSync(to);
  fs.symlinkSync(legacyTarget, path.join(to, "config"));

  try {
    process.env.HOME = home;
    process.env.XDG_CONFIG_HOME = config;
    const inherited = inheritCredentials(from, to);

    assert.ok(inherited.includes("config/jcode/n.env"));
    assert.ok(inherited.includes("external/.claude/.credentials.json"));
    assert.ok(fs.lstatSync(path.join(to, "config", "jcode", "n.env")).isSymbolicLink());
    assert.ok(
      fs.lstatSync(path.join(to, "external", ".claude", ".credentials.json")).isSymbolicLink(),
    );
    assert.ok(!fs.lstatSync(path.join(to, "config", "jcode")).isSymbolicLink());
    assert.ok(!fs.lstatSync(path.join(to, "external", ".claude")).isSymbolicLink());
    assert.ok(!fs.existsSync(path.join(to, "config", "jcode", "usage.json")));
    assert.equal(
      fs.readFileSync(path.join(legacyTarget, "precious"), "utf8"),
      "untouched",
      "migrating an old directory link must not modify its target",
    );
  } finally {
    if (oldHome === undefined) delete process.env.HOME;
    else process.env.HOME = oldHome;
    if (oldXdg === undefined) delete process.env.XDG_CONFIG_HOME;
    else process.env.XDG_CONFIG_HOME = oldXdg;
    fs.rmSync(sandbox, { recursive: true, force: true });
  }
});

test("the user's jcode home is resolved independently of an instance", () => {
  assert.ok(userJcodeHome().length > 0);
  assert.ok(path.isAbsolute(userJcodeHome()));
});

/**
 * A missing jcode is the most likely first-run failure, so it must be an
 * ordinary catchable error.
 *
 * Node treats an unlistened "error" event on a child process as a fatal throw
 * from inside child_process, so without a listener this killed the caller's
 * process with a raw `spawn ENOENT` stack that no `try`/`catch` could reach.
 * The failing path also has to clean up after itself: the instance home is
 * created before the spawn, so an error path that returns early leaks it.
 */
test("a missing jcode binary is catchable, and leaks nothing", async () => {
  const { JcodeClient, HarnessError } = await import("../dist/index.js");
  const root = os.tmpdir();
  const before = fs
    .readdirSync(root)
    .filter((name) => name.startsWith("jcode-sdk-instance-")).length;

  await assert.rejects(
    () =>
      JcodeClient.launch({
        binary: "jcode-definitely-not-installed",
        startupTimeoutMs: 5000,
      }),
    (error: InstanceType<typeof HarnessError>) => {
      assert.equal(error.name, "HarnessError");
      assert.equal(error.code, "jcode_not_found");
      assert.match(error.message, /not installed, or not on PATH/);
      return true;
    },
  );

  const after = fs
    .readdirSync(root)
    .filter((name) => name.startsWith("jcode-sdk-instance-")).length;
  assert.equal(after, before, "a failed launch must not leave an instance home behind");
});
