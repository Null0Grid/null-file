//! Null File backend. Command and argument names match src/backend.js.

mod fsops;
mod search;
mod sysint;

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
struct AppState {
    jobs: Mutex<HashMap<u64, Job>>,
    next_job: AtomicU64,
    search_gen: Arc<AtomicU64>,
    /// Paths given on the command line (or by a second launch) waiting for the UI.
    pending: Mutex<Vec<String>>,
    watcher: Mutex<Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>,
    watched: Mutex<Vec<PathBuf>>,
    settings_lock: Mutex<()>,
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
fn list_dir(path: String, hidden: bool) -> R<Listing> {
    fsops::list_dir(&path, hidden)
}

#[tauri::command]
fn stat(path: String) -> R<Entry> {
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
fn dir_size(path: String) -> R<(u64, u64)> {
    let p = abs(&path)?;
    Ok(fsops::measure(&p))
}

#[tauri::command]
async fn search(st: State<'_, AppState>, root: String, query: String, hidden: bool) -> R<search::SearchResult> {
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
async fn graph(root: String, depth: u32, hidden: bool, max_nodes: usize) -> R<search::Graph> {
    let p = abs(&root)?;
    tauri::async_runtime::spawn_blocking(move || search::tree(&p, depth.min(12), hidden, max_nodes.clamp(10, 5000)))
        .await
        .map_err(|e| e.to_string())?
}

/// First chunk of a text file for the preview pane. Binary files return None.
#[tauri::command]
fn read_text(path: String, max: usize) -> R<Option<(String, bool)>> {
    use std::io::Read;
    let p = abs(&path)?;
    let mut f = std::fs::File::open(&p).map_err(|e| fsops::err(e, "Can't open", &p))?;
    let max = max.min(512 * 1024);
    let mut buf = vec![0u8; max + 1];
    let mut n = 0;
    while n < buf.len() {
        let k = f.read(&mut buf[n..]).map_err(|e| fsops::err(e, "Can't read", &p))?;
        if k == 0 {
            break;
        }
        n += k;
    }
    let more = n > max;
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
fn mkdir(parent: String, name: String) -> R<String> {
    fsops::mkdir(&parent, &name)
}

#[tauri::command]
fn mkfile(parent: String, name: String) -> R<String> {
    fsops::mkfile(&parent, &name)
}

#[tauri::command]
fn rename(path: String, name: String) -> R<String> {
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
fn trash(paths: Vec<String>) -> R<Vec<String>> {
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

#[tauri::command]
fn cancel_job(st: State<'_, AppState>, id: u64) {
    if let Some(j) = st.jobs.lock().unwrap().get(&id) {
        j.cancel.store(true, Ordering::Relaxed);
    }
}

/* ---------------- commands: system ---------------- */

#[tauri::command]
fn open_path(path: String) -> R<()> {
    sysint::open_default(&abs(&path)?)
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
fn make_default() -> R<sysint::DefaultStatus> {
    sysint::make_default()
}

#[tauri::command]
fn copy_to_clipboard(text: String) -> R<()> {
    sysint::copy_text(&text)
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
            default_status,
            make_default,
            copy_to_clipboard,
            get_settings,
            save_settings,
            take_pending,
            watch,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Null File");
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
