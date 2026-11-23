#!/usr/bin/env bash
# Package a portable zip for Windows (run in MSYS2 UCRT64 after `cargo build --release`).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
PREFIX="${MINGW_PREFIX:-/ucrt64}"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
EXE="${TARGET_DIR}/release/wooly-paint.exe"
if [[ ! -f "$EXE" ]]; then
  echo "Missing $EXE — build with: cargo build --release" >&2
  exit 1
fi
VER=$(awk -F'"' '/^version = / {print $2; exit}' Cargo.toml)
NAME="wooly-paint-${VER}-windows-x86_64"
EXEDIR="dist/$NAME"
rm -rf "$EXEDIR" "dist/${NAME}.zip"
mkdir -p "$EXEDIR"

cp "$EXE" "$EXEDIR/"
cp packaging/windows-portable.txt "$EXEDIR/README.txt"
cp src/assets/icon.ico "$EXEDIR/" 2>/dev/null || true

# Pull in every MSYS2 runtime DLL reachable from the exe (iterative closure).
round=0
while [[ $round -lt 40 ]]; do
  round=$((round + 1))
  changed=0
  shopt -s nullglob
  for f in "$EXEDIR"/*.exe "$EXEDIR"/*.dll; do
    [[ -f "$f" ]] || continue
    while read -r lib; do
      [[ -z "$lib" || ! -f "$lib" ]] && continue
      base=$(basename "$lib")
      [[ -f "$EXEDIR/$base" ]] && continue
      cp "$lib" "$EXEDIR/"
      changed=1
    done < <(ldd "$f" 2>/dev/null | awk -v pfx="${PREFIX}/" '$2 == "=>" && index($3, pfx) == 1 {print $3}')
  done
  shopt -u nullglob
  [[ $changed -eq 0 ]] && break
done

# GTK / Adwaita data (schemas, themes, loaders).
mkdir -p "$EXEDIR/share" "$EXEDIR/lib"
for d in glib-2.0 libadwaita-1 gtk-4.0; do
  if [[ -d "${PREFIX}/share/$d" ]]; then
    cp -a "${PREFIX}/share/$d" "$EXEDIR/share/"
  fi
done
if [[ -d "${PREFIX}/share/icons/Adwaita" ]]; then
  mkdir -p "$EXEDIR/share/icons"
  cp -a "${PREFIX}/share/icons/Adwaita" "$EXEDIR/share/icons/"
fi
if [[ -d "${PREFIX}/lib/gdk-pixbuf-2.0" ]]; then
  cp -a "${PREFIX}/lib/gdk-pixbuf-2.0" "$EXEDIR/lib/"
fi

cat > "$EXEDIR/run-wooly-paint.cmd" <<'EOF'
@echo off
setlocal
set "_HERE=%~dp0"
set "PATH=%_HERE%;%PATH%"
set "XDG_DATA_DIRS=%_HERE%share"
set "GTK_DATA_PREFIX=%_HERE%share"
set "GTK_EXE_PREFIX=%_HERE%"
for /d %%G in ("%_HERE%lib\gdk-pixbuf-2.0\*") do (
  if exist "%%G\loaders\" set "GDK_PIXBUF_MODULEDIR=%%G\loaders"
)
cd /d "%_HERE%"
"%_HERE%wooly-paint.exe" %*
EOF

if command -v zip >/dev/null 2>&1; then
  (cd dist && zip -r -q "${NAME}.zip" "$NAME")
elif [[ -x "${PREFIX}/bin/zip.exe" ]]; then
  (cd dist && "${PREFIX}/bin/zip.exe" -r -q "${NAME}.zip" "$NAME")
else
  echo "zip not found; leaving unpacked dir $EXEDIR" >&2
  exit 1
fi
echo "Wrote dist/${NAME}.zip"
