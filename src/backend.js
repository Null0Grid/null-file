// Bridge to the Rust side (Tauri commands and events).

const T = window.__TAURI__;
export const native = !!(T && T.core);

export async function api(cmd, args = {}) {
  if (!native) throw "Null File has to run as the desktop app (the browser can't reach your files).";
  return T.core.invoke(cmd, args);
}

export function onEvent(name, handler) {
  if (native) return T.event.listen(name, (e) => handler(e.payload));
}

/** URL the webview can load a local file from (thumbnails, previews). */
export function fileUrl(path) {
  if (!native) return "";
  // Vault files are decrypted on the fly by the app's own nfvault:// handler.
  if (path.startsWith("vault:")) return "nfvault://localhost/" + encodeURIComponent(path);
  return T.core.convertFileSrc(path);
}

/** Files dragged in from other apps. handler({type, paths, x, y}). */
export function onFileDrop(handler) {
  if (!native) return;
  const dpr = () => window.devicePixelRatio || 1;
  const pos = (p) => (p && p.position ? { x: p.position.x / dpr(), y: p.position.y / dpr() } : { x: -1, y: -1 });
  T.event.listen("tauri://drag-enter", (e) => handler({ type: "enter", paths: e.payload.paths || [], ...pos(e.payload) }));
  T.event.listen("tauri://drag-over", (e) => handler({ type: "over", paths: [], ...pos(e.payload) }));
  T.event.listen("tauri://drag-drop", (e) => handler({ type: "drop", paths: e.payload.paths || [], ...pos(e.payload) }));
  T.event.listen("tauri://drag-leave", () => handler({ type: "leave", paths: [], x: -1, y: -1 }));
}

export function setTitle(t) {
  if (native) T.window.getCurrentWindow().setTitle(t).catch(() => {});
  document.title = t;
}
