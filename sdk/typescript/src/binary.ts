import { createRequire } from "node:module";
import path from "node:path";

const require = createRequire(import.meta.url);

const PLATFORM_PACKAGES: Record<string, string> = {
  "linux-x64": "@1jehuang/jcode-linux-x64",
  "linux-arm64": "@1jehuang/jcode-linux-arm64",
  "darwin-x64": "@1jehuang/jcode-darwin-x64",
  "darwin-arm64": "@1jehuang/jcode-darwin-arm64",
  "win32-x64": "@1jehuang/jcode-win32-x64",
  "win32-arm64": "@1jehuang/jcode-win32-arm64",
};

/** The optional npm package containing the runtime for this machine. */
export function platformBinaryPackage(
  platform = process.platform,
  arch = process.arch,
): string | undefined {
  return PLATFORM_PACKAGES[`${platform}-${arch}`];
}

/**
 * Resolve the jcode executable installed as an optional platform dependency.
 * Returns undefined on unsupported platforms or when optional dependencies
 * were deliberately omitted, allowing launch() to fall back to PATH.
 */
export function bundledJcodeBinary(): string | undefined {
  const packageName = platformBinaryPackage();
  if (!packageName) return undefined;
  try {
    const manifest = require.resolve(`${packageName}/package.json`);
    return path.join(path.dirname(manifest), "bin", process.platform === "win32" ? "jcode.exe" : "jcode");
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === "MODULE_NOT_FOUND") return undefined;
    throw error;
  }
}
