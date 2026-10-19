import { useSignalEffect } from "@preact/signals";
import { useEffect, useRef, useState } from "preact/hooks";
import { comp, fps, mediaById, playhead, project, rev, run, scrollX, selection, setPlayhead, snapping, tool, zoom } from "../store";
import type { Comp, Item, Track } from "../types";
import { clamp, compDuration, fmtNum, fmtShort, isAnimated, itemLabel, kindColor, keyTimes, LABEL_COLORS, speedOf, staticVal } from "../util";

const RULER = 26;
const VH = 58;
const AH = 46;
const EDGE = 7;

interface Row {
  track: Track;
  y: number;
  h: number;
}

function layoutRows(c: Comp | null): Row[] {
  if (!c) return [];
  const v = c.tracks.filter((t) => t.kind === "video").sort((a, b) => parseInt(b.id.slice(1)) - parseInt(a.id.slice(1)));
  const a = c.tracks.filter((t) => t.kind === "audio").sort((x, y) => parseInt(x.id.slice(1)) - parseInt(y.id.slice(1)));
  const rows: Row[] = [];
  let y = 0;
  for (const t of v) {
    rows.push({ track: t, y, h: VH });
    y += VH;
  }
  y += 6;
  for (const t of a) {
    rows.push({ track: t, y, h: AH });
    y += AH;
  }
  return rows;
}

// ---- image caches (thumbnails / waveforms), redraw when loaded
const imgCache = new Map<string, HTMLImageElement | null>();
/** The last waveform drawn for a clip's source range, shown while another zoom level loads. */
const waveLast = new Map<string, HTMLImageElement>();
let requestRedraw: () => void = () => {};
function img(url: string): HTMLImageElement | null {
  if (imgCache.has(url)) {
    const i = imgCache.get(url)!;
    return i && i.complete && i.naturalWidth > 0 ? i : null;
  }
  if (imgCache.size > 1500) imgCache.clear(); // zooming makes new sizes; keep memory bounded
  const i = new Image();
  i.onload = () => requestRedraw();
  i.onerror = () => imgCache.set(url, null);
  i.src = url;
  imgCache.set(url, i);
  return null;
}

type Drag =
  | { mode: "scrub" }
  | { mode: "work"; t0: number; t1: number }
  | { mode: "move"; ids: string[]; x0: number; y0: number; dt: number; row: Row | null; moved: boolean; base: Item }
  /** edge drags: plain trim, ripple trim (B) or rate stretch (R) */
  | { mode: "trimL" | "trimR"; id: string; x0: number; dt: number; base: Item; how: "trim" | "ripple" | "rate" }
  /** rolling edit (N): the cut between `id` and `next` */
  | { mode: "roll"; id: string; next: Item; x0: number; dt: number; base: Item }
  | { mode: "slip" | "slide"; id: string; x0: number; dt: number; base: Item }
  | { mode: "hand"; x0: number; y0: number; sx: number; sy: number }
  | null;

const fx4 = (v: number) => fmtNum(v, 4);

/** The playhead is a positioned element over the canvas: moving it never redraws the clips. */
function PlayheadLine() {
  const x = Math.round(playhead.value * zoom.value - scrollX.value);
  if (x < -8) return null;
  return <div class="tl-playhead" style={{ transform: `translateX(${x}px)` }} />;
}
const signed = (v: number) => (v >= 0 ? "+" : "-") + fmtNum(Math.abs(v), 4);

export function Timeline() {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState({ w: 800, h: 300 });
  const [scrollY, setScrollY] = useState(0);
  const [hover, setHover] = useState<{ x: number; y: number } | null>(null);
  const [dropOver, setDropOver] = useState(false);
  const drag = useRef<Drag>(null);
  const [, force] = useState(0);
  const c = comp.value;
  const rows = layoutRows(c);
  // subscribe to state
  void rev.value;
  void zoom.value;
  void scrollX.value;
  void selection.value;
  void project.value;

  useEffect(() => {
    const el = wrapRef.current!;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    let raf = 0;
    requestRedraw = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => force((n) => n + 1));
    };
  }, []);

  const tToX = (t: number) => t * zoom.value - scrollX.value;
  const xToT = (x: number) => (x + scrollX.value) / zoom.value;

  function rowAt(y: number): Row | null {
    const yy = y - RULER + scrollY;
    return rows.find((r) => yy >= r.y && yy < r.y + r.h) || null;
  }

  function itemAt(x: number, y: number): { row: Row; item: Item; edge: "L" | "R" | null } | null {
    const r = rowAt(y);
    if (!r) return null;
    const t = xToT(x);
    for (const it of r.track.items) {
      if (t >= it.start && t <= it.start + it.dur) {
        const xl = tToX(it.start);
        const xr = tToX(it.start + it.dur);
        const edge = x - xl < EDGE && xr - xl > EDGE * 3 ? "L" : xr - x < EDGE && xr - xl > EDGE * 3 ? "R" : null;
        return { row: r, item: it, edge };
      }
    }
    return null;
  }

  function snapCandidates(exclude: Set<string>): number[] {
    const out = [0, playhead.value];
    if (!c) return out;
    for (const t of c.tracks) for (const i of t.items) if (!exclude.has(i.id)) out.push(i.start, i.start + i.dur);
    for (const m of c.markers || []) out.push(m.t);
    return out;
  }

  function snap(t: number, cands: number[]): number {
    if (!snapping.value) return t;
    const th = 8 / zoom.value;
    let best = t;
    let bd = th;
    for (const s of cands) {
      const d = Math.abs(s - t);
      if (d < bd) {
        bd = d;
        best = s;
      }
    }
    return best;
  }

  // ------------------------------------------------------------ drawing
  useEffect(() => {
    const cv = canvasRef.current;
    if (!cv || !c) return;
    const dpr = window.devicePixelRatio || 1;
    const W = size.w;
    const H = size.h;
    if (cv.width !== Math.round(W * dpr) || cv.height !== Math.round(H * dpr)) {
      cv.width = Math.round(W * dpr);
      cv.height = Math.round(H * dpr);
      cv.style.width = W + "px";
      cv.style.height = H + "px";
    }
    const g = cv.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, W, H);
    g.fillStyle = "#16161a";
    g.fillRect(0, 0, W, H);
    const z = zoom.value;
    const media = mediaById.value;
    const sel = new Set(selection.value);
    const dur = compDuration(c);
    // track lanes
    for (const r of rows) {
      const y = RULER + r.y - scrollY;
      if (y > H || y + r.h < RULER) continue;
      g.fillStyle = r.track.kind === "video" ? "#1c1c22" : "#1a1f1d";
      g.fillRect(0, y, W, r.h - 1);
      g.fillStyle = "#24242b";
      g.fillRect(0, y + r.h - 1, W, 1);
    }
    // comp end shade
    const xe = tToX(dur);
    if (xe < W) {
      g.fillStyle = "rgba(0,0,0,0.25)";
      g.fillRect(Math.max(0, xe), RULER, W - xe, H);
    }
    // items
    const d = drag.current;
    for (const r of rows) {
      const y0 = RULER + r.y - scrollY;
      if (y0 > H || y0 + r.h < RULER) continue;
      const items = r.track.items;
      for (let i = 0; i < items.length; i++) {
        const it = items[i];
        let start = it.start;
        let len = it.dur;
        let yy = y0;
        let ghost = false;
        if (d && d.mode === "move" && d.ids.includes(it.id) && d.moved) {
          start = it.start + d.dt;
          if (d.row && d.ids.length === 1) yy = RULER + d.row.y - scrollY;
          ghost = true;
        } else if (d && (d.mode === "trimL" || d.mode === "trimR") && d.id === it.id) {
          if (d.mode === "trimL") {
            start = d.how === "ripple" ? it.start : it.start + d.dt;
            len = it.dur - d.dt;
          } else len = it.dur + d.dt;
        } else if (d && d.mode === "roll" && (d.id === it.id || d.next.id === it.id)) {
          if (d.id === it.id) len = it.dur + d.dt;
          else {
            start = it.start + d.dt;
            len = it.dur - d.dt;
          }
        } else if (d && d.mode === "slide" && d.id === it.id) {
          start = it.start + d.dt;
          ghost = true;
        }
        const x = tToX(start);
        const w = Math.max(2, len * z);
        if (x > W || x + w < 0) continue;
        drawItem(g, it, r, x, yy + 2, w, r.h - 5, sel.has(it.id), ghost, media);
        // transition to next
        const nx = items[i + 1];
        if (it.tout) {
          const adj = nx && Math.abs(nx.start - (it.start + it.dur)) < 0.5 / c.fps;
          const td = it.tout.dur * z;
          if (adj) {
            const cx = tToX(it.start + it.dur);
            drawTransition(g, cx - td / 2, y0 + 2, td, r.h - 5, it.tout.kind);
          } else {
            drawFade(g, x + w - td, y0 + 2, td, r.h - 5, false);
          }
        }
        if (it.tin) {
          const pv = items[i - 1];
          const adj = pv && pv.tout && Math.abs(it.start - (pv.start + pv.dur)) < 0.5 / c.fps;
          if (!adj) drawFade(g, x, y0 + 2, it.tin.dur * z, r.h - 5, true);
        }
      }
    }
    // ruler
    g.fillStyle = "#18181c";
    g.fillRect(0, 0, W, RULER);
    g.fillStyle = "#2a2a33";
    g.fillRect(0, RULER - 1, W, 1);
    const steps = [1 / c.fps, 0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600];
    const major = steps.find((s) => s * z >= 80) || 600;
    const minor = steps.slice().reverse().find((s) => s < major && s * z >= 8) || major;
    const t0 = Math.floor(xToT(0) / minor) * minor;
    g.font = "10px Inter, system-ui";
    for (let t = t0; tToX(t) < W; t += minor) {
      const x = Math.round(tToX(t)) + 0.5;
      const isMajor = Math.abs(t / major - Math.round(t / major)) < 1e-6;
      g.strokeStyle = isMajor ? "#55556a" : "#33333f";
      g.beginPath();
      g.moveTo(x, isMajor ? 10 : 17);
      g.lineTo(x, RULER - 1);
      g.stroke();
      if (isMajor) {
        g.fillStyle = "#8a8a9a";
        g.fillText(fmtShort(Math.max(0, t)), x + 3, 11);
      }
    }
    // work area (sequence in/out)
    const wa = d && d.mode === "work" ? [Math.min(d.t0, d.t1), Math.max(d.t0, d.t1)] : c.work;
    if (wa) {
      const [xa, xb] = [tToX(wa[0]), tToX(wa[1])];
      g.fillStyle = "rgba(139,124,246,0.55)";
      g.fillRect(xa, RULER - 6, Math.max(2, xb - xa), 5);
      g.fillStyle = "rgba(139,124,246,0.06)";
      g.fillRect(xa, RULER, Math.max(2, xb - xa), H);
    }
    for (const m of c.markers || []) {
      const x = tToX(m.t);
      if (m.dur && m.dur > 0) {
        g.fillStyle = (m.color || "#fbbf24") + "88";
        g.fillRect(x, RULER - 4, m.dur * z, 3);
      }
      if (x < -20 || x > W) continue;
      g.fillStyle = m.color || "#fbbf24";
      g.beginPath();
      g.moveTo(x - 5, RULER - 9);
      g.lineTo(x + 5, RULER - 9);
      g.lineTo(x, RULER - 2);
      g.fill();
      if (m.name && z > 20) {
        g.fillStyle = "#c9a33a";
        g.fillText(m.name, x + 6, RULER - 3);
      }
    }
    // razor hover line
    if (tool.value === "razor" && hover) {
      g.strokeStyle = "#fbbf24";
      g.setLineDash([4, 3]);
      g.beginPath();
      g.moveTo(hover.x + 0.5, RULER);
      g.lineTo(hover.x + 0.5, H);
      g.stroke();
      g.setLineDash([]);
    }
  });

  function drawItem(g: CanvasRenderingContext2D, it: Item, r: Row, x: number, y: number, w: number, h: number, selected: boolean, ghost: boolean, media: Map<string, any>) {
    const m = it.src ? media.get(it.src) : undefined;
    const tag = staticVal(it.props?.label);
    const base = typeof tag === "string" && LABEL_COLORS[tag] ? LABEL_COLORS[tag] : it.kind === "solid" && typeof it.props?.color === "string" ? (it.props.color as string).slice(0, 7) : kindColor(it.kind, r.track.kind);
    g.save();
    g.globalAlpha = it.off ? 0.35 : ghost ? 0.75 : 1;
    g.beginPath();
    g.roundRect(x, y, w, h, 5);
    g.fillStyle = base;
    g.fill();
    g.clip();
    // thumbnails / waveform
    if (m && r.track.kind === "video" && (m.kind === "video" || m.kind === "image" || m.kind === "svg") && w > 30) {
      const tw = Math.max(40, Math.round(((h - 16) * m.w) / Math.max(1, m.h)));
      const sp = speedOf(it);
      for (let tx = x; tx < x + w; tx += tw) {
        if (tx + tw < 0 || tx > g.canvas.width) continue;
        const local = (tx - x) / zoom.value;
        let st = (it.in || 0) + local * sp;
        st = m.kind === "video" ? Math.round(Math.min(st, Math.max(0, m.dur - 0.1)) * 2) / 2 : 0;
        const im = img(`/api/thumb?media=${m.id}&t=${st}&w=96`);
        if (im) g.drawImage(im, tx, y + 14, tw, h - 14);
      }
      g.fillStyle = "rgba(0,0,0,0.25)";
      g.fillRect(x, y + 14, w, h - 14);
    } else if (m && m.has_audio && r.track.kind === "audio" && w > 10 && m.dur > 0) {
      // just this clip's part of the file, at the width it is drawn (rounded to a power of
      // two so zooming reuses images); the server draws it from the file's peak data
      const sp = speedOf(it);
      const from = it.in || 0;
      const to = Math.min(m.dur, from + it.dur * sp);
      const shown = ((to - from) / sp) * zoom.value;
      const qw = Math.min(4096, 2 ** Math.ceil(Math.log2(Math.max(64, shown))));
      const range = `${m.id}|${from.toFixed(3)}|${to.toFixed(3)}`;
      let im = img(`/api/waveform?media=${m.id}&w=${qw}&h=64&from=${from.toFixed(3)}&to=${to.toFixed(3)}`);
      if (im) waveLast.set(range, im);
      else im = waveLast.get(range) || null;
      if (im && to > from) {
        g.globalAlpha *= 0.9;
        g.drawImage(im, x, y + 12, shown, h - 12);
      }
    }
    // label
    g.fillStyle = "rgba(0,0,0,0.35)";
    g.fillRect(x, y, w, 14);
    g.fillStyle = "#fff";
    g.font = "600 10.5px Inter, system-ui";
    const label = itemLabel(it, it.kind === "comp" ? project.value?.comps.find((c) => c.id === it.src)?.name : m?.name);
    const sp = speedOf(it);
    const extra = (sp !== 1 ? ` ${sp}x` : "") + (it.props?.reverse ? " ⟲" : "") + ((it.fx || []).length ? " ƒ" : "");
    g.fillText(label + extra, x + 5, y + 10.5, Math.max(10, w - 8));
    // keyframes of the selected item
    if (selected) {
      const kt = new Set<number>();
      for (const a of Object.values(it.props || {})) if (isAnimated(a)) keyTimes(a).forEach((t) => kt.add(t));
      for (const e of it.fx || []) for (const a of Object.values(e.props || {})) keyTimes(a).forEach((t) => kt.add(t));
      g.fillStyle = "#ffd60a";
      for (const t of kt) {
        if (t < 0 || t > it.dur) continue;
        const kx = x + t * zoom.value;
        g.beginPath();
        g.moveTo(kx, y + h - 9);
        g.lineTo(kx + 4, y + h - 5);
        g.lineTo(kx, y + h - 1);
        g.lineTo(kx - 4, y + h - 5);
        g.fill();
      }
    }
    g.restore();
    g.beginPath();
    g.roundRect(x + 0.5, y + 0.5, w - 1, h - 1, 5);
    g.strokeStyle = selected ? "#fff" : "rgba(0,0,0,0.5)";
    g.lineWidth = selected ? 2 : 1;
    g.stroke();
    g.lineWidth = 1;
  }

  function drawTransition(g: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, kind: string) {
    g.save();
    g.beginPath();
    g.roundRect(x, y + h * 0.35, w, h * 0.5, 4);
    g.fillStyle = "rgba(255,255,255,0.85)";
    g.fill();
    g.clip();
    g.strokeStyle = "rgba(139,124,246,0.6)";
    for (let i = -h; i < w; i += 6) {
      g.beginPath();
      g.moveTo(x + i, y + h);
      g.lineTo(x + i + h, y);
      g.stroke();
    }
    g.fillStyle = "#222";
    g.font = "600 9.5px Inter, system-ui";
    if (w > 30) g.fillText(kind, x + 4, y + h * 0.66, w - 6);
    g.restore();
  }

  function drawFade(g: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, isIn: boolean) {
    g.save();
    g.fillStyle = "rgba(0,0,0,0.45)";
    g.beginPath();
    if (isIn) {
      g.moveTo(x, y);
      g.lineTo(x + w, y);
      g.lineTo(x, y + h);
    } else {
      g.moveTo(x, y);
      g.lineTo(x + w, y);
      g.lineTo(x + w, y + h);
    }
    g.fill();
    g.restore();
  }

  // ------------------------------------------------------------ interaction
  /** The clip right after / before `it` on its track (touching it). */
  function neighbour(r: Row, it: Item, after: boolean): Item | undefined {
    const f = 0.5 / fps.value;
    return r.track.items.find((o) => o.id !== it.id && (after ? Math.abs(o.start - (it.start + it.dur)) < f : Math.abs(o.start + o.dur - it.start) < f));
  }

  function onDown(e: PointerEvent) {
    if (!c) return;
    const rect = canvasRef.current!.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    const tl = tool.value;
    if (tl === "hand" || e.button === 1) {
      drag.current = { mode: "hand", x0: x, y0: y, sx: scrollX.value, sy: scrollY };
      return;
    }
    if (tl === "zoom") {
      const t = xToT(x);
      const nz = clamp(zoom.value * (e.altKey || e.shiftKey ? 1 / 1.6 : 1.6), 2, 3000);
      zoom.value = nz;
      scrollX.value = Math.max(0, t * nz - x);
      return;
    }
    if (y < RULER) {
      if (e.shiftKey) {
        // Shift+drag on the ruler sets the work area
        const t = snap(xToT(x), snapCandidates(new Set()));
        drag.current = { mode: "work", t0: t, t1: t };
        return;
      }
      drag.current = { mode: "scrub" };
      setPlayhead(xToT(x));
      return;
    }
    const hit = itemAt(x, y);
    if (!hit) {
      if (!e.shiftKey) selection.value = [];
      setPlayhead(xToT(x));
      drag.current = { mode: "scrub" };
      return;
    }
    if (tl === "razor") {
      const t = snap(xToT(x), [playhead.value]);
      run(e.shiftKey ? `cut all at=${t.toFixed(3)}` : `cut ${hit.item.id} at=${t.toFixed(3)}`);
      return;
    }
    if (hit.row.track.locked) return;
    if (tl === "track") {
      // track select forward: everything from here on (Shift: this track only)
      const t = hit.item.start - 1e-4;
      const tracks = e.shiftKey ? [hit.row.track] : c.tracks;
      selection.value = tracks.flatMap((tr) => tr.items.filter((i) => i.start >= t).map((i) => i.id));
      drag.current = { mode: "move", ids: selection.value.slice(), x0: x, y0: y, dt: 0, row: null, moved: false, base: hit.item };
      return;
    }
    if (e.shiftKey || e.ctrlKey || e.metaKey) {
      const s = new Set(selection.value);
      s.has(hit.item.id) ? s.delete(hit.item.id) : s.add(hit.item.id);
      selection.value = [...s];
    } else if (!selection.value.includes(hit.item.id)) {
      selection.value = [hit.item.id];
    } else {
      // keep the multi-selection, but make this item the primary (last)
      selection.value = [...selection.value.filter((s) => s !== hit.item.id), hit.item.id];
    }
    if (tl === "roll" && hit.edge) {
      const left = hit.edge === "R" ? hit.item : neighbour(hit.row, hit.item, false);
      const right = hit.edge === "R" ? neighbour(hit.row, hit.item, true) : hit.item;
      if (left && right) {
        drag.current = { mode: "roll", id: left.id, next: right, x0: x, dt: 0, base: left };
        return;
      }
    }
    if ((tl === "slip" || tl === "slide") && !hit.edge) {
      drag.current = { mode: tl, id: hit.item.id, x0: x, dt: 0, base: hit.item };
      return;
    }
    const how = tl === "ripple" ? "ripple" : tl === "rate" ? "rate" : "trim";
    if (hit.edge === "L") drag.current = { mode: "trimL", id: hit.item.id, x0: x, dt: 0, base: hit.item, how };
    else if (hit.edge === "R") drag.current = { mode: "trimR", id: hit.item.id, x0: x, dt: 0, base: hit.item, how };
    else drag.current = { mode: "move", ids: selection.value.slice(), x0: x, y0: y, dt: 0, row: hit.row, moved: false, base: hit.item };
  }

  function cursorFor(x: number, y: number): string {
    const tl = tool.value;
    if (tl === "hand") return "grab";
    if (tl === "zoom") return "zoom-in";
    if (tl === "razor") return "crosshair";
    if (y < RULER) return "col-resize";
    const hit = itemAt(x, y);
    if (!hit) return "default";
    if (tl === "roll" && hit.edge) return "col-resize";
    if (hit.edge) return tl === "rate" ? "ew-resize" : tl === "ripple" ? "w-resize" : "ew-resize";
    if (tl === "slip" || tl === "slide") return "move";
    if (tl === "track") return "e-resize";
    return "grab";
  }

  function onMove(e: PointerEvent) {
    const rect = canvasRef.current!.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    setHover({ x, y });
    const d = drag.current;
    const cv = canvasRef.current!;
    if (!d) {
      cv.style.cursor = cursorFor(x, y);
      if (tool.value === "razor") force((n) => n + 1);
      return;
    }
    if (d.mode === "scrub") {
      setPlayhead(Math.max(0, xToT(x)));
      return;
    }
    if (d.mode === "work") {
      d.t1 = Math.max(0, snap(xToT(x), snapCandidates(new Set())));
      force((n) => n + 1);
      return;
    }
    if (d.mode === "hand") {
      scrollX.value = Math.max(0, d.sx - (x - d.x0));
      const total = rows.length ? rows[rows.length - 1].y + rows[rows.length - 1].h : 0;
      setScrollY(clamp(d.sy - (y - d.y0), 0, Math.max(0, total - (size.h - RULER) + 20)));
      return;
    }
    if (d.mode === "move") {
      let dt = (x - d.x0) / zoom.value;
      if (Math.abs(x - d.x0) > 3 || Math.abs(y - d.y0) > 3) d.moved = true;
      const ex = new Set(d.ids);
      const cands = snapCandidates(ex);
      const ns = snap(d.base.start + dt, cands);
      const ne = snap(d.base.start + d.base.dur + dt, cands);
      if (ns !== d.base.start + dt) dt = ns - d.base.start;
      else if (ne !== d.base.start + d.base.dur + dt) dt = ne - d.base.start - d.base.dur;
      d.dt = Math.max(-d.base.start, dt);
      const r = rowAt(y);
      const srcRow = rows.find((rr) => rr.track.items.some((i) => i.id === d.base.id));
      if (r && srcRow && r.track.kind === srcRow.track.kind) d.row = r;
      cv.style.cursor = "grabbing";
      force((n) => n + 1);
      return;
    }
    const raw = (x - d.x0) / zoom.value;
    const frame = 1 / fps.value;
    if (d.mode === "trimL" || d.mode === "trimR") {
      let dt = raw;
      const cands = snapCandidates(new Set([d.id]));
      if (d.mode === "trimL") {
        const ns = snap(d.base.start + dt, cands);
        dt = clamp(ns - d.base.start, -d.base.start, d.base.dur - frame);
      } else {
        const ne = snap(d.base.start + d.base.dur + dt, cands);
        dt = Math.max(ne - d.base.start - d.base.dur, -(d.base.dur - frame));
      }
      d.dt = dt;
    } else if (d.mode === "roll") {
      const cut = snap(d.base.start + d.base.dur + raw, snapCandidates(new Set([d.id, d.next.id])));
      d.dt = clamp(cut - d.base.start - d.base.dur, -(d.base.dur - frame), d.next.dur - frame);
    } else {
      d.dt = raw;
    }
    force((n) => n + 1);
  }

  async function onUp() {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (d.mode === "work") {
      const [a, b] = [Math.min(d.t0, d.t1), Math.max(d.t0, d.t1)];
      if (b - a > 1 / fps.value) await run(`workarea ${fx4(a)} ${fx4(b)}`, true);
      force((n) => n + 1);
      return;
    }
    if (d.mode === "move" && d.moved) {
      const dt = d.dt;
      const ids = d.ids;
      const srcRow = rows.find((rr) => rr.track.items.some((i) => i.id === d.base.id));
      const trackChange = d.row && srcRow && d.row.track.id !== srcRow.track.id && ids.length === 1;
      if (Math.abs(dt) < 1e-4 && !trackChange) {
        force((n) => n + 1);
        return;
      }
      if (ids.length === 1) await run(`move ${ids[0]} at=${(d.base.start + dt).toFixed(4)}${trackChange ? ` track=${d.row!.track.id}` : ""}`, true);
      else await run(`move ${ids.join(",")} by=${signed(dt)}`, true);
    } else if ((d.mode === "trimL" || d.mode === "trimR") && Math.abs(d.dt) > 1e-4) {
      const b = d.base;
      if (d.how === "rate") {
        // rate stretch: same media, new duration (speed changes)
        const nd = d.mode === "trimR" ? b.dur + d.dt : b.dur - d.dt;
        const ns = b.start + d.dt;
        if (d.mode === "trimR") await run(`fit ${d.id} dur=${fx4(nd)}`, true);
        else if (d.dt > 0) await run(`fit ${d.id} dur=${fx4(nd)}\nmove ${d.id} at=${fx4(ns)}`, true);
        else await run(`move ${d.id} at=${fx4(ns)}\nfit ${d.id} dur=${fx4(nd)}`, true);
      } else {
        const ripple = d.how === "ripple" ? " ripple" : "";
        if (d.mode === "trimL") await run(`trim ${d.id} start=${fx4(b.start + d.dt)}${ripple}`, true);
        else await run(`trim ${d.id} end=${fx4(b.start + b.dur + d.dt)}${ripple}`, true);
      }
    } else if (d.mode === "roll" && Math.abs(d.dt) > 1e-4) {
      await run(`roll ${d.id} to=${fx4(d.base.start + d.base.dur + d.dt)}`, true);
    } else if (d.mode === "slip" && Math.abs(d.dt) > 1e-4) {
      // dragging right shows earlier frames, like Premiere
      await run(`slip ${d.id} ${signed(-d.dt * speedOf(d.base))}`, true);
    } else if (d.mode === "slide" && Math.abs(d.dt) > 1e-4) {
      await run(`slide ${d.id} ${signed(d.dt)}`, true);
    }
    force((n) => n + 1);
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const rect = canvasRef.current!.getBoundingClientRect();
    const x = e.clientX - rect.left;
    if (e.ctrlKey || e.metaKey) {
      const t = xToT(x);
      const nz = clamp(zoom.value * (e.deltaY < 0 ? 1.15 : 1 / 1.15), 2, 3000);
      zoom.value = nz;
      scrollX.value = Math.max(0, t * nz - x);
    } else if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
      scrollX.value = Math.max(0, scrollX.value + (e.deltaX || e.deltaY));
    } else {
      const total = rows.length ? rows[rows.length - 1].y + rows[rows.length - 1].h : 0;
      setScrollY(clamp(scrollY + e.deltaY, 0, Math.max(0, total - (size.h - RULER) + 20)));
    }
  }

  function onDrop(e: DragEvent) {
    e.preventDefault();
    setDropOver(false);
    const mid = e.dataTransfer?.getData("text/moodcut-media");
    if (!mid || !c) return;
    const rect = canvasRef.current!.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;
    const t = Math.max(0, snap(xToT(x), snapCandidates(new Set())));
    const r = rowAt(y);
    const m = mediaById.value.get(mid);
    const wantAudio = m?.kind === "audio";
    const track = r && (r.track.kind === "audio") === wantAudio ? ` ${r.track.id}` : "";
    run(`add ${mid}${track} at=${t.toFixed(3)}`);
  }

  // follow the playhead while it moves past the right edge (without re-rendering every frame)
  const width = useRef(size.w);
  width.current = size.w;
  useSignalEffect(() => {
    const t = playhead.value;
    const z = zoom.peek();
    const x = t * z - scrollX.peek();
    if (x > width.current - 40) scrollX.value = Math.max(0, t * z - width.current * 0.2);
    else if (x < 0) scrollX.value = Math.max(0, t * z - 40);
  });

  const heads = rows.map((r) => {
    const t = r.track;
    const top = RULER + r.y - scrollY;
    return (
      <div class="tl-head" style={{ top: top + "px", height: r.h + "px" }} key={t.id}>
        <b style={{ color: t.kind === "video" ? "#8fb3ff" : "#7fe0b3" }}>{t.id}</b>
        {t.kind === "video" && (
          <span class={"tbtn" + (t.hidden ? " on" : "")} title="Hide track" onClick={() => run(`track set ${t.id} hidden=${t.hidden ? 0 : 1}`, true)}>
            {t.hidden ? "◌" : "◉"}
          </span>
        )}
        <span class={"tbtn" + (t.muted ? " on" : "")} title="Mute" onClick={() => run(`track set ${t.id} mute=${t.muted ? 0 : 1}`, true)}>
          M
        </span>
        {t.kind === "audio" && (
          <span class={"tbtn" + (t.solo ? " on" : "")} title="Solo" onClick={() => run(`track set ${t.id} solo=${t.solo ? 0 : 1}`, true)}>
            S
          </span>
        )}
        <span class={"tbtn" + (t.locked ? " on" : "")} title="Lock" onClick={() => run(`track set ${t.id} lock=${t.locked ? 0 : 1}`, true)}>
          {t.locked ? "🔒" : "🔓"}
        </span>
        <span style={{ color: "#6d6d7c", fontSize: "11px", marginLeft: "auto", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{t.name || (t.duck ? "ducked" : "")}</span>
      </div>
    );
  });

  return (
    <div class="tl-body">
      <div class="tl-heads">
        <div style={{ height: RULER + "px", borderBottom: "1px solid var(--line)", display: "flex", alignItems: "center", gap: "4px", padding: "0 6px" }}>
          <button class="btn sm ghost" title="Add video track" onClick={() => run("track add V", true)}>
            +V
          </button>
          <button class="btn sm ghost" title="Add audio track" onClick={() => run("track add A", true)}>
            +A
          </button>
        </div>
        {heads}
      </div>
      <div
        class="tl-canvas-wrap"
        ref={wrapRef}
        onDragOver={(e) => {
          e.preventDefault();
          setDropOver(true);
        }}
        onDragLeave={() => setDropOver(false)}
        onDrop={onDrop}
      >
        <canvas ref={canvasRef} onPointerDown={onDown} onPointerMove={onMove} onPointerUp={onUp} onPointerLeave={() => setHover(null)} onWheel={onWheel} onDblClick={(e) => {
          const rect = canvasRef.current!.getBoundingClientRect();
          const hit = itemAt(e.clientX - rect.left, e.clientY - rect.top);
          if (hit) setPlayhead(Math.max(playhead.value, hit.item.start));
        }} />
        <PlayheadLine />
        {dropOver && <div class="drop-hint">Drop to place on the timeline</div>}
        {!c?.tracks.some((t) => t.items.length) && (
          <div class="empty" style={{ position: "absolute", left: 0, right: 0, top: "40%", pointerEvents: "none" }}>
            Drag media here, double-click it in the Project panel, or let your AI assistant edit via MCP.
          </div>
        )}
      </div>
    </div>
  );
}
