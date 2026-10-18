// Essential Sound and the Audio Track Mixer. Every control is a script line (sound, track set, master, afx).
import { useState } from "preact/hooks";
import { levels } from "../meter";
import { catalog, comp, findItem, mediaById, run, selection } from "../store";
import type { Effect, Track } from "../types";
import { fmtNum } from "../util";
import { NumField, Section, Slider } from "./Controls";

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

function FxChips({ list, target }: { list: Effect[]; target: string }) {
  const cat = catalog.value;
  return (
    <div class="fx-chips">
      {list.map((e) => (
        <span key={e.id} class={"chip" + (e.on === false ? " off" : "")} title={e.id}>
          {e.kind}
        </span>
      ))}
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
    </div>
  );
}

export function AudioPanel() {
  const c = comp.value;
  const [amount, setAmount] = useState(1);
  const [match, setMatch] = useState(true);
  const [duck, setDuck] = useState(-12);
  if (!c) return null;
  const m = c.master || {};
  const clips = selection.value.filter((id) => {
    const f = findItem(id);
    const md = f?.item.src ? mediaById.value.get(f.item.src) : undefined;
    return !!md && (md.has_audio || md.kind === "audio");
  });
  const audio = c.tracks.filter((t) => t.kind === "audio").sort((a, b) => parseInt(a.id.slice(1)) - parseInt(b.id.slice(1)));
  const sel = clips.length ? findItem(clips[clips.length - 1])?.item : undefined;
  const kind = sel?.afx?.some((e) => e.kind === "voice") ? "dialogue" : undefined;
  return (
    <div class="scroll audio-panel">
      <Section title="Essential Sound">
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
                <button class="btn sm" style={{ marginLeft: "auto", flex: "0 0 auto" }} onClick={() => run(`sound ${clips.join(",")} clear`)}>
                  Clear tags
                </button>
              </div>
              <span />
            </div>
          </>
        )}
      </Section>
      <Section title="Audio Track Mixer">
        <div class="mixer">
          {audio.map((t) => (
            <Strip key={t.id} t={t} />
          ))}
          <div class="strip master">
            <b>Master</b>
            <small>{m.lufs !== undefined ? `${m.lufs} LUFS` : " "}</small>
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
            <FxChips list={m.afx || []} target="master" />
          </div>
        </div>
        <div style={{ display: "flex", gap: "6px", marginTop: "8px" }}>
          <button class="btn sm" onClick={() => run("track add A", true)}>
            + Audio track
          </button>
        </div>
      </Section>
    </div>
  );
}
