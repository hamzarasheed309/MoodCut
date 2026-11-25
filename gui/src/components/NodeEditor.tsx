// DaVinci Resolve's node editor for the selected clip. A clip's color effects are its node graph:
// consecutive effects with the same `node` label form one corrector node (one key, one mix), an
// unlabeled effect is a node of its own. Grouping mirrors crates/moodcut/src/ops/nodes.rs.
// Every control writes the `node` script line an AI would send.
import { Fragment } from "preact";
import { useEffect, useState } from "preact/hooks";
import { catalog, run } from "../store";
import type { Catalog, Effect, Item } from "../types";
import { fmtNum, staticVal } from "../util";
import { Slider, TextField } from "./Controls";

const COMPOSITE = ["normal", "add", "subtract", "multiply", "screen", "overlay", "softlight", "hardlight", "darken", "lighten", "difference", "color", "luminosity"];
const WINDOWS: [string, string][] = [
  ["circle", "Circle"],
  ["linear", "Linear"],
  ["polygon", "Polygon"],
  ["gradient", "Gradient"],
];
const QUALIFIERS: [string, string][] = [
  ["hsl", "HSL"],
  ["luma", "Luma"],
  ["rgb", "RGB"],
  ["3d", "3D"],
];

type Conn = "serial" | "parallel" | "layer";

interface NodeV {
  label: string | null;
  fx: Effect[];
  conn: Conn;
  composite: string;
  key: string;
  mix: number;
  on: boolean;
}

function sv(e: Effect, k: string) {
  return staticVal(e.props?.[k]);
}

/** The item's nodes in order, grouped as the renderer groups them. */
export function nodesOf(it: Item, cat: Catalog): NodeV[] {
  const nodable = (e: Effect) => !!cat.effects.find((d) => d.name === e.kind)?.node;
  const out: NodeV[] = [];
  let prevLabel: string | null = null;
  let prevIdx = -2;
  (it.fx || []).forEach((e, i) => {
    if (!nodable(e)) {
      prevLabel = null;
      prevIdx = -2;
      return;
    }
    const l = sv(e, "node");
    const label = typeof l === "string" && l.trim() ? l : null;
    if (label !== null && label === prevLabel && prevIdx === i - 1) out[out.length - 1].fx.push(e);
    else {
      const comp = sv(e, "composite");
      const mix = sv(e, "mix");
      out.push({
        label,
        fx: [e],
        conn: sv(e, "parallel") === true ? "parallel" : sv(e, "layer") === true ? "layer" : "serial",
        composite: typeof comp === "string" ? comp : "normal",
        key: "",
        mix: typeof mix === "number" ? mix : 1,
        on: true,
      });
    }
    prevLabel = label;
    prevIdx = i;
  });
  for (const n of out) {
    n.key = n.fx.map((e) => sv(e, "mask")).find((m): m is string => typeof m === "string" && !!m.trim() && m !== "none") || "";
    n.on = n.fx.some((e) => e.on !== false);
  }
  return out;
}

const GLYPH: Record<Conn, string> = { serial: "→", parallel: "∥", layer: "≡" };

export function NodeEditor({ it }: { it: Item }) {
  const cat = catalog.value;
  const [sel, setSel] = useState(0);
  const [win, setWin] = useState("");
  useEffect(() => setSel(0), [it.id]);
  if (!cat) return null;
  const nodes = nodesOf(it, cat);
  const n = Math.min(sel, Math.max(0, nodes.length - 1));
  const cur = nodes[n];
  const num = n + 1;
  const node = (rest: string) => run(`node ${it.id} ${num} ${rest}`, true);
  const tools = cat.effects.filter((d) => d.node);
  const colorTools = tools.filter((d) => d.cat === "color");
  const otherTools = tools.filter((d) => d.cat !== "color");
  return (
    <div class="node-editor">
      <div class="node-chain" style={{ display: "flex", flexWrap: "wrap", gap: "4px", alignItems: "center", margin: "4px 0 8px" }}>
        <span class="node-io" title="Clip input" style={{ fontSize: "11px", color: "var(--faint)" }}>
          IN
        </span>
        {nodes.map((nd, i) => (
          <Fragment key={i}>
            <span title={nd.conn === "serial" ? "serial: takes the previous node's output" : nd.conn === "parallel" ? "parallel: shares the previous node's input; the results are mixed" : `layer mixer (${nd.composite}): composited over the previous node`} style={{ color: "var(--faint)" }}>
              {GLYPH[nd.conn]}
            </span>
            <button
              class={"btn sm node-chip" + (i === n ? " on" : "")}
              style={{ opacity: nd.on ? 1 : 0.45, display: "flex", flexDirection: "column", alignItems: "flex-start", lineHeight: 1.15, padding: "3px 6px" }}
              title={`${nd.fx.map((e) => `${e.id} ${e.kind}`).join(", ")}${nd.key ? ` · key ${nd.key}` : ""}${nd.mix < 0.999 ? ` · mix ${fmtNum(nd.mix, 2)}` : ""}`}
              onClick={() => setSel(i)}
            >
              <b>
                {i + 1} {nd.label || ""}
              </b>
              <small style={{ opacity: 0.8 }}>
                {nd.fx.map((e) => e.kind).join(" + ")}
                {nd.key ? " ◐" : ""}
              </small>
            </button>
          </Fragment>
        ))}
        <span style={{ color: "var(--faint)" }}>→</span>
        <span class="node-io" title="Clip output" style={{ fontSize: "11px", color: "var(--faint)" }}>
          OUT
        </span>
      </div>
      <div style={{ display: "flex", gap: "5px", flexWrap: "wrap", marginBottom: "8px" }}>
        <button class="btn sm" title="Serial node after the selected one (Alt+S in Resolve)" onClick={() => run(`node ${it.id} add serial after=${nodes.length ? num : 0}`, true).then(() => setSel(nodes.length ? n + 1 : 0))}>
          + Serial
        </button>
        <button class="btn sm" disabled={!nodes.length} title="Parallel node: shares the selected node's input" onClick={() => run(`node ${it.id} add parallel after=${num}`, true).then(() => setSel(n + 1))}>
          + Parallel
        </button>
        <button class="btn sm" disabled={!nodes.length} title="Layer mixer node: composited over the selected node" onClick={() => run(`node ${it.id} add layer after=${num} composite=normal`, true).then(() => setSel(n + 1))}>
          + Layer
        </button>
        <button class="btn sm" disabled={!cur?.key} title="Outside node: grades everything outside the selected node's key" onClick={() => run(`node ${it.id} add outside after=${num}`, true).then(() => setSel(n + 1))}>
          + Outside
        </button>
      </div>
      {!cur ? (
        <div class="hint">No nodes yet. Add a serial node, or grade with the sections below: each tool becomes a node. MCP: `node {it.id} add`, `node {it.id} 1 primaries gainR=1.03`.</div>
      ) : (
        <div class="node-detail">
          <div class="row">
            <label>Node {num}</label>
            <div class="ctl" style={{ display: "flex", gap: "5px", alignItems: "center" }}>
              <TextField value={cur.label || ""} placeholder="name" style={{ width: "90px" }} onCommit={(v) => v.trim() && node(`name=${v.trim().replace(/\s+/g, "_")}`)} />
              <label style={{ display: "flex", gap: "3px", alignItems: "center" }} title="Bypass the node (Ctrl+D in Resolve)">
                <input type="checkbox" checked={cur.on} onChange={(e) => node((e.target as HTMLInputElement).checked ? "on" : "off")} /> on
              </label>
            </div>
            <span />
          </div>
          <div class="row">
            <label>Input</label>
            <div class="ctl" style={{ display: "flex", gap: "5px" }}>
              <select value={cur.conn} disabled={num === 1} onChange={(e) => node((e.target as HTMLSelectElement).value)}>
                <option value="serial">serial</option>
                <option value="parallel">parallel</option>
                <option value="layer">layer mixer</option>
              </select>
              {cur.conn === "layer" && (
                <select value={cur.composite} title="Composite mode" onChange={(e) => node(`layer composite=${(e.target as HTMLSelectElement).value}`)}>
                  {COMPOSITE.map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </select>
              )}
            </div>
            <span />
          </div>
          <Slider label="Key mix" title="The node's output blend (Resolve's key output gain)" value={cur.mix} min={0} max={1} step={0.01} def={1} onCommit={(v) => node(`mix=${fmtNum(v, 3)}`)} />
          <div class="sub">Tools</div>
          {cur.fx.map((e) => (
            <div class="row" key={e.id}>
              <label title={`${it.id}.${e.id}`}>{e.kind}</label>
              <div class="ctl" style={{ display: "flex", gap: "5px", alignItems: "center" }}>
                <input type="checkbox" checked={e.on !== false} title="Enable this tool" onChange={(ev) => run(`set ${it.id}.${e.id} on=${(ev.target as HTMLInputElement).checked ? 1 : 0}`, true)} />
                <span style={{ fontSize: "11px", color: "var(--faint)" }}>{it.id}.{e.id}</span>
                {cur.fx.length > 1 && (
                  <button class="btn sm ghost" title="Remove this tool from the node" onClick={() => run(`del ${it.id}.${e.id}`, true)}>
                    ×
                  </button>
                )}
              </div>
              <span />
            </div>
          ))}
          <div class="row">
            <label>Add tool</label>
            <div class="ctl">
              <select value="" onChange={(e) => { const v = (e.target as HTMLSelectElement).value; if (v) node(v); }}>
                <option value="">add to node {num}…</option>
                <optgroup label="Color">
                  {colorTools.filter((d) => d.name !== "lut").map((d) => (
                    <option key={d.name} value={d.name} title={d.desc}>
                      {d.name}
                    </option>
                  ))}
                </optgroup>
                <optgroup label="Other">
                  {otherTools.map((d) => (
                    <option key={d.name} value={d.name} title={d.desc}>
                      {d.name}
                    </option>
                  ))}
                </optgroup>
              </select>
            </div>
            <span />
          </div>
          <div class="sub">Key {cur.key ? `· ${cur.key}` : "· whole frame"}</div>
          <div class="row">
            <label>Window</label>
            <div class="ctl" style={{ display: "flex", gap: "4px", flexWrap: "wrap" }}>
              {WINDOWS.map(([k, label]) => (
                <button key={k} class="btn sm" title={`Add a ${label.toLowerCase()} window to node ${num}`} onClick={() => node(`window add ${k}`)}>
                  + {label}
                </button>
              ))}
            </div>
            <span />
          </div>
          <div class="row">
            <label>Qualifier</label>
            <div class="ctl" style={{ display: "flex", gap: "4px", flexWrap: "wrap" }}>
              {QUALIFIERS.map(([k, label]) => (
                <button key={k} class="btn sm" title={`Key node ${num} with a ${label} qualifier (ANDed with its windows)`} onClick={() => node(`qualify ${k}`)}>
                  {label}
                </button>
              ))}
            </div>
            <span />
          </div>
          <div class="row">
            <label>Key</label>
            <div class="ctl" style={{ display: "flex", gap: "4px" }}>
              <TextField value={win || cur.key} placeholder="m1+m2*m3" style={{ flex: 1 }} title="Windows (+) and qualifiers (*), e.g. (m1+m2)*m3; -m1 inverts" onCommit={(v) => { setWin(""); node(`window=${v.trim() ? v.trim().replace(/\s+/g, "") : "none"}`); }} />
              <button class="btn sm ghost" disabled={!cur.key} title="Grade the whole frame" onClick={() => node("window none")}>
                Clear
              </button>
              <button class="btn sm" disabled={!cur.key} title="Track the node's windows through the clip" onClick={() => run(`node ${it.id} ${num} track`)}>
                Track
              </button>
            </div>
            <span />
          </div>
          <div style={{ display: "flex", gap: "5px", flexWrap: "wrap", marginTop: "6px" }}>
            <button class="btn sm" disabled={num === 1} title="Move the node earlier" onClick={() => node("up").then(() => setSel(Math.max(0, n - 1)))}>
              ↑ Earlier
            </button>
            <button class="btn sm" disabled={num === nodes.length} title="Move the node later" onClick={() => node("down").then(() => setSel(n + 1))}>
              ↓ Later
            </button>
            <button class="btn sm ghost" title="Tools back to neutral; key and mix kept" onClick={() => node("reset")}>
              Reset
            </button>
            <button class="btn sm ghost" title="Delete the node (its masks stay)" onClick={() => node("del").then(() => setSel(Math.max(0, n - 1)))}>
              Delete
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
