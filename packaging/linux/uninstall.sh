#!/usr/bin/env sh
# Remove Null File installed by install.sh (your settings are kept).
#   ./uninstall.sh                 from ~/.local
#   sudo ./uninstall.sh --system   from /usr/local
set -eu
if [ "${1:-}" = "--system" ]; then prefix=/usr/local; else prefix="$HOME/.local"; fi
rm -f "$prefix/bin/null-file" "$prefix/share/applications/dev.nullgrid.file.desktop"
for s in 32 128 256 512; do rm -f "$prefix/share/icons/hicolor/${s}x${s}/apps/dev.nullgrid.file.png"; done
rm -f "$prefix/share/icons/hicolor/scalable/apps/dev.nullgrid.file.svg"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$prefix/share/applications" 2>/dev/null || true
echo "Removed Null File from $prefix. Your settings (~/.config/dev.nullgrid.file) were not touched."
