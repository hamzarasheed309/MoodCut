// Editable fields of HTML graphics: the `{{name|default}}` values of a page that
// people change in the editor (and the AI with `vars`) without touching its code.
import type { Brand } from "./types";

export type FieldType = "text" | "longtext" | "color" | "number" | "toggle" | "select" | "media";

export interface GField {
  name: string;
  type: FieldType;
  label: string;
  /** what renders while the field isn't set */
  default: string;
  /** the default as plain text, when its HTML differs (entities, <br>) */
  plain?: string;
  min?: number;
  max?: number;
  step?: number;
  unit?: string;
  options?: string[];
  group?: string;
  help?: string;
  /** the brand color it follows by default */
  brand?: string;
  /** html, attr, css, js; empty = read by scripts only */
  used: string[];
}

export interface GfxInfo {
  fields: GField[];
  warnings: string[];
  /** the page times itself from the graphic's length (--mc-dur / MC.duration) */
  uses_duration: boolean;
  /** visible texts written into the code instead of a field */
  hardcoded: string[];
}

export interface GfxDetail extends GfxInfo {
  id: string;
  name: string;
  html: string;
  file: string;
  dur: number;
  fps: number;
  w: number;
  h: number;
  alpha: boolean;
  at: number;
  vars: Vars;
  /** clips using it */
  uses: number;
  /** rendered with an older brand kit */
  stale: boolean;
}

export interface GfxPreview extends GfxInfo {
  token: string;
  url: string;
  w: number;
  h: number;
  fps: number;
  dur: number;
  alpha: boolean;
  at: number;
  missing: string[];
}

export type Vars = Record<string, string>;

/** A field's value as shown: set, else its default (as plain text). */
export const shown = (f: GField, vars: Vars) => vars[f.name] ?? f.plain ?? f.default;

/** JSON API call that never throws: errors come back as {ok: false, error}. */
export async function api<T>(url: string, body?: unknown): Promise<({ ok: true } & T) | { ok: false; error: string }> {
  try {
    const r = await fetch(url, body === undefined ? undefined : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
    const j = await r.json().catch(() => null);
    if (!j) return { ok: false, error: r.statusText || `HTTP ${r.status}` };
    if (!r.ok || j.ok === false) return { ok: false, error: j.error || r.statusText || `HTTP ${r.status}` };
    return j;
  } catch {
    return { ok: false, error: "MoodCut isn't responding" };
  }
}

const BRAND_DEFAULTS: Record<string, string> = { primary: "#111111", accent: "#ff3b6b", text: "#ffffff", dark: "#0b0b10" };

/** The brand color a `var(--brand-…)` value refers to. */
export function brandOf(v: string): string | null {
  return /^\s*var\(--brand-(primary|accent|text|dark)\b/i.exec(v)?.[1].toLowerCase() || null;
}

let ctx: CanvasRenderingContext2D | null = null;

/** #rrggbb for a color input (brand variables resolved; anything unreadable is black). */
export function toHex(css: string, brand: Brand | undefined): string {
  const b = brandOf(css);
  const c = b ? (brand as Record<string, string | undefined> | undefined)?.[b] || BRAND_DEFAULTS[b] : css.trim();
  if (/^#[0-9a-f]{6}$/i.test(c)) return c.toLowerCase();
  if (/^#[0-9a-f]{8}$/i.test(c)) return c.slice(0, 7).toLowerCase();
  ctx = ctx || document.createElement("canvas").getContext("2d");
  if (!ctx) return "#000000";
  ctx.fillStyle = "#000000";
  ctx.fillStyle = c;
  const out = String(ctx.fillStyle);
  if (out.startsWith("#")) return out;
  const m = /rgba?\((\d+),\s*(\d+),\s*(\d+)/.exec(out);
  return m ? "#" + [m[1], m[2], m[3]].map((x) => Number(x).toString(16).padStart(2, "0")).join("") : "#000000";
}

/** A number with an optional unit. */
export function numUnit(v: string): { n: number; unit: string } | null {
  const m = /^\s*([-+]?(?:\d+\.?\d*|\.\d+))\s*([a-z%]*)\s*$/i.exec(v);
  return m ? { n: parseFloat(m[1]), unit: m[2] } : null;
}

const norm = (s: string) => s.replace(/\n/g, "").replace(/\s+/g, " ").trim().toLowerCase();

/** The text field a click in the preview means: `texts` run from the clicked element up to the page. */
export function matchPick(fields: GField[], vars: Vars, texts: string[]): string | null {
  const cands = fields.filter((f) => f.type === "text" || f.type === "longtext");
  const ts = texts.map(norm);
  for (const t of ts) {
    const hit = cands.find((f) => norm(shown(f, vars)) === t);
    if (hit) return hit.name;
  }
  for (const t of ts) {
    const hit = cands.find((f) => {
      const v = norm(shown(f, vars));
      return v.length > 1 && t.includes(v);
    });
    if (hit) return hit.name;
  }
  return null;
}

/** "1280x720" -> [1280, 720]. */
export function parseSize(s: string): [number, number] | null {
  const m = /^\s*(\d{2,5})\s*[x×:]\s*(\d{2,5})\s*$/i.exec(s);
  return m ? [parseInt(m[1]), parseInt(m[2])] : null;
}
