# Releasing `@jcode/sdk`

Everything except the two decisions only you can make is automated and checked
in CI. This document exists so the release is a short command list rather than
a research exercise.

## One-time: claim a package name

The package is currently named `@jcode/sdk`, a scoped name. On npm a scope maps
to a user or an organisation that must exist before you can publish under it.
As of this writing `@jcode` is **unclaimed**, `jcode-sdk` is **free**, and the
unscoped `jcode` is **taken** by an unrelated 2024 project.

Pick one:

| Option | What it needs |
| --- | --- |
| Keep `@jcode/sdk` | Create the free `jcode` org at <https://www.npmjs.com/org/create>. Free for public packages. |
| Switch to `@1jehuang/sdk` | Nothing. Your user scope already exists. |
| Switch to `jcode-sdk` | Nothing, while the name stays free. |

To switch, change `name` in `sdk/typescript/package.json`, then update the
install line in `sdk/typescript/README.md`, the repo `README.md`, and the
website's `/sdk` page.

## Publishing

```bash
npm login                       # once per machine
cd sdk/typescript
npm run check                   # typecheck, build, unit tests
bash ../../scripts/test_sdk_package.sh   # the tarball as a consumer sees it
npm publish                     # publishConfig already sets public access
```

`prepack` rebuilds `dist/` from a clean slate, so a stale build cannot be
published. `files` limits the tarball to `dist`, `README.md`, and `LICENSE`;
confirm with `npm pack --dry-run`.

## Verifying a published release

```bash
cd "$(mktemp -d)"
npm init -y >/dev/null
npm install @jcode/sdk
node --input-type=module -e '
  import { JcodeClient } from "@jcode/sdk";
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
