import { useEffect, useState } from "preact/hooks";
import { graphicEditor, playhead, refresh, refreshLogs, toast } from "../store";
import { fmtNum } from "../util";

const STARTER = `<style>
  .l3 { position: absolute; left: 6%; bottom: 12%; font: 800 4.4vh Inter; color: #fff }
  .bar { height: .6vh; margin: .8vh 0; background: #ff3b6b; transform-origin: left;
         animation: grow .6s cubic-bezier(.2,.8,.2,1) both }
  .name { animation: up .6s .1s both }
  .role { font: 500 2.6vh Inter; opacity: .85; animation: up .6s .25s both }
  @keyframes grow { from { transform: scaleX(0) } }
  @keyframes up { from { opacity: 0; transform: translateY(40%) } }
</style>
<div class="l3">
  <div class="name">{{name}}</div>
  <div class="bar"></div>
  <div class="role">{{role}}</div>
</div>`;

const parseVars = (s: string) => Object.fromEntries(s.split("\n").map((l) => l.split("=")).filter((p) => p.length >= 2 && p[0].trim()).map(([k, ...v]) => [k.trim(), v.join("=").trim()]));

/** Write HTML/CSS/JS, render it to a video with alpha, and see the result. */
export function GraphicEditor() {
  const ge = graphicEditor.value;
  const [html, setHtml] = useState("");
  const [name, setName] = useState("");
  const [dur, setDur] = useState("4");
  const [fps, setFps] = useState("");
  const [alpha, setAlpha] = useState(true);
  const [vars, setVars] = useState("");
  const [place, setPlace] = useState(true);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ text: string; sheet?: string | null } | null>(null);

  useEffect(() => {
    if (!ge) return;
    setResult(null);
    if (!ge.id) {
      setHtml(STARTER);
      setName("Lower third");
      setDur("4");
      setFps("");
      setAlpha(true);
      setVars("name=Jane Doe\nrole=Founder");
      return;
    }
    fetch(`/api/graphic?id=${ge.id}`)
      .then((r) => r.json())
      .then((g) => {
        if (!g.ok) return toast(g.error, true);
        setHtml(g.html);
        setName(g.name);
        setDur(String(g.dur));
        setFps(String(g.fps));
        setAlpha(g.alpha);
        setVars(Object.entries(g.vars || {}).map(([k, v]) => `${k}=${v}`).join("\n"));
      });
  }, [ge?.id, !!ge]);

  if (!ge) return null;
  const close = () => (graphicEditor.value = null);
  const submit = async (mode: "update" | "new") => {
    setBusy(true);
    const body: any = { html, name, dur: parseFloat(dur) || 0, alpha, vars: parseVars(vars), preview: 6 };
    if (parseFloat(fps) > 0) body.fps = parseFloat(fps);
    if (mode === "update" && ge.id) body.id = ge.id;
    if (mode === "new" && place) body.place = `at=${fmtNum(playhead.value, 4)}`;
    try {
      const r = await fetch("/api/graphic", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) }).then((x) => x.json());
      if (!r.ok) {
        setResult({ text: r.error });
        toast(r.error, true);
        return;
      }
      setResult({ text: r.text, sheet: r.sheet });
      toast(r.text.split("\n")[0]);
      await refresh();
      refreshLogs();
      if (mode === "new") graphicEditor.value = { id: r.id }; // later renders update this graphic
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="modal-bg">
      <div class="modal gfx-editor" onKeyDown={(e) => e.stopPropagation()}>
        <div class="modal-h">
          HTML motion graphic {ge.id ? <small style={{ color: "var(--faint)" }}>{ge.id}</small> : "· new"}
          <button class="btn ghost icon x" onClick={close}>
            ✕
          </button>
        </div>
        <div class="modal-b gfx-body">
          <textarea
            class="code"
            spellcheck={false}
            value={html}
            onInput={(e) => setHtml((e.target as HTMLTextAreaElement).value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) submit(ge.id ? "update" : "new");
              if (e.key === "Tab") {
                e.preventDefault();
                const t = e.target as HTMLTextAreaElement;
                const p = t.selectionStart;
                setHtml(html.slice(0, p) + "  " + html.slice(t.selectionEnd));
                requestAnimationFrame(() => (t.selectionStart = t.selectionEnd = p + 2));
              }
            }}
          />
          <div class="gfx-side">
            <div class="field">
              <label>Name</label>
              <input value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
            </div>
            <div style={{ display: "flex", gap: "8px" }}>
              <div class="field" style={{ flex: 1 }}>
                <label>Seconds (0 = still)</label>
                <input value={dur} onInput={(e) => setDur((e.target as HTMLInputElement).value)} />
              </div>
              <div class="field" style={{ flex: 1 }}>
                <label>FPS</label>
                <input value={fps} placeholder="comp" onInput={(e) => setFps((e.target as HTMLInputElement).value)} />
              </div>
            </div>
            <label class="check">
              <input type="checkbox" checked={alpha} onChange={(e) => setAlpha((e.target as HTMLInputElement).checked)} /> Transparent background
            </label>
            {!ge.id && (
              <label class="check">
                <input type="checkbox" checked={place} onChange={(e) => setPlace((e.target as HTMLInputElement).checked)} /> Add to timeline at playhead
              </label>
            )}
            <div class="field">
              <label>Variables ({"{{key}}"} → value, one per line)</label>
              <textarea rows={4} value={vars} onInput={(e) => setVars((e.target as HTMLTextAreaElement).value)} />
            </div>
            {result?.sheet && <img class="gfx-sheet" src={result.sheet} alt="rendered frames" />}
            {result && <pre class="gfx-log">{result.text}</pre>}
            {!result && (
              <div class="hint">
                Animate with CSS, Web Animations, GSAP or canvas; every frame renders exactly. Use <code>mc://m3</code> for library media. Ctrl+Enter renders.
              </div>
            )}
          </div>
        </div>
        <div class="modal-f">
          <button class="btn" onClick={close}>
            Close
          </button>
          {ge.id && (
            <button class="btn" disabled={busy} onClick={() => submit("new")}>
              Render as new
            </button>
          )}
          <button class="btn primary" disabled={busy} onClick={() => submit(ge.id ? "update" : "new")}>
            {busy ? "Rendering…" : ge.id ? "Re-render" : "Render"}
          </button>
        </div>
      </div>
    </div>
  );
}
