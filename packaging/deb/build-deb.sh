#!/usr/bin/env bash
# Build the Null File .deb for this machine (amd64 or arm64). It runs on distros with a glibc at least
# as new as the build machine's (the release workflow builds on Ubuntu 22.04).
#
#   packaging/deb/build-deb.sh
#
# Needs: Rust, libwebkit2gtk-4.1-dev, libssl-dev, dpkg-deb.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
tauri="$root/src-tauri"
V="$(python3 -c "import json;print(json.load(open('$tauri/tauri.conf.json'))['version'])")"
A="$(dpkg --print-architecture)"
echo "Null File $V ($A)"

( cd "$tauri" && cargo build --release --features tauri/custom-protocol )

R="$here/null-file_${V}_${A}"
rm -rf "$R"
mkdir -p "$R/DEBIAN" "$R/usr/bin" "$R/usr/share/applications" "$R/usr/share/doc/null-file" "$R/usr/share/metainfo"
install -m755 "$tauri/target/release/null-file" "$R/usr/bin/null-file"
for s in 32 128 256 512; do mkdir -p "$R/usr/share/icons/hicolor/${s}x${s}/apps"; done
cp "$tauri/icons/32x32.png"      "$R/usr/share/icons/hicolor/32x32/apps/dev.nullgrid.file.png"
cp "$tauri/icons/128x128.png"    "$R/usr/share/icons/hicolor/128x128/apps/dev.nullgrid.file.png"
cp "$tauri/icons/128x128@2x.png" "$R/usr/share/icons/hicolor/256x256/apps/dev.nullgrid.file.png"
cp "$tauri/icons/icon.png"       "$R/usr/share/icons/hicolor/512x512/apps/dev.nullgrid.file.png"
mkdir -p "$R/usr/share/icons/hicolor/scalable/apps"
cp "$root/packaging/icon/null-file.svg" "$R/usr/share/icons/hicolor/scalable/apps/dev.nullgrid.file.svg"
install -m644 "$root/packaging/linux/dev.nullgrid.file.desktop" "$R/usr/share/applications/dev.nullgrid.file.desktop"
install -m644 "$root/packaging/linux/dev.nullgrid.file.metainfo.xml" "$R/usr/share/metainfo/dev.nullgrid.file.metainfo.xml"
# The Null Grid package repository, so apt / Discover install updates (apt upgrade).
install -Dm644 "$root/packaging/repo/null-file.sources" "$R/etc/apt/sources.list.d/null-file.sources"
install -Dm644 "$root/packaging/repo/null-grid-archive-keyring.gpg" "$R/usr/share/keyrings/null-grid-archive-keyring.gpg"
echo "/etc/apt/sources.list.d/null-file.sources" > "$R/DEBIAN/conffiles"
# "Show in folder" (org.freedesktop.FileManager1): D-Bus starts Null File in the background.
# postinst puts it in /usr/share/dbus-1/services/ unless another package (Nautilus) owns that file.
install -Dm644 /dev/stdin "$R/usr/share/null-file/org.freedesktop.FileManager1.service" <<'EOT'
[D-BUS Service]
Name=org.freedesktop.FileManager1
Exec=/usr/bin/null-file --dbus
EOT

cat > "$R/usr/share/doc/null-file/copyright" <<'EOT'
Null File
Copyright NULLGRID. All rights reserved.
EOT

# Refresh the desktop and icon caches so "Open folder" and the menu see it right away.
svc=/usr/share/dbus-1/services/org.freedesktop.FileManager1.service
cat > "$R/DEBIAN/postinst" <<EOT
#!/bin/sh
set -e
# The FileManager1 activation file, unless another package (Nautilus) ships its own.
if ! dpkg -S $svc >/dev/null 2>&1; then
  mkdir -p /usr/share/dbus-1/services
  cp /usr/share/null-file/org.freedesktop.FileManager1.service $svc
fi
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true
exit 0
EOT
cat > "$R/DEBIAN/postrm" <<EOT
#!/bin/sh
set -e
if [ "\$1" = remove ] || [ "\$1" = purge ]; then
  grep -qx 'Exec=/usr/bin/null-file --dbus' $svc 2>/dev/null && ! dpkg -S $svc >/dev/null 2>&1 && rm -f $svc
fi
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true
exit 0
EOT
chmod 755 "$R/DEBIAN/postinst" "$R/DEBIAN/postrm"

size="$(du -sk "$R/usr" "$R/etc" | awk '{s+=$1} END {print s}')"
# the glibc this binary actually needs (it depends on the distro it was built on)
glibc="$(objdump -T "$R/usr/bin/null-file" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1 | cut -d_ -f2)"
cat > "$R/DEBIAN/control" <<EOT
Package: null-file
Version: $V
Architecture: $A
Maintainer: NULLGRID <volosteele55@proton.me>
Installed-Size: $size
Depends: libc6 (>= $glibc), libwebkit2gtk-4.1-0, libjavascriptcoregtk-4.1-0, libsoup-3.0-0, libgtk-3-0t64 | libgtk-3-0, libglib2.0-0t64 | libglib2.0-0, libdbus-1-3, xdg-utils
Recommends: wl-clipboard | xclip
Provides: x-file-manager
Section: utils
Priority: optional
Homepage: https://nullgrid.dev
Description: Null File - a Neon Lab file manager
 A fast, keyboard-friendly file manager for Linux, part of Null Grid:
 tabs, a command palette, grid and list views with thumbnails, a details
 panel with previews, undo for file operations, and a graph view that
 draws a folder tree as a force-directed map. Opens folders
 (inode/directory) and can be set as the default file manager.
EOT

dpkg-deb --root-owner-group --build "$R" >/dev/null
rm -rf "$R"
echo "Built $here/null-file_${V}_${A}.deb"
