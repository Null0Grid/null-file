//! Filesystem operations: listing, places, create, rename, copy, move, trash.
//! Every path that crosses the IPC boundary is an absolute path string.

use serde::Serialize;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

pub type R<T> = Result<T, String>;

#[derive(Serialize, Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub link: bool,
    pub hidden: bool,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub modified: i64,
    pub kind: &'static str,
    pub ext: String,
    pub mode: u32,
    /// Number of entries inside a directory (None when unreadable or not a dir).
    pub items: Option<u32>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Listing {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<Entry>,
    pub writable: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct Place {
    pub name: String,
    pub path: String,
    pub icon: &'static str,
}

pub fn err(e: io::Error, what: &str, p: &Path) -> String {
    let p = p.display();
    match e.kind() {
        io::ErrorKind::NotFound => format!("{what}: {p} no longer exists."),
        io::ErrorKind::PermissionDenied => format!("{what}: permission denied for {p}."),
        io::ErrorKind::AlreadyExists => format!("{what}: {p} already exists."),
        _ => format!("{what}: {p}: {e}"),
    }
}

pub fn abs(p: &str) -> R<PathBuf> {
    let pb = PathBuf::from(p);
    if !pb.is_absolute() {
        return Err(format!("\"{p}\" isn't an absolute path."));
    }
    Ok(pb)
}

pub fn kind_of(name: &str, dir: bool) -> &'static str {
    if dir {
        return "dir";
    }
    let ext = ext_of(name);
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "avif" | "tif" | "tiff" | "heic" => "img",
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "wmv" | "flv" => "video",
        "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" | "wma" => "audio",
        "md" | "markdown" | "txt" | "log" | "rst" | "org" | "csv" | "tsv" => "text",
        "rs" | "js" | "ts" | "tsx" | "jsx" | "py" | "sh" | "zsh" | "bash" | "c" | "h" | "cpp" | "hpp" | "go"
        | "java" | "kt" | "rb" | "php" | "lua" | "json" | "yml" | "yaml" | "toml" | "ini" | "conf" | "xml"
        | "html" | "htm" | "css" | "scss" | "sql" | "nix" | "vim" | "ps1" | "bat" | "mjs" | "cjs" => "code",
        "pdf" | "doc" | "docx" | "odt" | "rtf" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp" | "epub" => "doc",
        "zip" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "7z" | "rar" | "deb" | "rpm" | "iso" | "img"
        | "appimage" | "flatpak" | "snap" => "archive",
        _ => "file",
    }
}

pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn mtime(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn entry_for(path: &Path) -> io::Result<Entry> {
    let lmd = fs::symlink_metadata(path)?;
    let link = lmd.file_type().is_symlink();
    // Follow links for type and size; a broken link is shown as a file.
    let md = if link { fs::metadata(path).unwrap_or_else(|_| lmd.clone()) } else { lmd.clone() };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
    let dir = md.is_dir();
    let items = if dir { fs::read_dir(path).ok().map(|r| r.count().min(u32::MAX as usize) as u32) } else { None };
    let mut kind = kind_of(&name, dir);
    if kind == "file" && md.permissions().mode() & 0o111 != 0 && md.is_file() {
        kind = "exec";
    }
    Ok(Entry {
        hidden: name.starts_with('.'),
        ext: if dir { String::new() } else { ext_of(&name) },
        kind,
        name,
        path: path.display().to_string(),
        dir,
        link,
        size: if dir { 0 } else { md.len() },
        modified: mtime(&md),
        mode: md.mode() & 0o7777,
        items,
    })
}

pub fn list_dir(path: &str, hidden: bool) -> R<Listing> {
    let p = abs(path)?;
    let rd = fs::read_dir(&p).map_err(|e| err(e, "Can't open folder", &p))?;
    let mut entries = Vec::new();
    for de in rd.flatten() {
        let name = de.file_name().to_string_lossy().into_owned();
        if !hidden && name.starts_with('.') {
            continue;
        }
        if let Ok(e) = entry_for(&de.path()) {
            entries.push(e);
        }
    }
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| natural_cmp(&a.name, &b.name)));
    let writable = is_writable(&p);
    Ok(Listing {
        parent: p.parent().map(|x| x.display().to_string()),
        path: p.display().to_string(),
        entries,
        writable,
    })
}

fn is_writable(p: &Path) -> bool {
    use std::ffi::CString;
    let Ok(c) = CString::new(p.as_os_str().as_encoded_bytes()) else { return false };
    // SAFETY: access(2) only reads the NUL-terminated path.
    unsafe { libc_access(c.as_ptr(), 2) == 0 }
}

extern "C" {
    #[link_name = "access"]
    fn libc_access(path: *const std::os::raw::c_char, mode: i32) -> i32;
}

/// Case-insensitive natural sort: "file2" before "file10".
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Less,
            (_, None) => return Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                    nb.push(c);
                    bi.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if o != Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// XDG user dirs (Desktop, Documents…) from ~/.config/user-dirs.dirs, with defaults.
fn xdg_user_dir(key: &str, fallback: &str) -> PathBuf {
    let h = home();
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| h.join(".config"));
    if let Ok(s) = fs::read_to_string(cfg.join("user-dirs.dirs")) {
        for line in s.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(&format!("XDG_{key}_DIR=")) {
                let v = rest.trim_matches('"').replace("$HOME", &h.display().to_string());
                return PathBuf::from(v);
            }
        }
    }
    h.join(fallback)
}

pub fn places() -> Vec<Place> {
    let h = home();
    let mut out = vec![Place { name: "Home".into(), path: h.display().to_string(), icon: "home" }];
    for (key, fb, icon) in [
        ("DESKTOP", "Desktop", "desktop"),
        ("DOCUMENTS", "Documents", "docs"),
        ("DOWNLOAD", "Downloads", "down"),
        ("PICTURES", "Pictures", "img"),
        ("MUSIC", "Music", "audio"),
        ("VIDEOS", "Videos", "video"),
    ] {
        let p = xdg_user_dir(key, fb);
        if p.is_dir() && p != h {
            out.push(Place { name: p.file_name().unwrap().to_string_lossy().into_owned(), path: p.display().to_string(), icon });
        }
    }
    out.push(Place { name: "File System".into(), path: "/".into(), icon: "root" });
    out
}

/// Removable and extra drives from /proc/mounts.
pub fn drives() -> Vec<Place> {
    let Ok(m) = fs::read_to_string("/proc/mounts") else { return vec![] };
    parse_mounts(&m, &home())
}

pub fn parse_mounts(m: &str, home: &Path) -> Vec<Place> {
    let mut out: Vec<Place> = Vec::new();
    for line in m.lines() {
        let mut it = line.split_whitespace();
        let (Some(dev), Some(mnt), Some(fstype)) = (it.next(), it.next(), it.next()) else { continue };
        let mnt = mnt.replace("\\040", " ");
        let interesting = mnt.starts_with("/media/") || mnt.starts_with("/run/media/") || mnt.starts_with("/mnt/");
        let netfs = matches!(fstype, "nfs" | "nfs4" | "cifs" | "smb3" | "sshfs" | "fuse.sshfs" | "fuse.rclone");
        if !(interesting || (netfs && !mnt.starts_with(&*home.display().to_string()))) {
            continue;
        }
        if out.iter().any(|p| p.path == mnt) {
            continue;
        }
        let name = Path::new(&mnt).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(mnt.clone());
        let _ = dev;
        out.push(Place { name, path: mnt, icon: if netfs { "net" } else { "drive" } });
    }
    out
}

/// Reject names that can't be a single path component.
pub fn check_name(name: &str) -> R<()> {
    let n = name.trim();
    if n.is_empty() {
        return Err("The name can't be empty.".into());
    }
    if n == "." || n == ".." {
        return Err(format!("\"{n}\" isn't allowed as a name."));
    }
    if n.contains('/') || n.contains('\0') {
        return Err("Names can't contain \"/\".".into());
    }
    if n.len() > 255 {
        return Err("That name is too long (255 bytes max).".into());
    }
    Ok(())
}

/// "name.txt" → "name (2).txt", "name (3).txt"… until free.
pub fn unique_in(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    // Continue an existing "(n)" suffix instead of stacking them.
    let (base, mut n) = match stem.rfind(" (") {
        Some(i) if stem.ends_with(')') => match stem[i + 2..stem.len() - 1].parse::<u32>() {
            Ok(k) => (&stem[..i], k + 1),
            Err(_) => (stem, 2),
        },
        _ => (stem, 2),
    };
    loop {
        let cand = dir.join(format!("{base} ({n}){ext}"));
        if fs::symlink_metadata(&cand).is_err() {
            return cand;
        }
        n += 1;
    }
}

pub fn mkdir(parent: &str, name: &str) -> R<String> {
    check_name(name)?;
    let p = abs(parent)?.join(name.trim());
    fs::create_dir(&p).map_err(|e| err(e, "Can't create folder", &p))?;
    Ok(p.display().to_string())
}

pub fn mkfile(parent: &str, name: &str) -> R<String> {
    check_name(name)?;
    let p = abs(parent)?.join(name.trim());
    fs::OpenOptions::new().write(true).create_new(true).open(&p).map_err(|e| err(e, "Can't create file", &p))?;
    Ok(p.display().to_string())
}

pub fn rename(path: &str, new_name: &str) -> R<String> {
    check_name(new_name)?;
    let src = abs(path)?;
    let dst = src.parent().ok_or("Can't rename the root folder.")?.join(new_name.trim());
    if dst == src {
        return Ok(dst.display().to_string());
    }
    // Allow case-only renames on case-insensitive mounts, otherwise refuse to overwrite.
    let same_file = match (fs::symlink_metadata(&src), fs::symlink_metadata(&dst)) {
        (Ok(a), Ok(b)) => a.ino() == b.ino() && a.dev() == b.dev(),
        _ => false,
    };
    if fs::symlink_metadata(&dst).is_ok() && !same_file {
        return Err(format!("\"{}\" already exists here.", new_name.trim()));
    }
    fs::rename(&src, &dst).map_err(|e| err(e, "Can't rename", &src))?;
    Ok(dst.display().to_string())
}

/* ---------------- copy / move with progress ---------------- */

pub struct Progress<'a> {
    pub cancel: &'a AtomicBool,
    pub on: &'a mut dyn FnMut(u64, &Path),
}

/// Total bytes and file count under `p` (not following symlinks).
pub fn measure(p: &Path) -> (u64, u64) {
    let Ok(md) = fs::symlink_metadata(p) else { return (0, 0) };
    if md.is_dir() {
        let mut b = 0;
        let mut n = 0;
        if let Ok(rd) = fs::read_dir(p) {
            for de in rd.flatten() {
                let (bb, nn) = measure(&de.path());
                b += bb;
                n += nn;
            }
        }
        (b, n + 1)
    } else {
        (md.len(), 1)
    }
}

fn cancelled(pr: &Progress) -> io::Result<()> {
    if pr.cancel.load(Ordering::Relaxed) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
    } else {
        Ok(())
    }
}

/// Recursive copy that keeps symlinks as symlinks and preserves permissions and mtimes.
pub fn copy_tree(src: &Path, dst: &Path, pr: &mut Progress) -> io::Result<()> {
    cancelled(pr)?;
    let md = fs::symlink_metadata(src)?;
    let ft = md.file_type();
    if ft.is_symlink() {
        let target = fs::read_link(src)?;
        std::os::unix::fs::symlink(target, dst)?;
        (pr.on)(0, src);
    } else if ft.is_dir() {
        if dst.starts_with(src) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "can't copy a folder into itself"));
        }
        fs::create_dir(dst)?;
        for de in fs::read_dir(src)? {
            let de = de?;
            copy_tree(&de.path(), &dst.join(de.file_name()), pr)?;
        }
        fs::set_permissions(dst, md.permissions())?;
        let _ = filetime::set_file_mtime(dst, filetime::FileTime::from_last_modification_time(&md));
        (pr.on)(0, src);
    } else {
        copy_file(src, dst, &md, pr)?;
    }
    Ok(())
}

fn copy_file(src: &Path, dst: &Path, md: &fs::Metadata, pr: &mut Progress) -> io::Result<()> {
    use std::io::{Read, Write};
    let mut r = fs::File::open(src)?;
    let mut w = fs::OpenOptions::new().write(true).create_new(true).open(dst)?;
    let mut buf = vec![0u8; 1 << 20];
    let res = (|| -> io::Result<()> {
        loop {
            cancelled(pr)?;
            let n = r.read(&mut buf)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])?;
            (pr.on)(n as u64, src);
        }
        w.flush()
    })();
    if let Err(e) = res {
        drop(w);
        let _ = fs::remove_file(dst); // never leave a half-written copy behind
        return Err(e);
    }
    fs::set_permissions(dst, md.permissions())?;
    let _ = filetime::set_file_mtime(dst, filetime::FileTime::from_last_modification_time(md));
    (pr.on)(0, src);
    Ok(())
}

/// Move: a rename when possible, otherwise copy then remove the source.
pub fn move_tree(src: &Path, dst: &Path, pr: &mut Progress) -> io::Result<()> {
    if dst.starts_with(src) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "can't move a folder into itself"));
    }
    match fs::rename(src, dst) {
        Ok(()) => {
            let (b, _) = measure(dst);
            (pr.on)(b, src);
            Ok(())
        }
        Err(e) if e.raw_os_error() == Some(18) => {
            // EXDEV: different filesystem.
            copy_tree(src, dst, pr)?;
            let md = fs::symlink_metadata(src)?;
            if md.is_dir() {
                fs::remove_dir_all(src)
            } else {
                fs::remove_file(src)
            }
        }
        Err(e) => Err(e),
    }
}

/* ---------------- trash ---------------- */

pub fn trash_paths(paths: &[PathBuf]) -> R<()> {
    trash::delete_all(paths).map_err(|e| format!("Couldn't move to Trash: {e}"))
}

/// Put back the newest trashed items whose original location matches.
pub fn restore(originals: &[String]) -> R<usize> {
    restore_except(originals, &[])
}

/// Trash `to_trash`, then restore `originals` from the Trash, skipping the
/// items that were just trashed (undo of a "replace": the copy and the
/// original it replaced have the same path).
pub fn trash_and_restore(to_trash: &[String], originals: &[String]) -> R<usize> {
    use trash::os_limited;
    let before: Vec<std::ffi::OsString> = os_limited::list().map(|v| v.into_iter().map(|i| i.id).collect()).unwrap_or_default();
    if !to_trash.is_empty() {
        let ps: Vec<PathBuf> = to_trash.iter().map(PathBuf::from).collect();
        trash_paths(&ps)?;
    }
    if originals.is_empty() {
        return Ok(0);
    }
    let fresh: Vec<std::ffi::OsString> = os_limited::list()
        .map(|v| v.into_iter().map(|i| i.id).filter(|id| !before.contains(id)).collect())
        .unwrap_or_default();
    restore_except(originals, &fresh)
}

fn restore_except(originals: &[String], skip: &[std::ffi::OsString]) -> R<usize> {
    use trash::os_limited;
    let items = os_limited::list().map_err(|e| format!("Couldn't read the Trash: {e}"))?;
    let mut pick = Vec::new();
    for o in originals {
        let want = PathBuf::from(o);
        if let Some(it) = items
            .iter()
            .filter(|it| it.original_parent.join(&it.name) == want && !skip.contains(&it.id))
            .max_by_key(|it| it.time_deleted)
        {
            pick.push(it.clone());
        }
    }
    let n = pick.len();
    if n == 0 {
        return Err("Those items aren't in the Trash any more.".into());
    }
    os_limited::restore_all(pick).map_err(|e| format!("Couldn't restore from Trash: {e}"))?;
    Ok(n)
}

/// Restore items picked from the Trash folder view (paths inside Trash/files).
pub fn restore_from_trash(files: &[String]) -> R<usize> {
    use trash::os_limited;
    let items = os_limited::list().map_err(|e| format!("Couldn't read the Trash: {e}"))?;
    let want: Vec<String> = files
        .iter()
        .filter_map(|f| Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let pick: Vec<_> = items
        .into_iter()
        .filter(|it| {
            let stem = Path::new(&it.id).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            want.contains(&stem)
        })
        .collect();
    let n = pick.len();
    if n == 0 {
        return Err("Couldn't find those items in the Trash.".into());
    }
    os_limited::restore_all(pick).map_err(|e| format!("Couldn't restore: {e}"))?;
    Ok(n)
}

pub fn trash_dir() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"));
    data.join("Trash/files")
}

pub fn empty_trash() -> R<usize> {
    use trash::os_limited;
    let items = os_limited::list().map_err(|e| format!("Couldn't read the Trash: {e}"))?;
    let n = items.len();
    os_limited::purge_all(items).map_err(|e| format!("Couldn't empty the Trash: {e}"))?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn noop() -> impl FnMut(u64, &Path) {
        |_, _| {}
    }

    #[test]
    fn natural_sort() {
        let mut v = vec!["file10", "File2", "file1", "a", "B"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a", "B", "file1", "File2", "file10"]);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of("x.PNG", false), "img");
        assert_eq!(kind_of("notes.md", false), "text");
        assert_eq!(kind_of("main.rs", false), "code");
        assert_eq!(kind_of(".bashrc", false), "file");
        assert_eq!(kind_of("anything", true), "dir");
        assert_eq!(ext_of(".hidden"), "");
        assert_eq!(ext_of("a.tar.gz"), "gz");
    }

    #[test]
    fn names_are_checked() {
        assert!(check_name("ok.txt").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("  ").is_err());
        assert!(check_name("..").is_err());
        assert!(check_name("a/b").is_err());
    }

    #[test]
    fn unique_names() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(unique_in(t.path(), "a.txt"), t.path().join("a.txt"));
        fs::write(t.path().join("a.txt"), "x").unwrap();
        assert_eq!(unique_in(t.path(), "a.txt"), t.path().join("a (2).txt"));
        fs::write(t.path().join("a (2).txt"), "x").unwrap();
        assert_eq!(unique_in(t.path(), "a (2).txt"), t.path().join("a (3).txt"));
        fs::create_dir(t.path().join("dir")).unwrap();
        assert_eq!(unique_in(t.path(), "dir"), t.path().join("dir (2)"));
    }

    #[test]
    fn listing_sorts_dirs_first_and_hides_dotfiles() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("b.txt"), "hi").unwrap();
        fs::write(t.path().join(".secret"), "").unwrap();
        fs::create_dir(t.path().join("zdir")).unwrap();
        let l = list_dir(t.path().to_str().unwrap(), false).unwrap();
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zdir", "b.txt"]);
        assert_eq!(l.entries[0].items, Some(0));
        assert_eq!(l.entries[1].size, 2);
        let l = list_dir(t.path().to_str().unwrap(), true).unwrap();
        assert_eq!(l.entries.len(), 3);
        assert!(l.writable);
    }

    #[test]
    fn rename_refuses_overwrite() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        fs::write(&a, "1").unwrap();
        fs::write(t.path().join("b"), "2").unwrap();
        assert!(rename(a.to_str().unwrap(), "b").is_err());
        let n = rename(a.to_str().unwrap(), "c").unwrap();
        assert_eq!(fs::read_to_string(n).unwrap(), "1");
    }

    #[test]
    fn copy_tree_keeps_structure_links_and_modes() {
        let t = tempfile::tempdir().unwrap();
        let s = t.path().join("src");
        fs::create_dir_all(s.join("sub")).unwrap();
        fs::write(s.join("sub/f.sh"), "echo").unwrap();
        fs::set_permissions(s.join("sub/f.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("sub/f.sh", s.join("link")).unwrap();
        let c = AtomicBool::new(false);
        let mut seen = 0u64;
        let mut on = |b: u64, _: &Path| seen += b;
        copy_tree(&s, &t.path().join("dst"), &mut Progress { cancel: &c, on: &mut on }).unwrap();
        let d = t.path().join("dst");
        assert_eq!(fs::read_to_string(d.join("sub/f.sh")).unwrap(), "echo");
        assert_eq!(fs::metadata(d.join("sub/f.sh")).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(fs::symlink_metadata(d.join("link")).unwrap().file_type().is_symlink());
        assert_eq!(seen, 4);
    }

    #[test]
    fn copy_into_itself_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let s = t.path().join("s");
        fs::create_dir(&s).unwrap();
        let c = AtomicBool::new(false);
        let mut on = noop();
        assert!(copy_tree(&s, &s.join("inner"), &mut Progress { cancel: &c, on: &mut on }).is_err());
        assert!(move_tree(&s, &s.join("inner"), &mut Progress { cancel: &c, on: &mut on }).is_err());
    }

    #[test]
    fn cancelled_copy_leaves_no_partial_file() {
        let t = tempfile::tempdir().unwrap();
        let s = t.path().join("big");
        fs::write(&s, vec![7u8; 3 << 20]).unwrap();
        let c = AtomicBool::new(false);
        let mut on = |_: u64, _: &Path| c.store(true, Ordering::Relaxed);
        let r = copy_tree(&s, &t.path().join("out"), &mut Progress { cancel: &c, on: &mut on });
        assert!(r.is_err());
        assert!(!t.path().join("out").exists());
    }

    #[test]
    fn move_renames_within_fs() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a"), "x").unwrap();
        let c = AtomicBool::new(false);
        let mut on = noop();
        move_tree(&t.path().join("a"), &t.path().join("b"), &mut Progress { cancel: &c, on: &mut on }).unwrap();
        assert!(!t.path().join("a").exists());
        assert_eq!(fs::read_to_string(t.path().join("b")).unwrap(), "x");
    }

    #[test]
    fn mounts_are_filtered() {
        let m = "/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /media/luke/USB\\040STICK vfat rw 0 0\n\
                 tmpfs /run tmpfs rw 0 0\n//nas/share /srv/nas cifs rw 0 0\n/dev/sdc1 /mnt/data ext4 rw 0 0\n";
        let p = parse_mounts(m, Path::new("/home/luke"));
        let names: Vec<_> = p.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["USB STICK", "nas", "data"]);
        assert_eq!(p[1].icon, "net");
    }
}
