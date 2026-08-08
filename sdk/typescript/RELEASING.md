# Releasing `@1jehuang/jcode-sdk`

Everything except the two decisions only you can make is automated and checked
in CI. This document exists so the release is a short command list rather than
a research exercise.

## Package ownership

The package is published as `@1jehuang/jcode-sdk`. The `1jehuang` user scope is owned
by the maintainer account and does not require an npm organization. The shorter
`@jcode` scope belongs to someone else and must not be used in package metadata
or documentation.

If the package name changes in the future, update `name` in
`sdk/typescript/package.json`, the install examples in this README and the SDK
README, the repository references, and the website's `/sdk` page together.

## Publishing

```bash
npm login                       # once per machine
cd sdk/typescript
npm run check                   # typecheck, build, unit tests
bash ../../scripts/test_sdk_package.sh   # the tarball as a consumer sees it
npm publish                     # publishConfig already sets public access
```

Use the **Publish TypeScript SDK** workflow and provide an existing jcode release
tag. It downloads that release's six runtime artifacts, publishes the matching
platform packages first, and then publishes the SDK. All seven package manifests
must have the same version. The main package uses exact optional dependency
versions so an SDK release can never silently pick up a different runtime.

`prepack` rebuilds `dist/` from a clean slate, so a stale build cannot be
published. `files` limits the main tarball to `dist`, `README.md`, and `LICENSE`;
confirm with `npm pack --dry-run`.

## Verifying a published release

```bash
cd "$(mktemp -d)"
npm init -y >/dev/null
npm install @1jehuang/jcode-sdk
node --input-type=module -e '
  import { JcodeClient } from "@1jehuang/jcode-sdk";
  const client = await JcodeClient.launch({ workingDir: process.cwd() });
  const session = await client.createSession();
  console.log((await client.run(session.session_id, "say hello")).text);
  await client.close();
'
```

This is the same shape as the consumer check in
`scripts/test_sdk_package.sh`, run against the registry rather than a local
tarball.

## Versioning

Semver against protocol v1 (see the SDK README's stability section):

- **Patch** for fixes that change no types and no wire shape.
- **Minor** for new methods, new events, and new optional fields.
- **Major** only for a protocol major bump, which the handshake rejects rather
  than half-supporting.

Two mechanical guards make a schema change hard to land halfway, and both run in
CI: a Rust test fails if a variant or field is missing from
`sdk/typescript/src/protocol.ts`, and a Node test fails if the tag sets diverge.

## Platform support

macOS and Linux are exercised end to end. Windows compiles and is wired up (the
bridge uses a named pipe, and the SDK derives the same pipe name, pinned by
tests on both sides) but has no live coverage yet. Do not describe it as
supported until something actually runs there.
