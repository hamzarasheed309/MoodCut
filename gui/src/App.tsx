import { useEffect, useRef, useState } from "preact/hooks";
import { fitTimeline, installShortcuts, liftOrExtract, markWork, splitAtPlayhead } from "./actions";
import { AudioPanel } from "./components/AudioPanel";
import { ColorPanel } from "./components/ColorPanel";
import { HistoryPanel } from "./components/HistoryPanel";
import { MarkersPanel } from "./components/MarkersPanel";
import { ToolsPanel, toolName } from "./components/Tools";
import { Console } from "./components/Console";
import { Inspector } from "./components/Inspector";
import { LeftPanel } from "./components/LeftPanel";
import { CheckPanel } from "./components/CheckPanel";
import { FfmpegBanner } from "./components/FfmpegBanner";
import { GraphicEditor } from "./components/GraphicEditor";
import { ConnectAI, FileBrowser, RenderPanel, Toasts } from "./components/Modals";
import { Monitor } from "./components/Monitor";
import { Timeline } from "./components/Timeline";
import {
  aiPulse,
  comp,
  connected,
  dirty,
  logs,
  pickFiles,
  playhead,
  project,
  project_,
  projectPath,
  redoN,
  rev,
  run,
  showCheck,
  showRender,
  tool,
  undoN,
  zoom,
} from "./store";
import { compDuration, fmtNum } from "./util";

function TopBar({ onConnect }: { onConnect: () => void }) {
  const p = project.value;
  const [pulse, setPulse] = useState(false);
  useEffect(() => {
    if (aiPulse.value === 0) return;
    setPulse(true);
    const h = setTimeout(() => setPulse(false), 1000);
    return () => clearTimeout(h);
  }, [aiPulse.value]);
  const aiEdits = logs.value.filter((l) => l.source === "mcp").length;
  return (
    <div class="topbar">
      <div class="brand">
        <div class="brand-mark" />
        MoodCut
      </div>
      <button class="btn sm ghost" onClick={async () => {
        const name = prompt("Project name", "Untitled");
        if (name) project_({ action: "new", name, size: "1920x1080", fps: 30 });
      }}>
        New
      </button>
      <button class="btn sm ghost" onClick={async () => {
        const f = await pickFiles("open", "project");
        if (f[0]) project_({ action: "open", path: f[0] });
      }}>
        Open
      </button>
      <button class="btn sm ghost" onClick={async () => {
        if (projectPath.value) return project_({ action: "save" });
        const f = await pickFiles("save", "project");
        if (f[0]) project_({ action: "save", path: f[0] });
      }}>
        Save{dirty.value ? " •" : ""}
      </button>
      <span class="proj" title={projectPath.value || "not saved yet (autosaved)"}>
        {p?.name} <span style={{ color: "var(--faint)" }}>· rev {rev.value}</span>
      </span>
      <span class="spacer" />
      <button class="btn sm ghost icon" disabled={!undoN.value} title="Undo (Ctrl+Z)" onClick={() => run("undo", true)}>
        ↶
      </button>
      <button class="btn sm ghost icon" disabled={!redoN.value} title="Redo (Ctrl+Shift+Z)" onClick={() => run("redo", true)}>
        ↷
      </button>
      <div class="ai-pill" title="AI edits arrive over MCP and show up live" onClick={onConnect} style={{ cursor: "pointer" }}>
        <span class={"ai-dot" + (connected.value ? " live" : "") + (pulse ? " pulse" : "")} />
        {connected.value ? `AI link · ${aiEdits} edit${aiEdits === 1 ? "" : "s"}` : "reconnecting…"}
      </div>
      <button class="btn sm" onClick={onConnect}>
        Connect AI
      </button>
      <button class="btn sm" title="Find problems before exporting (gaps, title-safe, contrast, loudness…)" onClick={() => (showCheck.value = true)}>
        Check
      </button>
      <button class="btn sm primary" onClick={() => (showRender.value = true)}>
        Export
      </button>
    </div>
  );
}

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

type RightTab = "inspector" | "color" | "audio" | "markers" | "history" | "ai";
const RIGHT_TABS: [RightTab, string, string][] = [
  ["inspector", "Controls", "Inspector: properties, effects, masks, keyframes"],
  ["color", "Lumetri", "Lumetri Color: correction, curves, wheels, HSL secondary"],
  ["audio", "Audio", "Essential Sound and the Audio Track Mixer"],
  ["markers", "Markers", "Markers"],
  ["history", "History", "Undo history (yours and the AI's)"],
  ["ai", "AI", "AI & Script console"],
];

export function App() {
  const [tlH, setTlH] = useState(320);
  const [connect, setConnect] = useState(false);
  const [rightTab, setRightTab] = useState<RightTab>(() => {
    try {
      return (localStorage.getItem("moodcut.rightTab") as RightTab) || "inspector";
    } catch {
      return "inspector";
    }
  });
  const pickTab = (t: RightTab) => {
    setRightTab(t);
    try {
      localStorage.setItem("moodcut.rightTab", t);
    } catch {}
  };
  const resizing = useRef(false);

  useEffect(installShortcuts, []);

  return (
    <div
      class="app"
      style={{ "--tl-h": tlH + "px" } as any}
      onPointerMove={(e) => {
        if (resizing.current) setTlH(Math.max(140, Math.min(window.innerHeight - 220, window.innerHeight - e.clientY)));
      }}
      onPointerUp={() => (resizing.current = false)}
    >
      <TopBar onConnect={() => setConnect(true)} />
      <FfmpegBanner />
      <LeftPanel />
      <Monitor />
      <div class="panel right">
        <div class="tabs compact">
          {RIGHT_TABS.map(([k, name, tip]) => (
            <div key={k} class={"tab" + (rightTab === k ? " active" : "")} title={tip} onClick={() => pickTab(k)}>
              {name}
            </div>
          ))}
        </div>
        {rightTab === "inspector" ? <Inspector /> : rightTab === "color" ? <ColorPanel /> : rightTab === "audio" ? <AudioPanel /> : rightTab === "markers" ? <MarkersPanel /> : rightTab === "history" ? <HistoryPanel /> : <Console />}
      </div>
      <div class="split" onPointerDown={() => (resizing.current = true)} />
      <div class="panel tl">
        <ToolsPanel />
        <div class="tl-main">
          <TimelineToolbar />
          <Timeline />
        </div>
      </div>
      <RenderPanel />
      <FileBrowser />
      <GraphicEditor />
      <CheckPanel />
      {connect && <ConnectAI onClose={() => setConnect(false)} />}
      <Toasts />
    </div>
  );
}
