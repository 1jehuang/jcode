import fs from "node:fs";
import path from "node:path";
import { HarnessError } from "./errors.js";

/** Refuse both textual jcode paths and existing aliases into its installation. */
export function assertKcodePath(input: string): string {
  const resolved = path.resolve(input);
  const belongsToJcode = (value: string) => value.split(/[\\/]+/u).some((part) =>
    /^(?:jcode(?:[.-].*)?|\.jcode.*)$/iu.test(part),
  );
  let existing = resolved;
  while (!fs.existsSync(existing) && path.dirname(existing) !== existing) {
    existing = path.dirname(existing);
  }
  if (belongsToJcode(resolved) || belongsToJcode(fs.realpathSync(existing))) {
    throw new HarnessError("cross_product_path", "KCode cannot use jcode binaries or storage");
  }
  return resolved;
}
