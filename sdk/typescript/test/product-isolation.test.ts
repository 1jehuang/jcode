import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { bundledJcodeBinary, platformBinaryPackage, resolveKcodeBinary } from "../dist/binary.js";
import { inheritCredentials, launchInstance, userJcodeHome } from "../dist/launch.js";
import { assertKcodePath } from "../dist/product.js";
import { apiSocketPath, legacySocketPath } from "../dist/sockets.js";

test("no platform silently selects an upstream npm runtime", () => {
  for (const platform of ["win32", "darwin", "linux"]) {
    for (const arch of ["x64", "arm64"]) assert.equal(platformBinaryPackage(platform, arch), undefined);
  }
  assert.equal(bundledJcodeBinary(), undefined);
  const manifest = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8"));
  assert.equal(manifest.name, "@kooshapari/kcode-sdk");
  assert.equal(manifest.private, true);
  assert.equal(manifest.optionalDependencies, undefined);
});

test("legacy home and executable overrides never win", () => {
  const saved = { legacy: process.env.JCODE_HOME, current: process.env.KCODE_HOME,
    oldBinary: process.env.JCODE_BINARY, binary: process.env.KCODE_BINARY };
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "kcode-sdk-boundary-"));
  try {
    process.env.JCODE_HOME = path.join(root, ".jcode");
    process.env.KCODE_HOME = path.join(root, ".kcode");
    process.env.JCODE_BINARY = "jcode";
    delete process.env.KCODE_BINARY;
    assert.equal(userJcodeHome(), path.join(root, ".kcode"));
    assert.equal(resolveKcodeBinary(), "kcode");
    assert.throws(() => resolveKcodeBinary("jcode"), /cross_product_path/);
    assert.throws(() => resolveKcodeBinary("jcode.exe"), /cross_product_path/);
  } finally {
    for (const [key, value] of Object.entries({ JCODE_HOME: saved.legacy, KCODE_HOME: saved.current,
      JCODE_BINARY: saved.oldBinary, KCODE_BINARY: saved.binary })) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    fs.rmSync(root, { recursive: true });
  }
});

test("credential inheritance cannot create a write-back link to jcode", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "kcode-sdk-boundary-"));
  try {
    const source = path.join(root, ".jcode");
    const destination = path.join(root, ".kcode");
    fs.mkdirSync(source);
    const marker = path.join(source, "marker");
    fs.writeFileSync(marker, "source must remain unchanged");
    const before = [fs.readFileSync(marker), fs.statSync(marker).mtimeMs, fs.statSync(source).mtimeMs];
    assert.throws(() => inheritCredentials(source, destination), /cross_product_path/);
    assert.equal(fs.existsSync(destination), false);
    assert.deepEqual([fs.readFileSync(marker), fs.statSync(marker).mtimeMs, fs.statSync(source).mtimeMs], before);
  } finally { fs.rmSync(root, { recursive: true }); }
});

test("legacy socket overrides cannot attach the SDK to jcode", () => {
  const keys = ["KCODE_RUNTIME_DIR", "KCODE_API_SOCKET", "KCODE_SOCKET", "JCODE_API_SOCKET", "JCODE_SOCKET"];
  const saved = new Map(keys.map((key) => [key, process.env[key]]));
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "kcode-sdk-boundary-"));
  try {
    process.env.KCODE_RUNTIME_DIR = path.join(root, "run");
    delete process.env.KCODE_API_SOCKET;
    delete process.env.KCODE_SOCKET;
    process.env.JCODE_API_SOCKET = path.join(root, ".jcode", "jcode-api.sock");
    process.env.JCODE_SOCKET = path.join(root, ".jcode", "jcode.sock");
    assert.equal(apiSocketPath(), path.join(root, "run", "kcode-api.sock"));
    assert.equal(legacySocketPath(), path.join(root, "run", "kcode.sock"));
  } finally {
    for (const [key, value] of saved) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    fs.rmSync(root, { recursive: true });
  }
});

test("a junction alias cannot redirect SDK storage into jcode", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "kcode-sdk-boundary-"));
  try {
    const source = path.join(root, ".jcode");
    const alias = path.join(root, ".kcode");
    fs.mkdirSync(source);
    fs.symlinkSync(source, alias, process.platform === "win32" ? "junction" : "dir");
    assert.throws(() => assertKcodePath(path.join(alias, "sessions")), /cross_product_path/);
  } finally { fs.rmSync(root, { recursive: true }); }
});

test("an explicit jcode binary is rejected before instance creation", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "kcode-sdk-boundary-"));
  try {
    const home = path.join(root, ".kcode");
    await assert.rejects(launchInstance({ binary: "jcode", jcodeHome: home, inheritLogins: false }), /cross_product_path/);
    assert.equal(fs.existsSync(home), false);
  } finally { fs.rmSync(root, { recursive: true }); }
});
