// Timeline panel: its toolbar (tool name, split, markers, work area, lift/extract, zoom) and the
// canvas timeline. When the Tools panel is closed the timeline carries its own tool strip.
import { fitTimeline, liftOrExtract, markWork, splitAtPlayhead } from "../actions";
import { isOpen } from "../dock";
import { comp, playhead, run, tool, zoom } from "../store";
import { compDuration, fmtNum } from "../util";
import { Timeline } from "./Timeline";
import { ToolsPanel, toolName } from "./Tools";

function TimelineToolbar() {
  const c = comp.value;
  const w = c?.work;
  return (
    <div class="tl-toolbar">
      <span class="tool-name" title="Current tool (see the Tools strip on the left)">{toolName(tool.value)}</span>
      <button class="btn sm" title="Split selected / all at playhead (Ctrl+K)" onClick={splitAtPlayhead}>
        Split
      </button>
      <button class="btn sm" title="Add marker (M)" onClick={() => run(`marker add ${fmtNum(playhead.value, 4)}`, true)}>
        ◆ Marker
      </button>
      <span class="sep" />
      <button class="btn sm" title="Work area in at playhead (I)" onClick={() => markWork("in")}>
        {"{"} In
      </button>
      <button class="btn sm" title="Work area out at playhead (O)" onClick={() => markWork("out")}>
        Out {"}"}
      </button>
      {w && (
        <>
          <span class="badge" title="Work area (render range=work; lift and extract use it). Shift+drag on the ruler to set it.">
            {fmtNum(w[0], 2)}–{fmtNum(w[1], 2)}s
          </span>
          <button class="btn sm ghost" title="Lift: remove the work area, leave a gap (;)" onClick={() => liftOrExtract("lift")}>
            Lift
          </button>
          <button class="btn sm ghost" title="Extract: remove the work area and close the gap (')" onClick={() => liftOrExtract("extract")}>
            Extract
          </button>
          <button class="btn sm ghost" title="Clear work area (Ctrl+Shift+X)" onClick={() => run("workarea clear", true)}>
            ✕
          </button>
        </>
      )}
      <span style={{ flex: 1 }} />
      <span style={{ color: "var(--faint)", fontSize: "12px" }}>{c ? `${c.tracks.reduce((n, t) => n + t.items.length, 0)} items · ${fmtNum(compDuration(c), 2)}s` : ""}</span>
      <button class="btn sm ghost" title="Zoom out (-)" onClick={() => (zoom.value = Math.max(2, zoom.value / 1.4))}>
        −
      </button>
      <input type="range" min="1" max="100" value={Math.round((Math.log(zoom.value) / Math.log(3000)) * 100)} onInput={(e) => (zoom.value = Math.max(2, Math.pow(3000, parseFloat((e.target as HTMLInputElement).value) / 100)))} style={{ width: "120px" }} />
      <button class="btn sm ghost" title="Zoom in (+)" onClick={() => (zoom.value = Math.min(3000, zoom.value * 1.4))}>
        +
      </button>
      <button class="btn sm" title="Fit timeline (\)" onClick={fitTimeline}>
        Fit
      </button>
    </div>
  );
}

export function TimelinePanel() {
  return (
    <div class="tl">
      {!isOpen("tools") && <ToolsPanel />}
      <div class="tl-main">
        <TimelineToolbar />
        <Timeline />
      </div>
    </div>
  );
}
