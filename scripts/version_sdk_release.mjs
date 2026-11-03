// SDK semver follows the SDK base version, with the runtime preview number.
// Never let a runtime prerelease advance npm's stable `latest` tag.
import { readFileSync, writeFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

export function releaseVersion(base, runtimeTag) {
  if (!/^\d+\.\d+\.\d+$/.test(base)) throw new Error('Expected stable SDK base version');
  const preview = /^v\d+\.\d+\.\d+-preview\.([1-9]\d*)$/.exec(runtimeTag);
  if (preview) return { version: `${base}-preview.${preview[1]}`, tag: 'preview' };
  if (/^v\d+\.\d+\.\d+$/.test(runtimeTag)) return { version: base, tag: 'latest' };
  throw new Error('Unsupported runtime release tag');
}

export function versionPackages(root, runtimeTag) {
  const mainPath = resolve(root, 'sdk/typescript/package.json');
  const main = JSON.parse(readFileSync(mainPath, 'utf8'));
  const result = releaseVersion(main.version, runtimeTag);
  const runtimeRoot = resolve(root, 'sdk/npm');
  const packages = readdirSync(runtimeRoot).map(name => resolve(runtimeRoot, name, 'package.json'));
  for (const path of packages) {
    const pkg = JSON.parse(readFileSync(path, 'utf8'));
    if (!(pkg.name in main.optionalDependencies)) throw new Error(`Unreferenced runtime: ${pkg.name}`);
    pkg.version = result.version;
    main.optionalDependencies[pkg.name] = result.version;
    writeFileSync(path, JSON.stringify(pkg, null, 2) + '\n');
  }
  main.version = result.version;
  writeFileSync(mainPath, JSON.stringify(main, null, 2) + '\n');
  return result;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = fileURLToPath(new URL('../', import.meta.url));
  const result = versionPackages(root, process.argv[2]);
  console.log(`tag=${result.tag}\nversion=${result.version}`);
}
