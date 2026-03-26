#!/usr/bin/env bash
# Local build script for pgmicro npm packages.
# Builds NAPI bindings with default-postgres feature + pgmicro CLI binary,
# then moves artifacts into the correct platform package directory.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Detect platform
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
  darwin)
    case "$ARCH" in
      arm64) PLATFORM="darwin-arm64" ; NAPI_TARGET="aarch64-apple-darwin" ;;
      x86_64) PLATFORM="darwin-x64" ; NAPI_TARGET="x86_64-apple-darwin" ;;
      *) echo "Unsupported arch: $ARCH"; exit 1 ;;
    esac
    ;;
  linux)
    case "$ARCH" in
      x86_64) PLATFORM="linux-x64-gnu" ; NAPI_TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64) PLATFORM="linux-arm64-gnu" ; NAPI_TARGET="aarch64-unknown-linux-gnu" ;;
      *) echo "Unsupported arch: $ARCH"; exit 1 ;;
    esac
    ;;
  *) echo "Unsupported OS: $OS"; exit 1 ;;
esac

PLATFORM_DIR="$SCRIPT_DIR/$PLATFORM"
NODE_FILE="pgmicro.${PLATFORM}.node"

echo "==> Building pgmicro NAPI bindings for $NAPI_TARGET (features: default-postgres)"
cd "$REPO_ROOT/bindings/javascript"
npx napi build \
  --platform \
  --features default-postgres \
  --manifest-path "$REPO_ROOT/bindings/javascript/Cargo.toml" \
  --output-dir "$SCRIPT_DIR/pgmicro" \
  --target "$NAPI_TARGET"

# napi build produces index.<platform>.node — rename to pgmicro.<platform>.node
INDEX_NODE=$(ls "$SCRIPT_DIR/pgmicro"/index.*.node 2>/dev/null | head -1)
if [ -n "$INDEX_NODE" ]; then
  BASENAME=$(basename "$INDEX_NODE" | sed 's/^index\./pgmicro./')
  mv "$INDEX_NODE" "$SCRIPT_DIR/pgmicro/$BASENAME"
  NODE_FILE="$BASENAME"
fi

echo "==> Building pgmicro CLI binary"
cd "$REPO_ROOT"
cargo build --bin pgmicro

# Find the built binary
CLI_BIN="$REPO_ROOT/target/debug/pgmicro"
if [ ! -f "$CLI_BIN" ]; then
  echo "Error: pgmicro binary not found at $CLI_BIN"
  exit 1
fi

echo "==> Copying artifacts to platform package: $PLATFORM_DIR"
cp "$SCRIPT_DIR/pgmicro/$NODE_FILE" "$PLATFORM_DIR/$NODE_FILE"
cp "$CLI_BIN" "$PLATFORM_DIR/pgmicro"
chmod +x "$PLATFORM_DIR/pgmicro"

echo "==> Compiling TypeScript"
cd "$SCRIPT_DIR/pgmicro"

# Install dependencies if needed
if [ ! -d "node_modules" ]; then
  npm install
fi

npx tsc

echo ""
echo "Build complete!"
echo "  .node: $PLATFORM_DIR/$NODE_FILE"
echo "  CLI:   $PLATFORM_DIR/pgmicro"
echo "  JS:    $SCRIPT_DIR/pgmicro/dist/"
