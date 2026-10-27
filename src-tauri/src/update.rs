//! Update checks and self-updates.
//!
//! - AppImage and the tar.gz install update themselves: download the new file,
//!   check it against the release's SHA256SUMS, swap it in, relaunch.
//! - .deb and .rpm installs come with the Null Grid package repository, so the
//!   system updater (apt, dnf, KDE Discover, GNOME Software) installs new versions.
//! - Flatpak, Snap and AUR are updated by their own tools; the app only says a new version is out.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

const REPO: &str = "Null0Grid/null-file";
pub const APT_SOURCE: &str = "/etc/apt/sources.list.d/null-file.sources";
pub const DNF_REPO: &str = "/etc/yum.repos.d/null-file.repo";
pub const ZYPP_REPO: &str = "/etc/zypp/repos.d/null-file.repo";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub enum Kind {
    AppImage,
    Tar,
    Deb,
    Rpm,
    Aur,
    Flatpak,
    Snap,
    Other,
}

#[derive(Serialize)]
pub struct Info {
    pub current: String,
    pub latest: String,
    pub newer: bool,
    pub kind: Kind,
    /// "self" (Update now button), "system" (your system updater does it), or "manual" (link only)
    pub mode: &'static str,
    pub how: String,
    pub notes: String,
    pub url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize, Clone)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        _ => "x86_64",
    }
}

/// "v1.2.3" / "1.2.3" → (1,2,3)
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next().unwrap_or(Some(0))?, it.next().unwrap_or(Some(0))?))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn owned_by(cmd: &str, args: &[&str]) -> bool {
    std::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn kind() -> Kind {
    if std::env::var_os("SNAP_NAME").is_some() {
        return Kind::Snap;
    }
    if Path::new("/.flatpak-info").exists() {
        return Kind::Flatpak;
    }
    if std::env::var_os("APPIMAGE").is_some() {
        return Kind::AppImage;
    }
    let exe = current_exe_path().unwrap_or_default();
    let es = exe.display().to_string();
    if es.starts_with("/usr/") {
        if Path::new("/var/lib/dpkg/info/null-file.list").exists() {
            return Kind::Deb;
        }
        if owned_by("pacman", &["-Qo", &es]) {
            return Kind::Aur;
        }
        if owned_by("rpm", &["-qf", &es]) {
            return Kind::Rpm;
        }
        return Kind::Other;
    }
    // A copy the user unpacked (tar.gz), which they can write to.
    if exe.parent().map(writable).unwrap_or(false) {
        return Kind::Tar;
    }
    Kind::Other
}

/// The running binary's path. After an update replaced it, Linux reports the
/// old file as "<path> (deleted)": the new binary is at the plain path.
fn current_exe_path() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?;
    let s = p.to_string_lossy();
    Some(match s.strip_suffix(" (deleted)") {
        Some(t) => PathBuf::from(t),
        None => p,
    })
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".null-file-write-test-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// True when NO_PROXY says `host` should be reached directly.
pub fn bypass_proxy(host: &str, no_proxy: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_lowercase();
    no_proxy.split(',').map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()).any(|e| {
        if e == "*" {
            return true;
        }
        let e = e.trim_start_matches('*').trim_start_matches('.');
        if let Some((net, bits)) = e.split_once('/') {
            // IPv4 CIDR like 127.0.0.0/8
            let (Ok(n), Ok(b), Ok(h)) = (net.parse::<std::net::Ipv4Addr>(), bits.parse::<u32>(), host.parse::<std::net::Ipv4Addr>()) else { return false };
            let mask = if b == 0 { 0 } else { u32::MAX << (32 - b.min(32)) };
            return u32::from(n) & mask == u32::from(h) & mask;
        }
        host == e || host.ends_with(&format!(".{e}"))
    })
}

fn agent_for(url: &str) -> ureq::Agent {
    let b = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .user_agent(&format!("null-file/{}", env!("CARGO_PKG_VERSION")));
    // Use the system's proxy (HTTPS_PROXY…), except for hosts listed in NO_PROXY.
    let host = url.split("://").nth(1).unwrap_or("").split(['/', '?']).next().unwrap_or("");
    let host = host.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map(|(h, _)| h).unwrap_or(host);
    let np = std::env::var("NO_PROXY").or_else(|_| std::env::var("no_proxy")).unwrap_or_default();
    b.try_proxy_from_env(!bypass_proxy(host, &np)).build()
}

fn http_err(e: ureq::Error, what: &str) -> String {
    match e {
        ureq::Error::Status(404, _) => format!("{what}: GitHub says it isn't there (404). The release may not be public yet."),
        ureq::Error::Status(code, _) => format!("{what}: GitHub answered {code}."),
        ureq::Error::Transport(t) => format!("{what}: no connection ({}).", t.kind()),
    }
}

fn latest() -> Result<Release, String> {
    // NULL_FILE_UPDATE_URL points at another releases/latest-style JSON (mirrors, testing).
    let url = std::env::var("NULL_FILE_UPDATE_URL").unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"));
    agent_for(&url)
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| http_err(e, "Couldn't check for updates"))?
        .into_json::<Release>()
        .map_err(|e| format!("Couldn't read the update info: {e}"))
}

fn asset_name(k: &Kind, v: &str) -> Option<String> {
    let a = arch();
    match k {
        Kind::AppImage => Some(format!("Null-File-{v}-{a}.AppImage")),
        Kind::Tar => Some(format!("null-file-{v}-linux-{a}.tar.gz")),
        Kind::Deb => Some(format!("null-file_{v}_{}.deb", if a == "aarch64" { "arm64" } else { "amd64" })),
        Kind::Rpm => Some(format!("null-file-{v}-1.{a}.rpm")),
        _ => None,
    }
}

#[tauri::command]
pub async fn update_check() -> Result<Info, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let cur = env!("CARGO_PKG_VERSION").to_string();
        // Sandboxed copies have no network permission and are updated by their store anyway.
        match kind() {
            Kind::Flatpak => return Ok(Info { latest: cur.clone(), current: cur, newer: false, kind: Kind::Flatpak, mode: "system", how: "This copy updates with flatpak update (or your software center).".into(), notes: String::new(), url: String::new() }),
            Kind::Snap => return Ok(Info { latest: cur.clone(), current: cur, newer: false, kind: Kind::Snap, mode: "system", how: "Snap keeps this copy up to date on its own.".into(), notes: String::new(), url: String::new() }),
            _ => {}
        }
        let r = latest()?;
        let lat = r.tag_name.trim_start_matches('v').to_string();
        let k = kind();
        let (mode, how) = match k {
            Kind::AppImage | Kind::Tar => ("self", "Null File downloads it, checks it and restarts.".to_string()),
            Kind::Deb if Path::new(APT_SOURCE).exists() => ("system", "It installs with your other updates (Discover, or sudo apt update && sudo apt upgrade).".into()),
            Kind::Rpm if Path::new(DNF_REPO).exists() || Path::new(ZYPP_REPO).exists() => ("system", "It installs with your other updates (Discover/GNOME Software, or sudo dnf upgrade).".into()),
            Kind::Deb | Kind::Rpm => ("manual", "Download the new package from the release page and install it.".into()),
            Kind::Flatpak => ("system", "Update it with flatpak update (or your software center).".into()),
            Kind::Snap => ("system", "Snap updates it on its own, or run sudo snap refresh null-file.".into()),
            Kind::Aur => ("system", "Update it with your AUR helper, for example yay -Syu.".into()),
            Kind::Other => ("manual", "Download the new version from the release page.".into()),
        };
        Ok(Info { newer: is_newer(&lat, &cur), current: cur, latest: lat, kind: k, mode, how, notes: r.body, url: r.html_url })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone)]
struct Progress {
    done: u64,
    total: u64,
    stage: &'static str,
}

fn download(app: &AppHandle, url: &str, to: &Path) -> Result<String, String> {
    let resp = agent_for(url).get(url).call().map_err(|e| http_err(e, "Download failed"))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut r = resp.into_reader();
    let mut f = std::fs::File::create(to).map_err(|e| format!("Couldn't save the download: {e}"))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    let mut last = std::time::Instant::now();
    loop {
        let n = r.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n]).map_err(|e| format!("Couldn't save the download: {e}"))?;
        h.update(&buf[..n]);
        done += n as u64;
        if last.elapsed() > Duration::from_millis(150) {
            last = std::time::Instant::now();
            let _ = app.emit("update-progress", Progress { done, total, stage: "download" });
        }
    }
    f.sync_all().map_err(|e| e.to_string())?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The sha256 for `name` in a SHA256SUMS file.
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let h = it.next()?;
        let n = it.next()?.trim_start_matches('*');
        (n == name && h.len() == 64).then(|| h.to_lowercase())
    })
}

/// Download, verify and install the latest version (AppImage and tar.gz only).
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let k = kind();
        if !matches!(k, Kind::AppImage | Kind::Tar) {
            return Err("This copy of Null File is updated by your system's package manager.".into());
        }
        let r = latest()?;
        let v = r.tag_name.trim_start_matches('v').to_string();
        if !is_newer(&v, env!("CARGO_PKG_VERSION")) {
            return Err("You already have the latest version.".into());
        }
        let name = asset_name(&k, &v).unwrap();
        let find = |n: &str| r.assets.iter().find(|a| a.name == n).cloned();
        let asset = find(&name).ok_or(format!("The release doesn't have {name}."))?;
        let sums_asset = find("SHA256SUMS").ok_or("The release has no SHA256SUMS to check the download against.")?;
        let sums = agent_for(&sums_asset.browser_download_url).get(&sums_asset.browser_download_url).call().map_err(|e| http_err(e, "Couldn't get the checksums"))?.into_string().map_err(|e| e.to_string())?;
        let want = sum_for(&sums, &name).ok_or(format!("SHA256SUMS doesn't list {name}."))?;

        // Download next to the file being replaced, so the final rename is atomic.
        let target: PathBuf = match k {
            Kind::AppImage => PathBuf::from(std::env::var_os("APPIMAGE").unwrap()),
            _ => current_exe_path().ok_or("Can't find where Null File is installed.")?,
        };
        let dir = target.parent().ok_or("Can't find where Null File is installed.")?.to_path_buf();
        let part = dir.join(format!(".{name}.part"));
        let got = download(&app, &asset.browser_download_url, &part);
        let got = match got {
            Ok(g) => g,
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        };
        if got != want {
            let _ = std::fs::remove_file(&part);
            return Err("The download didn't match its checksum, so it wasn't installed.".into());
        }
        let _ = app.emit("update-progress", Progress { done: 1, total: 1, stage: "install" });
        let new_bin = if k == Kind::Tar {
            let x = dir.join(format!(".null-file-update-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&x);
            std::fs::create_dir(&x).map_err(|e| e.to_string())?;
            let ok = std::process::Command::new("tar").arg("-xzf").arg(&part).arg("-C").arg(&x).status().map(|s| s.success()).unwrap_or(false);
            let _ = std::fs::remove_file(&part);
            let bin = x.join(format!("null-file-{v}-linux-{}", arch())).join("null-file");
            if !ok || !bin.is_file() {
                let _ = std::fs::remove_dir_all(&x);
                return Err("Couldn't unpack the update.".into());
            }
            let staged = dir.join(".null-file.new");
            std::fs::rename(&bin, &staged).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(&x);
            staged
        } else {
            part
        };
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_bin, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        std::fs::rename(&new_bin, &target).map_err(|e| format!("Couldn't replace Null File: {e}"))?;
        Ok(v)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Start the (updated) app again and quit this one. The new copy waits a
/// moment so this one has released the single-instance lock.
#[tauri::command]
pub fn relaunch(app: AppHandle) -> Result<(), String> {
    let exe = std::env::var_os("APPIMAGE").map(PathBuf::from).or_else(current_exe_path).ok_or("Can't find Null File.")?;
    use std::os::unix::process::CommandExt;
    std::process::Command::new("sh")
        .process_group(0) // its own group, so it outlives this process
        .arg("-c")
        .arg("sleep 1; exec \"$0\"")
        .arg(&exe)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_proxy_rules() {
        let np = "localhost,127.0.0.0/8,.internal.lan, example.com";
        assert!(bypass_proxy("127.0.0.1", np));
        assert!(bypass_proxy("localhost", np));
        assert!(bypass_proxy("nas.internal.lan", np));
        assert!(bypass_proxy("api.example.com", np));
        assert!(!bypass_proxy("api.github.com", np));
        assert!(!bypass_proxy("10.0.0.1", np));
        assert!(bypass_proxy("anything", "*"));
    }

    #[test]
    fn versions_compare() {
        assert!(is_newer("1.1.0", "1.0.0"));
        assert!(is_newer("v1.0.10", "1.0.9"));
        assert!(is_newer("2.0", "1.9.9"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.1.0"));
        assert!(!is_newer("junk", "1.0.0"));
        assert_eq!(parse_version("1.2.3-beta.1"), Some((1, 2, 3)));
    }

    #[test]
    fn checksums_are_found_by_exact_name() {
        let s = "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111  Null-File-1.1.0-x86_64.AppImage\n\
                 BBBB2222BBBB2222BBBB2222BBBB2222BBBB2222BBBB2222BBBB2222BBBB2222 *null-file-1.1.0-linux-x86_64.tar.gz\n";
        assert_eq!(sum_for(s, "Null-File-1.1.0-x86_64.AppImage").unwrap(), "aaaa1111".repeat(8));
        assert_eq!(sum_for(s, "null-file-1.1.0-linux-x86_64.tar.gz").unwrap(), "bbbb2222".repeat(8));
        assert!(sum_for(s, "Null-File-1.1.0").is_none());
    }

    #[test]
    fn asset_names_match_the_release_files() {
        assert_eq!(asset_name(&Kind::AppImage, "1.1.0").unwrap(), format!("Null-File-1.1.0-{}.AppImage", arch()));
        assert_eq!(asset_name(&Kind::Tar, "1.1.0").unwrap(), format!("null-file-1.1.0-linux-{}.tar.gz", arch()));
        assert!(asset_name(&Kind::Flatpak, "1.1.0").is_none());
    }
}
