import type { Anim, AnimObj, Comp, Item, Val } from "./types";

export function isAnimObj(a: Anim | undefined): a is AnimObj {
  return typeof a === "object" && a !== null && !Array.isArray(a);
}

/** Static (or first-key) value of an animatable prop. */
export function staticVal(a: Anim | undefined): Val | undefined {
  if (a === undefined) return undefined;
  if (isAnimObj(a)) {
    if (a.k && a.k.length) return a.k[0][1];
    return a.v;
  }
  return a;
}

export function isAnimated(a: Anim | undefined): boolean {
  return isAnimObj(a) && (!!(a.k && a.k.length) || !!a.x);
}

export function keyTimes(a: Anim | undefined): number[] {
  return isAnimObj(a) && a.k ? a.k.map((k) => k[0]) : [];
}

export function fmtNum(n: number, d = 3): string {
  if (!isFinite(n)) return "0";
  const s = n.toFixed(d);
  return s.includes(".") ? s.replace(/0+$/, "").replace(/\.$/, "") : s;
}

export function fmtTC(t: number, fps: number): string {
  const f = Math.max(1, Math.round(fps));
  const total = Math.round(t * fps);
  const fr = total % f;
  const s = Math.floor(total / f);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(Math.floor(s / 3600))}:${p(Math.floor(s / 60) % 60)}:${p(s % 60)}:${p(fr)}`;
}

export function fmtShort(t: number): string {
  const m = Math.floor(t / 60);
  const s = t - m * 60;
  return m > 0 ? `${m}:${s.toFixed(1).padStart(4, "0")}` : `${s.toFixed(1)}s`;
}

/** Mirrors the lexer: Windows-looking paths are read with literal backslashes. */
function looksLikePath(s: string): boolean {
  if (/^[A-Za-z]:\\/.test(s) || /^\\\\[A-Za-z0-9].*\\/.test(s)) return true;
  return s.includes("\\") && !/\s/.test(s) && /\.[A-Za-z0-9]{2,5}$/.test(s);
}

/** Quotes a string for MoodCut Script. */
export function q(s: string): string {
  if (!s.includes("'") && looksLikePath(s)) return `'${s}'`;
  return "'" + s.replace(/\\/g, "\\\\").replace(/'/g, "\\'").replace(/\n/g, "\\n") + "'";
}

export function valToScript(v: Val): string {
  if (Array.isArray(v)) return `${fmtNum(v[0])},${fmtNum(v[1])}`;
  if (typeof v === "number") return fmtNum(v, 4);
  if (typeof v === "boolean") return v ? "1" : "0";
  return /[\s,'"]/.test(v) || v === "" ? q(v) : v;
}

export function compDuration(c: Comp): number {
  if (c.dur) return c.dur;
  let e = 0;
  for (const t of c.tracks) for (const i of t.items) e = Math.max(e, i.start + i.dur);
  return e;
}

/** `srcName` is the media name for clips and the comp name for nested comps. */
export function itemLabel(it: Item, srcName?: string): string {
  if (it.name) return it.name;
  switch (it.kind) {
    case "text":
      return (it.text || "").replace(/\n/g, " ");
    case "clip":
      return srcName || it.src || it.id;
    case "shape":
      return (it.shapes || []).map((s) => s.kind).join(" + ") || "shape";
    case "solid":
      return "Solid";
    case "adjust":
      return "Adjustment";
    case "comp":
      return srcName || `Comp ${it.src}`;
    case "null":
      return "Null";
    case "camera":
      return "Camera";
  }
}

export function speedOf(it: Item): number {
  const s = staticVal(it.props?.speed);
  return typeof s === "number" && s > 0 ? s : 1;
}

export function clamp(v: number, a: number, b: number) {
  return Math.max(a, Math.min(b, v));
}

export function debounce<T extends (...a: any[]) => void>(fn: T, ms: number): T {
  let h: number | undefined;
  return ((...a: any[]) => {
    if (h) clearTimeout(h);
    h = window.setTimeout(() => fn(...a), ms);
  }) as T;
}

/** Timeline label colors (the `label` layer property). */
export const LABEL_COLORS: Record<string, string> = {
  red: "#d6455d",
  orange: "#e0803a",
  yellow: "#d8b737",
  green: "#4cae5b",
  aqua: "#33a9b5",
  blue: "#3d6fd6",
  violet: "#8e5bd8",
  pink: "#d65aa8",
  gray: "#7b7b88",
};

export function kindColor(kind: string, trackKind: string): string {
  if (trackKind === "audio") return "#2f9e6e";
  switch (kind) {
    case "clip":
      return "#3d6fd6";
    case "text":
      return "#9a55e0";
    case "shape":
      return "#e08a2e";
    case "solid":
      return "#b0485f";
    case "adjust":
      return "#6b7280";
    case "comp":
      return "#1f9aa6";
    case "null":
      return "#5b5b6b";
    case "camera":
      return "#4b6584";
    default:
      return "#555";
  }
}

/** Value of a keyframed number/vec at item-local time `t` (linear between keys; for display). */
export function valueAt(a: Anim | undefined, t: number): Val | undefined {
  if (!isAnimObj(a) || !a.k || !a.k.length) return staticVal(a);
  const k = a.k;
  if (t <= k[0][0]) return k[0][1];
  for (let i = 0; i < k.length - 1; i++) {
    const [t0, v0] = k[i];
    const [t1, v1] = k[i + 1];
    if (t >= t0 && t <= t1) {
      const f = t1 > t0 ? (t - t0) / (t1 - t0) : 1;
      if (typeof v0 === "number" && typeof v1 === "number") return v0 + (v1 - v0) * f;
      if (Array.isArray(v0) && Array.isArray(v1)) return [v0[0] + (v1[0] - v0[0]) * f, v0[1] + (v1[1] - v0[1]) * f];
      return f < 1 ? v0 : v1;
    }
  }
  return k[k.length - 1][1];
}

/** Cut points of a comp (item starts/ends and markers), sorted. */
export function editPoints(c: Comp): number[] {
  const s = new Set<number>([0]);
  for (const tr of c.tracks) for (const i of tr.items) {
    s.add(+i.start.toFixed(4));
    s.add(+(i.start + i.dur).toFixed(4));
  }
  return [...s].sort((a, b) => a - b);
}
