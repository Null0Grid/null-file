#!/usr/bin/env sh
# Install Null File from this folder.
#   ./install.sh                 for you only, into ~/.local (no sudo)
#   sudo ./install.sh --system   for everyone, into /usr/local
#   add --default to make it the file manager that opens folders
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
prefix="$HOME/.local"; makedefault=0
for a in "$@"; do
  case "$a" in
    --system) prefix=/usr/local ;;
    --default) makedefault=1 ;;
  esac
done
bin="$prefix/bin"
apps="$prefix/share/applications"
icons="$prefix/share/icons/hicolor"

mkdir -p "$bin" "$apps" "$icons/scalable/apps"
install -m755 "$here/null-file" "$bin/null-file"
for s in 32 128 256 512; do
  mkdir -p "$icons/${s}x${s}/apps"
  install -m644 "$here/icons/${s}x${s}.png" "$icons/${s}x${s}/apps/dev.nullgrid.file.png"
done
install -m644 "$here/icons/scalable.svg" "$icons/scalable/apps/dev.nullgrid.file.svg"
# full path in Exec: ~/.local/bin isn't on every desktop's PATH
sed "s|^Exec=null-file|Exec=$bin/null-file|" "$here/dev.nullgrid.file.desktop" > "$apps/dev.nullgrid.file.desktop"
chmod 644 "$apps/dev.nullgrid.file.desktop"

command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$apps" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$icons" 2>/dev/null || true
if [ "$makedefault" = 1 ]; then
  # sets the folder handler and the "Show in folder" D-Bus service for this user
  "$bin/null-file" --set-default && echo "Null File now opens folders."
fi

echo "Installed Null File to $bin/null-file."
echo "Find it in your app menu, or run: null-file"
[ "$makedefault" = 1 ] || echo "To make it the default file manager: null-file --set-default"
case ":$PATH:" in *":$bin:"*) ;; *) echo "(Note: $bin isn't on your PATH; the app menu entry works anyway.)";; esac
