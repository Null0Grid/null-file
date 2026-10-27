// Graph view: the folder tree as a force-directed graph (d3-force, the same
// engine as the Null Notes graph). Folders and files are nodes; each item is
// linked to the folder it lives in.

const d3 = window.d3;
export const KCOL = {
  dir: "#00e5ff", img: "#a58bff", video: "#ff6b3d", audio: "#39ff88", text: "#8a92b4",
  code: "#ffb000", doc: "#7fb2ff", archive: "#f5f06b", exec: "#39ff88", file: "#5a6288",
};
export const KNAME = {
  dir: "Folders", img: "Images", video: "Video", audio: "Audio", text: "Text",
  code: "Code", doc: "Documents", archive: "Archives", exec: "Programs", file: "Other",
};

export function createGraph(ctx) {
  // ctx: { canvas, onSelect(node|null), onOpen(node), menu(x, y, node), selected() }
  const cv = ctx.canvas;
  const c = cv.getContext("2d");
  let nodes = [], links = [], byId = new Map(), nb = new Map();
  let sim = null, open = false, frame = 0, rootId = null;
  let view = { k: 1, x: 0, y: 0 };
  let hover = null, drag = null, pan = null, lastClick = { id: null, t: 0 };
  let opts = { labels: true, glow: true };
  let pendingFit = false;
  const remembered = new Map();

  function radius(n) {
    if (n.id === rootId) return 14;
    if (n.dir) return 5 + Math.min(11, Math.sqrt(n.children || 0) * 1.7);
    const kb = Math.max(1, (n.size || 0) / 1024);
    return 3 + Math.min(7, Math.log10(kb) * 1.6);
  }

  function setData(g, showFiles) {
    nodes.forEach((n) => remembered.set(n.id, { x: n.x, y: n.y }));
    rootId = g.root;
    // Keep the root at the origin: shift remembered positions so re-rooting on a
    // subfolder (or a parent) doesn't make the whole layout jump.
    const r0 = remembered.get(rootId);
    if (r0 && (r0.x || r0.y)) {
      const o = { x: r0.x, y: r0.y };
      for (const v of remembered.values()) { v.x -= o.x; v.y -= o.y; }
      view.x += o.x * view.k; view.y += o.y * view.k;
    }
    const keep = new Set();
    const N = [];
    for (const n of g.nodes) {
      if (!showFiles && !n.dir) continue;
      keep.add(n.id);
      N.push({ ...n });
    }
    const L = g.edges.filter((e) => keep.has(e.source) && keep.has(e.target)).map((e) => ({ source: e.source, target: e.target }));
    const parent = new Map(L.map((l) => [l.target, l.source]));
    byId = new Map(N.map((n) => [n.id, n]));
    nb = new Map(N.map((n) => [n.id, new Set()]));
    L.forEach((l) => { nb.get(l.source).add(l.target); nb.get(l.target).add(l.source); });
    // Start new nodes next to their parent so the layout grows outward instead of exploding.
    N.forEach((n, i) => {
      n.r = radius(n);
      const prev = remembered.get(n.id);
      if (prev) { n.x = prev.x; n.y = prev.y; return; }
      const p = byId.get(parent.get(n.id));
      if (p && p.x !== undefined) { n.x = p.x + (Math.random() - 0.5) * 40; n.y = p.y + (Math.random() - 0.5) * 40; return; }
      const a = (i / Math.max(1, N.length)) * Math.PI * 2;
      n.x = n.id === rootId ? 0 : Math.cos(a) * 80; n.y = n.id === rootId ? 0 : Math.sin(a) * 80;
    });
    const root = byId.get(rootId);
    if (root) { root.fx = 0; root.fy = 0; }
    nodes = N; links = L;
    const fresh = N.filter((n) => !remembered.has(n.id)).length;
    pendingFit = fresh > N.length / 2;
    startSim(pendingFit ? 1 : 0.5);
  }

  function startSim(alpha) {
    if (sim) sim.stop();
    const many = nodes.length > 1200;
    sim = d3.forceSimulation(nodes)
      .force("link", d3.forceLink(links).id((d) => d.id).distance((l) => 22 + l.source.r + l.target.r + (l.target.dir ? 26 : 6)).strength(0.9))
      .force("charge", d3.forceManyBody().strength((d) => (d.dir ? -160 : -40)).distanceMax(many ? 400 : 700).theta(many ? 1.2 : 0.9))
      .force("x", d3.forceX(0).strength(0.03))
      .force("y", d3.forceY(0).strength(0.03))
      .force("collide", d3.forceCollide((d) => d.r + 3).strength(0.8))
      .velocityDecay(0.42)
      .alphaDecay(many ? 0.035 : 0.022)
      .on("tick", () => {
        if (pendingFit && sim.alpha() < 0.2) { pendingFit = false; fit(); }
        requestDraw();
      });
    sim.alpha(alpha).restart();
  }

  /* ---------- coordinates ---------- */
  const toWorld = (sx, sy) => ({ x: (sx - view.x) / view.k, y: (sy - view.y) / view.k });
  function nodeAt(sx, sy) {
    const w = toWorld(sx, sy);
    let best = null, bd = Infinity;
    for (const n of nodes) {
      const d = (n.x - w.x) ** 2 + (n.y - w.y) ** 2, hit = (n.r + 5 / view.k) ** 2;
      if (d < hit && d < bd) { best = n; bd = d; }
    }
    return best;
  }
  const local = (e) => { const r = cv.getBoundingClientRect(); return { x: e.clientX - r.left, y: e.clientY - r.top }; };

  /* ---------- drawing ---------- */
  function requestDraw() { if (!frame) frame = requestAnimationFrame(draw); }
  function draw() {
    frame = 0;
    if (!open) return;
    const W = cv.clientWidth, H = cv.clientHeight, dpr = devicePixelRatio || 1;
    if (!W || !H) return;
    if (cv.width !== Math.round(W * dpr) || cv.height !== Math.round(H * dpr)) { cv.width = Math.round(W * dpr); cv.height = Math.round(H * dpr); }
    c.setTransform(dpr, 0, 0, dpr, 0, 0);
    c.clearRect(0, 0, W, H);
    c.save();
    c.translate(view.x, view.y);
    c.scale(view.k, view.k);

    const sel = ctx.selected();
    const focus = drag ? drag.node : hover;
    const near = focus ? nb.get(focus.id) : null;
    const lit = (n) => !focus || n === focus || near.has(n.id);

    c.lineWidth = 1 / view.k;
    c.strokeStyle = focus ? "rgba(0,229,255,.06)" : "rgba(0,229,255,.2)";
    c.beginPath();
    for (const l of links) {
      if (focus && (l.source === focus || l.target === focus)) continue;
      c.moveTo(l.source.x, l.source.y); c.lineTo(l.target.x, l.target.y);
    }
    c.stroke();
    if (focus) {
      c.strokeStyle = "rgba(0,229,255,.85)";
      c.lineWidth = 1.6 / view.k;
      c.beginPath();
      for (const l of links) if (l.source === focus || l.target === focus) { c.moveTo(l.source.x, l.source.y); c.lineTo(l.target.x, l.target.y); }
      c.stroke();
    }

    const glow = opts.glow && nodes.length < 1500;
    for (const n of nodes) {
      const isRoot = n.id === rootId, isSel = sel.has(n.id);
      const col = isRoot ? "#ff2e88" : KCOL[n.kind] || KCOL.file;
      c.globalAlpha = lit(n) ? 1 : 0.14;
      if (glow) { c.shadowColor = col; c.shadowBlur = (n === focus || isSel || isRoot ? 18 : n.dir ? 8 : 4) * view.k; }
      c.fillStyle = col;
      c.beginPath();
      if (n.dir) c.arc(n.x, n.y, n.r, 0, Math.PI * 2);
      else c.rect(n.x - n.r * 0.85, n.y - n.r * 0.85, n.r * 1.7, n.r * 1.7);
      c.fill();
      c.shadowBlur = 0;
      if (n.more && n.dir) {
        // Dashed ring: this folder has contents that aren't drawn (depth limit).
        c.strokeStyle = col;
        c.lineWidth = 1.2 / view.k;
        c.setLineDash([3 / view.k, 3 / view.k]);
        c.beginPath(); c.arc(n.x, n.y, n.r + 4 / view.k + 1, 0, Math.PI * 2); c.stroke();
        c.setLineDash([]);
      }
      if (isSel) {
        c.strokeStyle = "#ffffff";
        c.lineWidth = 2 / view.k;
        c.beginPath(); c.arc(n.x, n.y, n.r + 3.5, 0, Math.PI * 2); c.stroke();
      }
    }

    if (opts.labels || focus) {
      const base = Math.max(0, Math.min(1, (view.k - 0.5) / 0.4));
      c.textAlign = "center";
      // Keep labels a steady on-screen size instead of growing with zoom.
      const fs = Math.max(9, Math.min(12, 11.5 * Math.sqrt(view.k))) / view.k;
      c.font = `${fs}px "IBM Plex Mono", monospace`;
      for (const n of nodes) {
        const always = n === focus || n.id === rootId || sel.has(n.id) || (near && near.has(n.id));
        let a = always ? 1 : !opts.labels ? 0 : focus ? base * 0.2 : n.dir ? base : base * 0.75;
        if (!n.dir && !always && view.k < 0.9) a *= Math.max(0, (view.k - 0.6) / 0.3);
        if (a < 0.04) continue;
        c.globalAlpha = a;
        c.fillStyle = always ? "#d9def0" : n.dir ? "#8fdcea" : "#8a92b4";
        c.fillText(n.name, n.x, n.y + n.r + fs + 3 / view.k);
      }
    }
    c.restore();
    c.globalAlpha = 1;
  }

  /* ---------- input ---------- */
  cv.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    const p = local(e), n = nodeAt(p.x, p.y);
    cv.setPointerCapture(e.pointerId);
    if (n) { drag = { node: n, moved: false, sx: p.x, sy: p.y }; }
    else pan = { sx: p.x, sy: p.y, vx: view.x, vy: view.y, moved: false };
  });
  cv.addEventListener("pointermove", (e) => {
    const p = local(e);
    if (drag) {
      if (!drag.moved && Math.abs(p.x - drag.sx) + Math.abs(p.y - drag.sy) > 3) {
        drag.moved = true; drag.node.fx = drag.node.x; drag.node.fy = drag.node.y; sim.alphaTarget(0.25).restart();
      }
      if (drag.moved) { const w = toWorld(p.x, p.y); drag.node.fx = w.x; drag.node.fy = w.y; }
      return;
    }
    if (pan) {
      if (Math.abs(p.x - pan.sx) + Math.abs(p.y - pan.sy) > 3) pan.moved = true;
      view.x = pan.vx + (p.x - pan.sx); view.y = pan.vy + (p.y - pan.sy); requestDraw(); return;
    }
    const h = nodeAt(p.x, p.y);
    if (h !== hover) { hover = h; cv.style.cursor = h ? "pointer" : "grab"; cv.title = h ? h.id : ""; requestDraw(); }
  });
  cv.addEventListener("pointerup", () => {
    if (drag) {
      const d = drag;
      drag = null;
      sim.alphaTarget(0);
      if (d.node.id !== rootId) { d.node.fx = null; d.node.fy = null; }
      if (!d.moved) {
        const now = performance.now();
        if (lastClick.id === d.node.id && now - lastClick.t < 400) { lastClick = { id: null, t: 0 }; ctx.onOpen(d.node); }
        else { lastClick = { id: d.node.id, t: now }; ctx.onSelect(d.node); }
      }
      requestDraw();
      return;
    }
    if (pan && !pan.moved) ctx.onSelect(null);
    pan = null;
  });
  cv.addEventListener("pointerleave", () => { if (hover && !drag) { hover = null; requestDraw(); } });
  cv.addEventListener("wheel", (e) => {
    e.preventDefault();
    const p = local(e), k = Math.max(0.05, Math.min(5, view.k * Math.exp(-e.deltaY * 0.0015)));
    view.x = p.x - ((p.x - view.x) / view.k) * k;
    view.y = p.y - ((p.y - view.y) / view.k) * k;
    view.k = k;
    requestDraw();
  }, { passive: false });
  cv.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    const p = local(e);
    ctx.menu(e.clientX, e.clientY, nodeAt(p.x, p.y));
  });

  function fit() {
    if (!nodes.length) return;
    const W = cv.clientWidth, H = cv.clientHeight;
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (const n of nodes) { minX = Math.min(minX, n.x); maxX = Math.max(maxX, n.x); minY = Math.min(minY, n.y); maxY = Math.max(maxY, n.y); }
    const k = Math.max(0.05, Math.min(1.15, Math.min((W - 100) / Math.max(1, maxX - minX), (H - 130) / Math.max(1, maxY - minY))));
    view.k = k;
    view.x = W / 2 - ((minX + maxX) / 2) * k;
    view.y = H / 2 - ((minY + maxY) / 2) * k + 10;
    requestDraw();
  }
  new ResizeObserver(() => requestDraw()).observe(cv);

  return {
    show() {
      open = true;
      const W = cv.clientWidth, H = cv.clientHeight;
      if (!nodes.length) view = { k: 1, x: W / 2, y: H / 2 };
      requestDraw();
    },
    hide() { open = false; if (sim) sim.stop(); },
    setData(g, showFiles) { hover = null; setData(g, showFiles); },
    setOpts(o) { Object.assign(opts, o); requestDraw(); },
    /** Forget positions when jumping to an unrelated folder. */
    resetLayout() { remembered.clear(); nodes = []; links = []; },
    fit,
    redraw: requestDraw,
    counts: () => ({ nodes: nodes.length, links: links.length }),
    kinds: () => [...new Set(nodes.map((n) => (n.id === rootId ? null : n.kind)).filter(Boolean))],
    isOpen: () => open,
  };
}
