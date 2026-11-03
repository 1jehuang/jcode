import { test } from 'node:test';
import assert from 'node:assert/strict';
import { cpSync, mkdtempSync, readFileSync, rmSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { releaseVersion, versionPackages } from './version_sdk_release.mjs';

test('preview is opt-in and stable retains latest', () => {
  assert.deepEqual(releaseVersion('1.2.0', 'v1.0.0-preview.30'), { version: '1.2.0-preview.30', tag: 'preview' });
  assert.deepEqual(releaseVersion('1.2.0', 'v1.0.0'), { version: '1.2.0', tag: 'latest' });
  for (const tag of ['master', 'v1.0.0-rc.1', 'v1.0.0-preview.0', 'v1.0.0-preview.30;evil']) {
    assert.throws(() => releaseVersion('1.2.0', tag));
  }
});

test('all six runtime dependencies pin the same preview version', () => {
  const root = mkdtempSync(resolve(tmpdir(), 'jcode-sdk-version-'));
  try {
    const source = new URL('../', import.meta.url);
    cpSync(new URL('sdk/typescript/package.json', source), resolve(root, 'sdk/typescript/package.json'), { recursive: true });
    for (const name of readdirSync(new URL('sdk/npm/', source))) {
      cpSync(new URL(`sdk/npm/${name}/package.json`, source), resolve(root, `sdk/npm/${name}/package.json`), { recursive: true });
    }
    versionPackages(root, 'v1.0.0-preview.42');
    const read = path => JSON.parse(readFileSync(resolve(root, path), 'utf8'));
    const main = read('sdk/typescript/package.json');
    assert.equal(main.version, '1.2.0-preview.42');
    assert.equal(Object.keys(main.optionalDependencies).length, 6);
    for (const name of readdirSync(resolve(root, 'sdk/npm'))) {
      const runtime = read(`sdk/npm/${name}/package.json`);
      assert.equal(runtime.version, main.version);
      assert.equal(main.optionalDependencies[runtime.name], runtime.version);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
