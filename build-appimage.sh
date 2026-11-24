#!/usr/bin/env bash
# Baut Handy-Spiegel-x86_64.AppImage – enthält App, GTK4/libadwaita, scrcpy und adb.
#   bash build-appimage.sh
set -euo pipefail
cd "$(dirname "$0")"
SCRCPY_VER=5.0.1
SC="vendor/scrcpy-linux-x86_64-v$SCRCPY_VER"
APPDIR="$PWD/build/AppDir"
T="$PWD/tools"
export APPIMAGE_EXTRACT_AND_RUN=1   # Werkzeuge laufen auch ohne FUSE

# scrcpy (offizieller statischer Build, enthält adb)
if [[ ! -x $SC/scrcpy ]]; then
  mkdir -p vendor/dl
  curl -fsSLo vendor/dl/sc.tar.gz "https://github.com/Genymobile/scrcpy/releases/download/v$SCRCPY_VER/scrcpy-linux-x86_64-v$SCRCPY_VER.tar.gz"
  tar xzf vendor/dl/sc.tar.gz -C vendor
fi

# Werkzeuge
mkdir -p "$T"
[[ -s $T/linuxdeploy ]] || curl -fsSLo "$T/linuxdeploy" https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
[[ -s $T/linuxdeploy-plugin-gtk.sh ]] || curl -fsSLo "$T/linuxdeploy-plugin-gtk.sh" https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh
[[ -s $T/appimagetool ]] || curl -fsSLo "$T/appimagetool" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
chmod +x "$T"/*
# gdk-pixbuf ≥ 2.44 (Arch) lädt Bilder über glycin, der alte Lader-Ordner fehlt → Plugin-Schritt dann überspringen
sed -i 's|^copy_lib_tree "\$gdk_pixbuf_binarydir" "\$APPDIR/"$|if [ -d "$gdk_pixbuf_binarydir" ]; then copy_lib_tree "$gdk_pixbuf_binarydir" "$APPDIR/"; else mkdir -p "$APPDIR/${gdk_pixbuf_moduledir/$LD_GTK_LIBRARY_PATH//usr/lib}"; fi  # glycin|' "$T/linuxdeploy-plugin-gtk.sh"

cargo build --release

rm -rf build && mkdir -p "$APPDIR/usr/lib/handy-spiegel"
install -m755 "$SC/scrcpy" "$SC/adb" "$APPDIR/usr/lib/handy-spiegel/"
install -m644 "$SC/scrcpy-server" "$SC/LICENSE" "$APPDIR/usr/lib/handy-spiegel/"

PATH="$T:$PATH" DEPLOY_GTK_VERSION=4 "$T/linuxdeploy" --appdir "$APPDIR" \
  --executable target/release/handy-spiegel \
  --desktop-file data/io.janeway.HandySpiegel.desktop \
  --icon-file data/io.janeway.HandySpiegel.svg \
  --plugin gtk

# Benutzte Symbole (Adwaita, CC-BY-SA) mitliefern – auch Systeme ohne Adwaita-Icons (z. B. KDE) zeigen sie dann
SYM="$APPDIR/usr/share/icons/hicolor/symbolic/apps"
mkdir -p "$SYM"
for i in $(grep -oE '"[a-z-]+-symbolic"' src/main.rs | sort -u | tr -d '"'); do
  f=$(find /usr/share/icons/Adwaita -name "$i.svg" | head -1)
  [[ -n $f ]] && install -m644 "$f" "$SYM/"
done

# GTK-Plugin-Vorgaben zurücknehmen: kein erzwungenes X11, kein festes GTK_THEME (libadwaita stylt selbst)
HOOK="$APPDIR/apprun-hooks/linuxdeploy-plugin-gtk.sh"
sed -i -e '/^export GDK_BACKEND=x11/d' -e '/^export GTK_THEME=/d' "$HOOK"

"$T/appimagetool" --no-appstream "$APPDIR" "Handy-Spiegel-x86_64.AppImage"

# Selbsttest ohne Fenster
./Handy-Spiegel-x86_64.AppImage --check
ls -lh Handy-Spiegel-x86_64.AppImage
echo "Fertig: $PWD/Handy-Spiegel-x86_64.AppImage"

# Gleich installieren, damit der Menüeintrag nie auf eine alte Version zeigt
bash ./install.sh
grep -q "Exec=$HOME/.local/bin/Handy-Spiegel.AppImage" "$HOME/.local/share/applications/io.janeway.HandySpiegel.desktop" \
  && cmp -s Handy-Spiegel-x86_64.AppImage "$HOME/.local/bin/Handy-Spiegel.AppImage" \
  && echo "✓ Menüeintrag zeigt auf die frisch gebaute Version" \
  || { echo "✗ Installation stimmt nicht mit dem Build überein"; exit 1; }
