import { useEffect, useState } from "preact/hooks";
import { catalog, comp, fileBrowser, jobs, pickFiles, refreshJobs, showRender, toast, toasts } from "../store";
import { compDuration, fmtNum } from "../util";

export function Toasts() {
  return (
    <div class="toasts">
      {toasts.value.map((t) => (
        <div class={"toast" + (t.err ? " err" : "")} key={t.id}>
          {t.msg}
        </div>
      ))}
    </div>
  );
}

export function RenderPanel() {
  const cat = catalog.value;
  const c = comp.value;
  const [preset, setPreset] = useState("mp4");
  const [out, setOut] = useState("");
  const [range, setRange] = useState("");
  const [scale, setScale] = useState("1");
  const [hw, setHw] = useState("off");
  useEffect(() => {
    refreshJobs();
    const h = setInterval(() => {
      if (jobs.value.some((j) => j.state === "running")) refreshJobs();
    }, 700);
    return () => clearInterval(h);
  }, []);
  if (!showRender.value) return null;
  const start = async () => {
    const r = await fetch("/api/render", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ preset, out: out || undefined, range: range || undefined, scale: parseFloat(scale), hw, comp: c?.id }) });
    const j = await r.json();
    if (j.ok === false) toast(j.error, true);
    else toast(`Render ${j.id} started → ${j.out}`);
    refreshJobs();
  };
  return (
    <div class="modal-bg" onClick={() => (showRender.value = false)}>
      <div class="modal" onClick={(e) => e.stopPropagation()}>
        <div class="modal-h">
          Export {c ? `· ${c.name} (${c.w}×${c.h}, ${fmtNum(compDuration(c), 2)}s)` : ""}
          <button class="btn ghost icon x" onClick={() => (showRender.value = false)}>
            ✕
          </button>
        </div>
        <div class="modal-b">
          <div class="field">
            <label>Preset</label>
            <select value={preset} onChange={(e) => setPreset((e.target as HTMLSelectElement).value)}>
              {cat?.presets.map((p) => (
                <option value={p.name} key={p.name}>
                  {p.name} — {p.desc}
                </option>
              ))}
            </select>
          </div>
          <div class="field">
            <label>Output file</label>
            <div style={{ display: "flex", gap: "6px" }}>
              <input style={{ flex: 1 }} placeholder="Videos/MoodCut Renders/… (auto)" value={out} onInput={(e) => setOut((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
              <button class="btn" onClick={async () => {
                const p = await pickFiles("save", "video");
                if (p[0]) setOut(p[0]);
              }}>
                Browse
              </button>
            </div>
          </div>
          <div class="field">
            <label>Range</label>
            <input placeholder="whole comp, or a-b seconds (e.g. 0-15)" value={range} onInput={(e) => setRange((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
          </div>
          <div class="field">
            <label>Resolution</label>
            <select value={scale} onChange={(e) => setScale((e.target as HTMLSelectElement).value)}>
              <option value="1">100% {c ? `(${c.w}×${c.h})` : ""}</option>
              <option value="0.5">50%</option>
              <option value="0.6667">66% (1080p→720p)</option>
              <option value="2">200%</option>
            </select>
          </div>
          <div class="field">
            <label>Encoder</label>
            <select value={hw} onChange={(e) => setHw((e.target as HTMLSelectElement).value)}>
              <option value="off">CPU (best quality)</option>
              <option value="auto">GPU auto (NVENC / QuickSync / AMF)</option>
              <option value="nvenc">NVIDIA NVENC</option>
              <option value="qsv">Intel QuickSync</option>
              <option value="amf">AMD AMF</option>
            </select>
          </div>
          {jobs.value.length > 0 && <div class="section-title" style={{ padding: "6px 0 0" }}>Render queue</div>}
          {jobs.value
            .slice()
            .reverse()
            .map((j) => (
              <div class="job" key={j.id}>
                <div class="meta">
                  <b>{j.id}</b>
                  <span>{j.preset}</span>
                  <span style={{ color: j.state === "error" ? "var(--danger)" : j.state === "done" ? "var(--ok)" : "var(--warn)" }}>{j.state}</span>
                  <span style={{ marginLeft: "auto" }}>{j.state === "running" ? `${j.pct}% · ${j.frames} · eta ${j.eta_s}s` : j.state === "done" ? (j.note ? j.note : `${fmtNum((j.bytes || 0) / 1e6, 1)} MB · ${j.secs}s · ${j.render_fps} fps`) : ""}</span>
                </div>
                <div class="progress">
                  <div style={{ width: `${j.pct ?? 100}%` }} />
                </div>
                <div style={{ fontSize: "11px", color: "var(--faint)", wordBreak: "break-all", userSelect: "text" }}>{j.error || j.out}</div>
              </div>
            ))}
        </div>
        <div class="modal-f">
          <button class="btn" onClick={() => (showRender.value = false)}>
            Close
          </button>
          <button class="btn primary" onClick={start}>
            Render
          </button>
        </div>
      </div>
    </div>
  );
}

export function FileBrowser() {
  const fb = fileBrowser.value;
  const [dir, setDir] = useState<any>(null);
  const [sel, setSel] = useState<string[]>([]);
  const [name, setName] = useState("");
  const load = async (path?: string) => {
    const r = await fetch(`/api/fs${path ? `?path=${encodeURIComponent(path)}` : ""}`);
    setDir(await r.json());
    setSel([]);
  };
  useEffect(() => {
    if (fb) load();
  }, [fb]);
  if (!fb) return null;
  const close = (paths: string[]) => {
    fileBrowser.value = null;
    fb.resolve(paths);
  };
  const sep = dir?.path?.includes("\\") ? "\\" : "/";
  return (
    <div class="modal-bg">
      <div class="modal" style={{ width: "min(720px, calc(100vw - 32px))" }}>
        <div class="modal-h">
          {fb.kind === "save" ? "Save as" : fb.kind === "folder" ? "Choose folder" : "Choose files"}
          <button class="btn ghost icon x" onClick={() => close([])}>
            ✕
          </button>
        </div>
        <div class="modal-b">
          <div style={{ display: "flex", gap: "6px" }}>
            <button class="btn sm" disabled={!dir?.parent} onClick={() => load(dir.parent)}>
              ↑ Up
            </button>
            <input style={{ flex: 1 }} value={dir?.path || ""} onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter") load((e.target as HTMLInputElement).value);
            }} />
            {dir?.roots?.length > 1 && (
              <select onChange={(e) => load((e.target as HTMLSelectElement).value)}>
                {dir.roots.map((r: string) => (
                  <option key={r}>{r}</option>
                ))}
              </select>
            )}
          </div>
          <div class="fs-list">
            {dir?.entries?.map((e: any) => (
              <div
                key={e.path}
                class={"fs-row" + (sel.includes(e.path) ? " sel" : "")}
                onClick={(ev) => {
                  if (e.dir) return load(e.path);
                  if (fb.kind === "save") return setName(e.name);
                  setSel(ev.ctrlKey || ev.metaKey || ev.shiftKey ? (sel.includes(e.path) ? sel.filter((s) => s !== e.path) : [...sel, e.path]) : [e.path]);
                }}
                onDblClick={() => !e.dir && fb.kind !== "save" && close([e.path])}
              >
                <span>{e.dir ? "📁" : "🎞"}</span>
                {e.name}
              </div>
            ))}
            {dir && dir.entries.length === 0 && <div class="empty">Empty folder</div>}
          </div>
          {fb.kind === "save" && (
            <div class="field">
              <label>File name</label>
              <input value={name} placeholder="my-video.mp4" onInput={(e) => setName((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
            </div>
          )}
        </div>
        <div class="modal-f">
          <button class="btn" onClick={() => close([])}>
            Cancel
          </button>
          <button
            class="btn primary"
            onClick={() => {
              if (fb.kind === "save") return name && close([dir.path + sep + name]);
              if (fb.kind === "folder") return close([dir.path]);
              close(sel);
            }}
          >
            {fb.kind === "save" ? "Save" : fb.kind === "folder" ? "Use this folder" : `Open ${sel.length || ""}`}
          </button>
        </div>
      </div>
    </div>
  );
}

type AiClient = { id: string; name: string; installed: boolean; registered: boolean; stale: boolean; config?: string; after: string };

/** AI apps on this computer, each connected with one click (MoodCut edits their MCP settings safely). */
function DesktopApps() {
  const [list, setList] = useState<AiClient[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [all, setAll] = useState(false);
  const load = async () => {
    try {
      const r = await fetch("/api/ai/clients").then((x) => x.json());
      setList(r.ok ? r.clients : []);
    } catch {
      setList([]);
    }
  };
  useEffect(() => {
    load();
  }, []);
  const act = async (c: AiClient, remove = false) => {
    setBusy(c.id);
    try {
      const r = await fetch("/api/ai/install", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ client: c.id, remove }) }).then((x) => x.json());
      toast(r.ok ? r.text : r.error, !r.ok);
      await load();
    } finally {
      setBusy(null);
    }
  };
  if (!list) return <div class="ai-apps ai-dim">Looking for AI apps…</div>;
  const shown = list.filter((c) => all || c.installed || c.registered);
  const todo = shown.filter((c) => c.installed && (!c.registered || c.stale));
  return (
    <div class="ai-apps">
      {shown.length === 0 && <div class="ai-dim">No AI apps found on this computer. Install Claude Desktop, Cursor or VS Code, or show every app below.</div>}
      {shown.map((c) => (
        <div class="ai-app" key={c.id}>
          <span class={"ai-state" + (c.registered && !c.stale ? " on" : c.installed ? " found" : "")} />
          <div class="ai-app-name">
            <b>{c.name}</b>
            <small title={c.config}>{c.registered ? (c.stale ? "points at another copy of MoodCut: reconnect" : `connected · ${c.after}`) : c.installed ? "found on this computer" : "not found (you can still prepare it)"}</small>
          </div>
          {c.registered && !c.stale ? (
            <button class="btn sm ghost" disabled={busy === c.id} onClick={() => act(c, true)} title="Remove MoodCut from this app's settings">
              Disconnect
            </button>
          ) : (
            <button class={"btn sm" + (c.installed ? " primary" : "")} disabled={busy === c.id} onClick={() => act(c)}>
              {busy === c.id ? "Connecting…" : c.stale ? "Reconnect" : "Connect"}
            </button>
          )}
        </div>
      ))}
      <div class="ai-apps-foot">
        {todo.length > 1 && (
          <button class="btn sm primary" disabled={!!busy} onClick={async () => { for (const c of todo) await act(c); }}>
            Connect all {todo.length}
          </button>
        )}
        <label class="ai-dim">
          <input type="checkbox" checked={all} onChange={(e) => setAll((e.target as HTMLInputElement).checked)} /> show every supported app
        </label>
      </div>
    </div>
  );
}

export function ConnectAI({ onClose }: { onClose: () => void }) {
  const url = `${location.origin}/mcp`;
  const port = location.port || "7878";
  const copy = (s: string) => navigator.clipboard?.writeText(s).then(() => toast("Copied"));
  const [web, setWeb] = useState<{ running: boolean; connector?: string; error?: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const tunnel = async (action: "status" | "start" | "stop") => {
    setBusy(action !== "status");
    try {
      const r = await fetch("/api/tunnel", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ action }) }).then((x) => x.json());
      setWeb(r.ok ? r : { running: false, error: r.error });
      if (r.ok && action === "start" && r.connector) copy(r.connector);
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    tunnel("status");
  }, []);
  const pre = { display: "block", padding: "8px", whiteSpace: "pre-wrap", wordBreak: "break-all" } as const;
  return (
    <div class="modal-bg" onClick={onClose}>
      <div class="modal" style={{ width: "min(700px, calc(100vw - 32px))" }} onClick={(e) => e.stopPropagation()}>
        <div class="modal-h">
          Connect an AI assistant
          <button class="btn ghost icon x" onClick={onClose}>
            ✕
          </button>
        </div>
        <div class="modal-b" style={{ userSelect: "text" }}>
          <div>
            <b>AI apps on this computer</b>: one click adds MoodCut to the app's settings (your other tools stay as they are, and a backup is kept).
            <DesktopApps />
          </div>
          <div class="web-link">
            <b>Claude on the web (claude.ai) or ChatGPT</b>: they run in the cloud, so MoodCut gives them a private HTTPS link.
            {web?.running && web.connector ? (
              <>
                <pre class="kbd" style={pre} onClick={() => copy(web.connector!)} title="Click to copy">
                  {web.connector}
                </pre>
                <ol>
                  <li>
                    <b>claude.ai</b>: Settings → Connectors → Add custom connector → paste the link → Add. Then enable MoodCut in a chat's tools menu.
                  </li>
                  <li>
                    <b>ChatGPT</b>: Settings → Connectors → Advanced → turn on Developer mode → Create → paste the link (no authentication).
                  </li>
                </ol>
                <div style={{ display: "flex", gap: "8px", alignItems: "center", flexWrap: "wrap" }}>
                  <button class="btn sm" onClick={() => copy(web.connector!)}>
                    Copy link
                  </button>
                  <button class="btn sm" disabled={busy} onClick={() => tunnel("stop")}>
                    Stop web link
                  </button>
                  <span style={{ color: "var(--faint)", fontSize: "12px" }}>The link contains a secret. Keep MoodCut open while you work.</span>
                </div>
              </>
            ) : (
              <div style={{ display: "flex", gap: "8px", alignItems: "center", marginTop: "8px", flexWrap: "wrap" }}>
                <button class="btn primary sm" disabled={busy} onClick={() => tunnel("start")}>
                  {busy ? "Starting… (first time downloads cloudflared)" : "Create web link"}
                </button>
                {web?.error && <span style={{ color: "var(--danger)", fontSize: "12px" }}>{web.error}</span>}
              </div>
            )}
          </div>
          <div>
            <b>Any other MCP client on this computer</b> (Open WebUI, MCP Inspector, your own agent…): Streamable HTTP at
            <pre class="kbd" style={pre} onClick={() => copy(url)} title="Click to copy">
              {url}
            </pre>
            or stdio: <code>moodcut mcp</code>. From a terminal, <code>moodcut install-ai</code> connects every app found and <code>moodcut install-ai --list</code> shows what's connected.
          </div>
          <div style={{ color: "var(--muted)" }}>
            Your own domain or Tailscale Funnel instead of a temporary link: <code>moodcut connect --url https://your-host{port === "7878" ? "" : ` --port ${port}`}</code>. Edits made by
            the assistant appear here live, and everything you do here is visible to it.
          </div>
        </div>
      </div>
    </div>
  );
}
