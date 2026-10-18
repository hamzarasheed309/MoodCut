import { useEffect, useRef, useState } from "preact/hooks";
import { activeId, comp, fps, playhead, playing, project, rev, run, setPlayhead } from "../store";
import { compDuration, fmtTC } from "../util";

export function Monitor() {
  const stageRef = useRef<HTMLDivElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const [stage, setStage] = useState({ w: 640, h: 360 });
  const [src, setSrc] = useState<string>("");
  const [quality, setQuality] = useState(0.5);
  const [safe, setSafe] = useState(false);
  const [streamFrom, setStreamFrom] = useState(0);
  const loading = useRef(false);
  const pending = useRef<string | null>(null);
  const c = comp.value;
  const p = project.value;
  const dur = c ? compDuration(c) : 0;

  useEffect(() => {
    const el = stageRef.current!;
    const ro = new ResizeObserver(() => setStage({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const aspect = c ? c.w / c.h : 16 / 9;
  let fw = stage.w - 24;
  let fh = fw / aspect;
  if (fh > stage.h - 24) {
    fh = stage.h - 24;
    fw = fh * aspect;
  }
  fw = Math.max(32, fw);
  fh = Math.max(18, fh);

  // load frames: one in flight, newest wins
  function load(url: string) {
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
  }

  useEffect(() => {
    if (!c || playing.value) return;
    const w = Math.round(Math.min(c.w, fw * (window.devicePixelRatio || 1)) * quality * 2) || 320;
    load(`/api/frame?t=${playhead.value.toFixed(4)}&w=${Math.max(64, w)}&comp=${c.id}&r=${rev.value}`);
  }, [playhead.value, rev.value, c?.id, quality, Math.round(fw), playing.value]);

  // playback via streamed fragmented MP4
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    if (playing.value) {
      const from = playhead.value >= dur - 0.05 ? 0 : playhead.value;
      setStreamFrom(from);
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

  const step = (n: number) => setPlayhead(Math.max(0, playhead.value + n / fps.value));

  return (
    <div class="panel mon">
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
        <select value={String(quality)} onChange={(e) => setQuality(parseFloat((e.target as HTMLSelectElement).value))} title="Preview resolution">
          <option value="1">Full</option>
          <option value="0.5">1/2</option>
          <option value="0.25">1/4</option>
        </select>
        <button class={"btn sm" + (safe ? " on" : "")} onClick={() => setSafe(!safe)} title="Title/action safe guides">
          Safe
        </button>
      </div>
      <div class="mon-stage" ref={stageRef}>
        <div class="mon-frame" style={{ width: fw + "px", height: fh + "px", background: c?.bg && c.bg.length <= 7 ? c.bg : "transparent" }}>
          {src && <img src={src} draggable={false} />}
          <video ref={videoRef} playsInline style={{ visibility: playing.value ? "visible" : "hidden" }} />
          {safe && <div class="safe" />}
        </div>
      </div>
      <div class="mon-bar">
        <span class="tc">{fmtTC(playhead.value, fps.value)}</span>
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
