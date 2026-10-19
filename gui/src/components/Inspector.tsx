import { useEffect, useState } from "preact/hooks";
import { easeSelected, gotoKey } from "../actions";
import { catalog, comp, mediaById, playhead, run, selectedItem, selection, setPlayhead } from "../store";
import type { Anim, Effect, Item, PDef, Val } from "../types";
import { fmtNum, isAnimated, kindColor, q, staticVal, valToScript } from "../util";
import { NumField, TextField } from "./Controls";

function stepFor(p: PDef): number {
  if (p.max !== null && p.min !== null && p.max - p.min <= 2) return 0.01;
  if (p.name.includes("rot") || p.name === "angle") return 0.5;
  if (p.name === "pos" || p.name.endsWith("W") || p.name === "w" || p.name === "h") return 1;
  return 0.1;
}

function PropRow({ p, a, target, item }: { p: PDef; a: Anim | undefined; target: string; item: Item }) {
  const animated = isAnimated(a);
  const v: Val = staticVal(a) ?? p.def;
  const set = (s: string) => run(`set ${target} ${p.name}=${s}`, true);
  const key = () => {
    const t = Math.max(0, Math.min(item.dur, playhead.value - item.start));
    run(`kf+ ${target} ${p.name} ${fmtNum(t)}:${valToScript(v)}`, true);
  };
  let ctl;
  if (Array.isArray(p.type)) {
    ctl = (
      <select value={String(v)} onChange={(e) => set((e.target as HTMLSelectElement).value)}>
        {p.type.map((o) => (
          <option key={o}>{o}</option>
        ))}
      </select>
    );
  } else if (p.type === "bool") {
    ctl = <input type="checkbox" checked={!!v} onChange={(e) => set((e.target as HTMLInputElement).checked ? "1" : "0")} />;
  } else if (p.type === "color") {
    const hex = typeof v === "string" ? v : "#000000";
    const none = hex.length === 9 && hex.endsWith("00");
    ctl = (
      <>
        <input type="color" value={hex.slice(0, 7)} onChange={(e) => set((e.target as HTMLInputElement).value + (hex.length === 9 && !none ? hex.slice(7) : ""))} />
        <input value={none ? "none" : hex} onChange={(e) => set((e.target as HTMLInputElement).value || "none")} onKeyDown={(e) => e.stopPropagation()} />
      </>
    );
  } else if (p.type === "vec2") {
    const [x, y] = Array.isArray(v) ? v : [Number(v) || 0, Number(v) || 0];
    ctl = (
      <>
        <NumField value={x} step={stepFor(p)} onCommit={(nx) => set(`${fmtNum(nx)},${fmtNum(y)}`)} />
        <NumField value={y} step={stepFor(p)} onCommit={(ny) => set(`${fmtNum(x)},${fmtNum(ny)}`)} />
      </>
    );
  } else if (p.type === "num") {
    ctl = <NumField value={Number(v) || 0} step={stepFor(p)} min={p.min} max={p.max} onCommit={(n) => set(fmtNum(n, 4))} />;
  } else {
    ctl = <input value={String(v)} onChange={(e) => set(q((e.target as HTMLInputElement).value))} onKeyDown={(e) => e.stopPropagation()} />;
  }
  return (
    <div class="row" title={p.desc}>
      <label>{p.name}</label>
      <div class="ctl">{ctl}</div>
      {p.anim ? (
        <span class="kf-nav">
          {animated && target === item.id && (
            <button class="kf" title="Previous keyframe" onClick={() => gotoKey(-1, p.name)}>
              ‹
            </button>
          )}
        <button
          class={"kf" + (animated ? " on" : "")}
          title={animated ? "Add keyframe at playhead (right-click: clear animation)" : "Add keyframe at playhead"}
          onClick={key}
          onContextMenu={(e) => {
            e.preventDefault();
            if (animated) run(`kf- ${target} ${p.name}`, true);
          }}
        >
          {animated ? "◆" : "◇"}
        </button>
          {animated && target === item.id && (
            <button class="kf" title="Next keyframe" onClick={() => gotoKey(1, p.name)}>
              ›
            </button>
          )}
        </span>
      ) : (
        <span />
      )}
    </div>
  );
}

function Group({ title, children, open = true, acts }: { title: string; children: any; open?: boolean; acts?: any }) {
  const [o, setO] = useState(open);
  return (
    <div class="group">
      <div class="group-h" onClick={() => setO(!o)}>
        <span class="chev">{o ? "▾" : "▸"}</span>
        {title}
        <span class="acts" onClick={(e) => e.stopPropagation()}>
          {acts}
        </span>
      </div>
      {o && <div class="group-b">{children}</div>}
    </div>
  );
}

function SubList({ item, list, prefixVerb, defs, title, addOptions, addCmd }: { item: Item; list: Effect[]; prefixVerb: string; defs: (e: Effect) => PDef[]; title: string; addOptions?: string[]; addCmd?: (name: string) => string }) {
  return (
    <Group
      title={`${title} (${list.length})`}
      acts={
        addOptions && (
          <select
            value=""
            style={{ width: "92px", fontSize: "11px", padding: "1px 4px" }}
            onChange={(e) => {
              const v = (e.target as HTMLSelectElement).value;
              if (v) run(addCmd!(v));
            }}
          >
            <option value="">+ add</option>
            {addOptions.map((o) => (
              <option key={o}>{o}</option>
            ))}
          </select>
        )
      }
    >
      {list.length === 0 && <div style={{ color: "var(--faint)", fontSize: "12px" }}>none</div>}
      {list.map((e) => (
        <div key={e.id} style={{ marginBottom: "8px", paddingBottom: "6px", borderBottom: "1px dashed var(--line)" }}>
          <div style={{ display: "flex", alignItems: "center", gap: "6px", marginBottom: "4px" }}>
            <input type="checkbox" checked={e.on !== false} onChange={() => run(`set ${item.id}.${e.id} on=${e.on === false ? 1 : 0}`, true)} />
            <b style={{ fontSize: "12px" }}>{e.kind}</b>
            <span style={{ color: "var(--faint)", fontSize: "11px" }}>{e.id}</span>
            <span style={{ marginLeft: "auto", display: "flex", gap: "2px" }}>
              {prefixVerb === "fx" && (
                <>
                  <button class="btn sm ghost" onClick={() => run(`fx ${item.id}.${e.id} up`, true)}>
                    ↑
                  </button>
                  <button class="btn sm ghost" onClick={() => run(`fx ${item.id}.${e.id} down`, true)}>
                    ↓
                  </button>
                </>
              )}
              <button class="btn sm ghost" title="Delete" onClick={() => run(`del ${item.id}.${e.id}`, true)}>
                ✕
              </button>
            </span>
          </div>
          {defs(e).map((p) => (
            <PropRow key={p.name} p={p} a={e.props?.[p.name]} target={`${item.id}.${e.id}`} item={item} />
          ))}
        </div>
      ))}
    </Group>
  );
}

const ALIGN: [string, string, string][] = [
  ["left", "⇤", "Align left edges"],
  ["hcenter", "↔", "Align horizontal centers"],
  ["right", "⇥", "Align right edges"],
  ["top", "⤒", "Align top edges"],
  ["vcenter", "↕", "Align vertical centers"],
  ["bottom", "⤓", "Align bottom edges"],
];

/** After Effects Align panel: to the composition, the title-safe area, or the selection. */
function AlignGroup() {
  return (
    <Group title="Align & Distribute" open={false}>
      <AlignControls />
    </Group>
  );
}

/** The Align panel on its own. */
export function AlignPanel() {
  if (!selection.value.length) return <div class="empty">Select layers to align them to the composition, the title-safe area or each other.</div>;
  return (
    <div class="scroll">
      <div class="group-b" style={{ paddingTop: "10px" }}>
        <AlignControls />
      </div>
    </div>
  );
}

function AlignControls() {
  const [to, setTo] = useState("comp");
  const ids = selection.value.join(",");
  const n = selection.value.length;
  const at = () => fmtNum(playhead.peek(), 4);
  return (
    <>
      <div class="row">
        <label>align to</label>
        <div class="ctl">
          <select value={to} onChange={(e) => setTo((e.target as HTMLSelectElement).value)}>
            <option value="comp">composition</option>
            <option value="safe">title safe</option>
            <option value="sel" disabled={n < 2}>
              selection
            </option>
          </select>
        </div>
        <span />
      </div>
      <div class="btn-row">
        {ALIGN.map(([k, icon, tip]) => (
          <button key={k} class="btn sm" title={tip} onClick={() => run(`align ${ids} ${k} to=${to === "sel" && n < 2 ? "comp" : to} at=${at()}`, true)}>
            {icon}
          </button>
        ))}
        <button class="btn sm" title="Center in the frame" onClick={() => run(`align ${ids} center to=${to === "sel" && n < 2 ? "comp" : to} at=${at()}`, true)}>
          ⊕
        </button>
      </div>
      <div class="btn-row">
        <button class="btn sm" disabled={n < 3} title="Distribute horizontally (3+ layers)" onClick={() => run(`distribute ${ids} h`, true)}>
          ⇹ distribute H
        </button>
        <button class="btn sm" disabled={n < 3} title="Distribute vertically (3+ layers)" onClick={() => run(`distribute ${ids} v`, true)}>
          ⇳ distribute V
        </button>
      </div>
    </>
  );
}

/** Character panel (After Effects): the selected text layer's content and type settings. */
export function CharacterPanel() {
  const sel = selectedItem.value;
  const cat = catalog.value;
  const it = sel?.item;
  const [text, setText] = useState("");
  useEffect(() => setText(it?.text || ""), [it?.id, it?.text]);
  if (!it || it.kind !== "text" || !cat) return <div class="empty">Select a text layer to set its font, size, tracking, color and stroke.</div>;
  return (
    <div class="scroll">
      <div class="group-b" style={{ paddingTop: "10px" }}>
        <textarea rows={3} style={{ width: "100%" }} value={text} onInput={(e) => setText((e.target as HTMLTextAreaElement).value)} onBlur={() => text !== it.text && run(`set ${it.id} text=${q(text)}`, true)} onKeyDown={(e) => e.stopPropagation()} />
        {cat.props.text.map((p) => (
          <PropRow key={p.name} p={p} a={it.props?.[p.name]} target={it.id} item={it} />
        ))}
      </div>
    </div>
  );
}

/** Keyframe Assistant: easing, reverse, sequence layers, and keyframe navigation (J / K). */
function KeyframeGroup({ item }: { item: Item }) {
  const ids = selection.value.join(",");
  const n = selection.value.length;
  const keyed = Object.values(item.props || {}).some(isAnimated) || (item.fx || []).some((e) => Object.values(e.props || {}).some(isAnimated));
  return (
    <Group title="Keyframe Assistant" open={false}>
      <div class="btn-row">
        <button class="btn sm" title="Previous keyframe (J)" onClick={() => gotoKey(-1)}>
          ◀ key
        </button>
        <button class="btn sm" title="Next keyframe (K)" onClick={() => gotoKey(1)}>
          key ▶
        </button>
        <button class="btn sm" title="Go to the layer's in point" onClick={() => setPlayhead(item.start)}>
          in
        </button>
        <button class="btn sm" title="Go to the layer's out point" onClick={() => setPlayhead(item.start + item.dur - 1e-3)}>
          out
        </button>
      </div>
      <div class="btn-row">
        <button class="btn sm" disabled={!keyed} title="Easy Ease (F9)" onClick={() => easeSelected("easy")}>
          Easy Ease
        </button>
        <button class="btn sm" disabled={!keyed} title="Easy Ease In (Shift+F9)" onClick={() => easeSelected("easeIn")}>
          In
        </button>
        <button class="btn sm" disabled={!keyed} title="Easy Ease Out (Ctrl+Shift+F9)" onClick={() => easeSelected("easeOut")}>
          Out
        </button>
        <button class="btn sm" disabled={!keyed} title="Linear interpolation" onClick={() => easeSelected("linear")}>
          Linear
        </button>
        <button class="btn sm" disabled={!keyed} title="Hold (toggle) keyframes" onClick={() => easeSelected("hold")}>
          Hold
        </button>
      </div>
      <div class="btn-row">
        <button class="btn sm" disabled={!keyed} title="Time-reverse keyframes" onClick={() => run(`kf ${item.id} all reverse`, true)}>
          Reverse
        </button>
        <button class="btn sm" title="Bake expressions and loops into keyframes" onClick={() => run(`kf ${item.id} all bake`, true)}>
          Bake
        </button>
        <button class="btn sm" disabled={!keyed} title="Remove every keyframe" onClick={() => run(`kf ${item.id} all clear`, true)}>
          Clear
        </button>
        <button class="btn sm" disabled={n < 2} title="Sequence layers: one after the other" onClick={() => run(`stagger ${ids} seq`, true)}>
          Sequence
        </button>
        <button class="btn sm" disabled={n < 2} title="Offset each layer by 0.1s" onClick={() => run(`stagger ${ids} by=0.1`, true)}>
          Stagger
        </button>
      </div>
    </Group>
  );
}

function CompSettings() {
  const c = comp.value;
  if (!c) return null;
  const m = c.master || {};
  return (
    <div class="scroll">
      <div class="insp-head">
        <span class="kind-badge" style={{ background: "#444" }}>
          comp
        </span>
        <b>{c.name}</b>
        <span class="badge">{c.id}</span>
      </div>
      <Group title="Composition">
        <div class="row">
          <label>name</label>
          <div class="ctl">
            <TextField value={c.name} onCommit={(v) => run(`comp set ${c.id} name=${q(v)}`, true)} />
          </div>
          <span />
        </div>
        <div class="row">
          <label>size</label>
          <div class="ctl">
            <NumField value={c.w} step={2} onCommit={(v) => run(`comp set ${c.id} w=${Math.round(v / 2) * 2}`, true)} />
            <NumField value={c.h} step={2} onCommit={(v) => run(`comp set ${c.id} h=${Math.round(v / 2) * 2}`, true)} />
          </div>
          <span />
        </div>
        <div class="row">
          <label>fps</label>
          <div class="ctl">
            <select value={String(c.fps)} onChange={(e) => run(`comp set ${c.id} fps=${(e.target as HTMLSelectElement).value}`, true)}>
              {[23.976, 24, 25, 29.97, 30, 50, 59.94, 60].map((f) => (
                <option key={f}>{f}</option>
              ))}
            </select>
          </div>
          <span />
        </div>
        <div class="row">
          <label>background</label>
          <div class="ctl">
            <input type="color" value={c.bg.slice(0, 7)} onChange={(e) => run(`comp set ${c.id} bg=${(e.target as HTMLInputElement).value}`, true)} />
            <input value={c.bg} onChange={(e) => run(`comp set ${c.id} bg=${(e.target as HTMLInputElement).value}`, true)} onKeyDown={(e) => e.stopPropagation()} />
          </div>
          <span />
        </div>
      </Group>
      <Group title="Master audio">
        <div class="row">
          <label>volume dB</label>
          <div class="ctl">
            <NumField value={m.vol || 0} step={0.5} onCommit={(v) => run(`master vol=${fmtNum(v)}`, true)} />
          </div>
          <span />
        </div>
        <div class="row">
          <label>loudness</label>
          <div class="ctl">
            <select value={m.lufs === undefined ? "off" : String(m.lufs)} onChange={(e) => run(`master lufs=${(e.target as HTMLSelectElement).value}`, true)}>
              <option value="off">off</option>
              <option value="-14">-14 LUFS (YouTube/Spotify)</option>
              <option value="-16">-16 LUFS (podcast)</option>
              <option value="-23">-23 LUFS (broadcast)</option>
            </select>
          </div>
          <span />
        </div>
        <div class="row">
          <label>limiter</label>
          <div class="ctl">
            <input type="checkbox" checked={!!m.limiter} onChange={(e) => run(`master limiter=${(e.target as HTMLInputElement).checked ? 1 : 0}`, true)} />
          </div>
          <span />
        </div>
      </Group>
      <div class="empty">
        Select a clip to edit its properties.
        <br />
        <span class="kbd">Space</span> play · <span class="kbd">C</span> razor · <span class="kbd">Ctrl+K</span> split · <span class="kbd">Del</span> delete
      </div>
    </div>
  );
}

export function Inspector() {
  const sel = selectedItem.value;
  const cat = catalog.value;
  const [text, setText] = useState("");
  const it = sel?.item;
  useEffect(() => setText(it?.text || ""), [it?.id, it?.text]);
  if (!sel || !it || !cat) return <CompSettings />;
  const media = it.src ? mediaById.value.get(it.src) : undefined;
  const audioOnly = media?.kind === "audio";
  const kindProps: Record<string, string> = { clip: "clip", text: "text", solid: "solid", comp: "comp", camera: "camera" };
  const own = cat.props[kindProps[it.kind]] || [];
  const layer = cat.props.layer;
  const audioNames = new Set(["vol", "pan", "mute", "fadeIn", "fadeOut"]);
  const ownVisual = own.filter((p) => !audioNames.has(p.name));
  const ownAudio = own.filter((p) => audioNames.has(p.name));
  const trNames = cat.transitions.map((t) => t.name);
  const anims = [...cat.anims.map((a) => a.name), ...(it.kind === "text" ? cat.text_anims.map((a) => a.name) : [])];
  const fxDefs = (e: Effect) => cat.effects.find((f) => f.name === e.kind)?.params || [];
  const afxDefs = (e: Effect) => cat.audio_effects.find((f) => f.name === e.kind)?.params || [];
  const shapeDefs = (e: Effect) => [...(cat.shape_kinds.find((s) => s.name === e.kind)?.params || []), ...cat.props.shape];
  const trRow = (edge: "in" | "out") => {
    const t = edge === "in" ? it.tin : it.tout;
    return (
      <div class="row">
        <label>{edge === "in" ? "transition in" : "transition out"}</label>
        <div class="ctl">
          <select value={t?.kind || "none"} onChange={(e) => run(`tr ${it.id} ${edge} ${(e.target as HTMLSelectElement).value} ${t?.dur || 0.5}`, true)}>
            <option value="none">none</option>
            {trNames.map((n) => (
              <option key={n}>{n}</option>
            ))}
          </select>
          {t && <NumField value={t.dur} step={0.05} min={0.04} onCommit={(v) => run(`tr ${it.id} ${edge} ${t.kind} ${fmtNum(v)}`, true)} />}
        </div>
        <span />
      </div>
    );
  };
  return (
    <div class="scroll">
      <div class="insp-head">
        <span class="kind-badge" style={{ background: kindColor(it.kind, sel.track.kind) }}>
          {audioOnly ? "audio" : it.kind}
        </span>
        <TextField key={it.id} style={{ flex: 1 }} value={it.name || ""} placeholder={it.id} onCommit={(v) => run(`name ${it.id} ${q(v || "-")}`, true)} />
        <span class="badge">
          {it.id} · {sel.track.id}
        </span>
      </div>
      <Group title="Timing">
        <div class="row">
          <label>enabled</label>
          <div class="ctl">
            <input type="checkbox" checked={!it.off} title="Disable to hide the clip without deleting it (Shift+E)" onChange={() => run(`set ${it.id} off=${it.off ? 0 : 1}`, true)} />
          </div>
          <span />
        </div>
        <div class="row">
          <label>start</label>
          <div class="ctl">
            <NumField value={it.start} step={1 / (sel.comp.fps || 30)} min={0} onCommit={(v) => run(`move ${it.id} at=${fmtNum(v, 4)}`, true)} />
          </div>
          <span />
        </div>
        <div class="row">
          <label>duration</label>
          <div class="ctl">
            <NumField value={it.dur} step={1 / (sel.comp.fps || 30)} min={0.01} onCommit={(v) => run(`set ${it.id} dur=${fmtNum(v, 4)}`, true)} />
          </div>
          <span />
        </div>
        {(it.kind === "clip" || it.kind === "comp") && (
          <div class="row">
            <label>source in</label>
            <div class="ctl">
              <NumField value={it.in || 0} step={1 / (sel.comp.fps || 30)} min={0} onCommit={(v) => run(`set ${it.id} in=${fmtNum(v, 4)}`, true)} />
            </div>
            <span />
          </div>
        )}
        {media && (
          <div style={{ color: "var(--faint)", fontSize: "11px", marginTop: "4px", wordBreak: "break-all" }}>
            {media.name} · {media.kind} {media.w ? `${media.w}×${media.h}` : ""} {media.dur ? `${fmtNum(media.dur, 2)}s` : ""}
          </div>
        )}
      </Group>
      {it.kind === "text" && (
        <Group title="Text">
          <textarea
            rows={3}
            style={{ width: "100%" }}
            value={text}
            onInput={(e) => setText((e.target as HTMLTextAreaElement).value)}
            onBlur={() => text !== it.text && run(`set ${it.id} text=${q(text)}`, true)}
            onKeyDown={(e) => e.stopPropagation()}
          />
          {ownVisual.map((p) => (
            <PropRow key={p.name} p={p} a={it.props?.[p.name]} target={it.id} item={it} />
          ))}
        </Group>
      )}
      {!audioOnly && (
        <Group title="Transform">
          {layer.map((p) => (
            <PropRow key={p.name} p={p} a={it.props?.[p.name]} target={it.id} item={it} />
          ))}
        </Group>
      )}
      {!audioOnly && <AlignGroup />}
      {!audioOnly && <KeyframeGroup item={it} />}
      {it.kind !== "text" && ownVisual.length > 0 && !audioOnly && (
        <Group title={it.kind === "clip" ? "Clip" : it.kind === "solid" ? "Solid" : it.kind === "camera" ? "Camera" : "Nested comp"}>
          {ownVisual.map((p) => (
            <PropRow key={p.name} p={p} a={it.props?.[p.name]} target={it.id} item={it} />
          ))}
        </Group>
      )}
      {it.kind === "shape" && <SubList item={it} list={it.shapes || []} prefixVerb="shape" defs={shapeDefs} title="Shapes" addOptions={cat.shape_kinds.map((s) => s.name)} addCmd={(n) => `shape add ${it.id} ${n}`} />}
      {!audioOnly && (
        <Group title="Animate" open={false}>
          {trRow("in")}
          {trRow("out")}
          <div class="row">
            <label>preset</label>
            <div class="ctl">
              <select value="" onChange={(e) => (e.target as HTMLSelectElement).value && run(`anim ${it.id} ${(e.target as HTMLSelectElement).value}`)}>
                <option value="">apply animation…</option>
                {anims.map((a) => (
                  <option key={a} value={a.split("|")[0]}>
                    {a}
                  </option>
                ))}
              </select>
            </div>
            <span />
          </div>
          <div class="row">
            <label>color look</label>
            <div class="ctl">
              <select value="" onChange={(e) => (e.target as HTMLSelectElement).value && run(`color ${it.id} ${(e.target as HTMLSelectElement).value}`)}>
                <option value="">apply grade…</option>
                {cat.colors.map((c) => (
                  <option key={c.name} value={c.name}>
                    {c.name} — {c.desc}
                  </option>
                ))}
              </select>
            </div>
            <span />
          </div>
        </Group>
      )}
      {!audioOnly && <SubList item={it} list={it.fx || []} prefixVerb="fx" defs={fxDefs} title="Effects" addOptions={cat.effects.filter((f) => !f.ff || it.kind === "clip").map((f) => f.name)} addCmd={(n) => `fx ${it.id} ${n}`} />}
      {!audioOnly && <SubList item={it} list={it.masks || []} prefixVerb="mask" defs={() => cat.props.mask} title="Masks" addOptions={["rect", "ellipse", "polygon", "star"]} addCmd={(n) => `mask ${it.id} ${n}`} />}
      {it.kind === "text" && <SubList item={it} list={it.anims || []} prefixVerb="anim" defs={() => cat.props.animator} title="Text animators" addOptions={cat.text_anims.map((a) => a.name)} addCmd={(n) => `anim ${it.id} ${n}`} />}
      {(ownAudio.length > 0 && (media?.has_audio || it.kind === "comp")) && (
        <Group title="Audio">
          {ownAudio.map((p) => (
            <PropRow key={p.name} p={p} a={it.props?.[p.name]} target={it.id} item={it} />
          ))}
        </Group>
      )}
      {(media?.has_audio || it.kind === "comp") && <SubList item={it} list={it.afx || []} prefixVerb="afx" defs={afxDefs} title="Audio effects" addOptions={cat.audio_effects.map((f) => f.name)} addCmd={(n) => `afx ${it.id} ${n}`} />}
      <div style={{ display: "flex", flexWrap: "wrap", gap: "6px", padding: "10px" }}>
        <button class="btn sm" onClick={() => run(`fade ${it.id} in=0.5 out=0.5`)}>
          Fade in/out
        </button>
        <button class="btn sm" onClick={() => run(`dup ${it.id}`)}>
          Duplicate
        </button>
        {it.kind === "clip" && media?.kind === "video" && (
          <>
            <button class="btn sm" onClick={() => run(`reverse ${it.id}${it.props?.reverse ? " off" : ""}`)}>
              Reverse
            </button>
            <button class="btn sm" onClick={() => run(`freeze ${it.id} at=${fmtNum(playhead.value, 4)} dur=2`)}>
              Freeze frame
            </button>
            {media.has_audio && (
              <button class="btn sm" onClick={() => run(`detach ${it.id}`)}>
                Detach audio
              </button>
            )}
          </>
        )}
        <button class="btn sm" style={{ color: "var(--danger)" }} onClick={() => run(`del ${it.id}`)}>
          Delete
        </button>
      </div>
    </div>
  );
}
