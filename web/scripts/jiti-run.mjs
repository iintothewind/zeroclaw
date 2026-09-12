/**
 * Cross-platform jiti launcher.
 *
 * npm scripts cannot use `KEY=val cmd` on Windows cmd.exe. This wrapper accepts
 * the same KEY=val prefixes and forwards them through process.env before
 * starting jiti.
 *
 * Usage: node scripts/jiti-run.mjs [KEY=VAL ...] <file> [file...]
 */
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

const require = createRequire(import.meta.url);
const jitiRoot = dirname(require.resolve("jiti/package.json"));
const jitiCli = join(jitiRoot, "lib", "jiti-cli.mjs");

const env = { ...process.env };
const files = [];
for (const arg of process.argv.slice(2)) {
  const match = /^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/.exec(arg);
  // Paths and source files are never env assignments, even if they contain '='.
  const looksLikePath =
    arg.includes("/") ||
    arg.includes("\\") ||
    /\.(?:[cm]?[jt]sx?|mjs|cjs)$/i.test(arg);
  if (match && !looksLikePath) {
    env[match[1]] = match[2];
  } else {
    files.push(arg);
  }
}

if (files.length === 0) {
  console.error("usage: node scripts/jiti-run.mjs [KEY=VAL ...] <file> [file...]");
  process.exit(2);
}

const result = spawnSync(process.execPath, [jitiCli, ...files], {
  env,
  stdio: "inherit",
});
process.exit(result.status ?? 1);
