#!/usr/bin/env bash
# Run inside an Arch Linux container with the repository mounted at /src (see GitHub workflow).
set -euo pipefail
cd /src
export CARGO_TERM_COLOR=always

pacman -Sy --noconfirm --needed base-devel gtk4 libadwaita pkgconf curl

if [[ ! -f "$HOME/.cargo/env" ]]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
fi
# shellcheck source=/dev/null
source "$HOME/.cargo/env"

cargo build --verbose --release
bash ./scripts/package-linux-release.sh

# Restore ownership on the mounted workspace so the host runner can read artifacts.
if [[ -n "${HOST_UID:-}" && -n "${HOST_GID:-}" ]]; then
  chown -R "$HOST_UID:$HOST_GID" /src/dist /src/target 2>/dev/null || true
fi
