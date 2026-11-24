#!/usr/bin/env bash
# Installiert das AppImage für den aktuellen Benutzer (App-Menü-Eintrag + Icon).
#   bash install.sh [Pfad/zum/Handy-Spiegel-x86_64.AppImage]
set -euo pipefail
DIR=$(cd "$(dirname "$0")" && pwd)
SRC="${1:-$DIR/Handy-Spiegel-x86_64.AppImage}"
[[ -f $SRC ]] || { echo "AppImage fehlt – erst: bash $DIR/build-appimage.sh"; exit 1; }
BIN="$HOME/.local/bin"; APPS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"
mkdir -p "$BIN" "$APPS" "$ICONS"
install -m755 "$SRC" "$BIN/Handy-Spiegel.AppImage"
[[ -f $DIR/data/io.janeway.HandySpiegel.svg ]] && install -m644 "$DIR/data/io.janeway.HandySpiegel.svg" "$ICONS/"
sed "s|^Exec=.*|Exec=$BIN/Handy-Spiegel.AppImage|" "$DIR/data/io.janeway.HandySpiegel.desktop" > "$APPS/io.janeway.HandySpiegel.desktop"
# alte Namen (Handy-Bildschirm / handy-screen) aufräumen
rm -f "$APPS/io.janeway.HandyScreen.desktop" "$APPS/handy-screen.desktop" "$APPS/handy-screen-tv.desktop" \
      "$BIN/Handy-Bildschirm.AppImage" "$BIN/handy-screen" "$ICONS/io.janeway.HandyScreen.svg"
OLD="${XDG_CONFIG_HOME:-$HOME/.config}"
[[ -d $OLD/handy-screen && ! -e $OLD/handy-spiegel ]] && mv "$OLD/handy-screen" "$OLD/handy-spiegel"
update-desktop-database "$APPS" 2>/dev/null || true
gtk-update-icon-cache -q "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
echo "Installiert – im App-Menü: „Handy-Spiegel“"
