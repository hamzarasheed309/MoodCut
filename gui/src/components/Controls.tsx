import { useEffect, useRef, useState } from "preact/hooks";
import { fmtNum } from "../util";

/** Number field: drag horizontally to scrub, click to type. */
export function NumField({ value, onCommit, step = 0.1, min, max }: { value: number; onCommit: (v: number) => void; step?: number; min?: number | null; max?: number | null }) {
  const [text, setText] = useState(fmtNum(value));
  const [editing, setEditing] = useState(false);
  const drag = useRef<{ x: number; v: number; moved: boolean } | null>(null);
  const live = useRef(value);
  useEffect(() => {
    if (!editing && !drag.current) setText(fmtNum(value));
  }, [value, editing]);
  const lim = (v: number) => Math.min(max ?? Infinity, Math.max(min ?? -Infinity, v));
  return (
    <input
      class="num"
      value={text}
      readOnly={!editing}
      onPointerDown={(e) => {
        if (editing) return;
        drag.current = { x: e.clientX, v: value, moved: false };
        live.current = value;
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        const dx = e.clientX - d.x;
        if (Math.abs(dx) > 2) d.moved = true;
        if (!d.moved) return;
        const mult = e.shiftKey ? 10 : e.altKey ? 0.1 : 1;
        live.current = lim(d.v + dx * step * mult);
        setText(fmtNum(live.current));
      }}
      onPointerUp={(e) => {
        const d = drag.current;
        drag.current = null;
        if (!d) return;
        if (d.moved) onCommit(live.current);
        else {
          setEditing(true);
          (e.target as HTMLInputElement).select();
        }
      }}
      onInput={(e) => setText((e.target as HTMLInputElement).value)}
      onBlur={() => {
        if (!editing) return;
        setEditing(false);
        const v = parseFloat(text);
        if (isFinite(v) && v !== value) onCommit(lim(v));
        else setText(fmtNum(value));
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        if (e.key === "Escape") {
          setText(fmtNum(value));
          setEditing(false);
        }
        e.stopPropagation();
      }}
    />
  );
}

/** Text input that keeps what is being typed through live refreshes; commits on Enter or blur, Escape cancels. */
export function TextField({ value, onCommit, placeholder, cls, style, title }: { value: string; onCommit: (v: string) => void; placeholder?: string; cls?: string; style?: any; title?: string }) {
  const [text, setText] = useState(value);
  const focused = useRef(false);
  const cancel = useRef(false);
  useEffect(() => {
    if (!focused.current) setText(value);
  }, [value]);
  return (
    <input
      class={cls}
      style={style}
      title={title}
      value={text}
      placeholder={placeholder}
      onFocus={() => (focused.current = true)}
      onInput={(e) => setText((e.target as HTMLInputElement).value)}
      onBlur={(e) => {
        focused.current = false;
        const v = (e.target as HTMLInputElement).value;
        if (cancel.current) {
          cancel.current = false;
          setText(value);
        } else if (v !== value) onCommit(v);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        if (e.key === "Escape") {
          cancel.current = true;
          (e.target as HTMLInputElement).blur();
        }
        e.stopPropagation();
      }}
    />
  );
}

/** Labelled range slider; drags preview locally and commit once on release. Double-click the label to reset. */
/** Rounds to about three significant digits (for log-scale sliders). */
function roundSig(x: number, step: number): number {
  const q = Math.abs(x) >= 100 ? 1 : Math.abs(x) >= 10 ? 0.1 : Math.abs(x) >= 1 ? 0.01 : 0.001;
  const s = Math.max(q, step >= 1 ? step : 0);
  return Math.round(x / s) * s;
}

/** Labeled slider with a number field. `log` maps the track logarithmically (frequencies, times), so 20 Hz-20 kHz is usable. */
export function Slider({ label, value, min, max, step = 1, def = 0, title, onCommit, bg, log }: { label: string; value: number; min: number; max: number; step?: number; def?: number; title?: string; onCommit: (v: number) => void; bg?: string; log?: boolean }) {
  const [v, setV] = useState(value);
  const dragging = useRef(false);
  useEffect(() => {
    if (!dragging.current) setV(value);
  }, [value]);
  const commit = (n: number) => {
    dragging.current = false;
    if (Math.abs(n - value) > 1e-9) onCommit(n);
  };
  const lg = !!log && min > 0 && max > min;
  const toPos = (x: number) => (lg ? (Math.log(Math.max(min, x) / min) / Math.log(max / min)) * 1000 : x);
  // whole-number steps only when the range starts at 1 or more: 0.01-2000 ms must reach 0.5 ms
  const fromPos = (p: number) => (lg ? Math.min(max, Math.max(min, roundSig(min * Math.pow(max / min, p / 1000), min >= 1 ? step : 0))) : p);
  return (
    <div class={"sl" + (Math.abs(v - def) > 1e-9 ? " changed" : "")} title={title}>
      <label onDblClick={() => commit(def)} title={(title ? title + " · " : "") + "double-click to reset"}>
        {label}
      </label>
      <input
        type="range"
        min={lg ? 0 : min}
        max={lg ? 1000 : max}
        step={lg ? 1 : step}
        value={toPos(v)}
        style={bg ? { background: bg } : undefined}
        onPointerDown={() => (dragging.current = true)}
        onInput={(e) => setV(fromPos(parseFloat((e.target as HTMLInputElement).value)))}
        onChange={(e) => commit(fromPos(parseFloat((e.target as HTMLInputElement).value)))}
        onKeyDown={(e) => e.stopPropagation()}
      />
      <NumField value={v} step={step} min={min} max={max} onCommit={commit} />
    </div>
  );
}

/** Collapsible section with an optional on/off switch (Lumetri / Essential Sound style). */
export function Section({ title, children, open = true, on, onToggle, acts }: { title: string; children: any; open?: boolean; on?: boolean; onToggle?: (on: boolean) => void; acts?: any }) {
  const [o, setO] = useState(open);
  return (
    <div class="group">
      <div class="group-h" onClick={() => setO(!o)}>
        <span class="chev">{o ? "▾" : "▸"}</span>
        {onToggle && <input type="checkbox" checked={on !== false} title="Enable / bypass" onClick={(e) => e.stopPropagation()} onChange={(e) => onToggle((e.target as HTMLInputElement).checked)} />}
        {title}
        <span class="acts" onClick={(e) => e.stopPropagation()}>
          {acts}
        </span>
      </div>
      {o && <div class="group-b">{children}</div>}
    </div>
  );
}
