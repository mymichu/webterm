#!/usr/bin/env bash
# Builds the WebAssembly front end into web/pkg and the backend binary.
#
# Requirements: the wasm32-unknown-unknown Rust target and
#   cargo install wasm-bindgen-cli --version 0.2.130
# (the CLI version must match the wasm-bindgen crate version in crates/web).
set -euo pipefail
cd "$(dirname "$0")"

PROFILE="${PROFILE:-release}"
FLAG=$([ "$PROFILE" = release ] && echo --release || true)

cargo build -p web --target wasm32-unknown-unknown $FLAG
wasm-bindgen --target web --no-typescript --out-dir web/pkg \
  "target/wasm32-unknown-unknown/$PROFILE/web.wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Os web/pkg/web_bg.wasm -o web/pkg/web_bg.wasm
fi

cargo build -p server $FLAG
echo "Built. Run: ./target/$PROFILE/term-server  (then open http://127.0.0.1:8080)"
