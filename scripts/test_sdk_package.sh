#!/usr/bin/env bash
# Verify the *published* @jcode/sdk tarball, not just its source.
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
npm install "$tarball" --no-audit --no-fund --silent

echo "== ESM import =="
node --input-type=module -e '
import { JcodeClient, HarnessError, API_VERSION_MAJOR } from "@jcode/sdk";
if (typeof JcodeClient !== "function") throw new Error("JcodeClient missing");
if (typeof HarnessError !== "function") throw new Error("HarnessError missing");
if (API_VERSION_MAJOR !== 1) throw new Error("unexpected protocol version");
console.log("esm ok");
'

echo "== CJS require =="
node --input-type=commonjs -e '
const sdk = require("@jcode/sdk");
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
import { JcodeClient, HarnessError, type TurnResult, type ApiEvent } from "@jcode/sdk";

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
import { isKnownEvent, type AnyApiEvent, type ApiEvent } from "@jcode/sdk";

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

echo "SDK package check passed."
