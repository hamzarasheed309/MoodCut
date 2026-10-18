// Markers panel: every comp marker with its time, name, color, duration and note.
import { comp, fps, playhead, run, setPlayhead } from "../store";
import type { Marker } from "../types";
import { fmtNum, fmtTC, q } from "../util";
import { NumField, TextField } from "./Controls";

const COLORS = ["#fbbf24", "#ff5a5f", "#34d399", "#60a5fa", "#a78bfa", "#ec4899", "#f97316", "#e7e7ee"];

/** Markers are addressed by time (names need not be unique). */
const key = (m: Marker) => fmtNum(m.t, 4);

export function MarkersPanel() {
  const c = comp.value;
  const ms = c?.markers || [];
  const set = (m: Marker, kv: string) => run(`marker set ${key(m)} ${kv}`, true);
  return (
    <div class="scroll">
      <div class="panel-bar">
        <button class="btn sm primary" title="Add a marker at the playhead (M)" onClick={() => run(`marker add ${fmtNum(playhead.value, 4)}`, true)}>
          ◆ Add at playhead
        </button>
        <span style={{ flex: 1 }} />
        {ms.length > 0 && (
          <button class="btn sm ghost" onClick={() => confirm(`Delete all ${ms.length} markers?`) && run("marker clear", true)}>
            Clear all
          </button>
        )}
      </div>
      {ms.length === 0 && (
        <div class="empty">
          No markers. Press <span class="kbd">M</span> at the playhead, or ask the AI to mark the beats.
          <br />
          Markers are snap points and can be used as times: <code>at=@drop</code>
        </div>
      )}
      {ms.map((m) => {
        const here = Math.abs(m.t - playhead.value) < 0.5 / fps.value;
        return (
          <div class={"marker-row" + (here ? " here" : "")} key={key(m)}>
            <span class="swatch" style={{ background: m.color || "#fbbf24" }} title="Change color">
              <select value={m.color || "#fbbf24"} onChange={(e) => set(m, `color=${(e.target as HTMLSelectElement).value}`)}>
                {COLORS.map((cc) => (
                  <option key={cc} value={cc} style={{ background: cc }}>
                    {cc}
                  </option>
                ))}
              </select>
            </span>
            <button class="tc-btn" title="Go to marker" onClick={() => setPlayhead(m.t)}>
              {fmtTC(m.t, fps.value)}
            </button>
            <TextField cls="mk-name" value={m.name} placeholder="name" onCommit={(v) => set(m, `name=${q(v)}`)} />
            <span class="mk-dur" title="Duration (s): a range marker">
              <NumField value={m.dur || 0} step={1 / fps.value} min={0} onCommit={(v) => set(m, `dur=${fmtNum(v, 4)}`)} />
            </span>
            <button class="btn sm ghost" title="Delete marker" onClick={() => run(`marker del ${key(m)}`, true)}>
              ✕
            </button>
            <TextField cls="mk-note" value={m.note || ""} placeholder="comment…" onCommit={(v) => set(m, `note=${q(v)}`)} />
          </div>
        );
      })}
    </div>
  );
}
