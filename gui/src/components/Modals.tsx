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
                  <span style={{ marginLeft: "auto" }}>{j.state === "running" ? `${j.pct}% · ${j.frames} · eta ${j.eta_s}s` : j.state === "done" ? `${fmtNum((j.bytes || 0) / 1e6, 1)} MB · ${j.secs}s · ${j.render_fps} fps` : ""}</span>
                </div>
                <div class="progress">
                  <div style={{ width: `${j.pct}%` }} />
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

export function ConnectAI({ onClose }: { onClose: () => void }) {
  const url = `${location.origin}/mcp`;
  const copy = (s: string) => navigator.clipboard?.writeText(s).then(() => toast("Copied"));
  return (
    <div class="modal-bg" onClick={onClose}>
      <div class="modal" style={{ width: "min(640px, calc(100vw - 32px))" }} onClick={(e) => e.stopPropagation()}>
        <div class="modal-h">
          Connect an AI assistant
          <button class="btn ghost icon x" onClick={onClose}>
            ✕
          </button>
        </div>
        <div class="modal-b" style={{ userSelect: "text" }}>
          <div>
            <b>Claude Desktop</b> — run once in a terminal, then restart Claude:
            <pre class="kbd" style={{ display: "block", padding: "8px", whiteSpace: "pre-wrap" }}>moodcut install-claude</pre>
          </div>
          <div>
            <b>Claude Code</b>
            <pre class="kbd" style={{ display: "block", padding: "8px", whiteSpace: "pre-wrap" }} onClick={() => copy("claude mcp add moodcut -- moodcut mcp")}>
              claude mcp add moodcut -- moodcut mcp
            </pre>
          </div>
          <div>
            <b>Any MCP client over HTTP</b> (Cursor, VS Code, LM Studio, Open WebUI…):
            <pre class="kbd" style={{ display: "block", padding: "8px" }} onClick={() => copy(url)}>
              {url}
            </pre>
          </div>
          <div>
            <b>ChatGPT</b> (Settings → Connectors → Developer mode → Add MCP server) needs an HTTPS URL. Start
            <pre class="kbd" style={{ display: "block", padding: "8px", whiteSpace: "pre-wrap" }}>moodcut serve --public{"\n"}cloudflared tunnel --url http://localhost:7878</pre>
            then use <code>https://&lt;tunnel&gt;/mcp/&lt;token&gt;</code> (the token is printed by <code>serve --public</code>).
          </div>
          <div style={{ color: "var(--muted)" }}>Edits made by the assistant appear here live, and everything you do here is visible to it.</div>
        </div>
      </div>
    </div>
  );
}
