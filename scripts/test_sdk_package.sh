#!/usr/bin/env bash
# Verify the *published* @1jehuang/jcode-sdk tarball, not just its source.
#
# `npm run check` compiles src/ and runs tests against it. A consumer never
# sees src/: they see whatever `files`, `exports`, `main`, and `types` let out
# of the tarball. Those are easy to get wrong in ways no source test notices
# (a missing dist/, a stale build, types that resolve only inside the repo), and
# the failure lands on the user at `npm install`. So pack it, install it into a
# throwaway project, and import it as ESM, as CJS, and through tsc.
#
# Usage: scripts/test_sdk_package.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk_dir="$repo_root/sdk/typescript"
work="$(mktemp -d "${TMPDIR:-/tmp}/jcode-sdk-pack-XXXXXX")"
trap 'rm -rf "$work"' EXIT

echo "== packing =="
npm --prefix "$sdk_dir" install --no-audit --no-fund --silent
tarball="$(cd "$sdk_dir" && npm pack --silent | tail -n 1)"
tarball="$sdk_dir/$tarball"
trap 'rm -rf "$work"; rm -f "$tarball"' EXIT
echo "packed $tarball"

echo "== installing into a fresh consumer =="
cd "$work"
npm init -y --silent >/dev/null
runtime_tarball=""
if command -v jcode >/dev/null 2>&1 && [ "$(uname -s)" = Linux ]; then
  case "$(uname -m)" in
    x86_64) runtime_package=linux-x64 ;;
    aarch64) runtime_package=linux-arm64 ;;
    *) runtime_package="" ;;
  esac
  if [ -n "$runtime_package" ]; then
    runtime_stage="$work/runtime-stage"
    mkdir -p "$runtime_stage/bin"
    cp "$repo_root/sdk/npm/$runtime_package/package.json" "$runtime_stage/"
    cp "$repo_root/sdk/npm/$runtime_package/README.md" "$runtime_stage/"
    cp "$(command -v jcode)" "$runtime_stage/bin/jcode"
    chmod +x "$runtime_stage/bin/jcode"
    runtime_tarball="$(cd "$runtime_stage" && npm pack --silent)"
    runtime_tarball="$runtime_stage/$runtime_tarball"
  fi
fi

if [ -n "$runtime_tarball" ]; then
  npm install "$runtime_tarball" "$tarball" --no-audit --no-fund --silent
else
  npm install "$tarball" --no-audit --no-fund --silent
fi

echo "== ESM import =="
node --input-type=module -e '
import { JcodeClient, HarnessError, API_VERSION_MAJOR } from "@1jehuang/jcode-sdk";
if (typeof JcodeClient !== "function") throw new Error("JcodeClient missing");
if (typeof HarnessError !== "function") throw new Error("HarnessError missing");
if (API_VERSION_MAJOR !== 1) throw new Error("unexpected protocol version");
console.log("esm ok");
'

if [ -n "$runtime_tarball" ]; then
  echo "== bundled runtime resolution =="
  node --input-type=module -e '
  import { bundledJcodeBinary } from "@1jehuang/jcode-sdk";
  const binary = bundledJcodeBinary();
  if (!binary || !binary.includes("@1jehuang/jcode-linux-")) {
    throw new Error(`platform runtime was not resolved: ${binary}`);
  }
  console.log(`runtime ok: ${binary}`);
  '
fi

echo "== CJS require =="
node --input-type=commonjs -e '
const sdk = require("@1jehuang/jcode-sdk");
if (typeof sdk.JcodeClient !== "function") throw new Error("JcodeClient missing under require");
console.log("cjs ok");
'

echo "== TypeScript types resolve for a consumer =="
npm install --no-audit --no-fund --silent typescript @types/node
cat > tsconfig.json <<'JSON'
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "noEmit": true,
    "types": ["node"]
  },
  "include": ["consumer.ts", "narrowing.ts"]
}
JSON
cat > consumer.ts <<'TS'
import { JcodeClient, HarnessError, type TurnResult, type ApiEvent } from "@1jehuang/jcode-sdk";

export async function demo(prompt: string): Promise<TurnResult> {
  const client = await JcodeClient.connect({ clientName: "package-test/1.0" });
  try {
    const session = await client.createSession(process.cwd());
    return await client.run(session.session_id, prompt, {
      autoApprove: true,
      onEvent: (event: ApiEvent) => {
        if (event.ev === "text_delta") process.stdout.write(event.text);
      },
    });
  } catch (error) {
    if (error instanceof HarnessError) console.error(error.code);
    throw error;
  } finally {
    client.close();
  }
}
TS
# Narrowing is the whole reason the event union is a discriminated union. It
# broke once already: a `{ ev: string; [k: string]: unknown }` catch-all member
# widened the discriminant, so `event.ev === "text_delta"` narrowed to nothing
# and every field came back `unknown`. That compiles fine inside the repo and
# only bites consumers, so assert it from a consumer.
cat > narrowing.ts <<'TS'
import { isKnownEvent, type AnyApiEvent, type ApiEvent } from "@1jehuang/jcode-sdk";

export function summarize(event: ApiEvent): string {
  switch (event.ev) {
    case "text_delta":
      return event.text;
    case "tool_done":
      return `${event.name}: ${event.output}`;
    case "token_usage":
      return `${event.input + event.output} tokens`;
    case "permission_request":
      return event.request_id;
    default:
      return event.ev;
  }
}

/** An unknown kind must still be accepted, and narrow through `isKnownEvent`. */
export function summarizeAny(event: AnyApiEvent): string {
  return isKnownEvent(event) ? summarize(event) : `unknown:${event.ev}`;
}
TS
npx --no-install tsc -p tsconfig.json
echo "types ok"

# Everything above proves the tarball imports and typechecks. It does not prove
# the thing a consumer actually does: install the package and launch jcode from
# PATH, with no repo, no cargo, and no locally built binary. That path has its
# own failure modes (a missing `api-bridge` subcommand in the installed build,
# a leaked daemon, an instance home that never gets removed), and none of them
# are visible from inside the repo.
if command -v jcode >/dev/null 2>&1; then
  echo "== launching a private instance as a consumer would =="
  cat > launch-consumer.mjs <<'JS'
import { JcodeClient } from "@1jehuang/jcode-sdk";
import fs from "node:fs";

// No `binary` option: use the platform npm package, exactly like a consumer.
const client = await JcodeClient.launch({ workingDir: process.cwd() });
const home = client.instanceHome;

const sessions = await client.listSessions();
if (sessions.length !== 0) {
  throw new Error(`a fresh instance reported ${sessions.length} sessions`);
}

await client.close();

// The daemon keeps writing for a moment after shutdown, so "gone" has to mean
// "stayed gone" rather than "was briefly absent".
await new Promise((resolve) => setTimeout(resolve, 5000));
if (fs.existsSync(home)) {
  throw new Error(`instance home leaked: ${fs.readdirSync(home).join(", ")}`);
}
console.log("launch ok");
JS

  daemons_before="$(pgrep -cf 'jcode --provider auto serve' || true)"
  node launch-consumer.mjs
  sleep 3
  daemons_after="$(pgrep -cf 'jcode --provider auto serve' || true)"
  if [ "$daemons_before" != "$daemons_after" ]; then
    echo "FAIL: launch() leaked a daemon ($daemons_before -> $daemons_after)"
    exit 1
  fi
  echo "no daemon leaked ($daemons_before -> $daemons_after)"
else
  echo "== skipping launch check: jcode is not on PATH =="
fi

echo "SDK package check passed."
