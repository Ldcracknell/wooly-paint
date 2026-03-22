#!/usr/bin/env bash
# Install Freedesktop menu entry + themed icon (~/.local/share).
# File managers do not show a custom icon on the raw ELF binary; the launcher does.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_ID="dev.woolymelon.WoolyPaint"
ICON_SRC="$ROOT/src/assets/icon.png"

BIN="${1:-}"
if [[ -z "$BIN" ]]; then
  if [[ -x "$ROOT/target/release/wooly-paint" ]]; then
    BIN="$ROOT/target/release/wooly-paint"
  else
    BIN="$ROOT/target/debug/wooly-paint"
  fi
fi

if [[ ! -f "$BIN" ]]; then
  echo "No binary found. Build first, or pass the path: $0 /path/to/wooly-paint" >&2
  exit 1
fi
if [[ ! -f "$ICON_SRC" ]]; then
  echo "Missing $ICON_SRC" >&2
  exit 1
fi

mkdir -p ~/.local/share/icons/hicolor/128x128/apps
cp "$ICON_SRC" ~/.local/share/icons/hicolor/128x128/apps/${APP_ID}.png

mkdir -p ~/.local/share/applications
cat > ~/.local/share/applications/${APP_ID}.desktop <<EOF
[Desktop Entry]
Type=Application
Name=Wooly Paint
Comment=Raster paint
Exec=$BIN %F
Icon=$APP_ID
Path=$ROOT
Terminal=false
Categories=Graphics;2DGraphics;
StartupWMClass=$APP_ID
EOF

gtk-update-icon-cache -f ~/.local/share/icons/hicolor/ 2>/dev/null || true
update-desktop-database ~/.local/share/applications/ 2>/dev/null || true
echo "Installed ~/.local/share/applications/${APP_ID}.desktop (icon $APP_ID)."
