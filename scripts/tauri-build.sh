#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-metal}"

if [[ $# -gt 0 ]]; then
  shift
fi

case "$MODE" in
  metal)
    CARGO_ARGS=(--features metal)
    ;;
  cpu)
    CARGO_ARGS=(--no-default-features)
    ;;
  *)
    echo "Usage: $0 [metal|cpu] [tauri build options...]" >&2
    exit 2
    ;;
esac

TAURI_BIN="$ROOT_DIR/node_modules/.bin/tauri"
if [[ ! -x "$TAURI_BIN" ]]; then
  echo "Tauri CLI is missing. Run npm install first." >&2
  exit 1
fi

echo "Cleaning Rust build artifacts..."
cargo clean --manifest-path "$ROOT_DIR/src-tauri/Cargo.toml"

DIARIZATION_MANIFEST="$ROOT_DIR/src-tauri/sidecars/smooth-diarize/Cargo.toml"
if [[ -f "$DIARIZATION_MANIFEST" ]]; then
  cargo clean --manifest-path "$DIARIZATION_MANIFEST"
fi

echo "Building Smooth ($MODE)..."
exec "$TAURI_BIN" build "$@" -- "${CARGO_ARGS[@]}"
