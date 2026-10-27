# Security

## How Null File treats your files

| Action | What happens |
|---|---|
| Delete | Always goes to the Trash (the freedesktop.org Trash in `~/.local/share/Trash`), never a permanent delete. Only **Empty Trash** deletes for good, after a confirmation. |
| Replace (when names clash) | The existing item is moved to the Trash first, so Undo can bring it back. |
| Copy | Writes to a new file only (`create_new`); a cancelled or failed copy removes its half-written file. Symlinks are copied as links, never followed. |
| Move | A rename when source and destination are on the same drive. Across drives: a full copy first, and the source is removed only after the copy succeeded. |
| Folders into themselves | Refused for both copy and move. |
| Graph view and search | Read-only. Symlinked folders aren't followed, so links can't loop. |
| Opening files | Handed to your desktop's default app via `xdg-open`. Null File never runs a file itself. |

## What Null File sends over the network

Nothing. It has no accounts, telemetry or update checks. Thumbnails and previews are read
from disk by the app itself through Tauri's local asset protocol.

## Permissions

All filesystem access goes through the app's own Rust commands (`src-tauri/src/lib.rs`); the
web view gets only Tauri's core permissions plus setting the window title. The page's
Content-Security-Policy allows scripts only from the app itself.

## Reporting a vulnerability

Please don't open a public issue. Email **volosteele55@proton.me** with the steps to reproduce, and you'll get an answer within a few days.
