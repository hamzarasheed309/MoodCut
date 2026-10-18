// Lumetri Color: basic correction, creative, curves, hue curves, color wheels, HSL secondary, vignette.
// Each control writes the same script an AI would: `color <id> k=v` or `fx <id> curves master='...'`.
import { useEffect, useRef, useState } from "preact/hooks";
import { setFx } from "../actions";
import { catalog, mediaById, playhead, run, selectedItem, showScopes } from "../store";
import type { Effect, FxDef, Item } from "../types";
import { clamp, fmtNum, q, staticVal } from "../util";
import { Section, Slider } from "./Controls";

const HUES = "linear-gradient(90deg,#f00,#ff0 16.7%,#0f0 33.3%,#0ff 50%,#00f 66.7%,#f0f 83.3%,#f00)";

function fxOf(it: Item, kind: string): Effect | undefined {
  return (it.fx || []).find((f) => f.kind === kind);
}

function num(e: Effect | undefined, name: string, def: number): number {
  const v = staticVal(e?.props?.[name]);
  return typeof v === "number" ? v : def;
}

function str(e: Effect | undefined, name: string): string {
  const v = staticVal(e?.props?.[name]);
  return typeof v === "string" ? v : "";
}

/** Sliders for params of one effect, created on first change. `via` overrides the command (lumetri uses `color`). */
function FxSliders({ it, def, names, via }: { it: Item; def: FxDef | undefined; names: string[]; via?: (kv: Record<string, string>) => void }) {
  if (!def) return null;
  const e = fxOf(it, def.name);
  return (
    <>
      {names.map((n) => {
        const p = def.params.find((x) => x.name === n);
        if (!p) return null;
        const d = typeof p.def === "number" ? p.def : 0;
        const lo = n === "hue" ? 0 : p.min ?? -100;
        const hi = n === "hue" ? 360 : p.max ?? 100;
        const step = hi - lo <= 2 ? 0.01 : hi - lo <= 20 ? 0.05 : 1;
        const bg = n === "hue" ? HUES : n === "temp" ? "linear-gradient(90deg,#3b82f6,#ccc,#f59e0b)" : n === "tint" ? "linear-gradient(90deg,#22c55e,#ccc,#d946ef)" : undefined;
        return (
          <Slider
            key={n}
            label={p.desc.split(/[(,:]/)[0].trim().slice(0, 18) || n}
            title={`${n}: ${p.desc}`}
            value={num(e, n, d)}
            def={d}
            min={Math.max(lo, -400)}
            max={Math.min(hi, 400)}
            step={step}
            bg={bg}
            onCommit={(v) => (via ? via({ [n]: fmtNum(v, 4) }) : setFx(it, def.name, { [n]: fmtNum(v, 4) }))}
          />
        );
      })}
    </>
  );
}

function toggleFx(it: Item, kind: string) {
  const e = fxOf(it, kind);
  return e ? { on: e.on !== false, onToggle: (on: boolean) => run(`set ${it.id}.${e.id} on=${on ? 1 : 0}`, true) } : {};
}

// ---------------------------------------------------------------- curves

type Pt = [number, number, number?];

function parsePts(s: string): Pt[] {
  return s
    .split(/\s+/)
    .map((p) => p.split(",").map(Number))
    .filter((v) => v.length >= 2 && v.every(isFinite))
    .map((v) => [v[0], v[1], v[2]] as Pt);
}

function fmtPts(pts: Pt[]): string {
  return pts.map((p) => [p[0], p[1], ...(p[2] !== undefined ? [p[2]] : [])].map((v) => fmtNum(v, 3)).join(",")).join(" ");
}

/** Fritsch-Carlson monotone cubic, as the renderer evaluates tone curves. */
function monotone(pts: Pt[]): (x: number) => number {
  const p = pts.slice().sort((a, b) => a[0] - b[0]);
  const n = p.length;
  const d = p.slice(0, -1).map((a, i) => (p[i + 1][1] - a[1]) / Math.max(1e-6, p[i + 1][0] - a[0]));
  const m = p.map((_, i) => (i === 0 ? d[0] : i === n - 1 ? d[n - 2] : d[i - 1] * d[i] <= 0 ? 0 : (d[i - 1] + d[i]) / 2));
  return (x) => {
    if (x <= p[0][0]) return p[0][1];
    if (x >= p[n - 1][0]) return p[n - 1][1];
    let i = 0;
    while (i < n - 2 && x > p[i + 1][0]) i++;
    const h = Math.max(1e-6, p[i + 1][0] - p[i][0]);
    const t = (x - p[i][0]) / h;
    const t2 = t * t;
    const t3 = t2 * t;
    return (2 * t3 - 3 * t2 + 1) * p[i][1] + (t3 - 2 * t2 + t) * h * m[i] + (-2 * t3 + 3 * t2) * p[i + 1][1] + (t3 - t2) * h * m[i + 1];
  };
}

/** Raised-cosine bumps, as the renderer evaluates hue curves. */
function bumps(pts: Pt[], periodic: boolean, defW: number): (x: number) => number {
  return (x) => {
    let o = 0;
    for (const [px, py, w0] of pts) {
      const w = Math.max(0.01, w0 ?? defW);
      let dd = Math.abs(x - px);
      if (periodic) dd = Math.min(dd, 1 - dd);
      if (dd < w) o += py * 0.5 * (1 + Math.cos((Math.PI * dd) / w));
    }
    return o;
  };
}

/** Curve editor. tone: y 0..1 through the points; bump: y -1..1 bumps around a flat line. Click adds, drag moves, right-click removes. */
function CurveBox({ value, kind, color, bg, periodic = false, defW = 0.15, onCommit }: { value: string; kind: "tone" | "bump"; color: string; bg?: string; periodic?: boolean; defW?: number; onCommit: (s: string) => void }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const init = (): Pt[] => {
    const p = parsePts(value);
    return kind === "tone" && p.length < 2 ? [[0, 0], [1, 1]] : p;
  };
  const [pts, setPts] = useState<Pt[]>(init);
  const drag = useRef<number | null>(null);
  useEffect(() => {
    if (drag.current === null) setPts(init());
  }, [value, kind]);
  const S = 232;
  const toY = (y: number) => (kind === "tone" ? (1 - y) * S : (0.5 - y / 2) * S);
  const fromY = (py: number) => (kind === "tone" ? clamp(1 - py / S, 0, 1) : clamp((0.5 - py / S) * 2, -1, 1));

  useEffect(() => {
    const cv = ref.current!;
    const dpr = window.devicePixelRatio || 1;
    cv.width = S * dpr;
    cv.height = S * dpr;
    const g = cv.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, S, S);
    g.strokeStyle = "rgba(255,255,255,0.08)";
    for (let i = 1; i < 4; i++) {
      g.beginPath();
      g.moveTo((i * S) / 4, 0);
      g.lineTo((i * S) / 4, S);
      g.moveTo(0, (i * S) / 4);
      g.lineTo(S, (i * S) / 4);
      g.stroke();
    }
    g.strokeStyle = "rgba(255,255,255,0.18)";
    g.beginPath();
    if (kind === "tone") {
      g.moveTo(0, S);
      g.lineTo(S, 0);
    } else {
      g.moveTo(0, S / 2);
      g.lineTo(S, S / 2);
    }
    g.stroke();
    const f = kind === "tone" ? (pts.length >= 2 ? monotone(pts) : (x: number) => x) : bumps(pts, periodic, defW);
    g.strokeStyle = color;
    g.lineWidth = 2;
    g.beginPath();
    for (let i = 0; i <= S; i += 2) {
      const y = toY(kind === "tone" ? clamp(f(i / S), 0, 1) : clamp(f(i / S), -1, 1));
      i === 0 ? g.moveTo(i, y) : g.lineTo(i, y);
    }
    g.stroke();
    g.lineWidth = 1;
    for (const p of pts) {
      g.fillStyle = "#fff";
      g.strokeStyle = "#000";
      g.beginPath();
      g.arc(p[0] * S, toY(p[1]), 4.5, 0, Math.PI * 2);
      g.fill();
      g.stroke();
    }
  }, [pts, kind, color]);

  const at = (e: PointerEvent) => {
    const r = ref.current!.getBoundingClientRect();
    return [clamp(((e.clientX - r.left) / r.width) * S, 0, S), clamp(((e.clientY - r.top) / r.height) * S, 0, S)];
  };
  const hit = (x: number, y: number) => pts.findIndex((p) => Math.hypot(p[0] * S - x, toY(p[1]) - y) < 8);
  const commit = (p: Pt[]) => {
    const isFlat = kind === "tone" ? p.length === 2 && p[0][0] === 0 && p[0][1] === 0 && p[1][0] === 1 && p[1][1] === 1 : p.length === 0;
    onCommit(isFlat ? "''" : q(fmtPts(p.slice().sort((a, b) => a[0] - b[0]))));
  };
  return (
    <canvas
      ref={ref}
      class="curve"
      style={{ background: bg || "#101014" }}
      onContextMenu={(e) => {
        e.preventDefault();
        const [x, y] = at(e as any);
        const i = hit(x, y);
        if (i >= 0 && (kind === "bump" || pts.length > 2)) {
          const n = pts.filter((_, j) => j !== i);
          setPts(n);
          commit(n);
        }
      }}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
        const [x, y] = at(e);
        let i = hit(x, y);
        if (i < 0) {
          const n: Pt[] = [...pts, [x / S, kind === "tone" ? monotone(pts)(x / S) : fromY(y)]];
          i = n.length - 1;
          setPts(n);
        }
        drag.current = i;
      }}
      onPointerMove={(e) => {
        const i = drag.current;
        if (i === null) return;
        const [x, y] = at(e);
        setPts((p) => p.map((pt, j) => (j === i ? [x / S, fromY(y), pt[2]] : pt)));
      }}
      onPointerUp={() => {
        if (drag.current === null) return;
        drag.current = null;
        commit(pts);
      }}
    />
  );
}

// ---------------------------------------------------------------- color wheels

function hexToRgba(h: string): [number, number, number, number] | null {
  const m = /^#([0-9a-f]{6})([0-9a-f]{2})?$/i.exec(h.trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255, m[2] ? parseInt(m[2], 16) / 255 : 1];
}

function hueOf(r: number, g: number, b: number): number {
  const mx = Math.max(r, g, b);
  const mn = Math.min(r, g, b);
  const d = mx - mn;
  if (d < 1e-6) return 0;
  const h = mx === r ? ((g - b) / d) % 6 : mx === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return ((h * 60 + 360) % 360) / 360;
}

function hueHex(h: number, a: number): string {
  const f = (n: number) => {
    const k = (n + h * 12) % 12;
    return Math.round(255 * (0.5 - 0.5 * Math.max(-1, Math.min(k - 3, 9 - k, 1))));
  };
  const x = (v: number) => v.toString(16).padStart(2, "0");
  return `#${x(f(0))}${x(f(8))}${x(f(4))}${x(Math.round(clamp(a, 0, 1) * 255))}`;
}

/** A Lumetri color wheel: angle = hue, distance from center = strength (the color's alpha). */
function Wheel({ label, value, onCommit }: { label: string; value: string; onCommit: (hex: string) => void }) {
  const R = 52;
  const parse = (): [number, number] => {
    const c = hexToRgba(value);
    return c && value !== "none" ? [hueOf(c[0], c[1], c[2]), c[3]] : [0, 0];
  };
  const [hv, setHv] = useState<[number, number]>(parse);
  const drag = useRef(false);
  useEffect(() => {
    if (!drag.current) setHv(parse());
  }, [value]);
  const [h, a] = hv;
  const th = h * Math.PI * 2;
  const pick = (e: PointerEvent) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const dx = e.clientX - r.left - R;
    const dy = e.clientY - r.top - R;
    const s = clamp(Math.hypot(dx, dy) / R, 0, 1);
    const hh = ((Math.atan2(dx, -dy) / (Math.PI * 2)) % 1 + 1) % 1;
    setHv([hh, s < 0.04 ? 0 : s]);
  };
  return (
    <div class="wheel-box">
      <div
        class="wheel"
        style={{ width: R * 2 + "px", height: R * 2 + "px" }}
        title="Drag to tint (distance = strength); double-click to reset"
        onPointerDown={(e) => {
          (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
          drag.current = true;
          pick(e);
        }}
        onPointerMove={(e) => drag.current && pick(e)}
        onPointerUp={() => {
          drag.current = false;
          onCommit(a < 0.04 ? "none" : hueHex(h, a));
        }}
        onDblClick={() => onCommit("none")}
      >
        <span class="wheel-dot" style={{ left: R + Math.sin(th) * a * R + "px", top: R - Math.cos(th) * a * R + "px" }} />
      </div>
      <small>{label}</small>
    </div>
  );
}

// ---------------------------------------------------------------- panel

const CURVE_TABS = [
  { k: "master", color: "#e7e7ee" },
  { k: "r", color: "#ff5a5f" },
  { k: "g", color: "#34d399" },
  { k: "b", color: "#60a5fa" },
];
const HUE_TABS = [
  { k: "hueSat", name: "Hue vs Sat", periodic: true },
  { k: "hueHue", name: "Hue vs Hue", periodic: true },
  { k: "hueLuma", name: "Hue vs Luma", periodic: true },
  { k: "lumaSat", name: "Luma vs Sat", periodic: false },
  { k: "satSat", name: "Sat vs Sat", periodic: false },
];

export function ColorPanel() {
  const sel = selectedItem.value;
  const cat = catalog.value;
  const [curve, setCurve] = useState("master");
  const [hueTab, setHueTab] = useState("hueSat");
  const it = sel?.item;
  const at = fmtNum(playhead.value, 4);
  if (!cat) return <div class="empty">Loading…</div>;
  const media = it?.src ? mediaById.value.get(it.src) : undefined;
  if (!it || it.kind === "null" || it.kind === "camera" || media?.kind === "audio")
    return (
      <div class="empty">
        Select a clip, title or adjustment layer to grade it.
        <br />
        <button class="btn sm" style={{ marginTop: "10px" }} onClick={() => run(`adjust at=${at} dur=5`)}>
          Add adjustment layer
        </button>
      </div>
    );
  const def = (n: string) => cat.effects.find((e) => e.name === n);
  const lum = fxOf(it, "lumetri");
  const colorCmd = (kv: Record<string, string>) =>
    run(
      `color ${it.id} ${Object.entries(kv)
        .map(([k, v]) => `${k}=${v}`)
        .join(" ")}`,
      true,
    );
  const curves = fxOf(it, "curves");
  const hc = fxOf(it, "huecurves");
  const cw = fxOf(it, "colorwheels");
  const ht = HUE_TABS.find((t) => t.k === hueTab)!;
  const tintRow = (name: "stint" | "htint", label: string) => {
    const c = hexToRgba(str(lum, name)) || [0.5, 0.5, 0.5, 0];
    const hex6 = "#" + [c[0], c[1], c[2]].map((v) => Math.round(v * 255).toString(16).padStart(2, "0")).join("");
    const alpha = str(lum, name) && str(lum, name) !== "none" ? c[3] : 0;
    const set = (h6: string, a: number) => colorCmd({ [name]: a <= 0.005 ? "none" : h6 + Math.round(a * 255).toString(16).padStart(2, "0") });
    return (
      <div class="tint-row">
        <label>{label}</label>
        <input type="color" value={hex6} onChange={(e) => set((e.target as HTMLInputElement).value, Math.max(alpha, 0.3))} />
        <Slider label="amount" value={Math.round(alpha * 100)} min={0} max={100} def={0} onCommit={(v) => set(hex6, v / 100)} />
      </div>
    );
  };
  return (
    <div class="scroll color-panel">
      <div class="insp-head">
        <b style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>Lumetri · {it.name || it.id}</b>
        <button class={"btn sm" + (showScopes.value ? " on" : "")} title="Show Lumetri scopes in the monitor" onClick={() => (showScopes.value = !showScopes.value)}>
          Scopes
        </button>
        <button class="btn sm" title="Remove every color effect from this clip" onClick={() => run((it.fx || []).filter((f) => ["lumetri", "curves", "huecurves", "colorwheels", "secondary", "vignette", "autolevels"].includes(f.kind)).map((f) => `del ${it.id}.${f.id}`).join("\n") || `color ${it.id} none`, true)}>
          Reset
        </button>
      </div>
      <Section title="Basic Correction" {...toggleFx(it, "lumetri")} acts={
        <button class="btn sm ghost" title="Auto tone: stretch levels and remove casts per frame (fx autolevels)" onClick={() => setFx(it, "autolevels", { perchannel: "1" })}>
          Auto
        </button>
      }>
        <div class="sub">White balance</div>
        <FxSliders it={it} def={def("lumetri")} names={["temp", "tint"]} via={colorCmd} />
        <div class="sub">Tone</div>
        <FxSliders it={it} def={def("lumetri")} names={["exposure", "contrast", "highlights", "shadows", "whites", "blacks"]} via={colorCmd} />
        <div class="sub">Color</div>
        <FxSliders it={it} def={def("lumetri")} names={["sat", "vibrance"]} via={colorCmd} />
      </Section>
      <Section title="Creative">
        <div class="row">
          <label>Look</label>
          <div class="ctl">
            <select value="" onChange={(e) => (e.target as HTMLSelectElement).value && run(`color ${it.id} ${(e.target as HTMLSelectElement).value}`)}>
              <option value="">apply a look…</option>
              {cat.colors.map((c) => (
                <option key={c.name} value={c.name}>
                  {c.name} — {c.desc}
                </option>
              ))}
            </select>
          </div>
          <span />
        </div>
        <FxSliders it={it} def={def("lumetri")} names={["fade"]} via={colorCmd} />
        {tintRow("stint", "Shadow tint")}
        {tintRow("htint", "Highlight tint")}
      </Section>
      <Section title="Curves" {...toggleFx(it, "curves")}>
        <div class="seg">
          {CURVE_TABS.map((t) => (
            <button key={t.k} class={"btn sm" + (curve === t.k ? " on" : "")} style={{ color: t.color }} onClick={() => setCurve(t.k)}>
              {t.k === "master" ? "RGB" : t.k.toUpperCase()}
            </button>
          ))}
        </div>
        <CurveBox kind="tone" value={str(curves, curve)} color={CURVE_TABS.find((t) => t.k === curve)!.color} onCommit={(s) => setFx(it, "curves", { [curve]: s })} />
        <div class="seg">
          {HUE_TABS.map((t) => (
            <button key={t.k} class={"btn sm" + (hueTab === t.k ? " on" : "")} onClick={() => setHueTab(t.k)}>
              {t.name}
            </button>
          ))}
        </div>
        <CurveBox
          kind="bump"
          value={str(hc, hueTab)}
          color="#fff"
          periodic={ht.periodic}
          defW={ht.periodic ? 0.08 : 0.15}
          bg={`linear-gradient(rgba(16,16,20,.6),rgba(16,16,20,.6)),${ht.periodic ? HUES : "linear-gradient(90deg,#000,#fff)"}`}
          onCommit={(s) => setFx(it, "huecurves", { [hueTab]: s })}
        />
        <div class="hint">Click to add a point, drag to bend, right-click to remove.</div>
      </Section>
      <Section title="Color Wheels" {...toggleFx(it, "colorwheels")}>
        <div class="wheels">
          {(["shadows", "midtones", "highlights"] as const).map((w) => (
            <div key={w} class="wheel-col">
              <Wheel label={w} value={str(cw, w) || "none"} onCommit={(hex) => setFx(it, "colorwheels", { [w]: hex })} />
              <input type="range" class="vert-l" min={-100} max={100} step={1} value={num(cw, w + "L", 0)} title={`${w} brightness (double-click: reset)`} onDblClick={() => setFx(it, "colorwheels", { [w + "L"]: "0" })} onChange={(e) => setFx(it, "colorwheels", { [w + "L"]: (e.target as HTMLInputElement).value })} />
            </div>
          ))}
        </div>
      </Section>
      <Section title="HSL Secondary" open={false} {...toggleFx(it, "secondary")}>
        <div class="sub">Key</div>
        <FxSliders it={it} def={def("secondary")} names={["hue", "hueRange", "satMin", "lumMin", "lumMax", "soft"]} />
        <div class="row">
          <label>show key</label>
          <div class="ctl">
            <input type="checkbox" checked={!!staticVal(fxOf(it, "secondary")?.props?.show)} onChange={(e) => setFx(it, "secondary", { show: (e.target as HTMLInputElement).checked ? "1" : "0" })} />
            <span style={{ color: "var(--faint)", fontSize: "11px" }}>invert</span>
            <input type="checkbox" checked={!!staticVal(fxOf(it, "secondary")?.props?.invert)} onChange={(e) => setFx(it, "secondary", { invert: (e.target as HTMLInputElement).checked ? "1" : "0" })} />
          </div>
          <span />
        </div>
        <div class="sub">Correction</div>
        <FxSliders it={it} def={def("secondary")} names={["shift", "sat", "light", "temp", "tint"]} />
      </Section>
      <Section title="Vignette" open={false} {...toggleFx(it, "vignette")}>
        <FxSliders it={it} def={def("vignette")} names={["amount", "size", "soft"]} />
      </Section>
    </div>
  );
}
