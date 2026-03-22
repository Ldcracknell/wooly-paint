#!/usr/bin/env bash
# Package a release tarball for Linux (run after `cargo build --release`).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
BIN="${TARGET_DIR}/release/wooly-paint"
if [[ ! -f "$BIN" ]]; then
  echo "Missing $BIN — build with: cargo build --release" >&2
  exit 1
fi
VER=$(awk -F'"' '/^version = / {print $2; exit}' Cargo.toml)
NAME="wooly-paint-${VER}-linux-arch-x86_64"
rm -rf "dist/$NAME" "dist/${NAME}.tar.gz"
mkdir -p "dist/$NAME"
cp "$BIN" "dist/$NAME/"
cp src/assets/icon.png "dist/$NAME/"
cp packaging/linux-portable.txt "dist/$NAME/README.txt"
tar -czvf "dist/${NAME}.tar.gz" -C dist "$NAME"
echo "Wrote dist/${NAME}.tar.gz"
