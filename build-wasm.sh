#!/usr/bin/env bash
# Build the OPTIONAL browser target. The native terminal binary is the product
# and needs none of this; see README.md.
#
#   ./build-wasm.sh          -> tools/web/asciicity.wasm
set -euo pipefail
cd "$(dirname "$0")"

if ! rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
  echo "need the wasm target: rustup target add wasm32-unknown-unknown" >&2
  exit 1
fi

# --no-default-features drops the terminal frontend and with it the only
# dependency the crate has, so the browser build is the engine and nothing else.
cargo build --release --target wasm32-unknown-unknown \
  --no-default-features --features wasm

mkdir -p tools/web
cp target/wasm32-unknown-unknown/release/asciicity.wasm tools/web/asciicity.wasm
echo "wasm -> tools/web/asciicity.wasm ($(du -h tools/web/asciicity.wasm | cut -f1))"
echo "serve with: python3 tools/serve.py   then open /tools/web/"
