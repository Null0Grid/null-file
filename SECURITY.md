# Security

## How Null File treats your files

| Action | What happens |
|---|---|
| Delete | Outside the Vault, always goes to the Trash (the freedesktop.org Trash in `~/.local/share/Trash`), never a permanent delete. Only **Empty Trash** deletes for good, after a confirmation. |
| Replace (when names clash) | The existing item is moved to the Trash first, so Undo can bring it back. |
| Copy | Writes to a new file only (`create_new`); a cancelled or failed copy removes its half-written file. Symlinks are copied as links, never followed. |
| Move | A rename when source and destination are on the same drive. Across drives: a full copy first, and the source is removed only after the copy succeeded. |
| Folders into themselves | Refused for both copy and move. |
| Graph view and search | Read-only. Symlinked folders aren't followed, so links can't loop. |
| Opening files | Handed to your desktop's default app via `xdg-open`. Null File never runs a file itself. |

## The Vault

| Part | How |
|---|---|
| Data key | 32 random bytes. Never stored unencrypted; it lives in memory only while the Vault is unlocked and is zeroed on lock. |
| PIN | 3 to 300 digits. Argon2id (64 MiB, t=3, p=1, random 16-byte salt) turns it into a key that wraps the data key with XChaCha20-Poly1305. The PIN itself is never stored. |
| Recovery key | 256 random bits, shown once as 24 BIP-39 words. SHA-256 of them (domain-separated) wraps a second copy of the data key. |
| File contents | One blob per file, random 128-bit name. The STREAM construction over XChaCha20-Poly1305 in 1 MiB chunks, with the blob's id as associated data, so a blob can't be truncated, reordered or swapped for another. |
| Names, folders, sizes, dates | In one encrypted index (`index.bin`). Nothing on disk shows what's inside. |
| Wrong PINs | Free for 3 tries, then 5 s doubling up to 15 min between tries. This is enforced by the app: someone who copies the vault files can guess offline, so a long PIN matters. |
| Opening in other apps | A decrypted copy in `$XDG_RUNTIME_DIR` (RAM-backed, readable only by you), zero-filled and removed on lock and on exit. Without a private runtime folder, Null File refuses instead of writing plaintext to disk. |
| Moving files in | The originals are encrypted first, then deleted. Deleting doesn't securely erase them from an SSD; for files that were never on this disk unencrypted, copy them in from a USB stick. |
| Deleting from the Vault | Permanent: the blob and its index entry are removed. Vault items never go to the system Trash. |
| Settings file | Never contains names from inside the Vault (tabs, recent folders and pins inside the Vault aren't saved). |

## What Null File sends over the network

Only the update check: one request a day to `api.github.com/repos/Null0Grid/null-file/releases/latest`
(and, if you click Update now, the download from GitHub). Turn it off in Settings → Updates; Flatpak and Snap
copies never check. There are no accounts and no telemetry. Thumbnails and previews are read from disk by the app itself.

## Updates are verified

- AppImage and tar.gz: the download must match the release's `SHA256SUMS` or it isn't installed.
- .deb and .rpm: the package repository is signed with the Null Grid Packages key (RSA 4096,
  `F479E27ABDCF2FFF9E8AB0F47AE17832BAEAD2EC`), which the packages install to
  `/usr/share/keyrings/` and `/etc/pki/rpm-gpg/`. RPMs are signed individually as well.

## Permissions

All filesystem access goes through the app's own Rust commands (`src-tauri/src/lib.rs`); the
web view gets only Tauri's core permissions plus setting the window title. The page's
Content-Security-Policy allows scripts only from the app itself.

## Reporting a vulnerability

Please don't open a public issue. Email **volosteele55@proton.me** with the steps to reproduce, and you'll get an answer within a few days.
