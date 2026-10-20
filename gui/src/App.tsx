import { useEffect, useRef, useState } from "preact/hooks";
import { installShortcuts } from "./actions";
import { CheckPanel } from "./components/CheckPanel";
import { Dock } from "./components/Dock";
import { FfmpegBanner } from "./components/FfmpegBanner";
import { GraphicEditor } from "./components/GraphicEditor";
import { Home, showHome } from "./components/Home";
import { ConnectAI, FileBrowser, RenderPanel, Toasts } from "./components/Modals";
import { LogoMark, Wordmark } from "./brand";
import { ALL_PANELS, closePanel, isOpen, maximized, narrow, PANEL_TITLES, resetWorkspace, setLayout, showPanel, switchWorkspace, workspace, WORKSPACES } from "./dock";
import { aiPulse, connected, dirty, logs, pickFiles, project, project_, projectPath, redoN, rev, run, showCheck, showRender, undoN } from "./store";

/** Workspace switcher, like the tabs at the top of Premiere Pro and After Effects. */
function Workspaces() {
  if (narrow.value) return null;
  return (
    <div class="workspaces" role="tablist" aria-label="Workspaces">
      {WORKSPACES.map((w) => (
        <button key={w.id} role="tab" aria-selected={workspace.value === w.id} class={"ws" + (workspace.value === w.id ? " on" : "")} title={w.desc} onClick={() => switchWorkspace(w.id)}>
          {w.name}
        </button>
      ))}
    </div>
  );
}

/** Window menu: open or close any panel, reset the workspace, maximize. */
function WindowMenu() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const off = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    window.addEventListener("pointerdown", off, true);
    return () => window.removeEventListener("pointerdown", off, true);
  }, [open]);
  return (
    <div class="dock-menu" ref={ref}>
      <button class={"btn sm ghost" + (open ? " on" : "")} onClick={() => setOpen(!open)} title="Panels and workspaces">
        Window ▾
      </button>
      {open && (
        <div class="menu-pop wide">
          <div class="menu-head">Panels</div>
          {ALL_PANELS.map((p) => {
            const on = isOpen(p);
            return (
              <div key={p} class="menu-item" onClick={() => (on ? setLayout((l) => closePanel(l, p)) : showPanel(p))}>
                <span class="check">{on ? "✓" : ""}</span>
                {PANEL_TITLES[p]}
              </div>
            );
          })}
          <div class="menu-sep" />
          <div class="menu-head">Workspace</div>
          {narrow.value && <div class="menu-item disabled">Narrow window: compact layout</div>}
          {WORKSPACES.map((w) => (
            <div key={w.id} class="menu-item" onClick={() => (switchWorkspace(w.id), setOpen(false))}>
              <span class="check">{workspace.value === w.id ? "•" : ""}</span>
              {w.name}
            </div>
          ))}
          <div class="menu-sep" />
          <div class="menu-item" onClick={() => (resetWorkspace(), setOpen(false))}>
            Reset to saved layout
          </div>
          {maximized.value && (
            <div class="menu-item" onClick={() => (maximized.value = null, setOpen(false))}>
              Restore maximized panel <span class="kbd">`</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

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
      <div class="brand" title="MoodCut Home: new project, recent projects, setup" onClick={() => (showHome.value = true)}>
        <LogoMark size={24} class="brand-mark" />
        <Wordmark />
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
      <WindowMenu />
      <span class="spacer" />
      <Workspaces />
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

export function App() {
  const [connect, setConnect] = useState(false);
  useEffect(installShortcuts, []);
  return (
    <div class="app">
      <TopBar onConnect={() => setConnect(true)} />
      <FfmpegBanner />
      <Dock />
      <RenderPanel />
      <FileBrowser />
      <GraphicEditor />
      <CheckPanel />
      <Home onConnect={() => setConnect(true)} />
      {connect && <ConnectAI onClose={() => setConnect(false)} />}
      <Toasts />
    </div>
  );
}
