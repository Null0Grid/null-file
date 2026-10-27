//! The org.freedesktop.FileManager1 D-Bus service: what a browser's "Show in
//! folder", or any app's "show this file in the file manager", calls.
//!
//! Null File claims the name when it's the default file manager (or when D-Bus
//! started it for this). Making Null File the default also writes a user-level
//! D-Bus service file, so a request starts Null File when it isn't running.

use crate::fsops;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

pub const NAME: &str = "org.freedesktop.FileManager1";
pub const PATH: &str = "/org/freedesktop/FileManager1";
pub const ACTIVATED_FLAG: &str = "--dbus-activated";

/// What the window should do with a request.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Show {
    /// "folders": open each as a tab. "items": open each one's folder with it selected.
    pub mode: &'static str,
    pub paths: Vec<String>,
}

/// file:// URIs (or plain absolute paths) → existing absolute paths.
pub fn uris_to_paths(uris: &[String]) -> Vec<String> {
    let mut out = vec![];
    for u in uris {
        let s = if let Some(rest) = u.strip_prefix("file://") {
            // file:///path or file://localhost/path
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            percent_encoding::percent_decode_str(rest).decode_utf8_lossy().into_owned()
        } else if u.starts_with('/') {
            u.clone()
        } else {
            continue; // other schemes (sftp://, smb://…) aren't local files
        };
        let p = Path::new(&s);
        if p.is_absolute() && std::fs::symlink_metadata(p).is_ok() && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

struct Fm {
    app: AppHandle,
}

impl Fm {
    fn send(&self, mode: &'static str, uris: Vec<String>) {
        let paths = uris_to_paths(&uris);
        if paths.is_empty() {
            return;
        }
        let show = Show { mode, paths };
        // D-Bus may start us for this request before the window is listening: keep it for the window.
        let st = self.app.state::<crate::AppState>();
        if st.ui_ready.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = self.app.emit("show-paths", show);
        } else {
            st.pending_shows.lock().unwrap().push(show);
        }
        if let Some(w) = self.app.get_webview_window("main") {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
}

#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl Fm {
    fn show_folders(&self, uris: Vec<String>, _startup_id: String) {
        self.send("folders", uris);
    }
    fn show_items(&self, uris: Vec<String>, _startup_id: String) {
        self.send("items", uris);
    }
    /// Null File has no separate properties window: selecting the item shows it in the details panel.
    fn show_item_properties(&self, uris: Vec<String>, _startup_id: String) {
        self.send("items", uris);
    }
}

/// Serve the interface and claim the name. Returns the connection (keep it
/// alive) and whether we got the name.
pub fn start(app: AppHandle, replace: bool) -> Result<(zbus::blocking::Connection, bool), String> {
    use zbus::fdo::{RequestNameFlags, RequestNameReply};
    let conn = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.serve_at(PATH, Fm { app }))
        .and_then(|b| b.build())
        .map_err(|e| format!("No D-Bus session bus: {e}"))?;
    let mut flags = RequestNameFlags::AllowReplacement | RequestNameFlags::DoNotQueue;
    if replace {
        flags |= RequestNameFlags::ReplaceExisting;
    }
    let got = matches!(
        conn.request_name_with_flags(NAME, flags),
        Ok(RequestNameReply::PrimaryOwner) | Ok(RequestNameReply::AlreadyOwner)
    );
    Ok((conn, got))
}

/// How D-Bus should start this copy of Null File.
pub fn exec_line() -> Option<String> {
    if std::env::var_os("SNAP_NAME").is_some() {
        return None; // a snap can't own this name
    }
    if Path::new("/.flatpak-info").exists() {
        return Some(format!("/usr/bin/flatpak run --command=null-file dev.nullgrid.file {ACTIVATED_FLAG}"));
    }
    let exe: PathBuf = match std::env::var_os("APPIMAGE") {
        Some(a) => a.into(),
        None => {
            let p = std::env::current_exe().ok()?;
            let s = p.to_string_lossy();
            PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(&s).to_string())
        }
    };
    let e = exe.display().to_string();
    // The service file's Exec= has no quoting rules for spaces worth relying on.
    if e.contains(char::is_whitespace) {
        return None;
    }
    Some(format!("{e} {ACTIVATED_FLAG}"))
}

pub fn service_file_contents(exec: &str) -> String {
    format!("[D-BUS Service]\nName={NAME}\nExec={exec}\n")
}

/// The user's D-Bus services folder (always on the host, even inside Flatpak).
fn user_services_dir() -> PathBuf {
    let data = if Path::new("/.flatpak-info").exists() {
        None
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
    };
    data.unwrap_or_else(|| fsops::home().join(".local/share")).join("dbus-1/services")
}

pub fn service_file_installed() -> bool {
    std::fs::read_to_string(user_services_dir().join(format!("{NAME}.service")))
        .map(|c| exec_line().map(|e| c.contains(&format!("Exec={e}"))).unwrap_or(false))
        .unwrap_or(false)
}

/// Write ~/.local/share/dbus-1/services/org.freedesktop.FileManager1.service.
/// User-level service files take priority over the ones Dolphin or Nautilus install.
pub fn install_service_file() -> Result<PathBuf, String> {
    let exec = exec_line().ok_or("This copy of Null File can't be started by D-Bus.")?;
    let dir = user_services_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let p = dir.join(format!("{NAME}.service"));
    std::fs::write(&p, service_file_contents(&exec)).map_err(|e| format!("Couldn't write {}: {e}", p.display()))?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_become_existing_paths() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("My File.txt"), "x").unwrap();
        let f = t.path().join("My File.txt").display().to_string();
        let uri = format!("file://{}", f.replace(' ', "%20"));
        let got = uris_to_paths(&[
            uri.clone(),
            format!("file://localhost{}", f.replace(' ', "%20")),
            uri,
            "file:///definitely/not/here".into(),
            "sftp://host/x".into(),
            t.path().display().to_string(),
        ]);
        assert_eq!(got, vec![f, t.path().display().to_string()]);
    }

    #[test]
    fn service_file_shape() {
        let s = service_file_contents("/usr/bin/null-file --dbus-activated");
        assert_eq!(s, "[D-BUS Service]\nName=org.freedesktop.FileManager1\nExec=/usr/bin/null-file --dbus-activated\n");
    }
}
