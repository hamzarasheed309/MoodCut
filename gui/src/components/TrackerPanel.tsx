// Tracker (After Effects' Tracker panel): the 3D camera tracker (solve, track points, ground plane,
// layers attached to points), the mask tracker with AE's methods, a track point driving a null,
// stabilizing and a planar corner pin. Every control writes the same script line an AI would send.
import { useEffect, useRef, useState } from "preact/hooks";
import { comp, pausedPlayhead, playhead, project, rev, run, selectedItem } from "../store";
import type { Item } from "../types";
import { fmtNum, q } from "../util";
import { Section } from "./Controls";

/** After Effects' mask tracking methods, as `track <clip>.<mask> mode=`. */
const METHODS: [string, string][] = [
  ["pos", "Position"],
  ["rotation", "Position & Rotation"],
  ["similarity", "Position, Scale & Rotation"],
  ["affine", "Position, Scale, Rotation & Skew"],
  ["perspective", "Perspective"],
];

/** The camera solve a selected clip (or the camera made from it) belongs to. */
function solveOf(it: Item | undefined): { cam: string; clip: string; attached: number } | null {
  const solves = project.value?.camsolves || {};
  if (!it) return null;
  for (const [cam, s] of Object.entries(solves)) {
    if (s.clip === it.id || cam === it.id) return { cam, clip: s.clip, attached: (s.attached || []).length };
  }
  return null;
}

/** Point ids (p3) in a reply. */
function pointIds(log: string[] | undefined): string[] {
  const out: string[] = [];
  for (const l of log || []) for (const m of l.matchAll(/^p(\d+)\b/gm)) out.push("p" + m[1]);
  return out;
}

function Row({ label, title, children }: { label: string; title?: string; children: any }) {
  return (
    <div class="row">
      <label title={title}>{label}</label>
      <div class="ctl" style={{ display: "flex", gap: "5px", flexWrap: "wrap", alignItems: "center" }}>
        {children}
      </div>
      <span />
    </div>
  );
}

/** The solved track points drawn through the solved camera at the playhead; click picks the nearest point. */
function PointsView({ id, onPick }: { id: string; onPick: (x: number, y: number) => void }) {
  const c = comp.value;
  const t = pausedPlayhead.value;
  const [data, setData] = useState<{ img?: string; text?: string; error?: string } | null>(null);
  useEffect(() => {
    if (t < 0 || !c) return;
    let live = true;
    const h = setTimeout(async () => {
      try {
        const r = await fetch(`/api/scope?kind=waveform&t=${t.toFixed(4)}&comp=${c.id}&w=640&view=points&id=${encodeURIComponent(id)}`).then((r) => r.json());
        if (live) setData(r);
      } catch {}
    }, 150);
    return () => {
      live = false;
      clearTimeout(h);
    };
  }, [id, t, rev.value, c?.id]);
  return (
    <div class="tracker-points">
      {data?.img && (
        <img
          src={data.img}
          alt="track points"
          title="Click near a point to add it to the selection"
          style={{ width: "100%", cursor: "crosshair", display: "block" }}
          onClick={(e) => {
            const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
            onPick((e.clientX - r.left) / r.width, (e.clientY - r.top) / r.height);
          }}
        />
      )}
      <pre style={{ whiteSpace: "pre-wrap", fontSize: "11px", margin: "4px 0" }}>{data?.text || data?.error || (t < 0 ? "Pause to see the track points." : "…")}</pre>
    </div>
  );
}

export function TrackerPanel() {
  const sel = selectedItem.value;
  const it = sel?.item;
  const solve = solveOf(it);
  const [fov, setFov] = useState("");
  const [nodal, setNodal] = useState(false);
  const [detail, setDetail] = useState("500");
  const [pts, setPts] = useState("");
  const [text, setText] = useState("Title");
  const [face, setFace] = useState("plane");
  const [method, setMethod] = useState("perspective");
  const [region, setRegion] = useState("0.5,0.5,0.1,0.1");
  const [smooth, setSmooth] = useState("1");
  const [pinOn, setPinOn] = useState("");
  const [info, setInfo] = useState("");
  const busy = useRef(false);
  useEffect(() => {
    setPts("");
    setInfo("");
  }, [it?.id]);
  if (!it || (it.kind !== "clip" && it.kind !== "camera" && it.kind !== "comp"))
    return <div class="empty">Select a video clip to track it, or the camera a camera solve made.</div>;
  const clip = solve ? solve.clip : it.id;
  const at = () => fmtNum(playhead.peek(), 4);
  const go = async (script: string) => {
    if (busy.current) return;
    busy.current = true;
    try {
      return await run(script);
    } finally {
      busy.current = false;
    }
  };
  const solveCmd = () => `camtrack ${clip}${fov.trim() ? ` fov=${fov.trim()}` : ""}${nodal ? " nodal" : ""}${detail && detail !== "500" ? ` points=${detail}` : ""}`;
  const pick = async (x: number, y: number) => {
    const r = await run(`camtrack ${clip} points t=${at()} near=${fmtNum(x, 3)},${fmtNum(y, 3)} n=1`, true);
    const p = pointIds(r.log)[0];
    if (p) setPts((s) => (s.split(",").map((v) => v.trim()).filter(Boolean).includes(p) ? s : [s, p].filter((v) => v.trim()).join(",")));
  };
  const ptsOk = /^p\d+(,p\d+)*$/.test(pts.replace(/\s+/g, ""));
  const ptsArg = pts.replace(/\s+/g, "");
  const masks = it.kind === "clip" || it.kind === "comp" ? it.masks || [] : [];
  return (
    <div class="scroll tracker-panel">
      <div class="insp-head">
        <b style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>Tracker · {it.name || it.id}</b>
      </div>
      <Section title="3D Camera Tracker">
        <Row label="Lens" title="Horizontal field of view in degrees; empty = solve it">
          <input style={{ width: "64px" }} value={fov} placeholder="auto" onInput={(e) => setFov((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
          <span style={{ color: "var(--faint)", fontSize: "11px" }}>° FOV</span>
          <label style={{ display: "flex", gap: "4px", alignItems: "center" }} title="Tripod shot: the camera only rotates (pans, tilts)">
            <input type="checkbox" checked={nodal} onChange={(e) => setNodal((e.target as HTMLInputElement).checked)} /> tripod
          </label>
        </Row>
        <Row label="Detail" title="Features tracked per frame (more = slower, steadier)">
          <select value={detail} onChange={(e) => setDetail((e.target as HTMLSelectElement).value)}>
            {["250", "500", "1000", "1500"].map((n) => (
              <option key={n} value={n}>
                {n} features
              </option>
            ))}
          </select>
        </Row>
        <div class="row-btns" style={{ display: "flex", gap: "5px", flexWrap: "wrap", margin: "6px 0" }}>
          <button class="btn sm" title="Solve the camera: a 3D camera that moves like the real one, plus 3D track points" onClick={() => go(solveCmd())}>
            {solve ? "Re-solve camera" : "Track camera"}
          </button>
          {solve && (
            <>
              <button
                class="btn sm"
                title="Solve error, lens and points"
                onClick={async () => {
                  const r = await run(`camtrack ${clip} info n=8`, true);
                  setInfo((r.log || []).join("\n"));
                }}
              >
                Info
              </button>
              <button class="btn sm ghost" title="Forget the solve (the camera and attached layers stay)" onClick={() => run(`camtrack ${clip} del`)}>
                Delete solve
              </button>
            </>
          )}
        </div>
        {info && <pre style={{ whiteSpace: "pre-wrap", fontSize: "11px" }}>{info}</pre>}
        {solve ? (
          <>
            <div class="hint">
              Camera {solve.cam} · {solve.attached} attached layer{solve.attached === 1 ? "" : "s"}. Green points have a low error, red ones a high error. Click
              near points to select them (3 for a plane, 1 for a spot).
            </div>
            <PointsView id={clip} onPick={pick} />
            <Row label="Points" title="Track points: p3 for a spot, p3,p7,p9 for a plane">
              <input style={{ flex: 1, minWidth: "90px" }} value={pts} placeholder="p3,p7,p9" onInput={(e) => setPts((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
              <button class="btn sm ghost" onClick={() => setPts("")}>
                Clear
              </button>
            </Row>
            <Row label="Ground" title="The plane through three points becomes the floor; the camera and attached layers follow">
              <button class="btn sm" disabled={!ptsOk || ptsArg.split(",").length < 3} onClick={() => run(`camtrack ${clip} ground=${ptsArg}`)}>
                Set ground plane
              </button>
            </Row>
            <Row label="Facing" title="plane: lies on the plane through the points; camera: faces the camera">
              <select value={face} onChange={(e) => setFace((e.target as HTMLSelectElement).value)}>
                <option value="plane">on the plane</option>
                <option value="camera">facing the camera</option>
              </select>
            </Row>
            <Row label="Create">
              <input style={{ flex: 1, minWidth: "90px" }} value={text} placeholder="Text" onInput={(e) => setText((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
              <button class="btn sm" disabled={!ptsOk || !text.trim()} onClick={() => run(`camtrack ${clip} attach text ${q(text.trim())} at=${ptsArg} face=${face}`)}>
                Text
              </button>
            </Row>
            <Row label="">
              <button class="btn sm" disabled={!ptsOk} title="A solid on the points (replace it with a screen or a sign)" onClick={() => run(`camtrack ${clip} attach solid at=${ptsArg} face=${face}`)}>
                Solid
              </button>
              <button class="btn sm" disabled={!ptsOk} title="A null on the points: parent layers to it" onClick={() => run(`camtrack ${clip} attach null at=${ptsArg} face=${face}`)}>
                Null
              </button>
              <button class="btn sm" disabled={!ptsOk} title="A shadow catcher: a soft ellipse on the floor" onClick={() => run(`camtrack ${clip} attach shape ellipse at=${ptsArg} face=${face}`)}>
                Shape
              </button>
            </Row>
          </>
        ) : (
          <div class="hint">
            Solves the clip's camera motion: a camera layer keyed every frame and numbered 3D track points to place text, solids and nulls on. The clip must fill
            the comp centered (precomp it otherwise). MCP: `camtrack {it.id}`, then `preview view=points id={it.id}`.
          </div>
        )}
      </Section>
      {(it.kind === "clip" || it.kind === "comp") && (
        <>
          <Section title="Track Mask">
            <Row label="Method" title="After Effects' mask tracking methods">
              <select value={method} onChange={(e) => setMethod((e.target as HTMLSelectElement).value)}>
                {METHODS.map(([k, n]) => (
                  <option key={k} value={k}>
                    {n}
                  </option>
                ))}
              </select>
            </Row>
            {masks.length ? (
              masks.map((m) => (
                <Row key={m.id} label={`${m.id} · ${m.kind}`}>
                  <button class="btn sm" title="Track from the playhead through the whole clip" onClick={() => go(`track ${it.id}.${m.id} mode=${method} at=${at()}`)}>
                    Track
                  </button>
                </Row>
              ))
            ) : (
              <div class="hint">Draw a mask (Mask Pen, P) around what should be followed, then track it. Affine and Perspective key the mask path itself.</div>
            )}
          </Section>
          <Section title="Track Point · Stabilize · Corner Pin" open={false}>
            <Row label="Region" title="Feature to follow: center x,y and size w,h as fractions of the frame">
              <input style={{ flex: 1, minWidth: "110px" }} value={region} onInput={(e) => setRegion((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
            </Row>
            <Row label="Null">
              <button class="btn sm" title="A null that follows the region (parent titles or effects to it)" onClick={() => go(`track ${it.id} region=${region.replace(/\s+/g, "")} to=null mode=${method === "affine" ? "similarity" : method} at=${at()}`)}>
                Track to null
              </button>
            </Row>
            <Row label="Stabilize" title="Smoothing window in seconds">
              <input style={{ width: "48px" }} value={smooth} onInput={(e) => setSmooth((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
              <span style={{ color: "var(--faint)", fontSize: "11px" }}>s smooth</span>
              <button class="btn sm" onClick={() => go(`track ${it.id} region=${region.replace(/\s+/g, "")} to=stabilize smooth=${smooth || "1"} at=${at()}`)}>
                Stabilize
              </button>
            </Row>
            <Row label="Corner pin" title="The layer whose corners follow the plane inside the region (a screen, a sign)">
              <input style={{ width: "70px" }} value={pinOn} placeholder="layer id" onInput={(e) => setPinOn((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
              <button
                class="btn sm"
                disabled={!pinOn.trim()}
                title="Planar track of the region's four corners driving the layer's corner pin"
                onClick={() => {
                  const [cx, cy, w, h] = region.split(",").map(Number);
                  if (![cx, cy, w, h].every(isFinite)) return;
                  const c = (x: number, y: number) => `${fmtNum(x, 4)},${fmtNum(y, 4)}`;
                  const corners = [c(cx - w / 2, cy - h / 2), c(cx + w / 2, cy - h / 2), c(cx + w / 2, cy + h / 2), c(cx - w / 2, cy + h / 2)].join(" ");
                  go(`track ${it.id} corners='${corners}' to=cornerpin on=${pinOn.trim()} at=${at()}`);
                }}
              >
                Track plane
              </button>
            </Row>
            <div class="hint">MCP: `track {it.id} region=.5,.5,.1,.1 to=null`, `track {it.id} region=... to=stabilize`, `track {it.id} corners='x,y x,y x,y x,y' to=cornerpin on=c5`.</div>
          </Section>
        </>
      )}
    </div>
  );
}
