import { useEffect, useState } from "preact/hooks";
import { installShortcuts } from "./actions";
import { CheckPanel } from "./components/CheckPanel";
import { Dock } from "./components/Dock";
import { FfmpegBanner } from "./components/FfmpegBanner";
import { GraphicEditor } from "./components/GraphicEditor";
import { Home, showHome } from "./components/Home";
import { MenuBar, ShortcutsModal } from "./components/MenuBar";
import { ConnectAI, FileBrowser, RenderPanel, Toasts } from "./components/Modals";
import { AIServices } from "./components/aiui";
import { LogoMark } from "./brand";
import { narrow, switchWorkspace, workspace, WORKSPACES } from "./dock";
import { aiPulse, connected, dirty, logs, project, projectPath, redoN, rev, run, showCheck, showRender, undoN } from "./store";
import { ErrorBoundary } from "./components/ErrorBoundary";

/** Workspace switcher, the tabs in the middle of Premiere Pro's header. */
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

const Ico = ({ d }: { d: string }) => (
  <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
    <path d={d} fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
  </svg>
);

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
        <LogoMark size={18} class="brand-mark" />
      </div>
      <MenuBar onConnect={onConnect} />
      <span class="proj" title={projectPath.value || "not saved to a file yet (autosaved; recover it from Home)"}>
        {p?.name}
        {dirty.value ? " *" : ""} <span class="proj-rev">rev {rev.value}</span>
      </span>
      <span class="spacer" />
      <Workspaces />
      <span class="spacer" />
      <button class="btn sm ghost icon" disabled={!undoN.value} title="Undo (Ctrl+Z)" onClick={() => run("undo", true)}>
        <Ico d="M5.5 4 2.5 7l3 3M3 7h6.5a3.5 3.5 0 0 1 0 7H7" />
      </button>
      <button class="btn sm ghost icon" disabled={!redoN.value} title="Redo (Ctrl+Shift+Z)" onClick={() => run("redo", true)}>
        <Ico d="M10.5 4l3 3-3 3M13 7H6.5a3.5 3.5 0 0 0 0 7H9" />
      </button>
      <div class="ai-pill" title="AI apps edit over MCP and their edits show up live. Click to connect one." onClick={onConnect}>
        <span class={"ai-dot" + (connected.value ? " live" : "") + (pulse ? " pulse" : "")} />
        {connected.value ? `AI link · ${aiEdits} edit${aiEdits === 1 ? "" : "s"}` : "reconnecting…"}
      </div>
      <button class="btn sm" title="Find problems before exporting (gaps, title-safe, contrast, loudness…)" onClick={() => (showCheck.value = true)}>
        Check
      </button>
      <button class="btn sm primary" title="Export Media (Ctrl+M)" onClick={() => (showRender.value = true)}>
        <Ico d="M8 2.5v7.5M5 7l3 3 3-3M3 12.5h10" /> Export
      </button>
    </div>
  );
}

export function App() {
  const [connect, setConnect] = useState(false);
  useEffect(installShortcuts, []);
  return (
    <div class="app">
      <ErrorBoundary name="The top bar">
        <TopBar onConnect={() => setConnect(true)} />
      </ErrorBoundary>
      <FfmpegBanner />
      <ErrorBoundary name="The workspace">
        <Dock />
      </ErrorBoundary>
      <ErrorBoundary name="Export" floating>
        <RenderPanel />
      </ErrorBoundary>
      <ErrorBoundary name="The file browser" floating>
        <FileBrowser />
      </ErrorBoundary>
      <ErrorBoundary name="The graphic editor" floating>
        <GraphicEditor />
      </ErrorBoundary>
      <ErrorBoundary name="Check" floating>
        <CheckPanel />
      </ErrorBoundary>
      <ErrorBoundary name="Home" floating>
        <Home onConnect={() => setConnect(true)} />
      </ErrorBoundary>
      <ErrorBoundary name="AI Services" floating>
        <AIServices />
      </ErrorBoundary>
      <ErrorBoundary name="Keyboard shortcuts" floating>
        <ShortcutsModal />
      </ErrorBoundary>
      {connect && (
        <ErrorBoundary name="Connect AI" floating>
          <ConnectAI onClose={() => setConnect(false)} />
        </ErrorBoundary>
      )}
      <Toasts />
    </div>
  );
}
