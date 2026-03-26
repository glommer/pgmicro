#!/usr/bin/env node
import { createRequire } from "node:module";
import { execFileSync } from "node:child_process";
import { chmodSync } from "node:fs";
import { join, dirname } from "node:path";

const require = createRequire(import.meta.url);

const platformPackages = {
  "darwin-arm64": "pg-micro-darwin-arm64",
  "darwin-x64": "pg-micro-darwin-arm64", // Rosetta 2
  "linux-x64": "pg-micro-linux-x64-gnu",
  "linux-arm64": "pg-micro-linux-arm64-gnu",
};

const key = `${process.platform}-${process.arch}`;
const pkg = platformPackages[key];

if (!pkg) {
  console.error(`pgmicro: unsupported platform ${key}`);
  process.exit(1);
}

let binaryPath;
try {
  const pkgJsonPath = require.resolve(`${pkg}/package.json`);
  binaryPath = join(dirname(pkgJsonPath), "pgmicro");
} catch (e) {
  console.error(`pgmicro: could not find platform package "${pkg}".`);
  console.error("Run: npm install");
  process.exit(1);
}

try {
  chmodSync(binaryPath, 0o755);
  execFileSync(binaryPath, process.argv.slice(2), { stdio: "inherit" });
} catch (e) {
  if (e.status != null) {
    process.exit(e.status);
  }
  throw e;
}
