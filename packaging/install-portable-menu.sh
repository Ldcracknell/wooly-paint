#!/usr/bin/env bash
# Install a Freedesktop launcher + themed icon so pinning the app keeps the correct icon.
# Run once from the extracted tarball directory (same folder as wooly-paint and icon.png).
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_ID="dev.woolymelon.WoolyPaint"
BIN="$DIR/wooly-paint"
ICON="$DIR/icon.png"

if [[ ! -f "$BIN" ]]; then
  echo "Missing binary: $BIN" >&2
  exit 1
fi
if [[ ! -f "$ICON" ]]; then
  echo "Missing icon: $ICON" >&2
  exit 1
fi

chmod +x "$BIN" 2>/dev/null || true

ICON_DEST="${HOME}/.local/share/icons/hicolor/128x128/apps/${APP_ID}.png"
mkdir -p ~/.local/share/icons/hicolor/128x128/apps
cp "$ICON" "$ICON_DEST"

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
Path=$DIR
Terminal=false
Categories=Graphics;2DGraphics;
StartupWMClass=$APP_ID
EOF

gtk-update-icon-cache -f ~/.local/share/icons/hicolor/ 2>/dev/null || true
update-desktop-database ~/.local/share/applications/ 2>/dev/null || true
# KDE Plasma keeps its own cache; without this, pinned taskbar icons can stay blank when the app is closed.
kbuildsycoca6 --noincremental 2>/dev/null || kbuildsycoca5 --noincremental 2>/dev/null || true

echo "Installed $DESKTOP"
echo "KDE Plasma: if the taskbar icon was already pinned, unpin it, run this script again if you changed anything, then pin Wooly Paint from the app menu once more."
echo "Otherwise: open the app menu, launch Wooly Paint, then pin that entry — the icon should stay when the app is closed."
