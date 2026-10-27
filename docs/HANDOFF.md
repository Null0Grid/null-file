# Null File — handoff notes

Null Grid's file manager for Linux, and the default file manager in Null OS.

- Repo: **Null0Grid/null-file** (proprietary, "Copyright (c) 2026 NULLGRID. All rights reserved."). Version **1.0.0**.
- Stack: **Tauri 2** (Rust backend + plain ES-module frontend, no bundler), same as Null Notes. Fonts (IBM Plex, Silkscreen) and d3-force are copied from Null Notes.
- App id / desktop id: `dev.nullgrid.file` / `dev.nullgrid.file.desktop` (deb, tar.gz, AUR, Flatpak). Tauri's rpm/AppImage bundler names its entry `null-file.desktop`; snaps use `null-file_null-file.desktop`. `sysint::desktop_id()` finds whichever is installed.
- Binary: `null-file`. Settings: `~/.config/dev.nullgrid.file/settings.json`.
- Only Null Notes feature borrowed: the **graph view** (folder tree only, parent → child edges). No sync, search integration or deep links with the other apps (by Luke's choice).

## Features (1.0.0)
Tabs (restored at startup), breadcrumbs + typed paths, sidebar (XDG places, pinned, drives from /proc/mounts, Trash), grid/list views with asset-protocol thumbnails, details panel with image/video/audio/text previews, copy/move jobs with progress + cancel, drag to move (Ctrl copies), marquee select, conflicts (keep both / replace-to-Trash / skip), undo (rename, move, copy, mkdir, trash), Ctrl+K palette with path completion, filter + recursive name search, graph view (depth 1–8, files toggle, labels, glow), terminal here (Konsole first; flatpak-spawn in Flatpak), live refresh via notify, single instance (second launch opens tabs).

## Default file manager
`null-file --set-default` or Settings → runs `xdg-mime default <desktop id> inode/directory` (via `flatpak-spawn --host` inside Flatpak). Every desktop entry declares `MimeType=inode/directory;`.

## Not done yet
- `org.freedesktop.FileManager1` D-Bus service (what "Show in folder" in Firefox/Chrome/KDE apps calls); without it those apps fall back to xdg-open on the folder, which still opens Null File.
- Dragging files out of Null File into other apps (Tauri limitation); dragging in works.
- Permanent delete (Shift+Delete) on purpose; only Empty Trash deletes for good.
