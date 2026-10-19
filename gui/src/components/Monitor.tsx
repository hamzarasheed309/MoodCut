import { useEffect, useRef, useState } from "preact/hooks";
import { placeSource } from "../actions";
import { attachMeter } from "../meter";
import { showPanel } from "../dock";
import { activeId, catalog, comp, createdIds, findItem, fps, mediaById, monitorTab, pausedPlayhead, playhead, playing, project, rev, run, selection, setPlayhead, shapeKind, sourceIn, sourceMedia, sourceOut, sourceT, tool } from "../store";
import type { Comp, Item, LayerGeom } from "../types";
import { compDuration, fmtNum, fmtTC, isAnimated } from "../util";

/** Loads images one at a time, newest request wins (scrubbing never queues up). */
function useLatestImage(): [string, (url: string) => void] {
  const [src, setSrc] = useState("");
  const loading = useRef(false);
  const pending = useRef<string | null>(null);
  const load = (url: string) => {
    if (loading.current) {
      pending.current = url;
      return;
    }
    loading.current = true;
    const im = new Image();
    im.onload = im.onerror = () => {
      loading.current = false;
      if (im.naturalWidth) setSrc(url);
      const nx = pending.current;
      pending.current = null;
      if (nx && nx !== url) load(nx);
    };
    im.src = url;
  };
  return [src, load];
}

const N = (v: number) => fmtNum(v, 2);

// ---------------------------------------------------------------- transform gizmo + drawing tools

type GDrag =
  | { mode: "move" | "scale" | "rot"; x0: number; y0: number; x: number; y: number }
  | { mode: "draw"; x0: number; y0: number; x: number; y: number; mask: boolean }
  | null;

/** Program monitor overlay: outline and handles of the selected layer (Selection tool), Type and Shape tools. */
function Overlay({ c }: { c: Comp }) {
  const t = pausedPlayhead.value;
  const id = selection.value.length === 1 ? selection.value[0] : null;
  const f = id ? findItem(id) : null;
  const it: Item | undefined = f && f.comp.id === c.id ? f.item : undefined;
  const visible = !!it && it.start <= t + 1e-4 && t < it.start + it.dur && f!.track.kind === "video";
  const [geom, setGeom] = useState<LayerGeom | null>(null);
  const [drag, setDrag] = useState<GDrag>(null);
  const svg = useRef<SVGSVGElement>(null);
  const tl = tool.value;

  useEffect(() => {
    if (!it || !visible || t < 0) return setGeom(null);
    let live = true;
    const h = setTimeout(async () => {
      try {
        const r = await fetch(`/api/layer?id=${it.id}&t=${t.toFixed(4)}`).then((r) => r.json());
        if (live) setGeom(r.geom || null);
      } catch {}
    }, 60);
    return () => {
      live = false;
      clearTimeout(h);
    };
  }, [it?.id, visible, t, rev.value]);

  const pt = (e: PointerEvent): [number, number] => {
    const r = svg.current!.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * c.w, ((e.clientY - r.top) / r.height) * c.h];
  };
  const active = tl === "type" || tl === "shape" || (tl === "select" && geom);
  if (!active || t < 0) return null;
  const at = fmtNum(t, 4);
  const anchor = geom?.anchor || [0, 0];
  const keyed = (p: string) => (it && isAnimated(it.props?.[p]) ? ` key at=${at}` : "");

  // live preview of a gizmo drag
  let xf = "";
  let readout = "";
  if (drag && drag.mode !== "draw" && geom) {
    const [ax, ay] = anchor;
    if (drag.mode === "move") {
      xf = `translate(${drag.x - drag.x0} ${drag.y - drag.y0})`;
      readout = `${N(drag.x - drag.x0)}, ${N(drag.y - drag.y0)} px`;
    } else if (drag.mode === "scale") {
      const s = Math.hypot(drag.x - ax, drag.y - ay) / Math.max(1, Math.hypot(drag.x0 - ax, drag.y0 - ay));
      xf = `translate(${ax} ${ay}) scale(${s}) translate(${-ax} ${-ay})`;
      readout = `${Math.round(s * 100)}%`;
    } else {
      const a = ((Math.atan2(drag.y - ay, drag.x - ax) - Math.atan2(drag.y0 - ay, drag.x0 - ax)) * 180) / Math.PI;
      xf = `rotate(${a} ${ax} ${ay})`;
      readout = `${N(a)}°`;
    }
  }

  async function finish(d: NonNullable<GDrag>, e: PointerEvent) {
    const dx = d.x - d.x0;
    const dy = d.y - d.y0;
    if (d.mode === "draw") {
      const [x0, x1] = [Math.min(d.x0, d.x), Math.max(d.x0, d.x)];
      const [y0, y1] = [Math.min(d.y0, d.y), Math.max(d.y0, d.y)];
      if (tl === "type") {
        const r = await run(`text 'Text' pos=${Math.round(d.x)},${Math.round(d.y)} at=${at} dur=3`, true);
        const ids = createdIds(r.log);
        if (ids.length) selection.value = ids;
        tool.value = "select";
        return;
      }
      if (x1 - x0 < 4 || y1 - y0 < 4) return;
      if (d.mask && it && geom) {
        // mask coordinates are fractions of the layer's box
        const xs = geom.quad.map((p) => p[0]);
        const ys = geom.quad.map((p) => p[1]);
        const [bx, by] = [Math.min(...xs), Math.min(...ys)];
        const [bw, bh] = [Math.max(1, Math.max(...xs) - bx), Math.max(1, Math.max(...ys) - by)];
        run(`mask ${it.id} ${shapeKind.value} x=${fmtNum(((x0 + x1) / 2 - bx) / bw, 4)} y=${fmtNum(((y0 + y1) / 2 - by) / bh, 4)} w=${fmtNum((x1 - x0) / bw, 4)} h=${fmtNum((y1 - y0) / bh, 4)}`, true);
        return;
      }
      const r = await run(`shape ${shapeKind.value} w=${Math.round(x1 - x0)} h=${Math.round(y1 - y0)} pos=${Math.round((x0 + x1) / 2)},${Math.round((y0 + y1) / 2)} fill=#8b7cf6 at=${at} dur=3`, true);
      const ids = createdIds(r.log);
      if (ids.length) selection.value = ids;
      return;
    }
    if (!it || !geom) return;
    const [ax, ay] = anchor;
    if (d.mode === "move" && Math.hypot(dx, dy) > 0.5) {
      // Shift constrains to the main axis
      const [mx, my] = !e.shiftKey ? [dx, dy] : Math.abs(dx) > Math.abs(dy) ? [dx, 0] : [0, dy];
      run(`nudge ${it.id} pos=${N(mx)},${N(my)}${keyed("pos")}`, true);
    } else if (d.mode === "scale") {
      const s = Math.hypot(d.x - ax, d.y - ay) / Math.max(1, Math.hypot(d.x0 - ax, d.y0 - ay));
      if (Math.abs(s - 1) > 0.002) run(`nudge ${it.id} scale=${fmtNum(s, 4)}${keyed("scale")}`, true);
    } else if (d.mode === "rot") {
      let a = ((Math.atan2(d.y - ay, d.x - ax) - Math.atan2(d.y0 - ay, d.x0 - ax)) * 180) / Math.PI;
      if (e.shiftKey) a = Math.round(a / 15) * 15;
      if (Math.abs(a) > 0.05) run(`nudge ${it.id} rot=${N(a)}${keyed("rot")}`, true);
    }
  }

  const start = (mode: "move" | "scale" | "rot" | "draw") => (e: PointerEvent) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    svg.current!.setPointerCapture(e.pointerId);
    const [x, y] = pt(e);
    setDrag(mode === "draw" ? { mode, x0: x, y0: y, x, y, mask: e.altKey && !!it && it.kind !== "shape" } : { mode, x0: x, y0: y, x, y });
  };
  const quad = geom?.quad;
  const hs = Math.max(6, c.w / 140);
  // rotation handle: beyond the middle of the top edge, away from the layer's center
  const top = quad ? [(quad[0][0] + quad[1][0]) / 2, (quad[0][1] + quad[1][1]) / 2] : null;
  let rotH: number[] | null = null;
  if (quad && top) {
    const cx = quad.reduce((a, p) => a + p[0], 0) / 4;
    const cy = quad.reduce((a, p) => a + p[1], 0) / 4;
    const len = Math.hypot(top[0] - cx, top[1] - cy) || 1;
    const off = Math.max(30, c.h / 18);
    rotH = [top[0] + ((top[0] - cx) / len) * off, top[1] + ((top[1] - cy) / len) * off];
  }
  return (
    <svg
      ref={svg}
      class={"overlay" + (tl === "type" ? " type" : tl === "shape" ? " draw" : "") + (drag ? " dragging" : "")}
      viewBox={`0 0 ${c.w} ${c.h}`}
      preserveAspectRatio="none"
      onPointerDown={tl === "type" || tl === "shape" ? start("draw") : undefined}
      onPointerMove={(e) => {
        if (!drag) return;
        const [x, y] = pt(e);
        setDrag({ ...drag, x, y });
      }}
      onPointerUp={(e) => {
        const d = drag;
        setDrag(null);
        if (d) finish({ ...d }, e);
      }}
    >
      {quad && tl === "select" && (
        <g transform={xf}>
          <polygon points={quad.map((p) => p.join(",")).join(" ")} class="gz-box" onPointerDown={start("move")} />
          {quad.map((p, i) => (
            <rect key={i} x={p[0] - hs / 2} y={p[1] - hs / 2} width={hs} height={hs} class="gz-h" onPointerDown={start("scale")} />
          ))}
          {rotH && top && (
            <>
              <line x1={top[0]} y1={top[1]} x2={rotH[0]} y2={rotH[1]} class="gz-line" />
              <circle cx={rotH[0]} cy={rotH[1]} r={hs * 0.7} class="gz-h rot" onPointerDown={start("rot")} />
            </>
          )}
          <circle cx={anchor[0]} cy={anchor[1]} r={hs * 0.6} class="gz-anchor" />
        </g>
      )}
      {drag && drag.mode === "draw" && tl === "shape" && (
        shapeKind.value === "rect" ? (
          <rect x={Math.min(drag.x0, drag.x)} y={Math.min(drag.y0, drag.y)} width={Math.abs(drag.x - drag.x0)} height={Math.abs(drag.y - drag.y0)} class={"gz-draw" + (drag.mask ? " mask" : "")} />
        ) : (
          <ellipse cx={(drag.x0 + drag.x) / 2} cy={(drag.y0 + drag.y) / 2} rx={Math.abs(drag.x - drag.x0) / 2} ry={Math.abs(drag.y - drag.y0) / 2} class={"gz-draw" + (drag.mask ? " mask" : "")} />
        )
      )}
      {readout && (
        <text x={anchor[0] + hs * 2} y={anchor[1] - hs * 2} class="gz-read" style={{ fontSize: c.h / 30 + "px" }}>
          {readout}
        </text>
      )}
    </svg>
  );
}

// ---------------------------------------------------------------- scopes

function Scopes({ c }: { c: Comp }) {
  const kinds = (catalog.value?.scopes || ["waveform", "parade", "vectorscope", "histogram"]).filter((k) => k !== "all");
  const [kind, setKind] = useState(kinds[0] || "waveform");
  const [data, setData] = useState<{ img?: string; text?: string; error?: string } | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const size = useSize(box);
  // a request per 64 px of width keeps resizing cheap
  const w = Math.max(192, Math.min(1024, Math.round((size.w - 16) / 64) * 64));
  const t = pausedPlayhead.value;
  useEffect(() => {
    if (t < 0) return;
    let live = true;
    const h = setTimeout(async () => {
      try {
        const r = await fetch(`/api/scope?kind=${kind}&t=${t.toFixed(4)}&comp=${c.id}&w=${w}`).then((r) => r.json());
        if (live) setData(r);
      } catch {}
    }, 150);
    return () => {
      live = false;
      clearTimeout(h);
    };
  }, [kind, t, rev.value, c.id, w]);
  return (
    <div class="scopes" ref={box}>
      <div class="seg">
        {kinds.map((k) => (
          <button key={k} class={"btn sm" + (k === kind ? " on" : "")} onClick={() => setKind(k)}>
            {k}
          </button>
        ))}
      </div>
      {data?.img && <img src={data.img} alt={kind} />}
      <pre>{data?.text || data?.error || "…"}</pre>
    </div>
  );
}

// ---------------------------------------------------------------- source monitor

function SourceView({ fw, fh }: { fw: number; fh: number }) {
  const id = sourceMedia.value;
  const m = id ? mediaById.value.get(id) : undefined;
  const [src, load] = useLatestImage();
  useEffect(() => {
    if (!m || m.kind === "audio") return;
    const w = Math.max(160, Math.round(Math.min(m.w || 1280, fw * (window.devicePixelRatio || 1))));
    load(`/api/thumb?media=${m.id}${m.kind === "video" ? `&t=${sourceT.value.toFixed(3)}` : ""}&w=${w}`);
  }, [m?.id, sourceT.value, Math.round(fw)]);
  if (!m) return <div class="empty">Click a clip in the Project panel to open it here, set In / Out (I, O), then Insert (,) or Overwrite (.) at the playhead.</div>;
  if (m.kind === "audio")
    return (
      <div class="mon-frame" style={{ width: fw + "px", height: fh + "px", background: "#0f1513" }}>
        <img src={`/api/waveform?media=${m.id}&w=1200&h=300`} draggable={false} style={{ objectFit: "fill" }} />
        <div class="src-ph" style={{ left: (sourceT.value / Math.max(0.01, m.dur)) * 100 + "%" }} />
      </div>
    );
  const aspect = m.w && m.h ? m.w / m.h : 16 / 9;
  let w = fw;
  let h = w / aspect;
  if (h > fh) {
    h = fh;
    w = h * aspect;
  }
  return <div class="mon-frame" style={{ width: w + "px", height: h + "px", background: "#000" }}>{src && <img src={src} draggable={false} />}</div>;
}

function SourceBar() {
  const id = sourceMedia.value;
  const m = id ? mediaById.value.get(id) : undefined;
  const dur = m?.dur || 0;
  const f = m?.fps || fps.value;
  const t = sourceT.value;
  const a = sourceIn.value;
  const b = sourceOut.value;
  return (
    <div class="mon-bar src-bar">
      <span class="tc">{fmtTC(t, f)}</span>
      <div class="src-scrub">
        {dur > 0 && (a !== null || b !== null) && <div class="src-range" style={{ left: ((a ?? 0) / dur) * 100 + "%", right: (1 - (b ?? dur) / dur) * 100 + "%" }} />}
        <input type="range" min={0} max={Math.max(0.01, dur)} step={1 / f} value={t} disabled={!dur} onInput={(e) => (sourceT.value = parseFloat((e.target as HTMLInputElement).value))} onKeyDown={(e) => e.stopPropagation()} />
      </div>
      <button class="btn sm" title="Mark In (I)" disabled={!dur} onClick={() => (sourceIn.value = t)}>
        {"{"} In
      </button>
      <button class="btn sm" title="Mark Out (O)" disabled={!dur} onClick={() => (sourceOut.value = t)}>
        Out {"}"}
      </button>
      <button class="btn sm" title="Insert at the playhead, rippling later clips (,)" disabled={!m} onClick={() => placeSource("insert")}>
        Insert
      </button>
      <button class="btn sm" title="Overwrite at the playhead (.)" disabled={!m} onClick={() => placeSource("overwrite")}>
        Overwrite
      </button>
      <span class="tc dim">{a !== null || b !== null ? `${fmtNum((b ?? dur) - (a ?? 0), 2)}s` : fmtTC(dur, f)}</span>
    </div>
  );
}

// ---------------------------------------------------------------- panels

/** Size of an element, kept current. */
function useSize(ref: { current: HTMLElement | null }) {
  const [size, setSize] = useState({ w: 640, h: 360 });
  useEffect(() => {
    const el = ref.current!;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return size;
}

/** The largest frame of `aspect` inside the stage, with a margin. */
function fitFrame(stage: { w: number; h: number }, aspect: number) {
  let fw = stage.w - 24;
  let fh = fw / aspect;
  if (fh > stage.h - 24) {
    fh = stage.h - 24;
    fw = fh * aspect;
  }
  return { fw: Math.max(32, fw), fh: Math.max(18, fh) };
}

/** Program monitor: the composition, playback, safe guides, transform gizmo, Type and Shape tools. */
export function ProgramMonitor() {
  const stageRef = useRef<HTMLDivElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const stage = useSize(stageRef);
  const [src, load] = useLatestImage();
  const [quality, setQuality] = useState(0.5);
  const [safe, setSafe] = useState(false);
  const [streamFrom, setStreamFrom] = useState(0);
  const c = comp.value;
  const p = project.value;
  const dur = c ? compDuration(c) : 0;
  const { fw, fh } = fitFrame(stage, c ? c.w / c.h : 16 / 9);

  useEffect(() => {
    const t = pausedPlayhead.value;
    if (!c || t < 0) return;
    const w = Math.round(Math.min(c.w, fw * (window.devicePixelRatio || 1)) * quality * 2) || 320;
    load(`/api/frame?t=${t.toFixed(4)}&w=${Math.max(64, w)}&comp=${c.id}&r=${rev.value}`);
  }, [pausedPlayhead.value, rev.value, c?.id, quality, Math.round(fw)]);

  // playback via streamed fragmented MP4
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    if (playing.value) {
      const from = playhead.value >= dur - 0.05 ? 0 : playhead.value;
      setStreamFrom(from);
      attachMeter(v);
      v.src = `/api/stream?from=${from.toFixed(3)}&scale=${quality}&comp=${activeId.value}&r=${Date.now()}`;
      v.play().catch(() => (playing.value = false));
    } else {
      v.pause();
      v.removeAttribute("src");
      v.load();
    }
  }, [playing.value]);

  useEffect(() => {
    const v = videoRef.current!;
    let raf = 0;
    const tick = () => {
      if (playing.value && !v.paused) setPlayhead(streamFrom + v.currentTime);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    const end = () => (playing.value = false);
    v.addEventListener("ended", end);
    return () => {
      cancelAnimationFrame(raf);
      v.removeEventListener("ended", end);
    };
  }, [streamFrom]);

  const step = (n: number) => setPlayhead(Math.max(0, playhead.peek() + n / fps.peek()));

  return (
    <div class="mon" onPointerDown={() => (monitorTab.value = "program")}>
      <div class="mon-head">
        <select value={activeId.value} onChange={(e) => run(`comp use ${(e.target as HTMLSelectElement).value}`, true)}>
          {p?.comps.map((cc) => (
            <option value={cc.id} key={cc.id}>
              {cc.id} · {cc.name} {cc.id === p.main ? "★" : ""}
            </option>
          ))}
        </select>
        <span class="badge">
          {c?.w}×{c?.h} · {c?.fps}fps
        </span>
        <span style={{ flex: 1 }} />
        <select value={String(quality)} onChange={(e) => setQuality(parseFloat((e.target as HTMLSelectElement).value))} title="Playback resolution">
          <option value="1">Full</option>
          <option value="0.5">1/2</option>
          <option value="0.25">1/4</option>
        </select>
        <button class={"btn sm" + (safe ? " on" : "")} onClick={() => setSafe(!safe)} title="Title/action safe guides">
          Safe
        </button>
        <button class="btn sm" onClick={() => showPanel("scopes")} title="Lumetri Scopes panel: waveform, RGB parade, vectorscope, histogram">
          Scopes
        </button>
      </div>
      <div class="mon-stage" ref={stageRef}>
        <div class="mon-frame" style={{ width: fw + "px", height: fh + "px", background: c?.bg && c.bg.length <= 7 ? c.bg : "transparent" }}>
          {src && <img src={src} draggable={false} />}
          <video ref={videoRef} playsInline style={{ visibility: playing.value ? "visible" : "hidden" }} />
          {safe && <div class="safe" />}
          {c && <Overlay c={c} />}
        </div>
      </div>
      <div class="mon-bar">
        <Timecode />
        <span style={{ flex: 1 }} />
        <button class="btn icon ghost" title="Start (Home)" onClick={() => setPlayhead(0)}>
          ⏮
        </button>
        <button class="btn icon ghost" title="Previous frame (←)" onClick={() => step(-1)}>
          ◀
        </button>
        <button class="btn icon" style={{ minWidth: "44px" }} title="Play / pause (Space)" onClick={() => (playing.value = !playing.value)}>
          {playing.value ? "❚❚" : "▶"}
        </button>
        <button class="btn icon ghost" title="Next frame (→)" onClick={() => step(1)}>
          ▶
        </button>
        <button class="btn icon ghost" title="End (End)" onClick={() => setPlayhead(dur)}>
          ⏭
        </button>
        <span style={{ flex: 1 }} />
        <span class="tc dim">{fmtTC(dur, fps.value)}</span>
      </div>
    </div>
  );
}

/** Only this re-renders on every frame of playback. */
function Timecode() {
  return <span class="tc">{fmtTC(playhead.value, fps.value)}</span>;
}

/** Source monitor: preview a media file, mark In / Out, insert or overwrite at the playhead. */
export function SourceMonitor() {
  const stageRef = useRef<HTMLDivElement>(null);
  const stage = useSize(stageRef);
  const m = sourceMedia.value ? mediaById.value.get(sourceMedia.value) : undefined;
  const { fw, fh } = fitFrame(stage, m && m.w && m.h ? m.w / m.h : 16 / 9);
  return (
    <div class="mon" onPointerDown={() => (monitorTab.value = "source")}>
      <div class="mon-head">
        <select value={sourceMedia.value || ""} onChange={(e) => {
          sourceMedia.value = (e.target as HTMLSelectElement).value || null;
          sourceT.value = 0;
          sourceIn.value = null;
          sourceOut.value = null;
        }}>
          <option value="">(no clip)</option>
          {project.value?.media.filter((x) => x.kind !== "svg").map((x) => (
            <option key={x.id} value={x.id}>
              {x.id} · {x.name}
            </option>
          ))}
        </select>
        {m && (
          <span class="badge">
            {m.kind} {m.w ? `${m.w}×${m.h}` : ""} {m.dur ? `${fmtNum(m.dur, 2)}s` : ""}
          </span>
        )}
      </div>
      <div class="mon-stage" ref={stageRef}>
        <SourceView fw={fw} fh={fh} />
      </div>
      <SourceBar />
    </div>
  );
}

/** Lumetri Scopes panel. */
export function ScopesPanel() {
  const c = comp.value;
  if (!c) return null;
  return <Scopes c={c} />;
}
