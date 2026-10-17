/** KCode has no maintained npm runtime channel for this branch. */
import { assertKcodePath } from "./product.js";

export function resolveKcodeBinary(explicit?: string): string {
  const binary = explicit ?? process.env.KCODE_BINARY ?? "kcode";
  assertKcodePath(binary);
  return binary;
}

export function platformBinaryPackage(
  _platform = process.platform,
  _arch = process.arch,
): string | undefined {
  return undefined;
}

/** Preserve the SDK API while refusing every upstream bundled runtime. */
export function bundledJcodeBinary(): string | undefined {
  return undefined;
}
