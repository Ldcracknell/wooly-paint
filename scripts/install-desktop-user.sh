#!/usr/bin/env bash
# Install Freedesktop menu entry + themed icon (~/.local/share).
# File managers do not show a custom icon on the raw ELF binary; the launcher does.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_ID="dev.woolymelon.WoolyPaint"

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

BIN_DIR="$(cd "$(dirname "$BIN")" && pwd)"
ICON_SRC=""
if [[ -f "$ROOT/src/assets/icon.png" ]]; then
  ICON_SRC="$ROOT/src/assets/icon.png"
elif [[ -f "$BIN_DIR/icon.png" ]]; then
  ICON_SRC="$BIN_DIR/icon.png"
fi
if [[ -z "$ICON_SRC" ]]; then
  echo "No icon found (expected $ROOT/src/assets/icon.png or $BIN_DIR/icon.png)." >&2
  exit 1
fi

# Working directory: repo root when developing; extract folder for portable tarball layout.
WORK_DIR="$ROOT"
if [[ "$ICON_SRC" == "$BIN_DIR/icon.png" ]]; then
  WORK_DIR="$BIN_DIR"
fi

ICON_DEST="${HOME}/.local/share/icons/hicolor/128x128/apps/${APP_ID}.png"
mkdir -p ~/.local/share/icons/hicolor/128x128/apps
cp "$ICON_SRC" "$ICON_DEST"

DESKTOP="${HOME}/.local/share/applications/${APP_ID}.desktop"
mkdir -p ~/.local/share/applications
cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=Wooly Paint
Comment=Raster paint
Exec=$BIN %F
TryExec=$BIN
Icon=$ICON_DEST
Path=$WORK_DIR
Terminal=false
Categories=Graphics;2DGraphics;
StartupWMClass=$APP_ID
EOF

gtk-update-icon-cache -f ~/.local/share/icons/hicolor/ 2>/dev/null || true
update-desktop-database ~/.local/share/applications/ 2>/dev/null || true
kbuildsycoca6 --noincremental 2>/dev/null || kbuildsycoca5 --noincremental 2>/dev/null || true
echo "Installed $DESKTOP (KDE: re-pin from the app menu if the taskbar icon was wrong while closed)."
