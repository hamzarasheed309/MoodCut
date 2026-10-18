import { useEffect, useRef, useState } from "preact/hooks";
import { Console } from "./components/Console";
import { Inspector } from "./components/Inspector";
import { LeftPanel } from "./components/LeftPanel";
import { GraphicEditor } from "./components/GraphicEditor";
import { ConnectAI, FileBrowser, RenderPanel, Toasts } from "./components/Modals";
import { Monitor } from "./components/Monitor";
import { Timeline } from "./components/Timeline";
import {
  aiPulse,
  comp,
  connected,
  dirty,
  fps,
  logs,
  pickFiles,
  playhead,
  playing,
  project,
  project_,
  projectPath,
  redoN,
  rev,
  run,
  scrollX,
  selection,
  setPlayhead,
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
      <button class="btn sm primary" onClick={() => (showRender.value = true)}>
        Export
      </button>
    </div>
  );
}

function TimelineToolbar() {
  const c = comp.value;
  return (
    <div class="tl-toolbar">
      <button class={"btn sm" + (tool.value === "select" ? " on" : "")} title="Select (V)" onClick={() => (tool.value = "select")}>
        ↖ Select
      </button>
      <button class={"btn sm" + (tool.value === "razor" ? " on" : "")} title="Razor (C)" onClick={() => (tool.value = "razor")}>
        ✂ Razor
      </button>
      <button class="btn sm" title="Split selected / all at playhead (Ctrl+K)" onClick={splitAtPlayhead}>
        Split
      </button>
      <button class="btn sm" title="Add marker (M)" onClick={() => run(`marker add ${fmtNum(playhead.value, 4)}`, true)}>
        ◆ Marker
      </button>
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

function fitTimeline() {
  const c = comp.value;
  if (!c) return;
  const w = (document.querySelector(".tl-canvas-wrap") as HTMLElement)?.clientWidth || 800;
  zoom.value = Math.max(2, (w - 40) / Math.max(1, compDuration(c)));
  scrollX.value = 0;
}

function splitAtPlayhead() {
  const t = fmtNum(playhead.value, 4);
  const sel = selection.value;
  if (sel.length) run(`cut ${sel.join(",")} at=${t}`);
  else run(`cut all at=${t}`);
}

export function App() {
  const [tlH, setTlH] = useState(320);
  const [connect, setConnect] = useState(false);
  const [rightTab, setRightTab] = useState<"inspector" | "ai">("inspector");
  const resizing = useRef(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
      const mod = e.ctrlKey || e.metaKey;
      const step = 1 / fps.value;
      if (e.key === " ") {
        e.preventDefault();
        playing.value = !playing.value;
      } else if (mod && e.key.toLowerCase() === "z") {
        e.preventDefault();
        run(e.shiftKey ? "redo" : "undo", true);
      } else if (mod && e.key.toLowerCase() === "y") {
        e.preventDefault();
        run("redo", true);
      } else if (mod && e.key.toLowerCase() === "k") {
        e.preventDefault();
        splitAtPlayhead();
      } else if (mod && e.key.toLowerCase() === "s") {
        e.preventDefault();
        if (projectPath.value) project_({ action: "save" });
      } else if (mod && e.key.toLowerCase() === "e") {
        e.preventDefault();
        showRender.value = true;
      } else if (e.key === "Delete" || e.key === "Backspace") {
        if (selection.value.length) run(`del ${selection.value.join(",")}${e.shiftKey ? " ripple" : ""}`, true);
      } else if (e.key === "ArrowLeft") {
        setPlayhead(Math.max(0, playhead.value - (e.shiftKey ? 1 : step)));
      } else if (e.key === "ArrowRight") {
        setPlayhead(playhead.value + (e.shiftKey ? 1 : step));
      } else if (e.key === "Home") {
        setPlayhead(0);
      } else if (e.key === "End") {
        const c = comp.value;
        if (c) setPlayhead(compDuration(c));
      } else if (e.key.toLowerCase() === "c" && !mod) {
        tool.value = "razor";
      } else if (e.key.toLowerCase() === "v" && !mod) {
        tool.value = "select";
      } else if (e.key.toLowerCase() === "m" && !mod) {
        run(`marker add ${fmtNum(playhead.value, 4)}`, true);
      } else if (e.key.toLowerCase() === "d" && mod) {
        e.preventDefault();
        if (selection.value.length) run(`dup ${selection.value[selection.value.length - 1]}`);
      } else if (e.key === "+" || e.key === "=") {
        zoom.value = Math.min(3000, zoom.value * 1.25);
      } else if (e.key === "-") {
        zoom.value = Math.max(2, zoom.value / 1.25);
      } else if (e.key === "\\") {
        fitTimeline();
      } else if (e.key === "Escape") {
        selection.value = [];
        tool.value = "select";
      } else if (mod && e.key.toLowerCase() === "a") {
        e.preventDefault();
        selection.value = (comp.value?.tracks || []).flatMap((t) => t.items.map((i) => i.id));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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
      <LeftPanel />
      <Monitor />
      <div class="panel right">
        <div class="tabs">
          <div class={"tab" + (rightTab === "inspector" ? " active" : "")} onClick={() => setRightTab("inspector")}>
            Inspector
          </div>
          <div class={"tab" + (rightTab === "ai" ? " active" : "")} onClick={() => setRightTab("ai")}>
            AI & Script
          </div>
        </div>
        {rightTab === "inspector" ? <Inspector /> : <Console />}
      </div>
      <div class="split" onPointerDown={() => (resizing.current = true)} />
      <div class="panel tl">
        <TimelineToolbar />
        <Timeline />
      </div>
      <RenderPanel />
      <FileBrowser />
      <GraphicEditor />
      {connect && <ConnectAI onClose={() => setConnect(false)} />}
      <Toasts />
    </div>
  );
}
