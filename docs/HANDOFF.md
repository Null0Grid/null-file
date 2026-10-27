# Null File — handoff notes

Null Grid's file manager for Linux, and the default file manager in Null OS.

- Repo: **Null0Grid/null-file** (proprietary, "Copyright (c) 2026 NULLGRID. All rights reserved."). Version **1.1.0** (1.0.0 released 2026-10-27; 1.1.0 adds the Vault and automatic updates).
- Stack: **Tauri 2** (Rust backend + plain ES-module frontend, no bundler), same as Null Notes. Fonts (IBM Plex, Silkscreen) and d3-force are copied from Null Notes.
- App id / desktop id: `dev.nullgrid.file` / `dev.nullgrid.file.desktop` (deb, tar.gz, AUR, Flatpak). Tauri's rpm/AppImage bundler names its entry `null-file.desktop`; snaps use `null-file_null-file.desktop`. `sysint::desktop_id()` finds whichever is installed.
- Binary: `null-file`. Settings: `~/.config/dev.nullgrid.file/settings.json`.
- Only Null Notes feature borrowed: the **graph view** (folder tree only, parent → child edges). No sync, search integration or deep links with the other apps (by Luke's choice).

## Features (1.0.0)
Tabs (restored at startup), breadcrumbs + typed paths, sidebar (XDG places, pinned, drives from /proc/mounts, Trash), grid/list views with asset-protocol thumbnails, details panel with image/video/audio/text previews, copy/move jobs with progress + cancel, drag to move (Ctrl copies), marquee select, conflicts (keep both / replace-to-Trash / skip), undo (rename, move, copy, mkdir, trash), Ctrl+K palette with path completion, filter + recursive name search, graph view (depth 1–8, files toggle, labels, glow), terminal here (Konsole first; flatpak-spawn in Flatpak), live refresh via notify, single instance (second launch opens tabs).

## Vault (1.1.0)
- Sidebar entry `Vault`; UI paths `vault:/<rel>`. Backend `src-tauri/src/vault.rs`, data in `~/.local/share/dev.nullgrid.file/vault/` (`vault.json`, `index.bin`, `blobs/`, `attempts.json`).
- PIN 3–300 digits (Luke's choice) → Argon2id 64 MiB/t3 wraps a random data key; 24-word BIP-39 recovery key wraps a second copy. Files: XChaCha20-Poly1305 STREAM, 1 MiB chunks, blob id as AAD; names only in the encrypted index.
- Wrong-PIN delay after 3 tries (5 s doubling, max 15 min). Auto-lock after idle minutes (default 5, setting), Ctrl+Shift+L, and on exit.
- Thumbnails/previews via the `nfvault://` URI scheme (decrypts in memory). "Open" puts a decrypted copy in `$XDG_RUNTIME_DIR`, synced back on focus/lock, wiped on lock.
- Moving real files in deletes the originals (one confirmation per session); deleting inside the Vault is permanent (never the system Trash). Vault names are never written to settings.json.

## Updates (1.1.0)
- `src-tauri/src/update.rs`: daily check of GitHub releases/latest (setting). AppImage + tar.gz self-update (SHA256SUMS check, atomic swap, relaunch). Flatpak/Snap never check.
- .deb/.rpm ship `/etc/apt/sources.list.d/null-file.sources` / `/etc/yum.repos.d/null-file.repo` (+ zypp) and the **Null Grid Packages** key (RSA 4096, `F479E27ABDCF2FFF9E8AB0F47AE17832BAEAD2EC`, public key in `packaging/repo/`).
- Release workflow job `repo` builds the signed APT/RPM repo (`packaging/repo/publish-repo.sh`) and pushes it to `gh-pages` → https://null0grid.github.io/null-file/. Needs repo **public**, secret **REPO_SIGNING_KEY**, and Pages on `gh-pages`. Release notes come from CHANGELOG.md.

## Default file manager
`null-file --set-default` or Settings → runs `xdg-mime default <desktop id> inode/directory` (via `flatpak-spawn --host` inside Flatpak). Every desktop entry declares `MimeType=inode/directory;`.

## Not done yet
- `org.freedesktop.FileManager1` D-Bus service (what "Show in folder" in Firefox/Chrome/KDE apps calls); without it those apps fall back to xdg-open on the folder, which still opens Null File.
- Dragging files out of Null File into other apps (Tauri limitation); dragging in works.
- Permanent delete (Shift+Delete) on purpose; only Empty Trash deletes for good (the Vault is the exception).
