#!/usr/bin/env bash
# Run wooly-paint under Valgrind on Arch (and similar) systems where ld.so is stripped.
# Without debug symbols for the dynamic linker, Valgrind fails with:
#   memcmp in ld-linux-x86-64.so.2 was not found
# Arch Linux publishes split debug via debuginfod; this URL is the standard endpoint.
#
# Noise reduction (see valgrind-session.log in this repo):
# - GSK_RENDERER=cairo avoids GTK's Vulkan/GL path (NVIDIA driver false positives, huge "possibly lost").
# - --show-realloc-size-zero=no silences vendor libs that call realloc(ptr, 0).
# - scripts/valgrind.supp: ring/rustls constant-time TLS, exit-time Fontconfig blocks.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export DEBUGINFOD_URLS="${DEBUGINFOD_URLS:-https://debuginfod.archlinux.org}"
export GSK_RENDERER="${GSK_RENDERER:-cairo}"
BIN="${BIN:-$ROOT/target/debug/wooly-paint}"
if [[ ! -x "$BIN" ]]; then
  echo "Missing binary: $BIN — run: cargo build" >&2
  exit 1
fi
# Leaks are listed in LEAK SUMMARY; do not count them as ERROR SUMMARY (noisy for GTK/fonts).
# Override when needed, e.g.  ./scripts/valgrind-wooly-paint.sh --errors-for-leak-kinds=definite
exec valgrind \
  --leak-check=full \
  --show-leak-kinds=definite,indirect,possible \
  --errors-for-leak-kinds=none \
  --show-realloc-size-zero=no \
  --suppressions="$ROOT/scripts/valgrind.supp" \
  "$@" \
  "$BIN"
