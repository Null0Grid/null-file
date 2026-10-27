//! Recursive filename search and the folder-tree graph.

use crate::fsops::{entry_for, kind_of, Entry};
use serde::Serialize;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Folders that are huge and almost never what someone is looking for.
const SKIP: &[&str] = &["node_modules", ".git", "target", ".cache", "__pycache__", ".venv", "proc", "sys"];

#[derive(Serialize)]
pub struct SearchResult {
    pub hits: Vec<Entry>,
    pub scanned: u64,
    pub truncated: bool,
}

/// Breadth-first so shallow matches come first. `generation` lets a newer search cancel this one.
pub fn find(root: &Path, query: &str, hidden: bool, limit: usize, gen: &AtomicU64, my_gen: u64) -> SearchResult {
    let terms: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
    let mut hits = Vec::new();
    let mut scanned = 0u64;
    let mut q = VecDeque::from([root.to_path_buf()]);
    let mut truncated = false;
    'outer: while let Some(dir) = q.pop_front() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for de in rd.flatten() {
            if gen.load(Ordering::Relaxed) != my_gen {
                truncated = true;
                break 'outer;
            }
            scanned += 1;
            let name = de.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let lname = name.to_lowercase();
            let ft = de.file_type().ok();
            let is_dir = ft.map(|t| t.is_dir()).unwrap_or(false);
            if !terms.is_empty() && terms.iter().all(|t| lname.contains(t.as_str())) {
                if let Ok(e) = entry_for(&de.path()) {
                    hits.push(e);
                }
                if hits.len() >= limit {
                    truncated = true;
                    break 'outer;
                }
            }
            if is_dir && !SKIP.contains(&name.as_str()) {
                q.push_back(de.path());
            }
            if scanned > 2_000_000 {
                truncated = true;
                break 'outer;
            }
        }
    }
    SearchResult { hits, scanned, truncated }
}

#[derive(Serialize, Debug)]
pub struct GNode {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    pub dir: bool,
    pub size: u64,
    pub depth: u32,
    /// For folders: how many children exist in total (shown or not).
    pub children: u32,
    /// True when a folder has children that were cut off by depth or the node limit.
    pub more: bool,
}

#[derive(Serialize, Debug)]
pub struct GEdge {
    pub source: String,
    pub target: String,
}

#[derive(Serialize, Debug)]
pub struct Graph {
    pub root: String,
    pub nodes: Vec<GNode>,
    pub edges: Vec<GEdge>,
    pub truncated: bool,
}

/// The folder tree under `root` as nodes and parent→child edges, breadth-first,
/// down to `depth` levels and at most `max_nodes` nodes.
pub fn tree(root: &Path, depth: u32, hidden: bool, max_nodes: usize) -> Result<Graph, String> {
    let md = fs::metadata(root).map_err(|e| format!("Can't open {}: {e}", root.display()))?;
    if !md.is_dir() {
        return Err(format!("{} isn't a folder.", root.display()));
    }
    let rid = root.display().to_string();
    let rname = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
    let mut nodes = vec![GNode { id: rid.clone(), name: rname, kind: "dir", dir: true, size: 0, depth: 0, children: 0, more: false }];
    let mut edges = Vec::new();
    let mut truncated = false;
    let mut q: VecDeque<(PathBuf, usize, u32)> = VecDeque::from([(root.to_path_buf(), 0usize, 0u32)]);
    while let Some((dir, idx, d)) = q.pop_front() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        let mut kids: Vec<_> = rd
            .flatten()
            .filter(|de| hidden || !de.file_name().to_string_lossy().starts_with('.'))
            .collect();
        nodes[idx].children = kids.len() as u32;
        if d >= depth {
            nodes[idx].more = !kids.is_empty();
            continue;
        }
        kids.sort_by_key(|de| de.file_name());
        let pid = nodes[idx].id.clone();
        for de in kids {
            if nodes.len() >= max_nodes {
                nodes[idx].more = true;
                truncated = true;
                break;
            }
            let p = de.path();
            let name = de.file_name().to_string_lossy().into_owned();
            // Don't follow symlinked folders: they can loop.
            let ft = de.file_type().ok();
            let link = ft.map(|t| t.is_symlink()).unwrap_or(false);
            let lmd = fs::metadata(&p).ok();
            let is_dir = lmd.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = lmd.as_ref().filter(|m| !m.is_dir()).map(|m| m.len()).unwrap_or(0);
            let id = p.display().to_string();
            edges.push(GEdge { source: pid.clone(), target: id.clone() });
            nodes.push(GNode { id, name: name.clone(), kind: kind_of(&name, is_dir), dir: is_dir, size, depth: d + 1, children: 0, more: false });
            if is_dir && !link {
                q.push_back((p, nodes.len() - 1, d + 1));
            } else if is_dir {
                let n = nodes.len() - 1;
                nodes[n].more = true;
            }
        }
    }
    Ok(Graph { root: rid, nodes, edges, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("Projects/null-file/src")).unwrap();
        fs::create_dir_all(r.join("Photos")).unwrap();
        fs::create_dir_all(r.join("node_modules/pkg")).unwrap();
        fs::write(r.join("Projects/null-file/src/main.rs"), "fn main(){}").unwrap();
        fs::write(r.join("Projects/notes.md"), "# hi").unwrap();
        fs::write(r.join("Photos/rack.png"), [0u8; 10]).unwrap();
        fs::write(r.join("node_modules/pkg/main.rs"), "").unwrap();
        fs::write(r.join(".hidden-main"), "").unwrap();
        t
    }

    #[test]
    fn find_matches_all_terms_and_skips_noise() {
        let t = fixture();
        let g = AtomicU64::new(1);
        let r = find(t.path(), "main", false, 100, &g, 1);
        let names: Vec<_> = r.hits.iter().map(|h| h.path.replace(&*t.path().display().to_string(), "")).collect();
        assert_eq!(names, ["/Projects/null-file/src/main.rs"]);
        let r = find(t.path(), "main", true, 100, &g, 1);
        assert_eq!(r.hits.len(), 2);
        let r = find(t.path(), "null src", false, 100, &g, 1);
        assert_eq!(r.hits.len(), 0); // both terms must be in one name
        let r = find(t.path(), "NULL-F", false, 100, &g, 1);
        assert_eq!(r.hits.len(), 1);
    }

    #[test]
    fn find_stops_when_superseded() {
        let t = fixture();
        let g = AtomicU64::new(2);
        let r = find(t.path(), "main", false, 100, &g, 1);
        assert!(r.truncated);
        assert!(r.hits.is_empty());
    }

    #[test]
    fn tree_respects_depth_and_limits() {
        let t = fixture();
        let g = tree(t.path(), 1, false, 500).unwrap();
        let names: Vec<_> = g.nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(&names[1..], ["Photos", "Projects", "node_modules"]);
        assert_eq!(g.edges.len(), 3);
        assert!(g.nodes.iter().find(|n| n.name == "Projects").unwrap().more);
        let g = tree(t.path(), 9, false, 500).unwrap();
        assert!(g.nodes.iter().any(|n| n.name == "main.rs" && n.depth == 4 && n.kind == "code"));
        assert_eq!(g.nodes.iter().find(|n| n.name == "rack.png").unwrap().size, 10);
        assert_eq!(g.nodes.len(), g.edges.len() + 1);
        let g = tree(t.path(), 9, false, 4).unwrap();
        assert_eq!(g.nodes.len(), 4);
        assert!(g.truncated);
    }

    #[test]
    fn tree_does_not_follow_symlink_loops() {
        let t = fixture();
        std::os::unix::fs::symlink(t.path(), t.path().join("Projects/loop")).unwrap();
        let g = tree(t.path(), 20, false, 10_000).unwrap();
        assert!(g.nodes.len() < 50);
    }
}
