// Essential Sound and the Audio Track Mixer. Every control is a script line (sound, track set, master, afx).
import { useState } from "preact/hooks";
import { levels } from "../meter";
import { catalog, comp, findItem, mediaById, run, selection } from "../store";
import type { Bus, Effect, PDef, Track } from "../types";
import { fmtNum, staticVal } from "../util";
import { NumField, Slider } from "./Controls";

const TYPES = [
  { k: "dialogue", icon: "🗣", desc: "Loudness match to -23 LUFS, voice EQ/compression and de-essing" },
  { k: "music", icon: "♫", desc: "Loudness match to -28 LUFS and duck it under the dialogue" },
  { k: "sfx", icon: "✸", desc: "Loudness match to -24 LUFS" },
  { k: "ambience", icon: "≋", desc: "Loudness match to -32 LUFS, ducked under dialogue" },
];

function Meter() {
  const [l, r] = levels.value;
  const pct = (d: number) => Math.max(0, Math.min(100, ((d + 60) / 60) * 100));
  return (
    <div class="meter" title={`peak ${fmtNum(Math.max(l, r), 1)} dBFS`}>
      {[l, r].map((d, i) => (
        <div key={i} class="meter-ch">
          <div class={"meter-fill" + (d > -1 ? " clip" : "")} style={{ height: pct(d) + "%" }} />
        </div>
      ))}
    </div>
  );
}

/** Settings of one track / master effect: a slider per number, a menu per choice. */
/** Slider range for an audio effect setting. Every number setting has a range in the catalog; the fallback is only for older engines. */
function fxRange(p: PDef): { min: number; max: number; step: number; log: boolean } {
  const d = Number(p.def) || 0;
  const min = p.min ?? Math.min(0, d * 2);
  const max = p.max ?? Math.max(1, Math.abs(d) * 4);
  const whole = ["bits", "downsample", "harmonics"].includes(p.name);
  const step = p.name === "freq" && min === 50 && max === 60 ? 10 : whole ? 1 : max - min > 200 ? 1 : max - min > 20 ? 0.1 : 0.01;
  return { min, max, step, log: min > 0 && max / min >= 50 };
}

function BusFxSettings({ e, target }: { e: Effect; target: string }) {
  const def = catalog.value?.audio_effects.find((f) => f.name === e.kind);
  if (!def) return null;
  const ref = `${target}.${e.id}`;
  return (
    <div class="bus-fx-settings">
      {def.params.map((p) => {
        const cur = staticVal(e.props?.[p.name]) ?? p.def;
        if (Array.isArray(p.type))
          return (
            <label key={p.name} class="sl" title={p.desc}>
              <span>{p.name}</span>
              <select value={String(cur)} onChange={(ev) => run(`afx ${ref} ${p.name}=${(ev.target as HTMLSelectElement).value}`, true)}>
                {p.type.map((o) => (
                  <option key={o}>{o}</option>
                ))}
              </select>
            </label>
          );
        if (p.type === "bool")
          return (
            <label key={p.name} class="sl" title={p.desc}>
              <span>{p.name}</span>
              <input type="checkbox" checked={!!cur} onChange={(ev) => run(`afx ${ref} ${p.name}=${(ev.target as HTMLInputElement).checked ? 1 : 0}`, true)} />
            </label>
          );
        if (p.type !== "num") return null;
        const { min, max, step, log } = fxRange(p);
        return <Slider key={p.name} label={p.name} title={p.desc} value={Number(cur)} def={Number(p.def)} min={min} max={max} step={step} log={log} onCommit={(v) => run(`afx ${ref} ${p.name}=${fmtNum(v, 3)}`, true)} />;
      })}
    </div>
  );
}

/** A track's or the master's effect chain: click a chip to bypass it, ⚙ for its settings, ✕ to remove it. */
function FxChips({ list, target }: { list: Effect[]; target: string }) {
  const cat = catalog.value;
  const [open, setOpen] = useState<string | null>(null);
  const shown = list.find((e) => e.id === open);
  return (
    <div class="fx-chips">
      {list.map((e) => (
        <span key={e.id} class={"chip bus-fx" + (e.on === false ? " off" : "") + (open === e.id ? " open" : "")}>
          <button class="chip-main" title={`${target}.${e.id} ${e.kind}: click to ${e.on === false ? "turn on" : "bypass"}`} onClick={() => run(`afx ${target}.${e.id} ${e.on === false ? "on" : "off"}`, true)}>
            {e.kind}
          </button>
          <button class="chip-act" title="Settings" onClick={() => setOpen(open === e.id ? null : e.id)}>
            ⚙
          </button>
          <button class="chip-act" title="Remove this effect" onClick={() => run(`afx ${target}.${e.id} del`, true)}>
            ✕
          </button>
        </span>
      ))}
      {shown && <BusFxSettings e={shown} target={target} />}
      <select
        value=""
        title="Add an audio effect"
        onChange={(e) => {
          const v = (e.target as HTMLSelectElement).value;
          if (v) run(`afx ${target} ${v}`, true);
        }}
      >
        <option value="">+ fx</option>
        {cat?.audio_effects.map((f) => (
          <option key={f.name}>{f.name}</option>
        ))}
      </select>
      {list.length > 0 && (
        <button class="btn sm ghost" title="Remove all effects" onClick={() => run(`afx ${target} clear`, true)}>
          ✕
        </button>
      )}
    </div>
  );
}

function Strip({ t }: { t: Track }) {
  const vol = t.vol ?? 0;
  return (
    <div class="strip">
      <b>{t.id}</b>
      <small title={t.name}>{t.name || (t.duck ? `ducks under ${t.duck.by}` : " ")}</small>
      <input class="pan" type="range" min={-1} max={1} step={0.05} value={t.pan ?? 0} title={`pan ${fmtNum(t.pan ?? 0, 2)} (double-click: center)`} onDblClick={() => run(`track set ${t.id} pan=0`, true)} onChange={(e) => run(`track set ${t.id} pan=${(e.target as HTMLInputElement).value}`, true)} />
      <input class="fader" type="range" min={-60} max={12} step={0.5} value={vol} title="Track volume (double-click: 0 dB)" onDblClick={() => run(`track set ${t.id} vol=0`, true)} onChange={(e) => run(`track set ${t.id} vol=${(e.target as HTMLInputElement).value}`, true)} />
      <NumField value={vol} step={0.5} min={-96} max={24} onCommit={(v) => run(`track set ${t.id} vol=${fmtNum(v, 2)}`, true)} />
      <div class="ms">
        <button class={"btn sm" + (t.muted ? " on" : "")} onClick={() => run(`track set ${t.id} mute=${t.muted ? 0 : 1}`, true)}>
          M
        </button>
        <button class={"btn sm" + (t.solo ? " on" : "")} onClick={() => run(`track set ${t.id} solo=${t.solo ? 0 : 1}`, true)}>
          S
        </button>
      </div>
      <FxChips list={t.afx || []} target={t.id} />
      <Routing id={t.id} out={t.out} sends={t.sends || []} verb={`track set ${t.id}`} />
    </div>
  );
}

/** Output bus and sends of a track or bus: `track set A1 out=B1`, `send A1 B2 -12dB`. */
function Routing({ id, out, sends, verb }: { id: string; out?: string; sends: { to: string; db: number; pre?: boolean }[]; verb: string }) {
  const buses = (comp.value?.buses || []).filter((b) => b.id !== id);
  if (!buses.length) return null;
  return (
    <div class="routing">
      <select value={out || "master"} title="Output" onChange={(e) => run(`${verb} out=${(e.target as HTMLSelectElement).value}`, true)}>
        <option value="master">→ master</option>
        {buses.map((b) => (
          <option key={b.id} value={b.id}>→ {b.id}{b.name ? ` ${b.name}` : ""}</option>
        ))}
      </select>
      {sends.map((s) => (
        <div key={s.to} class="send" title={`send to ${s.to}${s.pre ? " (pre-fader)" : ""}`}>
          <small>{s.to}</small>
          <NumField value={s.db} step={1} min={-96} max={12} onCommit={(v) => run(`send ${id} ${s.to} ${fmtNum(v, 1)}dB${s.pre ? " pre" : ""}`, true)} />
          <button class="btn sm ghost" title="Remove the send" onClick={() => run(`send ${id} ${s.to} off`, true)}>×</button>
        </div>
      ))}
      <select value="" title="Add a send" onChange={(e) => { const v = (e.target as HTMLSelectElement).value; if (v) run(`send ${id} ${v} -12dB`, true); }}>
        <option value="">+ send</option>
        {buses.filter((b) => !sends.some((s) => s.to === b.id)).map((b) => (
          <option key={b.id} value={b.id}>{b.id}{b.name ? ` ${b.name}` : ""}</option>
        ))}
      </select>
    </div>
  );
}

/** A bus strip: volume, pan, mute, effects and its own routing. */
function BusStrip({ b }: { b: Bus }) {
  const vol = b.vol ?? 0;
  return (
    <div class="strip bus">
      <b>{b.id}</b>
      <small title={b.name}>{b.name || (b.duck ? `ducks under ${b.duck.by}` : "bus")}</small>
      <input class="pan" type="range" min={-1} max={1} step={0.05} value={b.pan ?? 0} onDblClick={() => run(`bus set ${b.id} pan=0`, true)} onChange={(e) => run(`bus set ${b.id} pan=${(e.target as HTMLInputElement).value}`, true)} />
      <input class="fader" type="range" min={-60} max={12} step={0.5} value={vol} title="Bus volume (double-click: 0 dB)" onDblClick={() => run(`bus set ${b.id} vol=0`, true)} onChange={(e) => run(`bus set ${b.id} vol=${(e.target as HTMLInputElement).value}`, true)} />
      <NumField value={vol} step={0.5} min={-96} max={24} onCommit={(v) => run(`bus set ${b.id} vol=${fmtNum(v, 2)}`, true)} />
      <div class="ms">
        <button class={"btn sm" + (b.muted ? " on" : "")} onClick={() => run(`bus set ${b.id} mute=${b.muted ? 0 : 1}`, true)}>
          M
        </button>
        <button class="btn sm ghost" title="Delete the bus (its sources go back to the master)" onClick={() => run(`bus del ${b.id}`, true)}>
          ×
        </button>
      </div>
      <FxChips list={b.afx || []} target={b.id} />
      <Routing id={b.id} out={b.out} sends={b.sends || []} verb={`bus set ${b.id}`} />
    </div>
  );
}

/** Essential Sound: tag clips as dialogue, music, effects or ambience. */
export function EssentialSound() {
  const [amount, setAmount] = useState(1);
  const [match, setMatch] = useState(true);
  const [duck, setDuck] = useState(-12);
  const clips = selection.value.filter((id) => {
    const f = findItem(id);
    const md = f?.item.src ? mediaById.value.get(f.item.src) : undefined;
    return !!md && (md.has_audio || md.kind === "audio");
  });
  const sel = clips.length ? findItem(clips[clips.length - 1])?.item : undefined;
  const kind = sel?.afx?.some((e) => e.kind === "voice") ? "dialogue" : undefined;
  return (
    <div class="scroll audio-panel">
      <div class="group-b" style={{ paddingTop: "10px" }}>
        {clips.length === 0 ? (
          <div class="hint">Select clips with audio, then tag them. Music and ambience duck under the dialogue automatically.</div>
        ) : (
          <>
            <div class="hint">
              {clips.length} clip{clips.length > 1 ? "s" : ""}: {clips.join(", ")}
            </div>
            <div class="sound-types">
              {TYPES.map((ty) => (
                <button key={ty.k} class={"btn" + (kind === ty.k ? " on" : "")} title={ty.desc} onClick={() => run(`sound ${clips.join(",")} ${ty.k} amount=${fmtNum(amount, 2)} match=${match ? 1 : 0}${ty.k === "music" || ty.k === "ambience" ? ` duck=${duck}` : ""}`)}>
                  <span>{ty.icon}</span>
                  {ty.k}
                </button>
              ))}
            </div>
            <Slider label="Strength" value={amount} min={0} max={1} step={0.05} def={1} onCommit={setAmount} title="How strongly the dialogue chain is applied" />
            <Slider label="Duck music" value={duck} min={-30} max={0} step={1} def={-12} onCommit={setDuck} title="dB that music / ambience drops under dialogue" />
            <div class="row">
              <label title="Match each clip to the loudness target of its type">Loudness</label>
              <div class="ctl">
                <input type="checkbox" checked={match} onChange={(e) => setMatch((e.target as HTMLInputElement).checked)} />
                <button class="btn sm" style={{ marginLeft: "auto", flex: "0 0 auto" }} title="Measure hum, clipping, rumble and noise, then add only the fixes needed" onClick={() => run(`repair ${clips.join(",")}`)}>
                  Repair
                </button>
                <button class="btn sm" style={{ flex: "0 0 auto" }} onClick={() => run(`sound ${clips.join(",")} clear`)}>
                  Clear tags
                </button>
              </div>
              <span />
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/** Audio Track Mixer: a strip per audio track and the master. */
export function AudioMixer() {
  const c = comp.value;
  if (!c) return null;
  const m = c.master || {};
  const audio = c.tracks.filter((t) => t.kind === "audio").sort((a, b) => parseInt(a.id.slice(1)) - parseInt(b.id.slice(1)));
  return (
    <div class="scroll audio-panel">
      <div class="mixer">
        {audio.map((t) => (
          <Strip key={t.id} t={t} />
        ))}
        {(c.buses || []).map((b) => (
          <BusStrip key={b.id} b={b} />
        ))}
        <div class="strip master">
          <b>Master</b>
          <small>{m.lufs !== undefined ? `${m.lufs} LUFS` : "\u00a0"}</small>
          <Meter />
          <input class="fader" type="range" min={-60} max={12} step={0.5} value={m.vol || 0} title="Master volume (double-click: 0 dB)" onDblClick={() => run("master vol=0", true)} onChange={(e) => run(`master vol=${(e.target as HTMLInputElement).value}`, true)} />
          <NumField value={m.vol || 0} step={0.5} min={-96} max={24} onCommit={(v) => run(`master vol=${fmtNum(v, 2)}`, true)} />
          <select value={m.lufs === undefined ? "off" : String(m.lufs)} title="Normalize the export to a loudness target" onChange={(e) => run(`master lufs=${(e.target as HTMLSelectElement).value}`, true)}>
            <option value="off">no target</option>
            <option value="-14">-14 LUFS</option>
            <option value="-16">-16 LUFS</option>
            <option value="-23">-23 LUFS</option>
          </select>
          <label class="lim">
            <input type="checkbox" checked={!!m.limiter} onChange={(e) => run(`master limiter=${(e.target as HTMLInputElement).checked ? 1 : 0}`, true)} />
            limiter
          </label>
          <label class="lim" title="Limit true (inter-sample) peaks for -1 dBTP delivery">
            <input type="checkbox" checked={!!m.truepeak} onChange={(e) => run(`master truepeak=${(e.target as HTMLInputElement).checked ? 1 : 0}`, true)} />
            true peak
          </label>
          <FxChips list={m.afx || []} target="master" />
        </div>
        <button class="btn sm add-track" title="Add an audio track" onClick={() => run("track add A", true)}>
          + A
        </button>
        <button class="btn sm add-track" title="Add a submix bus (dialogue, music, a shared reverb)" onClick={() => run("bus add", true)}>
          + Bus
        </button>
      </div>
    </div>
  );
}
