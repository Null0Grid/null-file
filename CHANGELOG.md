# Changelog

The release workflow copies the section for each version into its GitHub release, and Null File shows it in the update dialog.

## 1.1.0 — 2026-10-27

- **The Vault.** A private folder in the sidebar, encrypted on this computer, that opens with a PIN (3 to 300 digits). Drag files in to encrypt them; names, folders and contents are all unreadable on disk. A 24-word recovery key resets a forgotten PIN. It locks after a few idle minutes (you choose how many), with Ctrl+Shift+L, and always when Null File closes.
- **Automatic updates.** Installing the .deb or .rpm adds the signed Null Grid package repository, so new versions arrive with your normal system updates (apt, dnf, zypper, KDE Discover, GNOME Software). The AppImage and the tar.gz update themselves after checking the download against SHA256SUMS. Flatpak, Snap and AUR users get a notice.
- Settings has new Vault and Updates sections.

## 1.0.0 — 2026-10-27

- First release: tabs, grid and list views, details panel with previews, copy and move with progress, undo, the Ctrl+K command palette, search through subfolders, and the graph view of your folders.
