// Null File frontend.
import { api, onEvent, fileUrl, onFileDrop, setTitle, native } from "./backend.js";
import { icon, iconFor } from "./icons.js";
import { createGraph, KCOL, KNAME } from "./graph.js";

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const baseName = (p) => (p === "/" ? "/" : p.replace(/\/+$/, "").split("/").pop());
const tabName = (p) => (p === P.home ? "Home" : p === P.trash ? "Trash" : baseName(p) || "/");
const parentOf = (p) => { if (p === "/") return null; const i = p.replace(/\/+$/, "").lastIndexOf("/"); return i <= 0 ? "/" : p.slice(0, i); };
const join = (dir, name) => (dir === "/" ? "/" + name : dir + "/" + name);
const errText = (e) => (typeof e === "string" ? e : e && e.message ? e.message : String(e));

const KIND_LABEL = { dir: "Folder", img: "Image", video: "Video", audio: "Audio", text: "Text", code: "Code", doc: "Document", archive: "Archive", exec: "Program", file: "File" };

function fmtSize(b) {
  if (b == null) return "";
  if (b < 1024) return b + " B";
  const u = ["KB", "MB", "GB", "TB"];
  let i = -1;
  do { b /= 1024; i++; } while (b >= 1024 && i < u.length - 1);
  return (b >= 100 ? b.toFixed(0) : b >= 10 ? b.toFixed(1) : b.toFixed(2)) + " " + u[i];
}
function fmtDate(s) {
  if (!s) return "";
  const d = new Date(s * 1000), now = new Date();
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === now.toDateString()) return "Today " + time;
  const y = new Date(now); y.setDate(now.getDate() - 1);
  if (d.toDateString() === y.toDateString()) return "Yesterday " + time;
  return d.toLocaleDateString([], { year: "numeric", month: "short", day: "numeric" }) + " " + time;
}
function fmtMode(m, dir) {
  let s = dir ? "d" : "-";
  for (let i = 2; i >= 0; i--) { const b = (m >> (i * 3)) & 7; s += (b & 4 ? "r" : "-") + (b & 2 ? "w" : "-") + (b & 1 ? "x" : "-"); }
  return s;
}
const homeShort = (p) => (P.home && (p === P.home || p.startsWith(P.home + "/")) ? "~" + p.slice(P.home.length) : p);

/* ---------------- state ---------------- */

let S = { pinned: [], recent: [], hidden: false, view: "grid", sort: "name", desc: false, preview: true, sidebar_w: 230, graph: null, last_tabs: [], restore_tabs: true, confirm_trash: false };
let P = { home: "/", places: [], drives: [], trash: "" };
const tabs = [];
let cur = 0;
let tabSeq = 0;
let visible = []; // entries shown in the current tab, in order
let clip = null; // { op: "copy" | "cut", paths: [] }
const undoStack = [];
const jobs = new Map();
const tab = () => tabs[cur];
const GDEF = { depth: 2, files: true, labels: true, glow: true };
const gopt = () => ({ ...GDEF, ...(S.graph || {}) });

function newTab(path) {
  return { id: ++tabSeq, path, hist: [], fwd: [], listing: null, error: null, sel: new Set(), anchor: null, filter: "", search: null, scroll: 0, focus: null, graphSel: null };
}

let saveTimer;
function saveSettings() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    S.last_tabs = tabs.map((t) => t.path);
    api("save_settings", { settings: S }).catch(() => {});
  }, 300);
}

/* ---------------- toast ---------------- */

let toastTimer;
function toast(msg, opts = {}) {
  const t = $("toast");
  t.className = "toast" + (opts.warn ? " warn" : "");
  t.innerHTML = `<span>${esc(msg)}</span>` + (opts.action ? `<button>${esc(opts.action)}</button>` : "");
  t.hidden = false;
  if (opts.action) t.querySelector("button").onclick = () => { t.hidden = true; opts.run(); };
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), opts.ms || (opts.action ? 6000 : 3200));
}
const fail = (e) => toast(errText(e), { warn: true, ms: 6000 });

/* ---------------- navigation ---------------- */

async function load(t, keepSel = false) {
  try {
    const l = await api("list_dir", { path: t.path, hidden: S.hidden });
    t.listing = l;
    t.error = null;
    if (keepSel) {
      const have = new Set(l.entries.map((e) => e.path));
      t.sel = new Set([...t.sel].filter((p) => have.has(p)));
    }
  } catch (e) {
    t.listing = null;
    t.error = errText(e);
  }
}

async function go(path, opts = {}) {
  const t = tab();
  if (!path) return;
  if (path !== t.path && opts.push !== false) { t.hist.push(t.path); t.fwd = []; }
  const from = t.path;
  t.path = path;
  t.filter = ""; t.search = null; $("q").value = "";
  t.sel = new Set();
  t.anchor = null;
  await load(t);
  if (t.error && opts.push !== false && from !== path && t.hist[t.hist.length - 1] === from) {
    // Couldn't open it: stay where we were.
    t.hist.pop();
    t.path = from;
    fail(t.error);
    await load(t);
  }
  if (opts.select) { t.sel = new Set([opts.select]); t.anchor = opts.select; t.focus = opts.select; }
  if (!t.error) {
    S.recent = [path, ...S.recent.filter((p) => p !== path)].slice(0, 20);
    saveSettings();
  }
  render({ scrollTop: true });
  watchTabs();
  if (S.view === "graph") loadGraph();
}

function back() { const t = tab(); if (!t.hist.length) return; t.fwd.push(t.path); go(t.hist.pop(), { push: false }); }
function forward() { const t = tab(); if (!t.fwd.length) return; t.hist.push(t.path); go(t.fwd.pop(), { push: false }); }
function up() { const t = tab(); const p = parentOf(t.path); if (p) go(p, { select: t.path }); }

async function refresh(t = tab(), quiet = true) {
  const scroll = t === tab() ? $("files").scrollTop : t.scroll;
  await load(t, true);
  if (t.search) await runSearch(t, t.search.query, true);
  if (t === tab()) { render(); $("files").scrollTop = scroll; if (S.view === "graph" && !quiet) loadGraph(); }
  else renderTabs();
}

function watchTabs() {
  api("watch", { paths: [...new Set(tabs.map((t) => t.path))] }).catch(() => {});
}

/* ---------------- tabs ---------------- */

function addTab(path, activate = true, select) {
  const t = newTab(path);
  tabs.splice(cur + 1, 0, t);
  if (activate) {
    stashScroll();
    cur = cur + 1;
    load(t).then(() => {
      if (select) { t.sel = new Set([select]); t.anchor = select; t.focus = select; }
      render({ scrollTop: true }); watchTabs(); if (S.view === "graph") loadGraph();
    });
  } else { load(t).then(() => { renderTabs(); watchTabs(); }); }
  renderTabs();
  saveSettings();
}
function closeTab(i = cur) {
  if (tabs.length === 1) { go(P.home); return; }
  tabs.splice(i, 1);
  if (cur >= tabs.length) cur = tabs.length - 1;
  else if (i < cur) cur--;
  restoreTab();
  saveSettings();
  watchTabs();
}
function stashScroll() { if (tab()) tab().scroll = $("files").scrollTop; }
function switchTab(i) {
  if (i === cur || i < 0 || i >= tabs.length) return;
  stashScroll();
  cur = i;
  restoreTab();
}
function restoreTab() {
  const t = tab();
  $("q").value = t.search ? t.search.query : t.filter;
  render();
  $("files").scrollTop = t.scroll;
  if (S.view === "graph") loadGraph();
}

function renderTabs() {
  const el = $("tabs");
  el.innerHTML = tabs.map((t, i) =>
    `<button class="tab${i === cur ? " on" : ""}" data-tab="${i}" data-drop="${esc(t.path)}" title="${esc(t.path)}"><span class="tn">${esc(tabName(t.path))}</span><span class="tx" data-close="${i}" role="button" aria-label="Close tab">×</span></button>`
  ).join("") + `<button class="tabnew" id="tabNew" title="New tab (Ctrl+T)" aria-label="New tab">+</button>`;
}
$("tabs").addEventListener("click", (e) => {
  const x = e.target.closest("[data-close]");
  if (x) { e.stopPropagation(); closeTab(+x.dataset.close); return; }
  if (e.target.id === "tabNew") return addTab(tab().path);
  const b = e.target.closest("[data-tab]");
  if (b) switchTab(+b.dataset.tab);
});
$("tabs").addEventListener("auxclick", (e) => { const b = e.target.closest("[data-tab]"); if (b && e.button === 1) closeTab(+b.dataset.tab); });
$("tabs").addEventListener("dblclick", (e) => { if (e.target === $("tabs")) addTab(tab().path); });

/* ---------------- breadcrumbs ---------------- */

function renderCrumbs() {
  const t = tab();
  const p = t.path;
  const parts = [];
  if (P.home && (p === P.home || p.startsWith(P.home + "/"))) {
    parts.push({ label: "Home", path: P.home });
    let acc = P.home;
    for (const seg of p.slice(P.home.length).split("/").filter(Boolean)) { acc = join(acc, seg); parts.push({ label: seg, path: acc }); }
  } else {
    parts.push({ label: "/", path: "/" });
    let acc = "/";
    for (const seg of p.split("/").filter(Boolean)) { acc = join(acc, seg); parts.push({ label: seg, path: acc }); }
  }
  // direction:rtl on the trail keeps the end visible when it overflows, so the
  // tokens go in reverse order (rtl flex puts the first child on the right).
  const tokens = [];
  parts.forEach((c, i) => {
    if (i) tokens.push('<span class="csep">/</span>');
    tokens.push(`<button class="crumb${i === parts.length - 1 ? " cur" : ""}" data-path="${esc(c.path)}" data-drop="${esc(c.path)}">${esc(c.label)}</button>`);
  });
  $("crumbtrail").innerHTML = tokens.reverse().join("");
}
$("crumbtrail").addEventListener("click", (e) => {
  const b = e.target.closest(".crumb");
  if (b) { e.stopPropagation(); if (b.dataset.path !== tab().path) go(b.dataset.path, { select: childOnPath(b.dataset.path) }); return; }
  editPath();
});
function childOnPath(ancestor) {
  const p = tab().path;
  if (!p.startsWith(ancestor) || p === ancestor) return undefined;
  const rest = p.slice(ancestor === "/" ? 1 : ancestor.length + 1).split("/")[0];
  return join(ancestor, rest);
}
$("crumbs").addEventListener("click", (e) => { if (e.target === $("crumbs")) editPath(); });
function editPath() {
  const inp = $("pathInput");
  $("crumbtrail").hidden = true;
  inp.hidden = false;
  inp.value = tab().path;
  inp.focus();
  inp.select();
}
function endPathEdit() { $("pathInput").hidden = true; $("crumbtrail").hidden = false; }
$("pathInput").addEventListener("keydown", async (e) => {
  if (e.key === "Escape") { endPathEdit(); $("files").focus(); }
  if (e.key === "Enter") {
    let v = $("pathInput").value.trim();
    if (v.startsWith("~")) v = P.home + v.slice(1);
    if (v.startsWith("file://")) v = decodeURIComponent(v.slice(7));
    if (!v.startsWith("/")) v = join(tab().path, v);
    v = v.replace(/\/+$/, "") || "/";
    endPathEdit();
    try {
      const st = await api("stat", { path: v });
      if (st.dir) go(v); else go(parentOf(v), { select: v });
    } catch (err) { fail(err); }
    $("files").focus();
  }
});
$("pathInput").addEventListener("blur", endPathEdit);

/* ---------------- sidebar ---------------- */

function placeRow(p, extra = "") {
  const on = tab() && tab().path === p.path ? " on" : "";
  return `<li><button class="place${on}" data-path="${esc(p.path)}" data-drop="${esc(p.path)}" ${extra} title="${esc(p.path)}">${icon(p.icon, "ic")}<span class="pn">${esc(p.name)}</span></button></li>`;
}
function renderSide() {
  $("places").innerHTML = P.places.map((p) => placeRow(p)).join("");
  $("pinned").innerHTML = S.pinned.length
    ? S.pinned.map((p) => placeRow({ path: p, name: baseName(p) || "/", icon: "pin" }, "data-pin")).join("")
    : '<li class="pinempty">Right-click a folder → Pin, or drag one here.</li>';
  $("drives").innerHTML = P.drives.map((p) => placeRow(p)).join("");
  $("drivesLabel").hidden = !P.drives.length;
  $("trashPlace").innerHTML = P.trash ? placeRow({ path: P.trash, name: "Trash", icon: "trash" }, "data-trash") : "";
}
$("side").addEventListener("click", (e) => {
  const b = e.target.closest(".place");
  if (b && b.dataset.path) go(b.dataset.path);
});
$("side").addEventListener("auxclick", (e) => { const b = e.target.closest(".place"); if (b && e.button === 1) addTab(b.dataset.path, false); });
$("side").addEventListener("contextmenu", (e) => {
  const b = e.target.closest(".place");
  if (!b) return;
  e.preventDefault();
  const p = b.dataset.path;
  const items = [
    { label: "Open", run: () => go(p) },
    { label: "Open in new tab", key: "Middle-click", run: () => addTab(p) },
    { label: "Open terminal here", run: () => terminalAt(p) },
  ];
  if (b.hasAttribute("data-pin")) items.push("-", { label: "Unpin", run: () => unpin(p) }, { label: "Move up", run: () => movePin(p, -1) }, { label: "Move down", run: () => movePin(p, 1) });
  else if (!b.hasAttribute("data-trash")) items.push("-", { label: "Pin to sidebar", run: () => pin(p) });
  if (b.hasAttribute("data-trash")) items.push("-", { label: "Empty Trash…", danger: true, run: emptyTrash });
  menu(e.clientX, e.clientY, items, homeShort(p));
});
function pin(p) { if (!S.pinned.includes(p)) { S.pinned.push(p); saveSettings(); renderSide(); toast(`Pinned ${baseName(p)}`); } }
function unpin(p) { S.pinned = S.pinned.filter((x) => x !== p); saveSettings(); renderSide(); }
function movePin(p, d) {
  const i = S.pinned.indexOf(p), j = i + d;
  if (j < 0 || j >= S.pinned.length) return;
  [S.pinned[i], S.pinned[j]] = [S.pinned[j], S.pinned[i]];
  saveSettings(); renderSide();
}

/* sidebar resize */
{
  const r = $("resizer");
  let start = null;
  r.addEventListener("pointerdown", (e) => { start = { x: e.clientX, w: S.sidebar_w }; r.setPointerCapture(e.pointerId); r.classList.add("on"); });
  r.addEventListener("pointermove", (e) => {
    if (!start) return;
    S.sidebar_w = Math.max(160, Math.min(420, start.w + e.clientX - start.x));
    document.documentElement.style.setProperty("--side", S.sidebar_w + "px");
  });
  r.addEventListener("pointerup", () => { if (start) { start = null; r.classList.remove("on"); saveSettings(); if (S.view === "graph") graph.redraw(); } });
}

/* ---------------- file list ---------------- */

function sortEntries(list) {
  const dir = S.desc ? -1 : 1;
  const nameCmp = (a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
  const by = {
    name: nameCmp,
    size: (a, b) => (a.dir && b.dir ? (a.items || 0) - (b.items || 0) : a.size - b.size) || nameCmp(a, b),
    modified: (a, b) => a.modified - b.modified || nameCmp(a, b),
    kind: (a, b) => (a.ext || a.kind).localeCompare(b.ext || b.kind) || nameCmp(a, b),
  }[S.sort] || nameCmp;
  return list.slice().sort((a, b) => (b.dir - a.dir) || by(a, b) * dir);
}

function computeVisible(t) {
  if (t.search) return t.search.hits;
  if (!t.listing) return [];
  let list = t.listing.entries;
  const f = t.filter.trim().toLowerCase();
  if (f) list = list.filter((e) => e.name.toLowerCase().includes(f));
  return sortEntries(list);
}

function thumbOf(e) {
  const pic = (e.kind === "img" && e.size < 25 * 1024 * 1024 && !/^(heic|tif|tiff|ico)$/.test(e.ext)) ;
  if (pic) return `<img class="pic" loading="lazy" decoding="async" src="${esc(fileUrl(e.path))}" alt="" draggable="false" onerror="this.replaceWith(document.createRange().createContextualFragment(this.dataset.fb))" data-fb="${esc(iconFor(e))}">`;
  return iconFor(e);
}

function itemHtml(e, i, t) {
  const cls = ["it", "k-" + e.kind];
  if (t.sel.has(e.path)) cls.push("sel");
  if (e.hidden) cls.push("hid");
  if (clip && clip.op === "cut" && clip.paths.includes(e.path)) cls.push("cut");
  const ext = e.dir ? "" : e.ext && e.kind !== "img" ? `<span class="ext k-${e.kind}">${esc(e.ext)}</span>` : "";
  const size = e.dir ? (e.items == null ? "" : `${e.items} item${e.items === 1 ? "" : "s"}`) : fmtSize(e.size);
  const where = t.search ? `<span class="where" title="${esc(parentOf(e.path))}">${esc(homeShort(parentOf(e.path)))}</span>` : "";
  return `<div class="${cls.join(" ")}" data-i="${i}" ${e.dir ? `data-drop="${esc(e.path)}"` : ""} title="${esc(e.name)}">`
    + `<span class="thumb">${thumbOf(e)}${ext}</span>${e.link ? '<span class="lnk" title="Symbolic link">↗</span>' : ""}`
    + `<span class="nm">${esc(e.name)}</span>${where}<span class="sz">${size}</span><span class="md">${fmtDate(e.modified)}</span><span class="ty">${e.dir ? "Folder" : esc(e.ext ? e.ext.toUpperCase() : KIND_LABEL[e.kind])}</span></div>`;
}

function renderFiles() {
  const t = tab();
  const files = $("files");
  visible = computeVisible(t);
  const center = $("center");
  center.className = "center " + (S.view === "list" ? "list" : "grid");
  files.classList.toggle("searching", !!t.search);
  $("sortbar").hidden = !!t.search;
  document.querySelectorAll("#sortbar button").forEach((b) => { b.classList.toggle("on", b.dataset.sort === S.sort); b.classList.toggle("desc", S.desc); });
  const head = t.search
    ? `<div class="searchhead"><span>${t.search.running ? "Searching…" : `${t.search.hits.length}${t.search.truncated ? "+" : ""} match${t.search.hits.length === 1 ? "" : "es"} for “${esc(t.search.query)}” in ${esc(homeShort(t.path))}`}</span><button class="btn sm" id="endSearch">Close search</button></div>`
    : "";
  files.innerHTML = head + visible.map((e, i) => itemHtml(e, i, t)).join("");
  const em = $("emptymsg");
  em.classList.toggle("err", !!t.error);
  if (t.error) { em.innerHTML = `<b>CAN'T OPEN</b><span>${esc(t.error)}</span>`; em.hidden = false; }
  else if (!visible.length && !(t.search && t.search.running)) {
    em.innerHTML = t.search ? `<b>NO MATCHES</b><span>Nothing named like “${esc(t.search.query)}” under this folder.</span>`
      : t.filter ? `<b>NO MATCHES</b><span>Nothing here matches “${esc(t.filter)}”. Press Enter to search subfolders too.</span>`
      : `<b>EMPTY FOLDER</b><span>Drop files here, or right-click for a new folder.</span>`;
    em.hidden = false;
  } else em.hidden = true;
  if (t.focus) {
    const i = visible.findIndex((e) => e.path === t.focus);
    if (i >= 0) requestAnimationFrame(() => itemEl(i)?.scrollIntoView({ block: "nearest" }));
  }
}

const itemEl = (i) => $("files").querySelector(`.it[data-i="${i}"]`);

function paintSelection() {
  const t = tab();
  $("files").querySelectorAll(".it").forEach((el) => el.classList.toggle("sel", t.sel.has(visible[+el.dataset.i]?.path)));
  renderInfo();
  renderStatus();
}

function render(o = {}) {
  const t = tab();
  renderTabs();
  renderCrumbs();
  renderSide();
  renderFiles();
  renderInfo();
  renderStatus();
  $("back").disabled = !t.hist.length;
  $("fwd").disabled = !t.fwd.length;
  $("up").disabled = !parentOf(t.path);
  setTitle(`${tabName(t.path)} — Null File`);
  if (o.scrollTop) $("files").scrollTop = 0;
}

function selectedEntries() {
  const t = tab();
  const map = new Map(visible.map((e) => [e.path, e]));
  if (S.view === "graph" && t.graphSel) return [t.graphSel];
  return [...t.sel].map((p) => map.get(p)).filter(Boolean);
}

/* selection */
function selectOnly(i) { const t = tab(); const e = visible[i]; if (!e) return; t.sel = new Set([e.path]); t.anchor = e.path; t.focus = e.path; }
function selectRange(i) {
  const t = tab();
  const a = visible.findIndex((e) => e.path === t.anchor);
  if (a < 0) return selectOnly(i);
  const [lo, hi] = a < i ? [a, i] : [i, a];
  t.sel = new Set(visible.slice(lo, hi + 1).map((e) => e.path));
  t.focus = visible[i].path;
}
function toggleSel(i) { const t = tab(); const p = visible[i].path; t.sel.has(p) ? t.sel.delete(p) : t.sel.add(p); t.anchor = p; t.focus = p; }

/* opening */
async function openEntry(e, newTabToo = false) {
  if (!e) return;
  if (e.dir) { if (newTabToo) addTab(e.path); else go(e.path); return; }
  if (tab().search) { /* fine: open the file wherever it is */ }
  try { await api("open_path", { path: e.path }); toast(`Opening ${e.name}…`, { ms: 1500 }); } catch (err) { fail(err); }
}
function openSelection() {
  const sel = selectedEntries();
  if (sel.length === 1) return openEntry(sel[0]);
  const dirs = sel.filter((e) => e.dir), files = sel.filter((e) => !e.dir);
  files.forEach((e) => openEntry(e));
  if (dirs.length === 1 && !files.length) go(dirs[0].path);
  else dirs.forEach((d) => addTab(d.path, false));
}
async function terminalAt(p) {
  try { const t = await api("open_terminal", { path: p }); toast(`Opened ${t} in ${baseName(p)}`, { ms: 1800 }); } catch (e) { fail(e); }
}

/* clicks, double clicks, marquee, dragging */
const files = $("files");
let press = null;
files.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  if (e.target.closest(".ren")) return;
  if (e.target.id === "endSearch") return;
  files.focus({ preventScroll: true });
  const el = e.target.closest(".it");
  if (!el) {
    // marquee on empty space
    const r = files.getBoundingClientRect();
    press = { type: "marquee", x: e.clientX - r.left + files.scrollLeft, y: e.clientY - r.top + files.scrollTop, add: e.ctrlKey || e.shiftKey, base: new Set(e.ctrlKey || e.shiftKey ? tab().sel : []) };
    if (!press.add) { tab().sel = new Set(); paintSelection(); }
    files.setPointerCapture(e.pointerId);
    return;
  }
  const i = +el.dataset.i;
  const t = tab();
  const wasSel = t.sel.has(visible[i].path);
  if (e.shiftKey) selectRange(i);
  else if (e.ctrlKey) toggleSel(i);
  else if (!wasSel) selectOnly(i);
  paintSelection();
  press = { type: "item", i, sx: e.clientX, sy: e.clientY, wasSel, plain: !e.shiftKey && !e.ctrlKey, id: e.pointerId };
});
files.addEventListener("pointermove", (e) => {
  if (!press) return;
  if (press.type === "marquee") return updateMarquee(e);
  if (press.type === "item" && Math.abs(e.clientX - press.sx) + Math.abs(e.clientY - press.sy) > 6) {
    const sel = selectedEntries();
    if (sel.length) startDrag(sel, e);
    press = null;
  }
});
files.addEventListener("pointerup", (e) => {
  if (!press) return;
  if (press.type === "item" && press.plain && press.wasSel && tab().sel.size > 1) { selectOnly(press.i); paintSelection(); }
  if (press.type === "marquee") endMarquee();
  press = null;
});
files.addEventListener("dblclick", (e) => {
  const el = e.target.closest(".it");
  if (el && !e.target.closest(".ren")) openEntry(visible[+el.dataset.i], e.ctrlKey);
});
files.addEventListener("auxclick", (e) => {
  const el = e.target.closest(".it");
  if (el && e.button === 1) { const it = visible[+el.dataset.i]; if (it.dir) addTab(it.path, false); }
});
files.addEventListener("click", (e) => { if (e.target.id === "endSearch") endSearch(); });

let mq = null;
function updateMarquee(e) {
  const r = files.getBoundingClientRect();
  const x = e.clientX - r.left + files.scrollLeft, y = e.clientY - r.top + files.scrollTop;
  if (!mq) { mq = document.createElement("div"); mq.className = "marquee"; files.appendChild(mq); }
  const L = Math.min(x, press.x), T = Math.min(y, press.y), W = Math.abs(x - press.x), H = Math.abs(y - press.y);
  Object.assign(mq.style, { left: L + "px", top: T + "px", width: W + "px", height: H + "px" });
  const t = tab();
  const next = new Set(press.base);
  files.querySelectorAll(".it").forEach((el) => {
    const ox = el.offsetLeft, oy = el.offsetTop;
    if (ox < L + W && ox + el.offsetWidth > L && oy < T + H && oy + el.offsetHeight > T) next.add(visible[+el.dataset.i].path);
  });
  t.sel = next;
  files.querySelectorAll(".it").forEach((el) => el.classList.toggle("sel", next.has(visible[+el.dataset.i].path)));
  // scroll when dragging near the edges
  if (e.clientY > r.bottom - 20) files.scrollTop += 14; else if (e.clientY < r.top + 20) files.scrollTop -= 14;
}
function endMarquee() { if (mq) { mq.remove(); mq = null; } const t = tab(); const last = [...t.sel].pop(); if (last) { t.anchor = last; t.focus = last; } paintSelection(); }

/* internal drag and drop: move by default, Ctrl to copy */
let dragging = null;
function startDrag(items, e) {
  const ghost = document.createElement("div");
  ghost.className = "ghost";
  document.body.appendChild(ghost);
  dragging = { items, ghost, target: null, copy: false };
  const move = (ev) => {
    dragging.copy = ev.ctrlKey;
    ghost.style.left = ev.clientX + "px"; ghost.style.top = ev.clientY + "px";
    const el = document.elementFromPoint(ev.clientX, ev.clientY);
    const tgt = el && el.closest("[data-drop]");
    let dest = tgt ? tgt.dataset.drop : null;
    if (!dest && el && el.closest("#files")) dest = tab().search ? null : tab().path;
    if (dest && items.some((it) => dest === it.path || dest.startsWith(it.path + "/"))) dest = null;
    document.querySelectorAll(".drop-on").forEach((x) => x.classList.remove("drop-on"));
    if (tgt && dest) tgt.classList.add("drop-on");
    else if (dest) files.classList.add("drop-on");
    if (el && el.closest("#pinned") && items.length === 1 && items[0].dir && !tgt) { dest = "@pin"; $("pinned").classList.add("drop-on"); }
    dragging.target = dest;
    const what = items.length === 1 ? items[0].name : `${items.length} items`;
    ghost.innerHTML = esc(what) + (dest === "@pin" ? "<small>pin</small>" : dest ? `<small>${dragging.copy ? "copy" : "move"} to ${esc(baseName(dest) || "/")}</small>` : "");
  };
  const up = () => {
    removeEventListener("pointermove", move, true);
    removeEventListener("pointerup", up, true);
    removeEventListener("keydown", key, true);
    document.querySelectorAll(".drop-on").forEach((x) => x.classList.remove("drop-on"));
    ghost.remove();
    const d = dragging; dragging = null;
    if (!d || !d.target) return;
    if (d.target === "@pin") return pin(d.items[0].path);
    const paths = d.items.map((i) => i.path);
    if (!d.copy && paths.every((p) => parentOf(p) === d.target)) return; // dropped where it already is
    paste({ op: d.copy ? "copy" : "move", paths }, d.target);
  };
  const key = (ev) => { if (ev.key === "Escape") { dragging.target = null; up(); } };
  addEventListener("pointermove", move, true);
  addEventListener("pointerup", up, true);
  addEventListener("keydown", key, true);
  move(e);
}

/* sorting */
$("sortbar").addEventListener("click", (e) => {
  const b = e.target.closest("[data-sort]");
  if (!b) return;
  if (S.sort === b.dataset.sort) S.desc = !S.desc; else { S.sort = b.dataset.sort; S.desc = b.dataset.sort === "modified" || b.dataset.sort === "size"; }
  saveSettings(); renderFiles();
});

/* ---------------- details panel ---------------- */

let infoSeq = 0;
async function renderInfo() {
  const box = $("info");
  $("main").classList.toggle("noinfo", !S.preview);
  if (!S.preview) return;
  const my = ++infoSeq;
  const t = tab();
  const sel = selectedEntries();
  if (sel.length > 1) {
    const total = sel.reduce((a, e) => a + (e.dir ? 0 : e.size), 0), nd = sel.filter((e) => e.dir).length;
    box.innerHTML = `<div class="big">${icon("file")}</div><h2>${sel.length} items selected</h2><div class="sub">${nd ? `${nd} folder${nd === 1 ? "" : "s"} · ` : ""}${sel.length - nd} file${sel.length - nd === 1 ? "" : "s"}</div>
      <dl><dt>Files</dt><dd>${fmtSize(total)}</dd></dl>
      <div class="acts"><button class="btn sm" data-act="copy">Copy</button><button class="btn sm" data-act="cut">Cut</button><button class="btn sm danger" data-act="trash">Trash</button></div>`;
    return;
  }
  let e = sel[0];
  if (!e) {
    if (!t.listing) { box.innerHTML = `<div class="none">${t.error ? esc(t.error) : ""}</div>`; return; }
    const n = t.listing.entries.length;
    e = { name: baseName(t.path) || "/", path: t.path, dir: true, kind: "dir", items: n, modified: 0, mode: 0, folderSelf: true };
  }
  const isTrash = P.trash && (e.path === P.trash || parentOf(e.path) === P.trash);
  const dl = [
    ["Type", e.dir ? "Folder" : `${KIND_LABEL[e.kind] || "File"}${e.ext ? " · ." + esc(e.ext) : ""}`],
    e.dir ? ["Contains", e.items == null ? "—" : `${e.items} item${e.items === 1 ? "" : "s"} <button class="calc" data-act="size">calc size</button><span id="dsize"></span>`] : ["Size", `${fmtSize(e.size)} <span style="color:var(--faint)">(${e.size.toLocaleString()} bytes)</span>`],
    e.modified ? ["Modified", fmtDate(e.modified)] : null,
    e.mode ? ["Access", `<span class="mono">${fmtMode(e.mode, e.dir)}</span>`] : null,
    e.link ? ["Link", "Symbolic link"] : null,
    ["Where", `<span class="mono">${esc(homeShort(e.folderSelf ? e.path : parentOf(e.path)))}</span>`],
  ].filter(Boolean);
  const acts = e.folderSelf
    ? `<button class="btn sm" data-act="term">Terminal</button><button class="btn sm" data-act="graph">Graph</button><button class="btn sm" data-act="path">Copy path</button>`
    : `<button class="btn sm primary" data-act="open">Open</button>${e.dir ? '<button class="btn sm" data-act="tab">New tab</button>' : ""}<button class="btn sm" data-act="path">Copy path</button><button class="btn sm" data-act="rename">Rename</button>${isTrash ? '<button class="btn sm" data-act="restore">Restore</button>' : '<button class="btn sm danger" data-act="trash">Trash</button>'}`;
  box.innerHTML = `<div class="big" id="bigprev">${e.dir ? icon(e.folderSelf ? "dirOpen" : "dir") : iconFor(e)}</div>
    <h2>${esc(e.name)}</h2><div class="sub">${e.folderSelf ? "THIS FOLDER" : esc(homeShort(e.path))}</div>
    <dl>${dl.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join("")}</dl><div class="acts">${acts}</div>`;
  box.dataset.path = e.path;
  // previews
  const big = $("bigprev");
  if (!e.dir && e.kind === "img" && e.size < 60 * 1024 * 1024) big.innerHTML = `<img src="${esc(fileUrl(e.path))}" alt="" draggable="false">`;
  else if (!e.dir && e.kind === "video") big.innerHTML = `<video src="${esc(fileUrl(e.path))}" controls preload="metadata"></video>`;
  else if (!e.dir && e.kind === "audio") big.innerHTML = `${icon("audio")}<audio src="${esc(fileUrl(e.path))}" controls preload="metadata"></audio>`;
  else if (!e.dir && (e.kind === "text" || e.kind === "code" || (e.kind === "file" && e.size < 512 * 1024) || e.kind === "exec") && e.size > 0) {
    try {
      const r = await api("read_text", { path: e.path, max: 24 * 1024 });
      if (my !== infoSeq || !r) return;
      big.innerHTML = `<pre>${esc(r[0])}${r[1] ? "\n…" : ""}</pre>`;
    } catch { /* keep the icon */ }
  }
}
$("info").addEventListener("click", async (ev) => {
  const a = ev.target.closest("[data-act]");
  if (!a) return;
  const path = $("info").dataset.path;
  const sel = selectedEntries();
  switch (a.dataset.act) {
    case "open": openSelection(); break;
    case "tab": addTab(path); break;
    case "path": copyPaths(sel.length && !sel[0].folderSelf ? sel.map((e) => e.path) : [path]); break;
    case "rename": startRename(); break;
    case "trash": trashSelection(); break;
    case "restore": restoreSelection(); break;
    case "copy": setClip("copy"); break;
    case "cut": setClip("cut"); break;
    case "term": terminalAt(path); break;
    case "graph": setView("graph"); break;
    case "size": {
      a.textContent = "counting…";
      try { const [b, n] = await api("dir_size", { path }); a.remove(); $("dsize").textContent = ` · ${fmtSize(b)} in ${(n - 1).toLocaleString()} items`; } catch (e) { a.textContent = "failed"; }
      break;
    }
  }
});

/* ---------------- status bar ---------------- */

function renderStatus() {
  const t = tab();
  const n = visible.length;
  $("stCount").textContent = t.error ? "" : `${n} item${n === 1 ? "" : "s"}`;
  const sel = selectedEntries();
  const sz = sel.reduce((a, e) => a + (e.dir ? 0 : e.size || 0), 0);
  $("stSel").textContent = sel.length && S.view !== "graph" ? `${sel.length} selected${sz ? " · " + fmtSize(sz) : ""}` : "";
  const parts = [];
  if (t.listing && !t.listing.writable) parts.push('<span class="ro">READ-ONLY</span>');
  if (clip) parts.push(`${clip.paths.length} on clipboard (${clip.op})`);
  if (S.hidden) parts.push("hidden files shown");
  parts.push(esc(homeShort(t.path)));
  $("stRight").innerHTML = parts.join(" · ");
}

/* ---------------- filter and search ---------------- */

let filterTimer;
$("q").addEventListener("input", () => {
  const t = tab();
  clearTimeout(filterTimer);
  filterTimer = setTimeout(() => {
    if (t.search) { t.search = null; $("deep").classList.remove("on"); api("cancel_search").catch(() => {}); }
    t.filter = $("q").value;
    renderFiles(); renderStatus();
  }, 60);
});
$("q").addEventListener("keydown", (e) => {
  if (e.key === "Enter") { e.preventDefault(); clearTimeout(filterTimer); const v = $("q").value.trim(); if (v) runSearch(tab(), v); }
  if (e.key === "Escape") { e.preventDefault(); if ($("q").value || tab().search) { $("q").value = ""; endSearch(); } files.focus(); }
  if (e.key === "ArrowDown") { e.preventDefault(); files.focus(); if (visible.length) { selectOnly(0); paintSelection(); } }
});
$("deep").addEventListener("click", () => { const v = $("q").value.trim(); if (v) runSearch(tab(), v); else { $("q").focus(); toast("Type a name, then press Enter to search every subfolder."); } });
async function runSearch(t, query, quiet = false) {
  t.search = { query, hits: quiet && t.search ? t.search.hits : [], running: true, truncated: false };
  t.filter = "";
  if (t === tab()) { $("deep").classList.add("on"); if (!quiet) renderFiles(); }
  try {
    const r = await api("search", { root: t.path, query, hidden: S.hidden });
    if (!t.search || t.search.query !== query) return;
    t.search = { query, hits: r.hits, running: false, truncated: r.truncated };
  } catch (e) { if (t.search) t.search.running = false; fail(e); }
  if (t === tab()) { renderFiles(); renderStatus(); }
}
function endSearch() {
  const t = tab();
  t.search = null; t.filter = "";
  $("deep").classList.remove("on");
  api("cancel_search").catch(() => {});
  renderFiles(); renderStatus();
}

/* ---------------- clipboard, paste, transfers ---------------- */

function setClip(op) {
  const sel = selectedEntries();
  if (!sel.length) return;
  clip = { op, paths: sel.map((e) => e.path) };
  renderFiles(); renderStatus();
  toast(`${op === "cut" ? "Cut" : "Copied"} ${sel.length === 1 ? sel[0].name : sel.length + " items"} · Ctrl+V to paste`, { ms: 2200 });
}
async function copyPaths(paths) {
  const text = paths.join("\n");
  try { await navigator.clipboard.writeText(text); }
  catch {
    try { await api("copy_to_clipboard", { text }); }
    catch { return fail("Couldn't reach the clipboard (install wl-clipboard or xclip)."); }
  }
  toast(paths.length === 1 ? `Copied path: ${homeShort(paths[0])}` : `Copied ${paths.length} paths`, { ms: 2000 });
}

async function paste(src = clip, dest = tab().path) {
  if (!src || !src.paths.length) return toast("Nothing to paste. Copy or cut something first.");
  const op = src.op === "cut" ? "move" : src.op;
  let conflict = "keep-both";
  // Look for name clashes in the destination (copying into the same folder just makes a copy).
  try {
    const l = await api("list_dir", { path: dest, hidden: true });
    const names = new Set(l.entries.map((e) => e.name));
    const clashes = src.paths.filter((p) => names.has(baseName(p)) && parentOf(p) !== dest);
    if (clashes.length) {
      conflict = await askConflict(clashes, dest);
      if (!conflict) return;
    }
  } catch (e) { return fail(e); }
  try {
    const id = await api("transfer", { op, paths: src.paths, dest, conflict });
    jobs.set(id, { id, op, dest, count: src.paths.length, from: src.paths.slice(), ev: null, cutClip: src === clip && clip.op === "cut" });
    renderJobs();
  } catch (e) { fail(e); }
}

onEvent("job", (ev) => {
  const j = jobs.get(ev.id) || { id: ev.id, op: ev.op, dest: "", count: 0 };
  j.ev = ev;
  jobs.set(ev.id, j);
  renderJobs();
  if (ev.state === "running") return;
  // finished
  const undo = { type: ev.op, moved: ev.moved, created: ev.created, trashed: ev.trashed, dest: j.dest };
  if (ev.moved.length || ev.created.length) pushUndo(undo);
  if (j.cutClip && ev.state === "done") clip = null;
  const landed = [...ev.created, ...ev.moved.map((m) => m[1])];
  const t = tab();
  if (landed.length && t.path === j.dest) {
    refresh(t).then(() => {
      t.sel = new Set(landed); t.focus = landed[landed.length - 1];
      paintSelection();
      landed.forEach((p) => { const i = visible.findIndex((e) => e.path === p); const el = itemEl(i); if (el) el.classList.add("fresh"); });
    });
  } else tabs.forEach((x) => refresh(x));
  setTimeout(() => { if (jobs.get(ev.id)?.ev?.state !== "running") { jobs.delete(ev.id); renderJobs(); } }, ev.state === "done" ? 2500 : 12000);
});

function renderJobs() {
  const box = $("jobs");
  const list = [...jobs.values()].filter((j) => j.ev);
  box.hidden = !list.length;
  box.innerHTML = list.map((j) => {
    const e = j.ev;
    const pct = e.total_bytes ? Math.round((e.done_bytes / e.total_bytes) * 100) : e.state === "running" ? 0 : 100;
    const verb = e.op === "move" ? (e.state === "running" ? "Moving" : "Moved") : e.state === "running" ? "Copying" : "Copied";
    const what = `${e.total_items > 1 ? e.total_items + " items" : esc(e.current || "")}`;
    const title = e.state === "cancelled" ? "Cancelled" : e.state === "error" ? `${verb} with problems` : `${verb} ${what}`;
    return `<div class="job ${e.state}"><div class="jh"><span class="jt">${title} → ${esc(baseName(j.dest) || "/")}</span>${e.state === "running" ? `<button class="btn sm" data-cancel="${e.id}">Cancel</button>` : ""}</div>
      <div class="jbar"><i style="width:${pct}%"></i></div>
      <div class="jm"><span>${esc(e.current)}</span><span>${fmtSize(e.done_bytes)} / ${fmtSize(e.total_bytes)} · ${pct}%</span></div>${e.error ? `<div class="jerr">${esc(e.error)}</div>` : ""}</div>`;
  }).join("");
}
$("jobs").addEventListener("click", (e) => { const b = e.target.closest("[data-cancel]"); if (b) api("cancel_job", { id: +b.dataset.cancel }); });

/* ---------------- create, rename, trash ---------------- */

function selectAfter(path) {
  const t = tab();
  return refresh(t).then(() => { t.sel = new Set([path]); t.anchor = path; t.focus = path; paintSelection(); const i = visible.findIndex((e) => e.path === path); itemEl(i)?.classList.add("fresh"); itemEl(i)?.scrollIntoView({ block: "nearest" }); });
}

async function newFolder() {
  const t = tab();
  const name = await prompt({ title: "New folder", label: "Name", value: "New Folder", ok: "Create" , check: (v) => existsHere(v) });
  if (name == null) return;
  try { const p = await api("mkdir", { parent: t.path, name }); pushUndo({ type: "mkdir", path: p }); await selectAfter(p); } catch (e) { fail(e); }
}
async function newFile() {
  const t = tab();
  const name = await prompt({ title: "New empty file", label: "Name", value: "untitled.txt", ok: "Create", selectStem: true, check: (v) => existsHere(v) });
  if (name == null) return;
  try { const p = await api("mkfile", { parent: t.path, name }); pushUndo({ type: "mkfile", path: p }); await selectAfter(p); } catch (e) { fail(e); }
}
function existsHere(v) { const t = tab(); return t.listing && t.listing.entries.some((e) => e.name === v.trim()) ? `"${v.trim()}" already exists here.` : ""; }

function startRename() {
  const sel = selectedEntries();
  if (sel.length !== 1 || S.view === "graph") {
    if (sel.length === 1) return renameModal(sel[0]);
    return;
  }
  const e = sel[0];
  const i = visible.findIndex((x) => x.path === e.path);
  const el = itemEl(i);
  if (!el) return;
  const nm = el.querySelector(".nm");
  const inp = document.createElement("input");
  inp.className = "ren";
  inp.value = e.name;
  inp.spellcheck = false;
  nm.replaceWith(inp);
  inp.focus();
  const dot = e.dir ? -1 : e.name.lastIndexOf(".");
  inp.setSelectionRange(0, dot > 0 ? dot : e.name.length);
  let done = false;
  const finish = async (commit) => {
    if (done) return; done = true;
    const v = inp.value.trim();
    if (!commit || !v || v === e.name) { renderFiles(); files.focus(); return; }
    try {
      const to = await api("rename", { path: e.path, name: v });
      pushUndo({ type: "rename", from: e.path, to });
      if (clip) clip.paths = clip.paths.map((p) => (p === e.path ? to : p));
      await selectAfter(to);
    } catch (err) { fail(err); renderFiles(); }
    files.focus();
  };
  inp.addEventListener("keydown", (k) => { k.stopPropagation(); if (k.key === "Enter") finish(true); if (k.key === "Escape") finish(false); });
  inp.addEventListener("blur", () => finish(true));
  inp.addEventListener("pointerdown", (k) => k.stopPropagation());
}
async function renameModal(e) {
  const v = await prompt({ title: "Rename", label: "New name", value: e.name, ok: "Rename", selectStem: !e.dir });
  if (v == null || v === e.name) return;
  try { const to = await api("rename", { path: e.path, name: v }); pushUndo({ type: "rename", from: e.path, to }); refresh(tab(), false); toast(`Renamed to ${v}`); } catch (err) { fail(err); }
}

async function trashSelection() {
  const sel = selectedEntries().filter((e) => !e.folderSelf);
  if (!sel.length) return;
  const t = tab();
  if (P.trash && (t.path === P.trash || t.path.startsWith(P.trash + "/"))) return toast("These are already in the Trash. Use Restore, or Empty Trash from the sidebar.");
  if (S.confirm_trash) {
    const ok = await confirmBox(`Move ${sel.length === 1 ? `“${sel[0].name}”` : sel.length + " items"} to the Trash?`, "You can restore them from the Trash later.", "Move to Trash");
    if (!ok) return;
  }
  const paths = sel.map((e) => e.path);
  // keep the cursor near where it was
  const idx = visible.findIndex((e) => e.path === paths[0]);
  try {
    await api("trash", { paths });
    pushUndo({ type: "trash", paths });
    await refresh(t);
    const next = visible[Math.min(idx, visible.length - 1)];
    t.sel = new Set(next ? [next.path] : []); t.anchor = next?.path; t.focus = next?.path;
    paintSelection();
    toast(`Moved ${sel.length === 1 ? sel[0].name : sel.length + " items"} to Trash`, { action: "Undo", run: undo });
    if (S.view === "graph") loadGraph();
  } catch (e) { fail(e); }
}
async function restoreSelection() {
  const sel = selectedEntries();
  if (!sel.length) return;
  try { const n = await api("restore_from_trash", { paths: sel.map((e) => e.path) }); toast(`Restored ${n} item${n === 1 ? "" : "s"}`); refresh(); } catch (e) { fail(e); }
}
async function emptyTrash() {
  const ok = await confirmBox("Empty the Trash?", "Everything in the Trash is deleted for good. This can't be undone.", "Empty Trash", true);
  if (!ok) return;
  try { const n = await api("empty_trash"); toast(`Deleted ${n} item${n === 1 ? "" : "s"} for good`); tabs.forEach((t) => refresh(t)); } catch (e) { fail(e); }
}

/* ---------------- undo ---------------- */

function pushUndo(u) { undoStack.push(u); if (undoStack.length > 50) undoStack.shift(); }
async function undo() {
  const u = undoStack.pop();
  if (!u) return toast("Nothing to undo.");
  try {
    switch (u.type) {
      case "rename": await api("move_exact", { from: u.to, to: u.from }); toast(`Undid rename`); break;
      case "mkdir": case "mkfile": await api("trash", { paths: [u.path] }); toast(`Undid create (moved to Trash)`); break;
      case "trash": { const n = await api("restore", { paths: u.paths }); toast(`Restored ${n} item${n === 1 ? "" : "s"} from Trash`); break; }
      case "copy":
        // Trash the copies, then bring back anything they replaced (never the copies themselves).
        await api("trash_and_restore", { trash: u.created, restore: u.trashed });
        toast(u.trashed.length ? `Undid replace (originals restored)` : `Undid copy (copies moved to Trash)`);
        break;
      case "move":
        for (const [from, to] of u.moved.slice().reverse()) await api("move_exact", { from: to, to: from });
        if (u.trashed.length) await api("trash_and_restore", { trash: [], restore: u.trashed });
        toast(`Moved ${u.moved.length} item${u.moved.length === 1 ? "" : "s"} back`);
        break;
    }
  } catch (e) { fail(e); }
  tabs.forEach((t) => refresh(t));
  if (S.view === "graph") loadGraph();
}

/* ---------------- context menu ---------------- */

function menu(x, y, items, header) {
  const m = $("ctx");
  m.innerHTML = (header ? `<div class="hd">${esc(header)}</div>` : "") + items.map((it, i) =>
    it === "-" ? "<hr>" : `<button data-i="${i}" class="${it.danger ? "danger" : ""}" ${it.disabled ? "disabled" : ""}><span>${esc(it.label)}</span>${it.key ? `<kbd>${esc(it.key)}</kbd>` : ""}</button>`
  ).join("");
  m.hidden = false;
  const r = m.getBoundingClientRect();
  m.style.left = Math.min(x, innerWidth - r.width - 6) + "px";
  m.style.top = Math.min(y, innerHeight - r.height - 6) + "px";
  m.onclick = (e) => { const b = e.target.closest("button[data-i]"); if (!b) return; m.hidden = true; items[+b.dataset.i].run(); };
  m.querySelector("button")?.focus();
}
addEventListener("pointerdown", (e) => { if (!e.target.closest("#ctx")) $("ctx").hidden = true; }, true);
addEventListener("blur", () => ($("ctx").hidden = true));
$("ctx").addEventListener("keydown", (e) => {
  const bs = [...$("ctx").querySelectorAll("button:not(:disabled)")];
  const i = bs.indexOf(document.activeElement);
  if (e.key === "ArrowDown") { e.preventDefault(); bs[(i + 1) % bs.length]?.focus(); }
  if (e.key === "ArrowUp") { e.preventDefault(); bs[(i - 1 + bs.length) % bs.length]?.focus(); }
  if (e.key === "Escape") { $("ctx").hidden = true; files.focus(); }
  e.stopPropagation();
});

files.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  const el = e.target.closest(".it");
  const t = tab();
  if (el) {
    const i = +el.dataset.i;
    if (!t.sel.has(visible[i].path)) { selectOnly(i); paintSelection(); }
    itemMenu(e.clientX, e.clientY);
  } else {
    t.sel = new Set(); paintSelection();
    folderMenu(e.clientX, e.clientY);
  }
});
function itemMenu(x, y) {
  const sel = selectedEntries();
  const one = sel.length === 1 ? sel[0] : null;
  const t = tab();
  const inTrash = P.trash && (t.path === P.trash || t.path.startsWith(P.trash + "/"));
  const items = [
    { label: one && one.dir ? "Open" : "Open", key: "Enter", run: openSelection },
  ];
  if (one && one.dir) items.push({ label: "Open in new tab", key: "Ctrl+Dbl-click", run: () => addTab(one.path) }, { label: "Open terminal here", key: "F4", run: () => terminalAt(one.path) }, { label: "Show as graph", run: () => { go(one.path).then(() => setView("graph")); } });
  if (t.search && one) items.push({ label: "Show in folder", run: () => go(parentOf(one.path), { select: one.path }) });
  items.push("-");
  if (inTrash) items.push({ label: "Restore", run: restoreSelection });
  else items.push({ label: "Cut", key: "Ctrl+X", run: () => setClip("cut") }, { label: "Copy", key: "Ctrl+C", run: () => setClip("copy") });
  if (one && one.dir && clip) items.push({ label: `Paste into ${one.name}`, key: "", run: () => paste(clip, one.path) });
  items.push({ label: sel.length > 1 ? "Copy paths" : "Copy path", key: "Ctrl+Shift+C", run: () => copyPaths(sel.map((e) => e.path)) });
  if (one) items.push({ label: "Rename…", key: "F2", run: startRename });
  if (one && one.dir) items.push({ label: S.pinned.includes(one.path) ? "Unpin from sidebar" : "Pin to sidebar", run: () => (S.pinned.includes(one.path) ? unpin(one.path) : pin(one.path)) });
  if (!inTrash) items.push("-", { label: "Move to Trash", key: "Delete", danger: true, run: trashSelection });
  menu(x, y, items, one ? one.name : `${sel.length} items`);
}
function folderMenu(x, y) {
  const t = tab();
  const inTrash = P.trash && t.path === P.trash;
  const items = inTrash ? [{ label: "Empty Trash…", danger: true, run: emptyTrash }, "-"] : [
    { label: "New folder…", key: "Ctrl+Shift+N", run: newFolder },
    { label: "New empty file…", key: "Ctrl+Alt+N", run: newFile },
    "-",
    { label: clip ? `Paste ${clip.paths.length} item${clip.paths.length === 1 ? "" : "s"}` : "Paste", key: "Ctrl+V", disabled: !clip, run: () => paste() },
  ];
  items.push(
    { label: "Select all", key: "Ctrl+A", run: selectAll },
    "-",
    { label: "Open terminal here", key: "F4", run: () => terminalAt(t.path) },
    { label: S.pinned.includes(t.path) ? "Unpin this folder" : "Pin this folder", run: () => (S.pinned.includes(t.path) ? unpin(t.path) : pin(t.path)) },
    { label: "Copy folder path", run: () => copyPaths([t.path]) },
    { label: S.hidden ? "Hide hidden files" : "Show hidden files", key: "Ctrl+H", run: toggleHidden },
    { label: "Refresh", key: "F5", run: () => refresh(t, false) },
  );
  menu(x, y, items, homeShort(t.path));
}

/* ---------------- modals ---------------- */

function prompt({ title, label, value = "", ok = "OK", selectStem = false, check }) {
  return new Promise((resolve) => {
    const m = $("modal");
    m.innerHTML = `<form><h2>${esc(title)}</h2><label class="f">${esc(label)}<input type="text" id="mIn" value="${esc(value)}" spellcheck="false" autocomplete="off"></label><div class="err" id="mErr"></div><div class="row2"><button type="button" class="btn" id="mCancel">Cancel</button><button class="btn primary">${esc(ok)}</button></div></form>`;
    m.hidden = false;
    const inp = $("mIn");
    inp.focus();
    const dot = value.lastIndexOf(".");
    if (selectStem && dot > 0) inp.setSelectionRange(0, dot); else inp.select();
    const close = (v) => { m.hidden = true; m.innerHTML = ""; files.focus(); resolve(v); };
    inp.addEventListener("input", () => ($("mErr").textContent = (check && check(inp.value)) || ""));
    $("mCancel").onclick = () => close(null);
    m.querySelector("form").onsubmit = (e) => {
      e.preventDefault();
      const v = inp.value.trim();
      if (!v) return ($("mErr").textContent = "The name can't be empty.");
      if (v.includes("/")) return ($("mErr").textContent = 'Names can\'t contain "/".');
      const c = check && check(v);
      if (c) return ($("mErr").textContent = c);
      close(v);
    };
    m.onkeydown = (e) => { e.stopPropagation(); if (e.key === "Escape") close(null); };
    m.onpointerdown = (e) => { if (e.target === m) close(null); };
  });
}
function confirmBox(title, body, ok, danger = false) {
  return new Promise((resolve) => {
    const m = $("modal");
    m.innerHTML = `<form><h2>${esc(title)}</h2><p>${esc(body)}</p><div class="row2"><button type="button" class="btn" id="mCancel">Cancel</button><button class="btn ${danger ? "danger" : "primary"}" id="mOk">${esc(ok)}</button></div></form>`;
    m.hidden = false;
    $("mOk").focus();
    const close = (v) => { m.hidden = true; m.innerHTML = ""; files.focus(); resolve(v); };
    $("mCancel").onclick = () => close(false);
    m.querySelector("form").onsubmit = (e) => { e.preventDefault(); close(true); };
    m.onkeydown = (e) => { e.stopPropagation(); if (e.key === "Escape") close(false); };
    m.onpointerdown = (e) => { if (e.target === m) close(false); };
  });
}
function askConflict(clashes, dest) {
  return new Promise((resolve) => {
    const m = $("modal");
    const list = clashes.slice(0, 12).map((p) => `<li>${esc(baseName(p))}</li>`).join("") + (clashes.length > 12 ? `<li>…and ${clashes.length - 12} more</li>` : "");
    m.innerHTML = `<form class="wide"><h2>${clashes.length === 1 ? "An item" : clashes.length + " items"} with the same name already ${clashes.length === 1 ? "exists" : "exist"} in ${esc(baseName(dest) || "/")}</h2><ul class="names">${list}</ul>
      <p>Replace moves the existing ${clashes.length === 1 ? "one" : "ones"} to the Trash first, so you can still get ${clashes.length === 1 ? "it" : "them"} back.</p>
      <div class="row2"><button type="button" class="btn" data-v="">Cancel</button><button type="button" class="btn" data-v="skip">Skip</button><button type="button" class="btn danger" data-v="replace">Replace</button><button type="button" class="btn primary" data-v="keep-both">Keep both</button></div></form>`;
    m.hidden = false;
    m.querySelector('[data-v="keep-both"]').focus();
    const close = (v) => { m.hidden = true; m.innerHTML = ""; files.focus(); resolve(v || null); };
    m.onclick = (e) => { const b = e.target.closest("[data-v]"); if (b) close(b.dataset.v); };
    m.onkeydown = (e) => { e.stopPropagation(); if (e.key === "Escape") close(null); };
  });
}

async function settingsModal() {
  const m = $("modal");
  const ds = await api("default_status").catch(() => ({ is_default: false, current: "", can_set: false }));
  const tog = (id, label, on) => `<label class="tog"><span>${label}</span><input type="checkbox" id="${id}" ${on ? "checked" : ""}><i></i></label>`;
  const defLine = ds.is_default ? '<div class="setnote ok">Null File opens folders on this computer.</div>'
    : `<div class="setnote">${ds.current ? `Folders currently open in <code>${esc(ds.current)}</code>.` : "No default file manager is set."}</div><div><button type="button" class="btn primary" id="sDef" ${ds.can_set ? "" : "disabled"}>Make Null File the default</button></div>${ds.can_set ? "" : '<div class="setnote warn">Needs xdg-mime (package xdg-utils).</div>'}`;
  m.innerHTML = `<form class="wide"><h2>Settings</h2>
    <div class="setsec"><h3>Browsing</h3>
      ${tog("sHidden", "Show hidden files", S.hidden)}
      ${tog("sPreview", "Show the details panel", S.preview)}
      ${tog("sRestore", "Reopen my tabs at startup", S.restore_tabs)}
      ${tog("sConfirm", "Ask before moving to Trash", S.confirm_trash)}
    </div>
    <div class="setsec"><h3>Default file manager</h3>${defLine}</div>
    <div class="setsec"><h3>Keyboard</h3><div class="keys">
      <kbd>Ctrl+K</kbd><span>Command palette</span><kbd>Ctrl+L</kbd><span>Type a path</span><kbd>Ctrl+F</kbd><span>Filter (Enter searches subfolders)</span>
      <kbd>Ctrl+T / Ctrl+W</kbd><span>New tab / close tab</span><kbd>Ctrl+Tab</kbd><span>Next tab</span><kbd>Alt+← → ↑</kbd><span>Back, forward, up</span>
      <kbd>Ctrl+1 2 3</kbd><span>Grid, list, graph</span><kbd>F2</kbd><span>Rename</span><kbd>Delete</kbd><span>Move to Trash</span>
      <kbd>Ctrl+C X V</kbd><span>Copy, cut, paste</span><kbd>Ctrl+Z</kbd><span>Undo</span><kbd>Ctrl+H</kbd><span>Hidden files</span>
      <kbd>Ctrl+Shift+N</kbd><span>New folder</span><kbd>F4</kbd><span>Terminal here</span><kbd>Ctrl+I</kbd><span>Details panel</span>
    </div></div>
    <div class="setnote">Null File ${esc(VERSION)} · NULLGRID. All rights reserved. · Your data. Your grid.</div>
    <div class="row2"><button class="btn primary">Done</button></div></form>`;
  m.hidden = false;
  const close = () => { m.hidden = true; m.innerHTML = ""; files.focus(); };
  m.querySelector("form").onsubmit = (e) => { e.preventDefault(); close(); };
  m.onkeydown = (e) => { e.stopPropagation(); if (e.key === "Escape") close(); };
  m.onpointerdown = (e) => { if (e.target === m) close(); };
  m.onchange = (e) => {
    const on = e.target.checked;
    if (e.target.id === "sHidden") { if (S.hidden !== on) toggleHidden(); }
    if (e.target.id === "sPreview") { S.preview = on; renderInfo(); if (S.view === "graph") graph.redraw(); }
    if (e.target.id === "sRestore") S.restore_tabs = on;
    if (e.target.id === "sConfirm") S.confirm_trash = on;
    saveSettings();
  };
  const def = $("sDef");
  if (def) def.onclick = async () => { try { const s = await api("make_default"); toast(s.is_default ? "Null File is now the default file manager" : "Couldn't change the default", { warn: !s.is_default }); close(); } catch (err) { fail(err); } };
}
let VERSION = "";

/* ---------------- views ---------------- */

const graph = createGraph({
  canvas: $("graphCanvas"),
  selected: () => new Set(tab().graphSel ? [tab().graphSel.path] : []),
  onSelect: async (n) => {
    const t = tab();
    if (!n) { t.graphSel = null; renderInfo(); graph.redraw(); return; }
    try { t.graphSel = await api("stat", { path: n.id }); } catch { t.graphSel = null; }
    renderInfo(); renderStatus(); graph.redraw();
  },
  onOpen: (n) => {
    if (n.dir) { if (n.id !== tab().path) go(n.id); }
    else api("open_path", { path: n.id }).catch(fail);
  },
  menu: (x, y, n) => {
    const t = tab();
    if (!n) return menu(x, y, [
      { label: "Fit to screen", run: () => graph.fit() },
      { label: "Re-layout", run: () => { graph.resetLayout(); loadGraph(); } },
      ...(parentOf(t.path) ? [{ label: "Go up a folder", key: "Alt+↑", run: up }] : []),
      { label: "Back to grid", key: "Ctrl+1", run: () => setView("grid") },
    ], "Graph · " + (baseName(t.path) || "/"));
    const items = n.dir
      ? [{ label: "Center graph here", run: () => go(n.id) }, { label: "Open in grid", run: () => go(n.id).then(() => setView("grid")) }, { label: "Open in new tab", run: () => addTab(n.id) }, { label: "Open terminal here", run: () => terminalAt(n.id) }]
      : [{ label: "Open", run: () => api("open_path", { path: n.id }).catch(fail) }, { label: "Show in folder", run: () => go(parentOf(n.id), { select: n.id }).then(() => setView("grid")) }];
    items.push({ label: "Copy path", run: () => copyPaths([n.id]) });
    if (n.dir && n.id !== t.path) items.push({ label: S.pinned.includes(n.id) ? "Unpin" : "Pin to sidebar", run: () => (S.pinned.includes(n.id) ? unpin(n.id) : pin(n.id)) });
    menu(x, y, items, n.name);
  },
});

let graphSeq = 0;
async function loadGraph() {
  const t = tab();
  const o = gopt();
  const my = ++graphSeq;
  $("gStat").textContent = "Mapping…";
  $("gStat").classList.remove("warn");
  try {
    const g = await api("graph", { root: t.path, depth: o.depth, hidden: S.hidden, maxNodes: o.files ? 2500 : 1500 });
    if (my !== graphSeq) return;
    if (g.nodes[0]) g.nodes[0].name = tabName(t.path);
    graph.setData(g, o.files);
    const c = graph.counts();
    $("gStat").textContent = `${c.nodes} nodes · ${c.links} links${g.truncated ? " · capped" : ""}`;
    $("gStat").classList.toggle("warn", g.truncated);
    $("gStat").title = g.truncated ? "This folder is big, so only part of it is drawn. Lower the depth or turn off files to see more structure." : "";
    $("glegend").innerHTML = `<span style="color:#ff2e88"><i></i>This folder</span>` + graph.kinds().map((k) => `<span style="color:${KCOL[k]}"><i></i>${KNAME[k]}</span>`).join("");
  } catch (e) {
    if (my !== graphSeq) return;
    $("gStat").textContent = errText(e);
    $("gStat").classList.add("warn");
  }
}

function setView(v) {
  const prev = S.view;
  S.view = v;
  saveSettings();
  ["Grid", "List", "Graph"].forEach((n) => $("v" + n).setAttribute("aria-pressed", String(v === n.toLowerCase())));
  const isG = v === "graph";
  $("graphhost").hidden = !isG;
  $("files").hidden = isG;
  if (isG) {
    $("emptymsg").hidden = true;
    const o = gopt();
    $("gDepth").value = o.depth; $("gDepthO").textContent = o.depth;
    $("gFiles").checked = o.files; $("gLabels").checked = o.labels; $("gGlow").checked = o.glow;
    graph.setOpts({ labels: o.labels, glow: o.glow });
    graph.show();
    loadGraph();
  } else {
    graph.hide();
    tab().graphSel = null;
    renderFiles();
    if (prev === "graph") files.focus();
  }
  renderInfo(); renderStatus();
}
$("vGrid").onclick = () => setView("grid");
$("vList").onclick = () => setView("list");
$("vGraph").onclick = () => setView("graph");
const setG = (k, v) => { S.graph = { ...gopt(), [k]: v }; saveSettings(); };
$("gDepth").addEventListener("input", (e) => { $("gDepthO").textContent = e.target.value; });
$("gDepth").addEventListener("change", (e) => { setG("depth", +e.target.value); loadGraph(); });
$("gFiles").addEventListener("change", (e) => { setG("files", e.target.checked); loadGraph(); });
$("gLabels").addEventListener("change", (e) => { setG("labels", e.target.checked); graph.setOpts({ labels: e.target.checked }); });
$("gGlow").addEventListener("change", (e) => { setG("glow", e.target.checked); graph.setOpts({ glow: e.target.checked }); });
$("gFit").onclick = () => graph.fit();

function toggleHidden() {
  S.hidden = !S.hidden;
  saveSettings();
  tabs.forEach((t) => refresh(t, false));
  toast(S.hidden ? "Showing hidden files" : "Hiding hidden files", { ms: 1500 });
}
function selectAll() { const t = tab(); t.sel = new Set(visible.map((e) => e.path)); paintSelection(); }
function togglePreview() { S.preview = !S.preview; saveSettings(); renderInfo(); if (S.view === "graph") graph.redraw(); }

/* ---------------- command palette ---------------- */

const COMMANDS = [
  { t: "New folder", k: "Ctrl+Shift+N", run: newFolder },
  { t: "New empty file", k: "Ctrl+Alt+N", run: newFile },
  { t: "New tab", k: "Ctrl+T", run: () => addTab(tab().path) },
  { t: "Close tab", k: "Ctrl+W", run: () => closeTab() },
  { t: "Paste", k: "Ctrl+V", run: () => paste() },
  { t: "Undo", k: "Ctrl+Z", run: undo },
  { t: "Select all", k: "Ctrl+A", run: selectAll },
  { t: "Rename selected", k: "F2", run: startRename },
  { t: "Move selected to Trash", k: "Delete", run: trashSelection },
  { t: "Copy path of selected", k: "Ctrl+Shift+C", run: () => { const s = selectedEntries(); copyPaths(s.length ? s.map((e) => e.path) : [tab().path]); } },
  { t: "Open terminal here", k: "F4", run: () => terminalAt(tab().path) },
  { t: "Show hidden files (toggle)", k: "Ctrl+H", run: toggleHidden },
  { t: "Details panel (toggle)", k: "Ctrl+I", run: togglePreview },
  { t: "Grid view", k: "Ctrl+1", run: () => setView("grid") },
  { t: "List view", k: "Ctrl+2", run: () => setView("list") },
  { t: "Graph view", k: "Ctrl+3", run: () => setView("graph") },
  { t: "Search every subfolder…", k: "Ctrl+F, Enter", run: () => { $("q").focus(); } },
  { t: "Go to path…", k: "Ctrl+L", run: editPath },
  { t: "Go up a folder", k: "Alt+↑", run: up },
  { t: "Refresh", k: "F5", run: () => refresh(tab(), false) },
  { t: "Pin this folder to the sidebar", run: () => pin(tab().path) },
  { t: "Sort by name", run: () => { S.sort = "name"; S.desc = false; saveSettings(); renderFiles(); } },
  { t: "Sort by size", run: () => { S.sort = "size"; S.desc = true; saveSettings(); renderFiles(); } },
  { t: "Sort by date modified", run: () => { S.sort = "modified"; S.desc = true; saveSettings(); renderFiles(); } },
  { t: "Sort by type", run: () => { S.sort = "kind"; S.desc = false; saveSettings(); renderFiles(); } },
  { t: "Empty Trash…", run: emptyTrash },
  { t: "Make Null File the default file manager", run: async () => { try { const s = await api("make_default"); toast(s.is_default ? "Null File is now the default file manager" : "Couldn't change the default", { warn: !s.is_default }); } catch (e) { fail(e); } } },
  { t: "Settings", run: settingsModal },
];

let pal = { items: [], on: 0, seq: 0 };
function fuzzy(q, s) {
  // subsequence match; score favors word starts and contiguous runs
  q = q.toLowerCase(); const L = s.toLowerCase();
  let qi = 0, score = 0, run = 0, last = -2;
  const hits = [];
  for (let i = 0; i < L.length && qi < q.length; i++) {
    if (L[i] === q[qi]) {
      run = i === last + 1 ? run + 1 : 1;
      score += run * 2 + (i === 0 || /[\s/._-]/.test(L[i - 1]) ? 6 : 0);
      hits.push(i); last = i; qi++;
    }
  }
  if (qi < q.length) return null;
  return { score: score - L.length * 0.05, hits };
}
const mark = (s, hits) => { if (!hits) return esc(s); const h = new Set(hits); return [...s].map((c, i) => (h.has(i) ? `<mark>${esc(c)}</mark>` : esc(c))).join(""); };

function openPalette(initial = "") {
  $("palette").hidden = false;
  $("pq").value = initial;
  $("pq").focus();
  updatePalette();
}
function closePalette() { $("palette").hidden = true; files.focus(); }
async function updatePalette() {
  const q = $("pq").value;
  const my = ++pal.seq;
  let items = [];
  if (/^[\/~]/.test(q)) {
    // path mode: complete folder names
    let v = q.startsWith("~") ? P.home + q.slice(1) : q;
    const slash = v.lastIndexOf("/");
    const dir = slash <= 0 ? "/" : v.slice(0, slash), part = v.slice(slash + 1);
    items.push({ kind: "go", t: homeShort(v.replace(/\/+$/, "") || "/"), sub: "go to this path", run: () => goPath(v) });
    try {
      const l = await api("list_dir", { path: dir, hidden: part.startsWith(".") || S.hidden });
      if (my !== pal.seq) return;
      for (const e of l.entries) {
        if (!e.dir) continue;
        const f = part ? fuzzy(part, e.name) : { score: 0 };
        if (!f) continue;
        items.push({ kind: "folder", t: homeShort(e.path), hits: null, score: f.score, run: () => go(e.path), fill: homeShort(e.path) + "/" });
      }
    } catch { /* not a folder yet */ }
    items = [items[0], ...items.slice(1).sort((a, b) => b.score - a.score).slice(0, 40)];
  } else {
    const t = tab();
    const pool = [
      ...COMMANDS.map((c) => ({ kind: "action", t: c.t, key: c.k, run: c.run })),
      ...[...new Set([...S.pinned, ...P.places.map((p) => p.path), ...S.recent])].map((p) => ({ kind: "folder", t: baseName(p) || "/", sub: homeShort(p), run: () => go(p) })),
      ...(t.listing ? t.listing.entries.map((e) => ({ kind: e.dir ? "here" : "file", t: e.name, sub: e.dir ? "folder here" : fmtSize(e.size), run: () => (e.dir ? go(e.path) : openEntry(e)) })) : []),
    ];
    if (!q.trim()) items = pool.filter((p) => p.kind !== "file").slice(0, 60);
    else {
      for (const it of pool) { const f = fuzzy(q.trim(), it.t); if (f) items.push({ ...it, score: f.score + (it.kind === "action" ? 2 : 0), hits: f.hits }); }
      items.sort((a, b) => b.score - a.score);
      items = items.slice(0, 60);
    }
  }
  pal.items = items;
  pal.on = 0;
  paintPalette();
}
function paintPalette() {
  $("plist").innerHTML = pal.items.map((it, i) =>
    `<button class="pitem${i === pal.on ? " on" : ""}" data-i="${i}"><span class="pk">${esc(it.kind)}</span><span class="pt">${mark(it.t, it.hits)}${it.sub ? `<small>${esc(it.sub)}</small>` : ""}</span>${it.key ? `<kbd>${esc(it.key)}</kbd>` : ""}</button>`
  ).join("") || '<div class="pitem"><span class="pt" style="color:var(--faint)">No matches</span></div>';
  $("plist").querySelector(".pitem.on")?.scrollIntoView({ block: "nearest" });
}
async function goPath(v) {
  v = v.replace(/\/+$/, "") || "/";
  try { const st = await api("stat", { path: v }); if (st.dir) go(v); else go(parentOf(v), { select: v }); } catch (e) { fail(e); }
}
$("pq").addEventListener("input", updatePalette);
$("pq").addEventListener("keydown", (e) => {
  e.stopPropagation();
  if (e.key === "Escape") return closePalette();
  if (e.key === "ArrowDown") { e.preventDefault(); pal.on = Math.min(pal.items.length - 1, pal.on + 1); paintPalette(); }
  if (e.key === "ArrowUp") { e.preventDefault(); pal.on = Math.max(0, pal.on - 1); paintPalette(); }
  if (e.key === "Tab") { e.preventDefault(); const it = pal.items[pal.on]; if (it && it.fill) { $("pq").value = it.fill; updatePalette(); } }
  if (e.key === "Enter") { e.preventDefault(); const it = pal.items[pal.on]; if (it) { closePalette(); it.run(); } }
});
$("plist").addEventListener("click", (e) => { const b = e.target.closest("[data-i]"); if (b) { const it = pal.items[+b.dataset.i]; closePalette(); it.run(); } });
$("palette").addEventListener("pointerdown", (e) => { if (e.target === $("palette")) closePalette(); });

/* ---------------- keyboard ---------------- */

function columns() {
  if (S.view !== "grid") return 1;
  const els = files.querySelectorAll(".it");
  if (els.length < 2) return 1;
  const top = els[0].offsetTop;
  let n = 0;
  for (const el of els) { if (el.offsetTop !== top) break; n++; }
  return Math.max(1, n);
}
function moveFocus(delta, extend) {
  const t = tab();
  if (!visible.length) return;
  let i = visible.findIndex((e) => e.path === t.focus);
  i = i < 0 ? 0 : Math.max(0, Math.min(visible.length - 1, i + delta));
  if (extend) selectRange(i); else selectOnly(i);
  paintSelection();
  itemEl(i)?.scrollIntoView({ block: "nearest" });
}

let typeBuf = "", typeTimer;
addEventListener("keydown", (e) => {
  if (!$("modal").hidden || !$("palette").hidden) return;
  const inInput = e.target.matches("input, textarea") && e.target.type !== "checkbox" && e.target.type !== "range";
  const k = e.key, ctrl = e.ctrlKey || e.metaKey;
  if (ctrl && k.toLowerCase() === "k") { e.preventDefault(); return openPalette(); }
  if (ctrl && k.toLowerCase() === "p" && !e.shiftKey) { e.preventDefault(); return openPalette(); }
  if (ctrl && k.toLowerCase() === "l") { e.preventDefault(); return editPath(); }
  if (ctrl && k.toLowerCase() === "f") { e.preventDefault(); $("q").focus(); $("q").select(); return; }
  if (ctrl && k.toLowerCase() === "t") { e.preventDefault(); return addTab(tab().path); }
  if (ctrl && k.toLowerCase() === "w") { e.preventDefault(); return closeTab(); }
  if (ctrl && k === "Tab") { e.preventDefault(); return switchTab((cur + (e.shiftKey ? -1 : 1) + tabs.length) % tabs.length); }
  if (ctrl && /^[1-3]$/.test(k)) { e.preventDefault(); return setView(["grid", "list", "graph"][+k - 1]); }
  if (e.altKey && /^[1-9]$/.test(k)) { e.preventDefault(); return switchTab(Math.min(tabs.length, +k) - 1); }
  if (e.altKey && k === "ArrowLeft") { e.preventDefault(); return back(); }
  if (e.altKey && k === "ArrowRight") { e.preventDefault(); return forward(); }
  if (e.altKey && k === "ArrowUp") { e.preventDefault(); return up(); }
  if (k === "F5" || (ctrl && k.toLowerCase() === "r")) { e.preventDefault(); return refresh(tab(), false); }
  if (ctrl && k.toLowerCase() === "h") { e.preventDefault(); return toggleHidden(); }
  if (ctrl && k.toLowerCase() === "i") { e.preventDefault(); return togglePreview(); }
  if (ctrl && e.shiftKey && k.toLowerCase() === "n") { e.preventDefault(); return newFolder(); }
  if (ctrl && e.altKey && k.toLowerCase() === "n") { e.preventDefault(); return newFile(); }
  if (k === "F4") { e.preventDefault(); return terminalAt(tab().path); }
  if (inInput) return;
  if (k === "Escape") {
    if (tab().search) return endSearch();
    if (tab().filter) { $("q").value = ""; tab().filter = ""; return renderFiles(); }
    tab().sel = new Set(); paintSelection(); return;
  }
  if (ctrl && k.toLowerCase() === "z") { e.preventDefault(); return undo(); }
  if (S.view === "graph") {
    if (k === "Backspace") { e.preventDefault(); return back(); }
    if (k === "Enter" && tab().graphSel) { const g = tab().graphSel; return g.dir ? go(g.path) : openEntry(g); }
    if (k.toLowerCase() === "f" && !ctrl) return graph.fit();
    return;
  }
  if (ctrl && e.shiftKey && k.toLowerCase() === "c") { e.preventDefault(); const s = selectedEntries(); return copyPaths(s.length ? s.map((x) => x.path) : [tab().path]); }
  if (ctrl && k.toLowerCase() === "c") { e.preventDefault(); return setClip("copy"); }
  if (ctrl && k.toLowerCase() === "x") { e.preventDefault(); return setClip("cut"); }
  if (ctrl && k.toLowerCase() === "v") { e.preventDefault(); return paste(); }
  if (ctrl && k.toLowerCase() === "a") { e.preventDefault(); return selectAll(); }
  if (k === "F2") { e.preventDefault(); return startRename(); }
  if (k === "Delete") { e.preventDefault(); return trashSelection(); }
  if (k === "Enter") { e.preventDefault(); return ctrl && selectedEntries().length === 1 ? openEntry(selectedEntries()[0], true) : openSelection(); }
  if (k === "Backspace") { e.preventDefault(); return back(); }
  const cols = columns();
  if (k === "ArrowRight") { e.preventDefault(); return moveFocus(S.view === "grid" ? 1 : 0, e.shiftKey); }
  if (k === "ArrowLeft") { e.preventDefault(); return moveFocus(S.view === "grid" ? -1 : 0, e.shiftKey); }
  if (k === "ArrowDown") { e.preventDefault(); return moveFocus(cols, e.shiftKey); }
  if (k === "ArrowUp") { e.preventDefault(); return moveFocus(-cols, e.shiftKey); }
  if (k === "Home") { e.preventDefault(); return moveFocus(-1e9, e.shiftKey); }
  if (k === "End") { e.preventDefault(); return moveFocus(1e9, e.shiftKey); }
  if (k === "PageDown") { e.preventDefault(); return moveFocus(cols * 8, e.shiftKey); }
  if (k === "PageUp") { e.preventDefault(); return moveFocus(-cols * 8, e.shiftKey); }
  // type-ahead: jump to the first name starting with what you type
  if (k.length === 1 && !ctrl && !e.altKey && k !== " ") {
    clearTimeout(typeTimer);
    typeBuf += k.toLowerCase();
    typeTimer = setTimeout(() => (typeBuf = ""), 800);
    const i = visible.findIndex((x) => x.name.toLowerCase().startsWith(typeBuf));
    if (i >= 0) { selectOnly(i); paintSelection(); itemEl(i)?.scrollIntoView({ block: "nearest" }); }
  }
});

/* top bar buttons */
$("back").onclick = back;
$("fwd").onclick = forward;
$("up").onclick = up;
$("menuBtn").onclick = (e) => {
  const r = e.currentTarget.getBoundingClientRect();
  menu(r.right - 230, r.bottom + 4, [
    { label: "Command palette", key: "Ctrl+K", run: () => openPalette() },
    { label: "New tab", key: "Ctrl+T", run: () => addTab(tab().path) },
    { label: "New folder…", key: "Ctrl+Shift+N", run: newFolder },
    "-",
    { label: S.hidden ? "Hide hidden files" : "Show hidden files", key: "Ctrl+H", run: toggleHidden },
    { label: S.preview ? "Hide details panel" : "Show details panel", key: "Ctrl+I", run: togglePreview },
    { label: "Undo", key: "Ctrl+Z", disabled: !undoStack.length, run: undo },
    "-",
    { label: "Settings & shortcuts", run: settingsModal },
  ]);
};

/* mouse back/forward buttons */
addEventListener("mouseup", (e) => { if (e.button === 3) back(); if (e.button === 4) forward(); });

/* ---------------- external drops ---------------- */

onFileDrop(({ type, paths, x, y }) => {
  const d = $("drop");
  const targetAt = () => {
    const el = document.elementFromPoint(x, y);
    const t = el && el.closest("[data-drop]");
    return t ? t.dataset.drop : tab().search ? null : tab().path;
  };
  if (type === "enter" || type === "over") {
    const dest = targetAt();
    d.hidden = !dest;
    $("dropTo").textContent = dest ? "into " + homeShort(dest) : "";
    document.querySelectorAll(".drop-on").forEach((e) => e.classList.remove("drop-on"));
    const el = document.elementFromPoint(x, y)?.closest("[data-drop]");
    if (el) el.classList.add("drop-on");
  } else if (type === "leave") {
    d.hidden = true;
    document.querySelectorAll(".drop-on").forEach((e) => e.classList.remove("drop-on"));
  } else if (type === "drop") {
    d.hidden = true;
    document.querySelectorAll(".drop-on").forEach((e) => e.classList.remove("drop-on"));
    const dest = targetAt();
    if (dest && paths.length) paste({ op: "copy", paths }, dest);
  }
});

/* ---------------- live updates ---------------- */

let changeTimer, changed = new Set();
onEvent("changed", (dirs) => {
  dirs.forEach((d) => changed.add(d));
  clearTimeout(changeTimer);
  changeTimer = setTimeout(() => {
    const set = changed; changed = new Set();
    tabs.forEach((t) => { if (set.has(t.path)) refresh(t); });
  }, 150);
});
onEvent("open-paths", (paths) => openPaths(paths));

async function openPaths(paths) {
  for (const p of paths) {
    try {
      const st = await api("stat", { path: p });
      if (st.dir) addTab(p); else addTab(parentOf(p), true, p);
    } catch (e) { fail(e); }
  }
}

/* ---------------- start ---------------- */

async function start() {
  if (!native) {
    document.body.innerHTML = '<div class="emptymsg" style="position:fixed;inset:0"><b>NULL FILE</b><span>Run this as the desktop app.</span></div>';
    return;
  }
  try { VERSION = await window.__TAURI__.app.getVersion(); } catch { VERSION = ""; }
  S = { ...S, ...(await api("get_settings")) };
  P = await api("places");
  document.documentElement.style.setProperty("--side", S.sidebar_w + "px");
  const pending = await api("take_pending");
  const startTabs = [];
  if (pending.length) {
    for (const p of pending) {
      try { const st = await api("stat", { path: p }); startTabs.push(st.dir ? { path: p } : { path: parentOf(p), select: p }); } catch { /* skip */ }
    }
  }
  if (!startTabs.length && S.restore_tabs && S.last_tabs.length) {
    for (const p of S.last_tabs) { try { const st = await api("stat", { path: p }); if (st.dir) startTabs.push({ path: p }); } catch { /* gone */ } }
  }
  if (!startTabs.length) startTabs.push({ path: P.home });
  for (const s of startTabs) {
    const t = newTab(s.path);
    await load(t);
    if (s.select) { t.sel = new Set([s.select]); t.anchor = s.select; t.focus = s.select; }
    tabs.push(t);
  }
  cur = 0;
  render();
  setView(S.view === "graph" ? "graph" : S.view === "list" ? "list" : "grid");
  watchTabs();
  files.focus();
  // Drives come and go: refresh the sidebar now and then.
  setInterval(async () => { try { const np = await api("places"); if (JSON.stringify(np.drives) !== JSON.stringify(P.drives)) { P = np; renderSide(); } } catch { /* ignore */ } }, 5000);
}
start();
