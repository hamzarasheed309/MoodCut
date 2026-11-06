// Brand Kit: the font and colors titles, captions, shapes and HTML graphics use by default.
import { useState } from "preact/hooks";
import { api } from "../gfx";
import { catalog, project, refresh, refreshHistory, refreshLogs, run, toast } from "../store";
import { q } from "../util";
import { TextField } from "./Controls";

/** HTML graphics are rendered videos: after a brand change they keep the old colors until rendered again. */
function StaleGraphics() {
  const [busy, setBusy] = useState(false);
  const stale = (project.value?.media || []).filter((m) => m.gfx?.stale);
  if (!stale.length) return null;
  const go = async () => {
    setBusy(true);
    const r = await api<{ text: string }>("/api/graphic/refresh", {});
    setBusy(false);
    if (!r.ok) return toast(r.error, true);
    toast(r.text.split("\n").length > 1 ? `${r.text.split("\n").length} graphics re-rendered` : r.text);
    await refresh();
    refreshLogs();
    refreshHistory();
  };
  return (
    <div class="gf-note" style={{ padding: "0 10px" }} title={stale.map((m) => `${m.id} ${m.name}`).join("\n")}>
      {stale.length} HTML graphic{stale.length === 1 ? " has" : "s have"} the old brand colors.
      <button class="btn sm" disabled={busy} onClick={go}>
        {busy ? "Rendering…" : "Re-render"}
      </button>
    </div>
  );
}

const WEIGHTS = ["light", "regular", "medium", "semibold", "bold", "extrabold", "black"];
const FIELDS: [keyof Swatches, string][] = [
  ["primary", "Primary"],
  ["accent", "Accent"],
  ["text", "Text"],
  ["dark", "Dark"],
];
type Swatches = { primary?: string; accent?: string; text?: string; dark?: string };

export function BrandPanel() {
  const b = project.value?.brand || {};
  const cat = catalog.value;
  const set = (kv: string) => run(`brand ${kv}`, true);
  const empty = !Object.keys(b).length;
  return (
    <div class="scroll brand-panel">
      <div class="brand-preview" style={{ background: b.dark || "#0b0b10", fontFamily: `'${b.font || "Inter"}', Inter, sans-serif` }}>
        <span class="bp-line" style={{ background: b.accent || "#ff3b6b" }} />
        <b style={{ color: b.text || "#fff", fontWeight: b.weight === "black" ? 900 : b.weight === "light" ? 300 : 800 }}>Headline</b>
        <span style={{ color: b.text || "#fff", opacity: 0.8 }}>
          Supporting line with an <em style={{ color: b.accent || "#ff3b6b", fontStyle: "normal" }}>accent</em>
        </span>
        <span class="bp-chip" style={{ background: b.primary || "#111", color: b.text || "#fff" }}>
          lower third
        </span>
      </div>
      {empty && <div class="hint" style={{ padding: "0 10px" }}>No brand kit yet: titles use generic defaults. Pick a preset or set your own font and colors.</div>}
      <StaleGraphics />
      <div class="group-b">
        <div class="row">
          <label>Font</label>
          <div class="ctl">
            <TextField value={b.font || ""} placeholder="Inter" onCommit={(v) => set(`font=${v ? q(v) : "-"}`)} />
          </div>
          <span />
        </div>
        <div class="row">
          <label>Headline weight</label>
          <div class="ctl">
            <select value={b.weight || ""} onChange={(e) => set(`weight=${(e.target as HTMLSelectElement).value || "-"}`)}>
              <option value="">(style default)</option>
              {WEIGHTS.map((w) => (
                <option key={w}>{w}</option>
              ))}
            </select>
          </div>
          <span />
        </div>
        {FIELDS.map(([k, label]) => (
          <div class="row" key={k}>
            <label>{label}</label>
            <div class="ctl">
              <input type="color" value={(b[k] || "#000000").slice(0, 7)} onChange={(e) => set(`${k}=${(e.target as HTMLInputElement).value}`)} />
              <TextField value={b[k] || ""} placeholder="none" onCommit={(v) => set(`${k}=${v || "-"}`)} />
            </div>
            <span />
          </div>
        ))}
        {!empty && (
          <button class="btn sm" style={{ marginTop: "6px" }} onClick={() => run("brand clear", true)}>
            Clear brand kit
          </button>
        )}
      </div>
      <div class="section-title">Presets</div>
      <div class="brand-presets">
        {cat?.brands.map((x) => (
          <button key={x.name} class="brand-card" title={`${x.use}\nfont ${x.font} ${x.weight}`} onClick={() => run(`brand ${x.name}`, true)}>
            <span class="sw">
              {[x.dark, x.primary, x.accent, x.text].map((c, i) => (
                <i key={i} style={{ background: c }} />
              ))}
            </span>
            <b>{x.name}</b>
            <small>{x.use}</small>
          </button>
        ))}
      </div>
    </div>
  );
}
