//! Desktop integration: open with the default app, terminal here, and
//! registering Null File as the default file manager (inode/directory).

use serde::Serialize;
use std::path::Path;
use std::process::{Command, Stdio};

pub const DESKTOP_ID: &str = "dev.nullgrid.file.desktop";

fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// The .desktop file this copy of Null File was installed with. Packages
/// differ: dev.nullgrid.file.desktop (deb, Flatpak, tar.gz, AUR),
/// null-file.desktop (rpm/AppImage from Tauri's bundler), and snaps rename
/// theirs to <snap>_<app>.desktop.
pub fn desktop_id() -> String {
    if let Ok(name) = std::env::var("SNAP_NAME") {
        return format!("{name}_null-file.desktop");
    }
    if in_flatpak() {
        return DESKTOP_ID.into();
    }
    let mut dirs: Vec<std::path::PathBuf> = vec![];
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(d.into());
    } else if let Some(h) = std::env::var_os("HOME") {
        dirs.push(Path::new(&h).join(".local/share"));
    }
    let sys = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(sys.split(':').filter(|s| !s.is_empty()).map(std::path::PathBuf::from));
    for id in [DESKTOP_ID, "null-file.desktop", "Null File.desktop"] {
        if dirs.iter().any(|d| d.join("applications").join(id).is_file()) {
            return id.into();
        }
    }
    DESKTOP_ID.into()
}

fn spawn(cmd: &str, args: &[&str], cwd: Option<&Path>) -> std::io::Result<()> {
    let mut c = Command::new(cmd);
    c.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    // Strip variables an AppImage injects so child apps use system libraries.
    for v in ["LD_LIBRARY_PATH", "GDK_PIXBUF_MODULE_FILE", "GIO_MODULE_DIR", "GTK_PATH"] {
        if std::env::var_os("APPIMAGE").is_some() {
            c.env_remove(v);
        }
    }
    c.spawn().map(|_| ())
}

fn which(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false)
}

pub fn open_default(p: &Path) -> Result<(), String> {
    if !p.exists() {
        return Err(format!("{} no longer exists.", p.display()));
    }
    let s = p.display().to_string();
    for (cmd, args) in [("xdg-open", vec![s.as_str()]), ("gio", vec!["open", s.as_str()]), ("kde-open", vec![s.as_str()])] {
        if which(cmd) && spawn(cmd, &args, None).is_ok() {
            return Ok(());
        }
    }
    Err("No program found to open files (install xdg-utils).".into())
}

/// Terminals to try, in order. Konsole first because Null OS ships it.
fn terminals() -> Vec<(String, Vec<String>)> {
    let mut v: Vec<(String, Vec<String>)> = vec![];
    if let Ok(t) = std::env::var("TERMINAL") {
        if !t.is_empty() {
            v.push((t, vec![]));
        }
    }
    for (c, a) in [
        ("konsole", vec!["--workdir", "."]),
        ("x-terminal-emulator", vec![]),
        ("gnome-terminal", vec![]),
        ("kgx", vec![]),
        ("xfce4-terminal", vec![]),
        ("alacritty", vec![]),
        ("kitty", vec![]),
        ("foot", vec![]),
        ("wezterm", vec!["start", "--cwd", "."]),
        ("xterm", vec![]),
    ] {
        v.push((c.into(), a.into_iter().map(String::from).collect()));
    }
    v
}

pub fn open_terminal(dir: &Path) -> Result<String, String> {
    if !dir.is_dir() {
        return Err(format!("{} isn't a folder.", dir.display()));
    }
    if in_flatpak() {
        // Terminals live on the host: ask Flatpak to start one there.
        let d = format!("--directory={}", dir.display());
        for (cmd, args) in terminals() {
            let probe = Command::new("flatpak-spawn").args(["--host", "which", &cmd]).output();
            if probe.map(|o| o.status.success()).unwrap_or(false) {
                let mut a: Vec<&str> = vec!["--host", &d, &cmd];
                a.extend(args.iter().map(|s| s.as_str()));
                if spawn("flatpak-spawn", &a, None).is_ok() {
                    return Ok(cmd);
                }
            }
        }
        return Err("No terminal app found on this computer.".into());
    }
    for (cmd, args) in terminals() {
        if which(&cmd) {
            let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            if spawn(&cmd, &a, Some(dir)).is_ok() {
                return Ok(cmd);
            }
        }
    }
    Err("No terminal app found. Set $TERMINAL or install Konsole.".into())
}

#[derive(Serialize)]
pub struct DefaultStatus {
    pub is_default: bool,
    pub current: String,
    pub can_set: bool,
}

/// xdg-mime, run on the host when we're inside a Flatpak (the sandbox has its own mimeapps.list).
fn xdg_mime(args: &[&str]) -> Command {
    if in_flatpak() {
        let mut c = Command::new("flatpak-spawn");
        c.arg("--host").arg("xdg-mime").args(args);
        c
    } else {
        let mut c = Command::new("xdg-mime");
        c.args(args);
        c
    }
}

pub fn default_status() -> DefaultStatus {
    let current = xdg_mime(&["query", "default", "inode/directory"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let id = desktop_id();
    DefaultStatus { is_default: current == id, current, can_set: in_flatpak() || which("xdg-mime") }
}

pub fn make_default() -> Result<DefaultStatus, String> {
    if !in_flatpak() && !which("xdg-mime") {
        return Err("xdg-mime isn't installed (package xdg-utils).".into());
    }
    let id = desktop_id();
    let ok = xdg_mime(&["default", &id, "inode/directory"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err("xdg-mime couldn't set the default file manager.".into());
    }
    Ok(default_status())
}

/// Copy text (paths) to the system clipboard with whichever tool exists.
pub fn copy_text(text: &str) -> Result<(), String> {
    use std::io::Write;
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let tools: Vec<(&str, Vec<&str>)> = if wayland {
        vec![("wl-copy", vec![]), ("xclip", vec!["-selection", "clipboard"]), ("xsel", vec!["-ib"])]
    } else {
        vec![("xclip", vec!["-selection", "clipboard"]), ("xsel", vec!["-ib"]), ("wl-copy", vec![])]
    };
    for (cmd, args) in tools {
        if !which(cmd) {
            continue;
        }
        if let Ok(mut child) = Command::new(cmd).args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            if let Some(mut si) = child.stdin.take() {
                let _ = si.write_all(text.as_bytes());
            }
            return Ok(());
        }
    }
    Err("NO_TOOL".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_id_prefers_the_installed_entry() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("applications")).unwrap();
        std::fs::write(t.path().join("applications/null-file.desktop"), "").unwrap();
        std::env::remove_var("SNAP_NAME");
        std::env::set_var("XDG_DATA_HOME", t.path());
        std::env::set_var("XDG_DATA_DIRS", t.path().join("none"));
        assert_eq!(desktop_id(), "null-file.desktop");
        std::fs::write(t.path().join("applications/dev.nullgrid.file.desktop"), "").unwrap();
        assert_eq!(desktop_id(), "dev.nullgrid.file.desktop");
        std::env::set_var("SNAP_NAME", "null-file");
        assert_eq!(desktop_id(), "null-file_null-file.desktop");
        std::env::remove_var("SNAP_NAME");
    }
}
