// Platform detection and native module loader for pgmicro
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);

let nativeBinding = null;
const loadErrors = [];

const platformPackages = {
  "darwin-arm64": "pg-micro-darwin-arm64",
  "darwin-x64": "pg-micro-darwin-arm64", // Rosetta 2
  "linux-x64": "pg-micro-linux-x64-gnu",
  "linux-arm64": "pg-micro-linux-arm64-gnu",
};

const key = `${process.platform}-${process.arch}`;
const pkg = platformPackages[key];

if (pkg) {
  try {
    nativeBinding = require(pkg);
  } catch (e) {
    loadErrors.push(e);
  }
}

// Fallback: try loading a local .node file (for development builds)
if (!nativeBinding) {
  const localNames = {
    "darwin-arm64": "./pgmicro.darwin-arm64.node",
    "darwin-x64": "./pgmicro.darwin-x64.node",
    "linux-x64": "./pgmicro.linux-x64-gnu.node",
    "linux-arm64": "./pgmicro.linux-arm64-gnu.node",
  };
  const localName = localNames[key];
  if (localName) {
    try {
      nativeBinding = require(localName);
    } catch (e) {
      loadErrors.push(e);
    }
  }
}

if (!nativeBinding) {
  const msg = [
    `pgmicro: no native binary found for ${process.platform}-${process.arch}.`,
    `Tried: ${pkg || "(no matching platform package)"}`,
    loadErrors.map((e) => e.message).join("\n"),
  ].join("\n");
  throw new Error(msg);
}

const { BatchExecutor, Database, Statement, EncryptionCipher } = nativeBinding;
export { BatchExecutor, Database, Statement, EncryptionCipher };
