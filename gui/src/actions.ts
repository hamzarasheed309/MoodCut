// Editor actions shared by panels and keyboard shortcuts. Every edit is a MoodCut Script
// line sent through `run`, the same path the AI uses over MCP.
import { clipboard, comp, findItem, fps, mediaById, monitorTab, playhead, playing, project_, projectPath, run, scrollX, selection, setPlayhead, showRender, snapping, sourceIn, sourceMedia, sourceOut, sourceT, toast, tool, zoom } from "./store";
import type { Tool } from "./store";
import { focusedPanel, groupOf, maximized, shown } from "./dock";
import type { Comp, Item } from "./types";
import { compDuration, editPoints, fmtNum, keyTimes } from "./util";

const T = (t: number) => fmtNum(t, 4);
const EPS = 1e-4;

export function splitAtPlayhead() {
  const t = T(playhead.value);
  const sel = selection.value;
  run(sel.length ? `cut ${sel.join(",")} at=${t}` : `cut all at=${t}`);
}

export function fitTimeline() {
  const c = comp.value;
  if (!c) return;
  const w = (document.querySelector(".tl-canvas-wrap") as HTMLElement)?.clientWidth || 800;
  zoom.value = Math.max(2, (w - 40) / Math.max(1, compDuration(c)));
  scrollX.value = 0;
}

/** Sets params on the item's effect of `kind`, adding the effect first if it isn't there. */
export function setFx(it: Item, kind: string, kv: Record<string, string>) {
  const e = (it.fx || []).find((f) => f.kind === kind);
  const args = Object.entries(kv)
    .map(([k, v]) => `${k}=${v}`)
    .join(" ");
  return run(e ? `set ${it.id}.${e.id} ${args}` : `fx ${it.id} ${kind} ${args}`, true);
}

/** The clip under the playhead: a selected one first, else the topmost video clip, else audio. */
export function clipAtPlayhead(c: Comp): Item | null {
  const t = playhead.value;
  const inside = (i: Item) => t >= i.start - EPS && t <= i.start + i.dur + EPS;
  for (const id of selection.value.slice().reverse()) {
    const f = findItem(id);
    if (f && inside(f.item)) return f.item;
  }
  const order = c.tracks.slice().sort((a, b) => (a.kind === b.kind ? parseInt(b.id.slice(1)) - parseInt(a.id.slice(1)) : a.kind === "video" ? -1 : 1));
  for (const tr of order) {
    if (tr.locked) continue;
    const it = tr.items.find(inside);
    if (it) return it;
  }
  return null;
}

/** Targets for clip commands: the selection, else the clip under the playhead. */
function targets(): string[] {
  if (selection.value.length) return selection.value;
  const c = comp.value;
  const it = c && clipAtPlayhead(c);
  return it ? [it.id] : [];
}

function need(): string[] | null {
  const t = targets();
  if (!t.length) toast("Select a clip first", true);
  return t.length ? t : null;
}

// ---------------------------------------------------------------- source monitor

/** `m1[2-5.5]` for the source monitor's media and in/out marks. */
export function sourceRange(): string | null {
  const id = sourceMedia.value;
  const m = id ? mediaById.value.get(id) : undefined;
  if (!m) return null;
  const a = sourceIn.value ?? 0;
  const b = sourceOut.value ?? m.dur;
  return m.dur > 0 && (a > EPS || b < m.dur - EPS) ? `${m.id}[${T(a)}-${T(Math.max(a + 1 / fps.value, b))}]` : m.id;
}

/** Insert (ripples) or overwrite the source range at the playhead, like Premiere's , and . keys. */
export async function placeSource(mode: "insert" | "overwrite") {
  const r = sourceRange();
  if (!r) return toast("Open a clip in the Source monitor first (click it in the Project panel)", true);
  // target track: the selected clip's track when it is the right kind, else V1 / A1
  const kind = mediaById.value.get(sourceMedia.value!)?.kind === "audio" ? "audio" : "video";
  const sel = selection.value.map((id) => findItem(id)?.track).find((t) => t?.kind === kind);
  const res = await run(`add ${r} ${sel ? sel.id : kind === "audio" ? "A1" : "V1"} at=${T(playhead.value)} mode=${mode}`);
  if (res.ok) monitorTab.value = "program";
}

// ---------------------------------------------------------------- work area

/** Sets the work area in (I) or out (O) point at the playhead, keeping the other end. */
export function markWork(edge: "in" | "out") {
  const c = comp.value;
  if (!c) return;
  const t = playhead.value;
  const end = Math.max(compDuration(c), t + 1 / c.fps);
  let [a, b] = c.work || [0, end];
  if (edge === "in") {
    a = t;
    if (b <= a + EPS) b = end;
  } else {
    b = t;
    if (b <= a + EPS) a = 0;
  }
  if (b > a + EPS) run(`workarea ${T(a)} ${T(b)}`, true);
}

export function liftOrExtract(verb: "lift" | "extract") {
  if (!comp.value?.work) return toast(`Set a work area first (I / O, or Shift+drag on the ruler) to ${verb}`, true);
  run(verb);
}

// ---------------------------------------------------------------- navigation

export function gotoEdit(dir: 1 | -1) {
  const c = comp.value;
  if (!c) return;
  const pts = editPoints(c);
  const t = playhead.value;
  const n = dir > 0 ? pts.find((p) => p > t + EPS) : pts.slice().reverse().find((p) => p < t - EPS);
  if (n !== undefined) setPlayhead(n);
}

export function gotoMarker(dir: 1 | -1) {
  const ms = (comp.value?.markers || []).map((m) => m.t);
  const t = playhead.value;
  const n = dir > 0 ? ms.find((p) => p > t + EPS) : ms.slice().reverse().find((p) => p < t - EPS);
  if (n !== undefined) setPlayhead(n);
}

/** Keyframe times (comp time) of the selected items, for J / K and the keyframe navigator. */
export function selectedKeyTimes(prop?: string): number[] {
  const s = new Set<number>();
  for (const id of selection.value) {
    const f = findItem(id);
    if (!f) continue;
    const it = f.item;
    const add = (a: any) => keyTimes(a).forEach((k) => s.add(+(it.start + k).toFixed(4)));
    if (prop) add(it.props?.[prop]);
    else {
      Object.values(it.props || {}).forEach(add);
      for (const e of it.fx || []) Object.values(e.props || {}).forEach(add);
    }
  }
  return [...s].sort((a, b) => a - b);
}

export function gotoKey(dir: 1 | -1, prop?: string) {
  const ks = selectedKeyTimes(prop);
  const t = playhead.value;
  const n = dir > 0 ? ks.find((p) => p > t + EPS) : ks.slice().reverse().find((p) => p < t - EPS);
  if (n !== undefined) setPlayhead(n);
}

// ---------------------------------------------------------------- clip edits

/** Q / W: ripple trim the clip under the playhead to it (start / end). */
function rippleTrimTo(edge: "start" | "end") {
  const c = comp.value;
  const it = c && clipAtPlayhead(c);
  if (!it) return;
  const t = playhead.value;
  if (t <= it.start + EPS || t >= it.start + it.dur - EPS) return;
  run(`trim ${it.id} ${edge}=${T(t)} ripple`, true).then(() => edge === "start" && setPlayhead(it.start));
}

function moveToTrack(dir: 1 | -1) {
  const lines: string[] = [];
  for (const id of selection.value) {
    const f = findItem(id);
    if (!f) continue;
    const n = parseInt(f.track.id.slice(1)) + (f.track.kind === "video" ? dir : -dir);
    if (n >= 1) lines.push(`move ${id} track=${f.track.id[0]}${n}`);
  }
  if (lines.length) run(lines.join("\n"), true);
}

export function toggleEnabled() {
  const ids = need();
  if (!ids) return;
  const off = !findItem(ids[ids.length - 1])?.item.off;
  run(ids.map((id) => `set ${id} off=${off ? 1 : 0}`).join("\n"), true);
}

export function easeSelected(kind: string) {
  const ids = need();
  if (ids) run(`ease ${ids.join(",")} all ${kind}`, true);
}

export function copySelection() {
  const id = selection.value[selection.value.length - 1];
  if (!id) return;
  clipboard.value = id;
  toast(`Copied ${id} (Ctrl+V pastes a copy, Ctrl+Alt+V pastes its attributes)`);
}

export function pasteClipboard(attributes: boolean) {
  const from = clipboard.value;
  if (!from || !findItem(from)) return toast("Copy a clip first (Ctrl+C)", true);
  if (attributes) {
    const to = selection.value.filter((s) => s !== from);
    if (!to.length) return toast("Select the clips to paste attributes onto", true);
    run(`paste ${from} ${to.join(",")}`);
  } else run(`dup ${from} at=${T(playhead.value)}`);
}

// ---------------------------------------------------------------- keyboard

export const TOOL_KEYS: Record<string, Tool> = { v: "select", a: "track", b: "ripple", n: "roll", r: "rate", c: "razor", y: "slip", u: "slide", h: "hand", z: "zoom", t: "type", g: "shape" };

/** Premiere Pro / After Effects keyboard shortcuts. */
export function installShortcuts(): () => void {
  const onKey = (e: KeyboardEvent) => {
    const tag = (e.target as HTMLElement)?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
    const mod = e.ctrlKey || e.metaKey;
    const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    const step = 1 / fps.value;
    const sel = selection.value;
    const done = () => e.preventDefault();
    if (k === " ") {
      done();
      playing.value = !playing.value;
    } else if (mod && k === "z") {
      done();
      run(e.shiftKey ? "redo" : "undo", true);
    } else if (mod && k === "y") {
      done();
      run("redo", true);
    } else if ((mod && k === "k") || (mod && e.shiftKey && k === "d")) {
      done();
      splitAtPlayhead();
    } else if (mod && k === "s") {
      done();
      if (projectPath.value) project_({ action: "save" });
    } else if (mod && k === "m") {
      done();
      // Ctrl+Shift+M previous marker, Ctrl+M export (Premiere)
      if (e.shiftKey) gotoMarker(-1);
      else showRender.value = true;
    } else if (mod && k === "e") {
      done();
      showRender.value = true;
    } else if (mod && k === "c" && !e.shiftKey && !e.altKey) {
      if (sel.length) copySelection();
    } else if (mod && k === "v") {
      done();
      pasteClipboard(e.altKey);
    } else if (mod && e.shiftKey && k === "c") {
      done();
      if (sel.length) run(`precomp ${sel.join(",")} name='Nested'`);
    } else if (mod && e.altKey && e.shiftKey && k === "y") {
      done();
      run(`null at=${T(playhead.value)}${sel.length ? ` children=${sel.join(",")}` : ""}`);
    } else if (mod && k === "d") {
      done();
      if (sel.length) run(`dup ${sel[sel.length - 1]}`);
    } else if (mod && k === "a") {
      done();
      selection.value = (comp.value?.tracks || []).flatMap((t) => t.items.map((i) => i.id));
    } else if (mod && e.shiftKey && k === "x") {
      done();
      run("workarea clear", true);
    } else if (k === "Delete" || k === "Backspace") {
      if (sel.length) run(`del ${sel.join(",")}${e.shiftKey || e.altKey ? " ripple" : ""}`, true);
    } else if (k === "ArrowLeft" || k === "ArrowRight") {
      const d = k === "ArrowLeft" ? -1 : 1;
      if (e.altKey && sel.length) {
        done();
        run(`move ${sel.join(",")} by=${d > 0 ? "+" : "-"}${e.shiftKey ? 5 : 1}f mode=overwrite`, true);
      } else if (mod && sel.length) {
        done();
        run(`nudge ${sel.join(",")} pos=${d * (e.shiftKey ? 10 : 1)},0`, true);
      } else setPlayhead(Math.max(0, playhead.value + d * (e.shiftKey ? 5 * step : step)));
    } else if (k === "ArrowUp" || k === "ArrowDown") {
      done();
      const d = k === "ArrowUp" ? -1 : 1;
      if (e.altKey) moveToTrack(d < 0 ? 1 : -1);
      else if (mod && sel.length) run(`nudge ${sel.join(",")} pos=0,${d * (e.shiftKey ? 10 : 1)}`, true);
      else gotoEdit(d as 1 | -1);
    } else if (k === "PageUp" || k === "PageDown") {
      done();
      gotoEdit(k === "PageUp" ? -1 : 1);
    } else if (k === "Home") {
      setPlayhead(0);
    } else if (k === "End") {
      const c = comp.value;
      if (c) setPlayhead(compDuration(c));
    } else if (k === "F9") {
      done();
      easeSelected(mod && e.shiftKey ? "easeOut" : e.shiftKey ? "easeIn" : "easy");
    } else if (mod || e.altKey) {
      // Alt+[ / Alt+] trim the layer's in / out point to the playhead (After Effects)
      const it = sel.length ? findItem(sel[sel.length - 1])?.item : null;
      if (e.altKey && it && (k === "[" || k === "]")) {
        done();
        run(`trim ${it.id} ${k === "[" ? "start" : "end"}=${T(playhead.value)}`, true);
      }
    } else if (k === "[" || k === "]") {
      // move the layer so its in / out point sits at the playhead (After Effects)
      const it = sel.length ? findItem(sel[sel.length - 1])?.item : null;
      if (it) run(`move ${it.id} at=${T(Math.max(0, k === "[" ? playhead.value : playhead.value - it.dur))}`, true);
    } else if (k === "i" || k === "o") {
      if (e.shiftKey) {
        const w = comp.value?.work;
        if (w) setPlayhead(k === "i" ? w[0] : w[1]);
      } else if (monitorTab.value === "source" && sourceMedia.value) {
        if (k === "i") sourceIn.value = sourceT.value;
        else sourceOut.value = sourceT.value;
      } else markWork(k === "i" ? "in" : "out");
    } else if (k === "x") {
      const t = targets();
      if (t.length) run(`workarea ${t[t.length - 1]}`, true);
    } else if (k === ";") {
      liftOrExtract("lift");
    } else if (k === "'") {
      liftOrExtract("extract");
    } else if (k === ",") {
      placeSource("insert");
    } else if (k === ".") {
      placeSource("overwrite");
    } else if (k === "q" || k === "w") {
      rippleTrimTo(k === "q" ? "start" : "end");
    } else if (k === "j" || k === "k") {
      gotoKey(k === "j" ? -1 : 1);
    } else if (k === "m") {
      if (e.shiftKey) gotoMarker(1);
      else run(`marker add ${T(playhead.value)}`, true);
    } else if (k === "e" && e.shiftKey) {
      toggleEnabled();
    } else if (k === "s") {
      snapping.value = !snapping.value;
      toast(`Snapping ${snapping.value ? "on" : "off"}`);
    } else if (TOOL_KEYS[k] && !e.shiftKey) {
      tool.value = TOOL_KEYS[k];
    } else if (k === "+" || k === "=") {
      zoom.value = Math.min(3000, zoom.value * 1.25);
    } else if (k === "-") {
      zoom.value = Math.max(2, zoom.value / 1.25);
    } else if (k === "\\") {
      fitTimeline();
    } else if (k === "`") {
      // maximize / restore the panel group with focus (Premiere's ` key)
      const gr = focusedPanel.value ? groupOf(shown.value, focusedPanel.value) : undefined;
      maximized.value = maximized.value ? null : gr?.id ?? null;
    } else if (k === "Escape") {
      if (maximized.value) maximized.value = null;
      selection.value = [];
      tool.value = "select";
    }
  };
  window.addEventListener("keydown", onKey);
  return () => window.removeEventListener("keydown", onKey);
}
