import { useEffect, useRef, useState } from "preact/hooks";
import { api, matchPick, parseSize, type GfxDetail, type GfxInfo, type GfxPreview, type Vars } from "../gfx";
import { comp, graphicEditor, playhead, refresh, refreshHistory, refreshLogs, toast } from "../store";
import { fmtNum } from "../util";
import { GraphicFields } from "./GraphicFields";

const STARTER = `<!-- Every {{name|default}} is a field: change it in the Fields tab, no code needed. -->
<style>
  .l3 { position: absolute; left: 6%; bottom: {{lift|12}}%; font: 800 4.4vh var(--brand-font); color: {{ink|var(--brand-text)}};
        animation: out .4s calc(var(--mc-dur) - .5s) ease-in forwards }
  .bar { height: .6vh; margin: .8vh 0; background: {{accent|var(--brand-accent)}}; transform-origin: left;
         animation: grow .6s cubic-bezier(.2,.8,.2,1) both }
  .name { animation: up .6s .1s both }
  .role { font: 500 2.6vh var(--brand-font); opacity: .85; animation: up .6s .25s both }
  @keyframes grow { from { transform: scaleX(0) } }
  @keyframes up { from { opacity: 0; transform: translateY(40%) } }
  @keyframes out { to { opacity: 0; transform: translateY(-20%) } }
</style>
<div class="l3">
  <div class="name">{{name|Jane Doe}}</div>
  <div class="bar"></div>
  <div class="role">{{role|Founder}}</div>
</div>
<script type="application/json" id="mc-fields">
{ "lift": { "label": "Height", "min": 4, "max": 60, "unit": "%" }, "ink": { "label": "Text color" } }
</script>`;

type Tab = "fields" | "code" | "settings";
interface Frame {
  id: number;
  src: string;
}

/**
 * HTML motion graphic editor: a live preview (the page runs sandboxed in an iframe on
 * the same virtual clock as the render, so scrubbing shows exact frames), the graphic's
 * fields as plain controls, its code, and its settings. Nothing renders until Render.
 */
export function GraphicEditor() {
  const ge = graphicEditor.value;
  const [html, setHtml] = useState("");
  const [vars, setVars] = useState<Vars>({});
  /** vars as last rendered: fields missing from `vars` now are reset */
  const [base, setBase] = useState<Vars>({});
  const [name, setName] = useState("");
  const [dur, setDur] = useState("4");
  const [fps, setFps] = useState("");
  const [size, setSize] = useState("");
  const [alpha, setAlpha] = useState(true);
  const [place, setPlace] = useState(true);
  const [tab, setTab] = useState<Tab>("fields");
  const [info, setInfo] = useState<GfxInfo | null>(null);
  const [detail, setDetail] = useState<{ uses: number; stale: boolean; file: string } | null>(null);
  const [scope, setScope] = useState<"all" | "clip">("all");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ text: string; sheet?: string | null } | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [saved, setSaved] = useState("");
  // live preview
  const [pv, setPv] = useState<{ url: string; w: number; h: number; fps: number; dur: number; alpha: boolean } | null>(null);
  const [pvError, setPvError] = useState<string | null>(null);
  const [frames, setFrames] = useState<Frame[]>([]);
  const [shownId, setShownId] = useState(0);
  const [t, setT] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [focus, setFocus] = useState<{ name: string; n: number } | null>(null);
  const [box, setBox] = useState({ w: 640, h: 360 });

  const token = useRef<string | null>(null);
  const reqSeq = useRef(0);
  const frameSeq = useRef(0);
  const shownRef = useRef(0);
  const newestRef = useRef(0);
  const iframes = useRef(new Map<number, HTMLIFrameElement>());
  const errors = useRef(new Map<number, string[]>());
  const tRef = useRef(0);
  const pending = useRef(false);
  const want = useRef<{ ms: number; mode: "exact" | "quick" } | null>(null);
  const urlRef = useRef("");
  const selfSwitch = useRef<string | null>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const state = useRef({ info, vars });
  state.current = { info, vars };

  const sz = parseSize(size);
  const durN = Math.max(0, parseFloat(dur) || 0);
  const still = durN <= 0;
  const snapshot = JSON.stringify({ html, vars, name, dur, fps, size, alpha });
  const dirty = loaded && saved !== "" && snapshot !== saved;

  const refetchDetail = (id: string) =>
    api<GfxDetail>(`/api/graphic?id=${encodeURIComponent(id)}`).then((g) => {
      if (g.ok) setDetail({ uses: g.uses, stale: g.stale, file: g.file });
    });

  // open a graphic (or a new one)
  useEffect(() => {
    if (!ge) return;
    if (ge.id && selfSwitch.current === ge.id) {
      // we just rendered this one (a new graphic or a variant): keep everything on screen
      selfSwitch.current = null;
      setScope("all");
      refetchDetail(ge.id);
      return;
    }
    setResult(null);
    setFrames([]);
    setPv(null);
    setPvError(null);
    setPlaying(false);
    setFocus(null);
    setScope("all");
    setLoaded(false);
    setLogs([]);
    setShownId(0);
    shownRef.current = 0;
    newestRef.current = 0;
    errors.current.clear();
    pending.current = false;
    want.current = null;
    token.current = null;
    tRef.current = 0;
    setT(0);
    if (!ge.id) {
      const next = { html: STARTER, vars: {}, name: "Lower third", dur: "4", fps: "", size: "", alpha: true };
      setHtml(next.html);
      setVars({});
      setBase({});
      setName(next.name);
      setDur(next.dur);
      setFps("");
      setSize("");
      setAlpha(true);
      setDetail(null);
      setInfo(null);
      setTab("fields");
      setSaved(JSON.stringify(next));
      setLoaded(true);
      return;
    }
    let live = true;
    api<GfxDetail>(`/api/graphic?id=${encodeURIComponent(ge.id)}`).then((g) => {
      if (!live) return;
      if (!g.ok) {
        toast(g.error, true);
        graphicEditor.value = null;
        return;
      }
      const next = { html: g.html, vars: g.vars || {}, name: g.name, dur: String(g.dur), fps: String(g.fps), size: `${g.w}x${g.h}`, alpha: g.alpha };
      setHtml(next.html);
      setVars(next.vars);
      setBase(next.vars);
      setName(next.name);
      setDur(next.dur);
      setFps(next.fps);
      setSize(next.size);
      setAlpha(next.alpha);
      if (g.dur <= 0) {
        tRef.current = (g.at || 0) * 1000;
        setT(tRef.current);
      }
      setInfo(g);
      setDetail({ uses: g.uses, stale: g.stale, file: g.file });
      setTab(g.fields.length ? "fields" : "code");
      setSaved(JSON.stringify(next));
      setLoaded(true);
    });
    return () => {
      live = false;
    };
  }, [ge?.id, !!ge]);

  // fit the frame in the stage
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setBox({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, [!!ge]);

  const isNewest = () => newestRef.current === shownRef.current;

  /** Sends the latest wanted time to the shown page, one seek at a time. */
  const pump = () => {
    if (pending.current || !want.current || !isNewest()) return;
    const w = iframes.current.get(shownRef.current)?.contentWindow;
    if (!w) return;
    const { ms, mode } = want.current;
    want.current = null;
    pending.current = true;
    w.postMessage({ mcPreview: 1, type: "seek", ms, mode }, "*");
  };
  const seekTo = (ms: number, mode: "exact" | "quick") => {
    tRef.current = ms;
    setT(ms);
    want.current = { ms, mode };
    pump();
  };
  /** Loads the page again at `ms`; the current one stays visible until it's ready. */
  const loadFrame = (url: string, ms: number) => {
    const id = ++frameSeq.current;
    newestRef.current = id;
    pending.current = false;
    setFrames((fs) => [...fs.filter((f) => f.id === shownRef.current), { id, src: `${url}?v=${id}#t=${Math.round(ms)}` }]);
  };

  // rebuild the preview when anything changes (debounced)
  useEffect(() => {
    if (!ge || !loaded) return;
    const seq = ++reqSeq.current;
    const timer = setTimeout(async () => {
      const body: Record<string, unknown> = { html, vars, unset: Object.keys(base).filter((k) => !(k in vars)), dur: durN, alpha, editable: false, token: token.current };
      if (ge.id) body.id = ge.id;
      if (parseFloat(fps) > 0) body.fps = parseFloat(fps);
      if (sz) body.size = `${sz[0]}x${sz[1]}`;
      const r = await api<GfxPreview>("/api/graphic/preview", body);
      if (seq !== reqSeq.current) return; // a newer change is on its way
      if (!r.ok) {
        setPvError(r.error);
        return;
      }
      setPvError(null);
      token.current = r.token;
      urlRef.current = r.url;
      setInfo(r);
      setPv({ url: r.url, w: r.w, h: r.h, fps: r.fps, dur: r.dur, alpha: r.alpha });
      loadFrame(r.url, tRef.current);
    }, 250);
    return () => clearTimeout(timer);
  }, [ge?.id, loaded, html, JSON.stringify(vars), JSON.stringify(base), dur, fps, size, alpha]);

  // messages from the preview pages
  useEffect(() => {
    if (!ge) return;
    const onMsg = (e: MessageEvent) => {
      const d = e.data;
      if (!d || d.mcPreview !== 1) return;
      let id = 0;
      for (const [k, el] of iframes.current) if (el.contentWindow === e.source) id = k;
      if (!id) return;
      switch (d.type) {
        case "ready":
          if (id < shownRef.current || id !== newestRef.current) return; // overtaken by a newer page
          shownRef.current = id;
          setShownId(id);
          setFrames((fs) => fs.filter((f) => f.id >= id));
          for (const k of errors.current.keys()) if (k < id) errors.current.delete(k);
          setLogs(errors.current.get(id) || []);
          pending.current = false;
          if (Math.abs(d.ms - tRef.current) > 0.5 && !want.current) want.current = { ms: tRef.current, mode: "exact" };
          pump();
          break;
        case "seeked":
          if (id === shownRef.current) {
            pending.current = false;
            pump();
          }
          break;
        case "rewind":
          // going back needs a fresh page (its scripts keep state of their own)
          if (id === shownRef.current && isNewest()) loadFrame(urlRef.current, want.current?.ms ?? d.ms);
          break;
        case "error": {
          const list = errors.current.get(id) || [];
          if (!list.includes(d.msg) && list.length < 30) list.push(String(d.msg));
          errors.current.set(id, list);
          if (id === shownRef.current) setLogs([...list]);
          break;
        }
        case "pick": {
          const { info: inf, vars: vs } = state.current;
          const hit = inf && matchPick(inf.fields, vs, d.texts || []);
          if (hit) {
            setTab("fields");
            setFocus({ name: hit, n: Date.now() });
          } else if (inf?.hardcoded.length) toast("That text is written into the code: Make editable turns it into a field");
          break;
        }
      }
    };
    addEventListener("message", onMsg);
    return () => removeEventListener("message", onMsg);
  }, [!!ge]);

  // playback: real time, as fast as the page can seek (loops)
  useEffect(() => {
    if (!playing || !pv || pv.dur <= 0) return;
    const durMs = pv.dur * 1000;
    const frameMs = 1000 / pv.fps;
    let start = performance.now() - (tRef.current >= durMs - frameMs ? 0 : tRef.current);
    let raf = 0;
    const tick = () => {
      let ms = performance.now() - start;
      if (ms >= durMs) {
        start = performance.now();
        ms = 0;
      }
      if (!pending.current && !want.current) seekTo(Math.min(durMs, Math.round(ms / frameMs) * frameMs), "exact");
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, pv?.url, pv?.dur, pv?.fps]);

  if (!ge) return null;

  const close = () => {
    if (busy) return;
    if (dirty && !confirm("Close without rendering your changes?")) return;
    graphicEditor.value = null;
  };
  const setVar = (n: string, v: string | null) =>
    setVars((cur) => {
      const x = { ...cur };
      if (v === null) delete x[n];
      else x[n] = v;
      return x;
    });
  const makeEditable = async (colors: boolean) => {
    const r = await api<GfxInfo & { html: string; added: { name: string; type: string }[] }>("/api/graphic/editable", { html, colors });
    if (!r.ok) return toast(r.error, true);
    if (!r.added.length) return toast("Nothing left to turn into fields");
    setHtml(r.html);
    setInfo(r);
    setTab("fields");
    toast(`${r.added.length} field${r.added.length === 1 ? "" : "s"} added: ${r.added.map((a) => a.name).join(", ")}. Render to keep them.`);
  };

  const submit = async (mode: "update" | "new") => {
    if (busy) return;
    setBusy(true);
    setPlaying(false);
    const body: Record<string, unknown> = { html, name, dur: durN, alpha, vars, unset: Object.keys(base).filter((k) => !(k in vars)), preview: 6, editable: false };
    if (parseFloat(fps) > 0) body.fps = parseFloat(fps);
    if (sz) body.size = `${sz[0]}x${sz[1]}`;
    if (still) body.at = Math.round(tRef.current) / 1000;
    if (mode === "update" && ge.id) {
      if (scope === "clip" && ge.item) {
        body.from = ge.id;
        body.replace = ge.item;
        body.name = `${name} · ${ge.item}`;
      } else body.id = ge.id;
    } else {
      if (ge.id) body.from = ge.id;
      if (!ge.id && place) body.place = `at=${fmtNum(playhead.value, 4)}`;
    }
    try {
      const r = await api<{ id: string; text: string; sheet?: string | null }>("/api/graphic", body);
      if (!r.ok) {
        setResult({ text: r.error });
        toast(r.error, true);
        return;
      }
      setResult({ text: r.text, sheet: r.sheet });
      toast(r.text.split("\n")[0]);
      setBase({ ...vars });
      setSaved(snapshot);
      await refresh();
      refreshLogs();
      refreshHistory();
      if (r.id !== ge.id) {
        // later renders update the new graphic (a variant now on this clip, or a new one)
        selfSwitch.current = r.id;
        graphicEditor.value = { id: r.id, item: mode === "update" ? ge.item : undefined };
      } else refetchDetail(r.id);
    } finally {
      setBusy(false);
    }
  };

  const fw = pv?.w || sz?.[0] || comp.value?.w || 1920;
  const fh = pv?.h || sz?.[1] || comp.value?.h || 1080;
  const scale = Math.max(0.05, Math.min((box.w - 16) / fw, (box.h - 16) / fh));
  const frameMs = 1000 / (pv?.fps || parseFloat(fps) || comp.value?.fps || 30);
  const maxMs = still ? 10000 : durN * 1000;
  const fields = info?.fields || [];
  const tabs: [Tab, string][] = [
    ["fields", `Fields${info ? ` (${fields.length})` : ""}`],
    ["code", "Code"],
    ["settings", "Settings"],
  ];

  return (
    <div class="modal-bg">
      <div
        class="modal gfx-editor"
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
            e.preventDefault();
            submit(ge.id ? "update" : "new");
          }
          if (e.key === "Escape") close();
        }}
      >
        <div class="modal-h">
          HTML motion graphic {ge.id ? <small style={{ color: "var(--faint)", marginLeft: "6px" }}>{ge.id}</small> : <small style={{ color: "var(--faint)", marginLeft: "6px" }}>new</small>}
          <button class="btn ghost icon x" title="Close (Esc)" onClick={close}>
            ✕
          </button>
        </div>
        <div class={"modal-b gfx-body" + (tab === "code" ? " code-wide" : "")}>
          <div class="gfx-main">
            <div class="gfx-stage" ref={stageRef}>
              <div class={"gfx-frame" + ((pv?.alpha ?? alpha) ? " checker" : "")} style={{ width: `${fw * scale}px`, height: `${fh * scale}px` }}>
                {frames.map((f) => (
                  <iframe
                    key={f.id}
                    ref={(el) => {
                      if (el) iframes.current.set(f.id, el);
                      else iframes.current.delete(f.id);
                    }}
                    src={f.src}
                    sandbox="allow-scripts"
                    title="Live preview"
                    class={f.id === shownId ? "on" : ""}
                    style={{ width: `${fw}px`, height: `${fh}px`, transform: `scale(${scale})` }}
                  />
                ))}
                {!shownId && <div class="gfx-wait">{pvError ? "The preview can't be shown: see below" : "Loading the preview…"}</div>}
              </div>
            </div>
            <div class="gfx-transport">
              {still ? (
                <span class="hint" title="A still is captured at this time of its animation">
                  Capture at
                </span>
              ) : (
                <button class="btn sm icon" title={playing ? "Pause" : "Play"} disabled={!pv} onClick={() => setPlaying(!playing)}>
                  {playing ? "❚❚" : "▶"}
                </button>
              )}
              <input
                type="range"
                min={0}
                max={maxMs}
                step={frameMs}
                value={Math.min(t, maxMs)}
                onInput={(e) => {
                  setPlaying(false);
                  seekTo(parseFloat((e.target as HTMLInputElement).value), "quick");
                }}
                onChange={(e) => seekTo(parseFloat((e.target as HTMLInputElement).value), "exact")}
              />
              <span class="gfx-time">
                {fmtNum(t / 1000, 2)}s{still ? "" : ` / ${fmtNum(durN, 2)}s`}
              </span>
            </div>
            <div class="gfx-console">
              {pvError && <div class="err">{pvError}</div>}
              {logs.map((l) => (
                <div class="err" key={l}>
                  {l}
                </div>
              ))}
              {info?.warnings.map((w) => (
                <div class="warn" key={w}>
                  {w}
                </div>
              ))}
              {result?.sheet && <img class="gfx-sheet" src={result.sheet} alt="rendered frames" />}
              {result && <pre class="gfx-log">{result.text}</pre>}
            </div>
          </div>
          <div class="gfx-side">
            <div class="tabs compact">
              {tabs.map(([k, label]) => (
                <div key={k} class={"tab" + (tab === k ? " active" : "")} onClick={() => setTab(k)}>
                  {label}
                </div>
              ))}
            </div>
            <div class="gfx-pane">
              {tab === "fields" &&
                (fields.length ? (
                  <>
                    <GraphicFields fields={fields} vars={vars} onChange={setVar} focus={focus} />
                    {info && info.hardcoded.length > 0 && (
                      <div class="hint">
                        Written into the code: {info.hardcoded.map((h) => `“${h}”`).join(", ")}.{" "}
                        <a href="#" onClick={(e) => (e.preventDefault(), makeEditable(false))}>
                          Make editable
                        </a>
                      </div>
                    )}
                    <div class="hint">Click a text in the preview to jump to its field. Ctrl+Enter renders.</div>
                  </>
                ) : (
                  <div class="gfx-empty">
                    <p>This graphic has no editable fields yet: its text and colors are written into the code.</p>
                    <button class="btn primary" disabled={!html.trim()} onClick={() => makeEditable(true)}>
                      Make text and colors editable
                    </button>
                    <p class="hint">Each text and color becomes a field. The graphic looks exactly the same until you change one.</p>
                  </div>
                ))}
              {tab === "code" && (
                <>
                  <div class="gfx-codebar">
                    <span class="hint">
                      <code>{"{{name|default}}"}</code> makes a field · <code>mc://m3</code> uses library media · <code>var(--mc-dur)</code> is the length
                    </span>
                    <button class="btn sm" title="Turn the texts and colors written into the code into fields" onClick={() => makeEditable(true)}>
                      Make editable
                    </button>
                  </div>
                  <textarea
                    class="code"
                    spellcheck={false}
                    value={html}
                    onInput={(e) => setHtml((e.target as HTMLTextAreaElement).value)}
                    onKeyDown={(e) => {
                      if (e.key === "Tab") {
                        e.preventDefault();
                        const ta = e.target as HTMLTextAreaElement;
                        const p = ta.selectionStart;
                        setHtml(html.slice(0, p) + "  " + html.slice(ta.selectionEnd));
                        requestAnimationFrame(() => (ta.selectionStart = ta.selectionEnd = p + 2));
                      }
                    }}
                  />
                </>
              )}
              {tab === "settings" && (
                <div class="gfx-settings">
                  <div class="field">
                    <label>Name</label>
                    <input value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
                  </div>
                  <div class="field">
                    <label>Seconds</label>
                    <input value={dur} placeholder="0 = still" onInput={(e) => setDur((e.target as HTMLInputElement).value)} />
                  </div>
                  {!still && info && !info.uses_duration && (
                    <div class="hint">This design's timing is written into its code: a longer graphic holds its last frame and a shorter one cuts the ending. Timing the exit from var(--mc-dur) makes it follow the length.</div>
                  )}
                  <div class="field">
                    <label>FPS</label>
                    <input value={fps} placeholder={`comp (${comp.value?.fps || 30})`} onInput={(e) => setFps((e.target as HTMLInputElement).value)} />
                  </div>
                  <div class="field">
                    <label>Size</label>
                    <input value={size} placeholder={`comp (${comp.value?.w || 1920}x${comp.value?.h || 1080})`} onInput={(e) => setSize((e.target as HTMLInputElement).value)} />
                  </div>
                  {size.trim() !== "" && !sz && <div class="hint" style={{ color: "var(--warn)" }}>Size is width x height, like 1920x1080.</div>}
                  <label class="check">
                    <input type="checkbox" checked={alpha} onChange={(e) => setAlpha((e.target as HTMLInputElement).checked)} /> Transparent background
                  </label>
                  {!ge.id && (
                    <label class="check">
                      <input type="checkbox" checked={place} onChange={(e) => setPlace((e.target as HTMLInputElement).checked)} /> Add to the timeline at the playhead
                    </label>
                  )}
                  {detail?.file && (
                    <div class="hint" style={{ wordBreak: "break-all" }}>
                      Source file: {detail.file}. Edits made to it in another editor are used on the next render.
                    </div>
                  )}
                  <div class="hint">Animate with CSS, Web Animations, GSAP or canvas: the preview and the render run on the same clock, frame by frame.</div>
                </div>
              )}
            </div>
          </div>
        </div>
        <div class="modal-f">
          {ge.id && ge.item && detail && detail.uses > 1 && (
            <label class="gfx-scope">
              Apply to
              <select value={scope} onChange={(e) => setScope((e.target as HTMLSelectElement).value as "all" | "clip")}>
                <option value="all">all {detail.uses} clips using it</option>
                <option value="clip">this clip only ({ge.item})</option>
              </select>
            </label>
          )}
          {detail?.stale && !dirty && <span class="gfx-dirty">Rendered with older brand colors</span>}
          {dirty && <span class="gfx-dirty">Not rendered yet</span>}
          <button class="btn" onClick={close}>
            Close
          </button>
          {ge.id && (
            <button class="btn" disabled={busy} title="Render a new graphic from these settings; this one stays as it is" onClick={() => submit("new")}>
              Render as new
            </button>
          )}
          <button class="btn primary" disabled={busy || !html.trim()} onClick={() => submit(ge.id ? "update" : "new")}>
            {busy ? "Rendering…" : ge.id ? "Render" : "Render and add"}
          </button>
        </div>
      </div>
    </div>
  );
}
