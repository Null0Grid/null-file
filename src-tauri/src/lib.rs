//! Null File backend. Command and argument names match src/backend.js.

mod fm1;
mod fsops;
mod search;
mod sysint;
mod update;
mod vault;

use fsops::{abs, Entry, Listing, Place, R};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};

/* ---------------- settings ---------------- */

/// App settings in ~/.config/dev.nullgrid.file/settings.json.
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Settings {
    pinned: Vec<String>,
    recent: Vec<String>,
    hidden: bool,
    view: String,
    sort: String,
    desc: bool,
    preview: bool,
    sidebar_w: u32,
    graph: serde_json::Value,
    last_tabs: Vec<String>,
    restore_tabs: bool,
    confirm_trash: bool,
    /// Lock the vault after this many idle minutes (0 = only when Null File closes).
    vault_autolock: u32,
    check_updates: bool,
    skip_version: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            pinned: vec![],
            recent: vec![],
            hidden: false,
            view: "grid".into(),
            sort: "name".into(),
            desc: false,
            preview: true,
            sidebar_w: 230,
            graph: serde_json::Value::Null,
            last_tabs: vec![],
            restore_tabs: true,
            confirm_trash: false,
            vault_autolock: 5,
            check_updates: true,
            skip_version: String::new(),
        }
    }
}

fn settings_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
}

fn load_settings(app: &AppHandle) -> Settings {
    settings_path(app)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_atomic(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, p)
}

/* ---------------- state ---------------- */

struct Job {
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub(crate) struct AppState {
    jobs: Mutex<HashMap<u64, Job>>,
    next_job: AtomicU64,
    search_gen: Arc<AtomicU64>,
    /// Paths given on the command line (or by a second launch) waiting for the UI.
    pending: Mutex<Vec<String>>,
    watcher: Mutex<Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>,
    watched: Mutex<Vec<PathBuf>>,
    settings_lock: Mutex<()>,
    /// The unlocked vault (None while locked).
    vault: vault::Shared,
    /// Our org.freedesktop.FileManager1 service, while we own the name.
    dbus: Mutex<Option<zbus::blocking::Connection>>,
    /// Set once the window listens for "show-paths"; requests before that wait here.
    pub(crate) ui_ready: AtomicBool,
    pub(crate) pending_shows: Mutex<Vec<fm1::Show>>,
}

/// Answer "show this file in the file manager" requests (in the background: D-Bus can be slow or missing).
fn start_fm1(app: AppHandle, replace: bool) {
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        if st.dbus.lock().unwrap().is_some() {
            return;
        }
        match fm1::start(app.clone(), replace) {
            Ok((conn, true)) => *st.dbus.lock().unwrap() = Some(conn),
            Ok((_, false)) => {} // another file manager has the name and won't give it up
            Err(e) => eprintln!("{e}"),
        }
    });
}

fn vstore() -> vault::Store {
    vault::Store::at(vault::Store::default_root())
}

/// Run `f` on the unlocked vault, or explain that it's locked.
fn with_vault<T>(st: &AppState, f: impl FnOnce(&mut vault::Open) -> R<T>) -> R<T> {
    let mut g = st.vault.lock().unwrap();
    let o = g.as_mut().ok_or("LOCKED")?;
    f(o)
}

#[derive(Serialize, Clone)]
struct JobEvent {
    id: u64,
    op: String,
    state: &'static str, // running | done | error | cancelled
    done_bytes: u64,
    total_bytes: u64,
    done_items: u64,
    total_items: u64,
    current: String,
    error: Option<String>,
    /// For undo: (from, to) pairs that actually happened.
    moved: Vec<(String, String)>,
    created: Vec<String>,
    trashed: Vec<String>,
}

/* ---------------- commands: browsing ---------------- */

#[tauri::command]
fn list_dir(st: State<'_, AppState>, path: String, hidden: bool) -> R<Listing> {
    if vault::is_vault(&path) {
        let r = vault::rel(&path)?;
        return with_vault(&st, |o| o.list(&r, hidden));
    }
    fsops::list_dir(&path, hidden)
}

#[tauri::command]
fn stat(st: State<'_, AppState>, path: String) -> R<Entry> {
    if vault::is_vault(&path) {
        let r = vault::rel(&path)?;
        return with_vault(&st, |o| o.entry(&r));
    }
    let p = abs(&path)?;
    fsops::entry_for(&p).map_err(|e| fsops::err(e, "Can't read", &p))
}

#[derive(Serialize)]
struct Places {
    home: String,
    places: Vec<Place>,
    drives: Vec<Place>,
    trash: String,
}

#[tauri::command]
fn places() -> Places {
    Places {
        home: fsops::home().display().to_string(),
        places: fsops::places(),
        drives: fsops::drives(),
        trash: fsops::trash_dir().display().to_string(),
    }
}

#[tauri::command]
fn dir_size(st: State<'_, AppState>, path: String) -> R<(u64, u64)> {
    if vault::is_vault(&path) {
        let r = vault::rel(&path)?;
        return with_vault(&st, |o| Ok(o.measure(&r)));
    }
    let p = abs(&path)?;
    Ok(fsops::measure(&p))
}

#[tauri::command]
async fn search(st: State<'_, AppState>, root: String, query: String, hidden: bool) -> R<search::SearchResult> {
    if vault::is_vault(&root) {
        let r = vault::rel(&root)?;
        let hits = with_vault(&st, |o| Ok(o.find(&r, &query, hidden, 500)))?;
        return Ok(search::SearchResult { truncated: hits.len() >= 500, scanned: 0, hits });
    }
    let p = abs(&root)?;
    let gen = st.search_gen.clone();
    let my = gen.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn_blocking(move || search::find(&p, &query, hidden, 500, &gen, my))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn cancel_search(st: State<'_, AppState>) {
    st.search_gen.fetch_add(1, Ordering::SeqCst);
}

#[tauri::command]
async fn graph(st: State<'_, AppState>, root: String, depth: u32, hidden: bool, max_nodes: usize) -> R<search::Graph> {
    if vault::is_vault(&root) {
        let r = vault::rel(&root)?;
        return with_vault(&st, |o| o.tree(&r, depth.min(12), hidden, max_nodes.clamp(10, 5000)));
    }
    let p = abs(&root)?;
    tauri::async_runtime::spawn_blocking(move || search::tree(&p, depth.min(12), hidden, max_nodes.clamp(10, 5000)))
        .await
        .map_err(|e| e.to_string())?
}

/// First chunk of a text file for the preview pane. Binary files return None.
#[tauri::command]
fn read_text(st: State<'_, AppState>, path: String, max: usize) -> R<Option<(String, bool)>> {
    use std::io::Read;
    let max = max.min(512 * 1024);
    let (mut buf, more) = if vault::is_vault(&path) {
        let r = vault::rel(&path)?;
        let all = match with_vault(&st, |o| o.read_all(&r, 8 * 1024 * 1024)) {
            Err(e) if e == "TOO_BIG" => return Ok(None),
            x => x?,
        };
        (all[..all.len().min(max)].to_vec(), all.len() > max)
    } else {
        let p = abs(&path)?;
        let mut f = std::fs::File::open(&p).map_err(|e| fsops::err(e, "Can't open", &p))?;
        let mut buf = vec![0u8; max + 1];
        let mut n = 0;
        while n < buf.len() {
            let k = f.read(&mut buf[n..]).map_err(|e| fsops::err(e, "Can't read", &p))?;
            if k == 0 {
                break;
            }
            n += k;
        }
        (buf, n > max)
    };
    let n = buf.len();
    buf.truncate(n.min(max));
    if buf.iter().take(8192).any(|&b| b == 0) {
        return Ok(None);
    }
    // Cut at a char boundary if we truncated mid-character.
    let s = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(e) => {
            let valid = e.utf8_error().valid_up_to();
            if valid + 4 < e.as_bytes().len() {
                return Ok(None);
            }
            String::from_utf8_lossy(&e.as_bytes()[..valid]).into_owned()
        }
    };
    Ok(Some((s, more)))
}

/* ---------------- commands: changing things ---------------- */

#[tauri::command]
fn mkdir(st: State<'_, AppState>, parent: String, name: String) -> R<String> {
    if vault::is_vault(&parent) {
        let r = vault::rel(&parent)?;
        return with_vault(&st, |o| o.mkdir(&r, &name));
    }
    fsops::mkdir(&parent, &name)
}

#[tauri::command]
fn mkfile(st: State<'_, AppState>, parent: String, name: String) -> R<String> {
    if vault::is_vault(&parent) {
        let r = vault::rel(&parent)?;
        return with_vault(&st, |o| o.mkfile(&r, &name));
    }
    fsops::mkfile(&parent, &name)
}

#[tauri::command]
fn rename(st: State<'_, AppState>, path: String, name: String) -> R<String> {
    if vault::is_vault(&path) {
        let r = vault::rel(&path)?;
        return with_vault(&st, |o| o.rename(&r, &name));
    }
    fsops::rename(&path, &name)
}

#[tauri::command]
fn trash_and_restore(trash: Vec<String>, restore: Vec<String>) -> R<usize> {
    for p in trash.iter().chain(restore.iter()) {
        abs(p)?;
    }
    fsops::trash_and_restore(&trash, &restore)
}

/// Move a single item to an exact path (used by undo).
#[tauri::command]
fn move_exact(from: String, to: String) -> R<()> {
    let (a, b) = (abs(&from)?, abs(&to)?);
    if std::fs::symlink_metadata(&b).is_ok() {
        return Err(format!("{} already exists.", b.display()));
    }
    let c = AtomicBool::new(false);
    let mut on = |_: u64, _: &Path| {};
    fsops::move_tree(&a, &b, &mut fsops::Progress { cancel: &c, on: &mut on }).map_err(|e| fsops::err(e, "Can't move", &a))
}

#[tauri::command]
fn trash(st: State<'_, AppState>, paths: Vec<String>) -> R<Vec<String>> {
    // Vault items never go to the system Trash (that would leave them unencrypted): they're deleted for good.
    if paths.iter().any(|p| vault::is_vault(p)) {
        let rels: Vec<String> = paths.iter().map(|p| vault::rel(p)).collect::<R<_>>()?;
        with_vault(&st, |o| o.delete(&rels))?;
        return Ok(vec![]);
    }
    let ps: Vec<PathBuf> = paths.iter().map(|p| abs(p)).collect::<R<_>>()?;
    fsops::trash_paths(&ps)?;
    Ok(paths)
}

#[tauri::command]
fn restore(paths: Vec<String>) -> R<usize> {
    fsops::restore(&paths)
}

#[tauri::command]
fn restore_from_trash(paths: Vec<String>) -> R<usize> {
    fsops::restore_from_trash(&paths)
}

#[tauri::command]
fn empty_trash() -> R<usize> {
    fsops::empty_trash()
}

/// Copy or move `paths` into `dest` on a background thread, reporting progress
/// with "job" events. `conflict` is keep-both | replace | skip.
#[tauri::command]
fn transfer(app: AppHandle, st: State<'_, AppState>, op: String, paths: Vec<String>, dest: String, conflict: String) -> R<u64> {
    if vault::is_vault(&dest) || paths.iter().any(|p| vault::is_vault(p)) {
        return vault_transfer(app, st, op, paths, dest, conflict);
    }
    let dest = abs(&dest)?;
    if !dest.is_dir() {
        return Err(format!("{} isn't a folder.", dest.display()));
    }
    let srcs: Vec<PathBuf> = paths.iter().map(|p| abs(p)).collect::<R<_>>()?;
    let id = st.next_job.fetch_add(1, Ordering::SeqCst) + 1;
    let cancel = Arc::new(AtomicBool::new(false));
    st.jobs.lock().unwrap().insert(id, Job { cancel: cancel.clone() });
    let is_move = op == "move";
    std::thread::spawn(move || {
        let (mut total_bytes, mut total_items) = (0, 0);
        for s in &srcs {
            let (b, n) = fsops::measure(s);
            total_bytes += b;
            total_items += n;
        }
        let mut ev = JobEvent {
            id,
            op: op.clone(),
            state: "running",
            done_bytes: 0,
            total_bytes,
            done_items: 0,
            total_items,
            current: String::new(),
            error: None,
            moved: vec![],
            created: vec![],
            trashed: vec![],
        };
        let _ = app.emit("job", ev.clone());
        let mut last = Instant::now();
        let mut errors: Vec<String> = vec![];
        for src in &srcs {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let Some(name) = src.file_name() else { continue };
            let name = name.to_string_lossy().into_owned();
            let mut target = dest.join(&name);
            let same_place = src.parent() == Some(dest.as_path());
            if is_move && same_place {
                continue; // moving into the folder it's already in
            }
            if std::fs::symlink_metadata(&target).is_ok() || same_place {
                match conflict.as_str() {
                    "skip" if !same_place => continue,
                    "replace" if !same_place => {
                        if let Err(e) = fsops::trash_paths(&[target.clone()]) {
                            errors.push(e);
                            continue;
                        }
                        ev.trashed.push(target.display().to_string());
                    }
                    _ => target = fsops::unique_in(&dest, &name),
                }
            }
            ev.current = name.clone();
            let mut on = |b: u64, _p: &Path| {
                ev.done_bytes += b;
                if b == 0 {
                    ev.done_items += 1;
                }
                if last.elapsed() > Duration::from_millis(120) {
                    last = Instant::now();
                    let _ = app.emit("job", ev.clone());
                }
            };
            let mut pr = fsops::Progress { cancel: &cancel, on: &mut on };
            let res = if is_move { fsops::move_tree(src, &target, &mut pr) } else { fsops::copy_tree(src, &target, &mut pr) };
            match res {
                Ok(()) => {
                    if is_move {
                        ev.moved.push((src.display().to_string(), target.display().to_string()));
                    } else {
                        ev.created.push(target.display().to_string());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    // A cancelled folder copy can leave a partial folder: remove what we made.
                    if !is_move && target.exists() {
                        let _ = std::fs::remove_dir_all(&target);
                    }
                    break;
                }
                Err(e) => errors.push(fsops::err(e, if is_move { "Can't move" } else { "Can't copy" }, src)),
            }
        }
        ev.state = if cancel.load(Ordering::Relaxed) {
            "cancelled"
        } else if errors.is_empty() {
            "done"
        } else {
            "error"
        };
        ev.done_bytes = ev.done_bytes.min(ev.total_bytes);
        ev.error = if errors.is_empty() { None } else { Some(errors.join("\n")) };
        let _ = app.emit("job", ev);
        if let Some(s) = app.try_state::<AppState>() {
            s.jobs.lock().unwrap().remove(&id);
        }
    });
    Ok(id)
}

fn vault_transfer(app: AppHandle, st: State<'_, AppState>, op: String, paths: Vec<String>, dest: String, conflict: String) -> R<u64> {
    if st.vault.lock().unwrap().is_none() {
        return Err("Unlock the vault first.".into());
    }
    let id = st.next_job.fetch_add(1, Ordering::SeqCst) + 1;
    let cancel = Arc::new(AtomicBool::new(false));
    st.jobs.lock().unwrap().insert(id, Job { cancel: cancel.clone() });
    let v = st.vault.clone();
    std::thread::spawn(move || {
        let (mut total_bytes, mut total_items) = (0, 0);
        for p in &paths {
            if let Ok(side) = vault::Side::parse(p) {
                let (b, n) = vault::measure(&v, &side);
                total_bytes += b;
                total_items += n;
            }
        }
        let mut ev = JobEvent {
            id,
            op: op.clone(),
            state: "running",
            done_bytes: 0,
            total_bytes,
            done_items: 0,
            total_items,
            current: String::new(),
            error: None,
            moved: vec![],
            created: vec![],
            trashed: vec![],
        };
        let _ = app.emit("job", ev.clone());
        let mut last = Instant::now();
        let (created, errors) = {
            let mut on = |b: u64, item: bool, cur: &str| {
                ev.done_bytes += b;
                if item {
                    ev.done_items += 1;
                }
                if ev.current != cur {
                    ev.current = cur.to_string();
                }
                if last.elapsed() > Duration::from_millis(120) {
                    last = Instant::now();
                    let _ = app.emit("job", ev.clone());
                }
            };
            let mut cx = vault::JobCtx { cancel: &cancel, on: &mut on };
            vault::transfer(&v, op == "move", &paths, &dest, &conflict, &mut cx)
        };
        ev.state = if cancel.load(Ordering::Relaxed) {
            "cancelled"
        } else if errors.is_empty() {
            "done"
        } else {
            "error"
        };
        ev.done_bytes = ev.done_bytes.min(ev.total_bytes);
        ev.error = if errors.is_empty() { None } else { Some(errors.join("\n")) };
        // Vault transfers can't be undone (the originals of a move are deleted for good).
        ev.created = created;
        ev.op = format!("vault-{op}");
        let _ = app.emit("job", ev);
        if let Some(s) = app.try_state::<AppState>() {
            s.jobs.lock().unwrap().remove(&id);
        }
    });
    Ok(id)
}

#[tauri::command]
fn cancel_job(st: State<'_, AppState>, id: u64) {
    if let Some(j) = st.jobs.lock().unwrap().get(&id) {
        j.cancel.store(true, Ordering::Relaxed);
    }
}

/* ---------------- commands: system ---------------- */

#[tauri::command]
fn open_path(st: State<'_, AppState>, path: String) -> R<()> {
    if vault::is_vault(&path) {
        let r = vault::rel(&path)?;
        let p = with_vault(&st, |o| o.open_copy(&r))?;
        return sysint::open_default(&p);
    }
    sysint::open_default(&abs(&path)?)
}

#[tauri::command]
fn open_url(url: String) -> R<()> {
    sysint::open_url(&url)
}

#[tauri::command]
fn open_terminal(path: String) -> R<String> {
    sysint::open_terminal(&abs(&path)?)
}

#[tauri::command]
fn default_status() -> sysint::DefaultStatus {
    sysint::default_status()
}

#[tauri::command]
fn make_default(app: AppHandle) -> R<sysint::DefaultStatus> {
    let s = sysint::make_default()?;
    if s.is_default {
        start_fm1(app, true);
    }
    Ok(s)
}

#[tauri::command]
fn copy_to_clipboard(text: String) -> R<()> {
    sysint::copy_text(&text)
}

/* ---------------- commands: vault ---------------- */

#[tauri::command]
fn vault_status(st: State<'_, AppState>) -> vault::Status {
    vault::status(&vstore(), st.vault.lock().unwrap().is_some())
}

#[tauri::command]
async fn vault_setup(st: State<'_, AppState>, pin: String) -> R<String> {
    let (o, words) = tauri::async_runtime::spawn_blocking(move || vault::setup(&vstore(), &pin)).await.map_err(|e| e.to_string())??;
    *st.vault.lock().unwrap() = Some(o);
    Ok(words)
}

#[tauri::command]
async fn vault_unlock(st: State<'_, AppState>, pin: String) -> R<()> {
    let o = tauri::async_runtime::spawn_blocking(move || vault::unlock(&vstore(), &pin)).await.map_err(|e| e.to_string())??;
    *st.vault.lock().unwrap() = Some(o);
    Ok(())
}

#[tauri::command]
async fn vault_recover(st: State<'_, AppState>, words: String, pin: String) -> R<()> {
    let o = tauri::async_runtime::spawn_blocking(move || vault::recover(&vstore(), &words, &pin)).await.map_err(|e| e.to_string())??;
    *st.vault.lock().unwrap() = Some(o);
    Ok(())
}

#[tauri::command]
async fn vault_change_pin(st: State<'_, AppState>, pin: String) -> R<()> {
    let v = st.vault.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let g = v.lock().unwrap();
        g.as_ref().ok_or("LOCKED")?.change_pin(&pin)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Lock: save back edits made in other apps, erase decrypted copies, forget the key.
#[tauri::command]
fn vault_lock(st: State<'_, AppState>) -> R<usize> {
    for j in st.jobs.lock().unwrap().values() {
        j.cancel.store(true, Ordering::Relaxed);
    }
    let o = st.vault.lock().unwrap().take();
    match o {
        Some(o) => o.close(),
        None => Ok(0),
    }
}

#[tauri::command]
fn vault_sync(st: State<'_, AppState>) -> R<usize> {
    with_vault(&st, |o| o.sync_back())
}

/// Serves decrypted vault files to the window for thumbnails and previews:
/// nfvault://localhost/<percent-encoded vault path>
fn vault_protocol(app: &AppHandle, req: &tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    use tauri::http::{Response, StatusCode};
    let fail = |code: StatusCode| Response::builder().status(code).body(Vec::new()).unwrap();
    let raw = req.uri().path().trim_start_matches('/');
    let path = percent_encoding::percent_decode_str(raw).decode_utf8_lossy().into_owned();
    let Ok(r) = vault::rel(&path) else { return fail(StatusCode::BAD_REQUEST) };
    let st = app.state::<AppState>();
    let (key, blobs, blob, size) = {
        let g = st.vault.lock().unwrap();
        let Some(o) = g.as_ref() else { return fail(StatusCode::FORBIDDEN) };
        let Ok(n) = o.node(&r) else { return fail(StatusCode::NOT_FOUND) };
        if n.dir {
            return fail(StatusCode::NOT_FOUND);
        }
        (o.key.clone(), o.store.blobs(), n.blob.clone(), n.size)
    };
    if size > 300 * 1024 * 1024 {
        return fail(StatusCode::PAYLOAD_TOO_LARGE);
    }
    match vault::read_blob(&key, &blobs, &blob) {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", sysint::mime_for(&r))
            .header("Cache-Control", "no-store")
            .body(bytes.to_vec())
            .unwrap(),
        Err(_) => fail(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/* ---------------- commands: settings, startup, watching ---------------- */

#[tauri::command]
fn get_settings(app: AppHandle) -> Settings {
    load_settings(&app)
}

#[tauri::command]
fn save_settings(app: AppHandle, st: State<'_, AppState>, settings: Settings) -> R<()> {
    let _g = st.settings_lock.lock().unwrap();
    let p = settings_path(&app).ok_or("No config folder.")?;
    write_atomic(&p, serde_json::to_string_pretty(&settings).unwrap().as_bytes()).map_err(|e| e.to_string())
}

/// The window is listening: hand over "show in folder" requests that arrived before it was.
#[tauri::command]
fn ui_ready(st: State<'_, AppState>) -> Vec<fm1::Show> {
    let mut q = st.pending_shows.lock().unwrap();
    st.ui_ready.store(true, Ordering::SeqCst);
    std::mem::take(&mut *q)
}

/// Paths passed on the command line, consumed once by the UI.
#[tauri::command]
fn take_pending(st: State<'_, AppState>) -> Vec<String> {
    std::mem::take(&mut *st.pending.lock().unwrap())
}

/// Watch these folders (non-recursively) and emit "changed" with the folder path.
#[tauri::command]
fn watch(app: AppHandle, st: State<'_, AppState>, paths: Vec<String>) -> R<()> {
    use notify::RecursiveMode;
    let mut w = st.watcher.lock().unwrap();
    if w.is_none() {
        let app2 = app.clone();
        let deb = notify_debouncer_mini::new_debouncer(Duration::from_millis(250), move |res: notify_debouncer_mini::DebounceEventResult| {
            if let Ok(evs) = res {
                let mut dirs: Vec<String> = evs
                    .iter()
                    .filter_map(|e| e.path.parent().map(|p| p.display().to_string()))
                    .collect();
                dirs.sort();
                dirs.dedup();
                let _ = app2.emit("changed", dirs);
            }
        })
        .map_err(|e| e.to_string())?;
        *w = Some(deb);
    }
    let deb = w.as_mut().unwrap();
    let mut watched = st.watched.lock().unwrap();
    let want: Vec<PathBuf> = paths.iter().filter_map(|p| abs(p).ok()).collect();
    for old in watched.iter() {
        if !want.contains(old) {
            let _ = deb.watcher().unwatch(old);
        }
    }
    for p in &want {
        if !watched.contains(p) {
            let _ = deb.watcher().watch(p, RecursiveMode::NonRecursive);
        }
    }
    *watched = want;
    Ok(())
}

/// Turn argv items (paths, file:// URIs) into absolute paths.
fn parse_args(args: &[String], cwd: &Path) -> Vec<String> {
    let mut out = vec![];
    for a in args {
        if a.starts_with('-') {
            continue;
        }
        let s = if let Some(rest) = a.strip_prefix("file://") {
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            percent_encoding::percent_decode_str(rest).decode_utf8_lossy().into_owned()
        } else {
            a.clone()
        };
        let p = if Path::new(&s).is_absolute() { PathBuf::from(&s) } else { cwd.join(&s) };
        if p.exists() {
            out.push(p.display().to_string());
        }
    }
    out
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("Null File {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--set-default") {
        match sysint::make_default() {
            Ok(s) => println!("Null File is the default file manager: {}", s.is_default),
            Err(e) => eprintln!("{e}"),
        }
        return;
    }
    let activated = args.iter().any(|a| a == fm1::ACTIVATED_FLAG);
    let cwd = std::env::current_dir().unwrap_or_else(|_| fsops::home());
    let state = AppState::default();
    *state.pending.lock().unwrap() = parse_args(&args, &cwd);

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            let paths = parse_args(&argv.into_iter().skip(1).collect::<Vec<_>>(), Path::new(&cwd));
            let _ = app.emit("open-paths", paths);
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .manage(state)
        .setup(move |app| {
            // Claim org.freedesktop.FileManager1 when we're the default file manager,
            // or when D-Bus started us to answer it.
            let h = app.handle().clone();
            std::thread::spawn(move || {
                if activated || sysint::default_status().is_default {
                    start_fm1(h, true);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_dir,
            stat,
            places,
            dir_size,
            search,
            cancel_search,
            graph,
            read_text,
            mkdir,
            mkfile,
            rename,
            move_exact,
            trash,
            restore,
            restore_from_trash,
            trash_and_restore,
            empty_trash,
            transfer,
            cancel_job,
            open_path,
            open_terminal,
            open_url,
            default_status,
            make_default,
            copy_to_clipboard,
            get_settings,
            save_settings,
            take_pending,
            ui_ready,
            watch,
            vault_status,
            vault_setup,
            vault_unlock,
            vault_recover,
            vault_change_pin,
            vault_lock,
            vault_sync,
            update::update_check,
            update::update_install,
            update::relaunch,
        ])
        .register_uri_scheme_protocol("nfvault", |ctx, req| vault_protocol(ctx.app_handle(), &req))
        .build(tauri::generate_context!())
        .expect("error while starting Null File")
        .run(|app, ev| {
            if let tauri::RunEvent::Exit = ev {
                // Never leave the vault open or decrypted copies behind.
                if let Some(o) = app.state::<AppState>().vault.lock().unwrap().take() {
                    let _ = o.close();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_accept_paths_and_file_uris() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("My Docs")).unwrap();
        let uri = format!("file://{}/My%20Docs", t.path().display());
        let got = parse_args(&["--new-window".into(), uri, "My Docs".into(), "missing".into()], t.path());
        let want = t.path().join("My Docs").display().to_string();
        assert_eq!(got, vec![want.clone(), want]);
    }

    #[test]
    fn settings_tolerate_missing_fields() {
        let s: Settings = serde_json::from_str(r#"{"hidden":true}"#).unwrap();
        assert!(s.hidden);
        assert_eq!(s.view, "grid");
        assert!(s.restore_tabs);
    }
}
