// Controls for the editable fields of an HTML graphic (the editor window and the Inspector).
import { Fragment } from "preact";
import { useEffect, useRef } from "preact/hooks";
import { brandOf, numUnit, shown, toHex, type GField, type Vars } from "../gfx";
import { project } from "../store";
import { fmtNum } from "../util";
import { NumField } from "./Controls";

type Change = (name: string, value: string | null) => void;

export function GraphicFields({ fields, vars, onChange, focus, onApply }: { fields: GField[]; vars: Vars; onChange: Change; focus?: { name: string; n: number } | null; onApply?: () => void }) {
  return (
    <div class="gf-list">
      {fields.map((f, i) => (
        <Fragment key={f.name}>
          {f.group && f.group !== fields[i - 1]?.group && <div class="gf-group">{f.group}</div>}
          <FieldRow f={f} vars={vars} onChange={onChange} focus={focus?.name === f.name ? focus.n : 0} onApply={onApply} />
        </Fragment>
      ))}
    </div>
  );
}

function FieldRow({ f, vars, onChange, focus, onApply }: { f: GField; vars: Vars; onChange: Change; focus: number; onApply?: () => void }) {
  const set = vars[f.name] !== undefined;
  const v = shown(f, vars);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!focus || !ref.current) return;
    ref.current.scrollIntoView({ block: "nearest" });
    (ref.current.querySelector("input:not([type=range]),textarea,select") as HTMLElement | null)?.focus();
  }, [focus]);
  // keep keys away from the timeline's shortcuts; Ctrl+Enter applies
  const keys = (e: KeyboardEvent, enterApplies = false) => {
    e.stopPropagation();
    if (e.key === "Enter" && onApply && (enterApplies || e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      onApply();
    }
  };
  const put = (x: string) => onChange(f.name, x);
  let ctl;
  switch (f.type) {
    case "longtext":
      ctl = <textarea rows={Math.min(5, Math.max(2, v.split("\n").length))} value={v} onInput={(e) => put((e.target as HTMLTextAreaElement).value)} onKeyDown={(e) => keys(e)} />;
      break;
    case "color":
      ctl = <ColorCtl v={v} put={put} keys={keys} />;
      break;
    case "number":
      ctl = <NumberCtl f={f} v={v} put={put} />;
      break;
    case "toggle":
      ctl = <input type="checkbox" checked={v.trim() === "true"} onChange={(e) => put((e.target as HTMLInputElement).checked ? "true" : "false")} />;
      break;
    case "select":
      ctl = (
        <select value={v} onChange={(e) => put((e.target as HTMLSelectElement).value)}>
          {!(f.options || []).includes(v) && <option value={v}>{v}</option>}
          {(f.options || []).map((o) => (
            <option key={o} value={o}>
              {o}
            </option>
          ))}
        </select>
      );
      break;
    case "media":
      ctl = <MediaCtl v={v} put={put} />;
      break;
    default:
      ctl = <input value={v} onInput={(e) => put((e.target as HTMLInputElement).value)} onKeyDown={(e) => keys(e, true)} />;
  }
  return (
    <div ref={ref} class={"gf-row" + (set ? " set" : "") + (focus ? " flash" : "")} key={focus}>
      <label title={`${f.label}  {{${f.name}}}${f.help ? "\n" + f.help : ""}${f.used.length ? "" : "\n(read by the graphic's script)"}`}>{f.label}</label>
      <div class="gf-ctl">{ctl}</div>
      <button class="kf" title={set ? "Back to the default" : "This is the default"} disabled={!set} onClick={() => onChange(f.name, null)}>
        ↺
      </button>
    </div>
  );
}

function ColorCtl({ v, put, keys }: { v: string; put: (x: string) => void; keys: (e: KeyboardEvent) => void }) {
  const b = brandOf(v);
  const trimmed = v.trim();
  // keep an 8-digit color's transparency when picking a new one
  const alpha = /^#[0-9a-f]{8}$/i.test(trimmed) ? trimmed.slice(7) : "";
  return (
    <>
      <input type="color" value={toHex(v, project.value?.brand)} title={b ? `Brand ${b} color: pick another to override it` : "Pick a color"} onInput={(e) => put((e.target as HTMLInputElement).value + alpha)} />
      {b ? (
        <span class="gf-brand" title="Follows the brand kit (Brand panel)">
          brand {b}
        </span>
      ) : (
        <input key={v} defaultValue={v} spellcheck={false} onChange={(e) => put((e.target as HTMLInputElement).value.trim())} onKeyDown={(e) => keys(e)} />
      )}
    </>
  );
}

function NumberCtl({ f, v, put }: { f: GField; v: string; put: (x: string) => void }) {
  const cur = numUnit(v);
  const n = cur?.n ?? 0;
  // a bare number keeps the unit the value is written with; f.unit may only label it
  const unit = cur?.unit || numUnit(f.default)?.unit || "";
  const label = cur?.unit || f.unit || "";
  const range = f.min != null && f.max != null && f.max > f.min;
  const step = f.step || (range ? niceStep((f.max! - f.min!) / 100) : Math.abs(n) >= 5 || Number.isInteger(n) ? 1 : 0.1);
  const emit = (x: number) => put(`${fmtNum(x, 4)}${unit}`);
  return (
    <>
      {range && <input type="range" min={f.min} max={f.max} step={step} value={n} onInput={(e) => emit(parseFloat((e.target as HTMLInputElement).value))} />}
      <NumField value={n} step={step} min={f.min} max={f.max} onCommit={emit} />
      {label && <span class="gf-unit">{label}</span>}
    </>
  );
}

function niceStep(x: number): number {
  if (!(x > 0)) return 1;
  const p = Math.pow(10, Math.floor(Math.log10(x)));
  return x / p >= 5 ? 5 * p : x / p >= 2 ? 2 * p : p;
}

function MediaCtl({ v, put }: { v: string; put: (x: string) => void }) {
  const list = (project.value?.media || []).filter((m) => m.kind !== "audio");
  const id = v.startsWith("mc://") ? v.slice(5) : "";
  const known = list.some((m) => m.id === id);
  return (
    <select value={known ? id : ""} onChange={(e) => (e.target as HTMLSelectElement).value && put("mc://" + (e.target as HTMLSelectElement).value)}>
      {!known && <option value="">{v || "(none)"}</option>}
      {list.map((m) => (
        <option key={m.id} value={m.id}>
          {m.id} · {m.name}
        </option>
      ))}
    </select>
  );
}
