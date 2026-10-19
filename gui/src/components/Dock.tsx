// Renders the dock layout: splits with draggable dividers, tab groups, floating windows,
// and drag-and-drop of panel tabs (drop on a tab bar or a group's center to add a tab,
// on a group's edge to split it, outside any group to float the panel).
import { signal } from "@preact/signals";
import { useEffect, useRef, useState } from "preact/hooks";
import { activate, closeGroup, closePanel, dock, float, focusedPanel, groups, maximized, moveFloat, narrow, PANEL_TITLES, resize, setLayout, shown } from "../dock";
import type { Floating, Group, Node, PanelId, Split, Zone } from "../dock";
import { PANELS } from "../panels";

type Target = { group: string; zone: Zone; index?: number; rect: { x: number; y: number; w: number; h: number } };
const drag = signal<{ panel: PanelId; from: string; x: number; y: number; target: Target | null } | null>(null);

function dockRect(): DOMRect {
  return (document.querySelector(".dock") as HTMLElement).getBoundingClientRect();
}

/** What lies under the pointer while dragging a tab. */
function hitTest(x: number, y: number, panel: PanelId, from: string): Target | null {
  const els = document.elementsFromPoint(x, y) as HTMLElement[];
  const bar = els.find((e) => e.dataset?.tabbar);
  if (bar) {
    const id = bar.dataset.tabbar!;
    const tabs = [...bar.querySelectorAll<HTMLElement>(".dock-tab")].filter((t) => t.dataset.panel !== panel);
    const index = tabs.filter((t) => {
      const r = t.getBoundingClientRect();
      return r.left + r.width / 2 < x;
    }).length;
    const r = bar.getBoundingClientRect();
    return { group: id, zone: "center", index, rect: { x: r.left, y: r.top, w: r.width, h: r.height } };
  }
  const el = els.find((e) => e.dataset?.group);
  if (!el) return null;
  const id = el.dataset.group!;
  const r = el.getBoundingClientRect();
  const solo = from === id && groups(shown.value).find((gr) => gr.id === id)?.panels.length === 1;
  const floating = !!el.closest(".dock-float");
  const fx = (x - r.left) / r.width;
  const fy = (y - r.top) / r.height;
  let zone: Zone = "center";
  if (!floating) zone = fx < 0.22 ? "left" : fx > 0.78 ? "right" : fy < 0.25 ? "top" : fy > 0.75 ? "bottom" : "center";
  if (solo) return null; // a group's only panel: docking it onto itself changes nothing
  const rect =
    zone === "left"
      ? { x: r.left, y: r.top, w: r.width * 0.35, h: r.height }
      : zone === "right"
        ? { x: r.left + r.width * 0.65, y: r.top, w: r.width * 0.35, h: r.height }
        : zone === "top"
          ? { x: r.left, y: r.top, w: r.width, h: r.height * 0.35 }
          : zone === "bottom"
            ? { x: r.left, y: r.top + r.height * 0.65, w: r.width, h: r.height * 0.35 }
            : { x: r.left, y: r.top, w: r.width, h: r.height };
  return { group: id, zone, rect };
}

function startTabDrag(e: PointerEvent, p: PanelId, gr: Group) {
  if (e.button !== 0) return;
  const x0 = e.clientX;
  const y0 = e.clientY;
  let started = false;
  const move = (ev: PointerEvent) => {
    if (!started && Math.hypot(ev.clientX - x0, ev.clientY - y0) < 6) return;
    started = true;
    drag.value = { panel: p, from: gr.id, x: ev.clientX, y: ev.clientY, target: hitTest(ev.clientX, ev.clientY, p, gr.id) };
  };
  const end = (ev: PointerEvent | null) => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("keydown", key, true);
    const d = drag.value;
    drag.value = null;
    if (!started || !ev || !d) {
      if (!started) {
        setLayout((l) => activate(l, p));
        focusedPanel.value = p;
      }
      return;
    }
    const t = hitTest(ev.clientX, ev.clientY, p, gr.id);
    if (t) setLayout((l) => dock(l, p, t.group, t.zone, t.index));
    else if (!(ev.target as HTMLElement)?.closest?.(`[data-group="${gr.id}"]`)) {
      const r = dockRect();
      setLayout((l) => float(l, p, ev.clientX - r.left - 80, ev.clientY - r.top - 12));
    }
  };
  const up = (ev: PointerEvent) => end(ev);
  const key = (ev: KeyboardEvent) => {
    if (ev.key === "Escape") {
      ev.stopPropagation();
      started = false;
      end(null);
    }
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("keydown", key, true);
}

function GroupMenu({ gr, floating }: { gr: Group; floating: boolean }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const off = (e: PointerEvent) => !ref.current?.contains(e.target as globalThis.Node) && setOpen(false);
    window.addEventListener("pointerdown", off, true);
    return () => window.removeEventListener("pointerdown", off, true);
  }, [open]);
  const p = gr.active;
  const act = (f: () => void) => () => {
    setOpen(false);
    f();
  };
  return (
    <div class="dock-menu" ref={ref}>
      <button class="dock-menu-btn" title="Panel menu" onClick={() => setOpen(!open)}>
        ≡
      </button>
      {open && (
        <div class="menu-pop">
          {p && !floating && !narrow.value && (
            <div class="menu-item" onClick={act(() => {
              const r = dockRect();
              setLayout((l) => float(l, p, r.width / 2 - 220, r.height / 2 - 180));
            })}>
              Undock panel
            </div>
          )}
          {p && (
            <div class="menu-item" onClick={act(() => setLayout((l) => closePanel(l, p)))}>
              Close panel
            </div>
          )}
          {gr.panels.length > 1 && (
            <div class="menu-item" onClick={act(() => setLayout((l) => {
              let n = l;
              for (const x of gr.panels) if (x !== p) n = closePanel(n, x);
              return n;
            }))}>
              Close other panels in group
            </div>
          )}
          {!floating && (
            <div class="menu-item" onClick={act(() => (maximized.value = maximized.value === gr.id ? null : gr.id))}>
              {maximized.value === gr.id ? "Restore panel size" : "Maximize panel"} <span class="kbd">`</span>
            </div>
          )}
          <div class="menu-item" onClick={act(() => setLayout((l) => closeGroup(l, gr.id)))}>
            Close panel group
          </div>
        </div>
      )}
    </div>
  );
}

function GroupView({ gr, floating = false, onHeaderDown }: { gr: Group; floating?: boolean; onHeaderDown?: (e: PointerEvent) => void }) {
  const p = gr.active;
  const Body = p ? PANELS[p] : null;
  const d = drag.value;
  return (
    <div class={"dock-group" + (p && focusedPanel.value === p ? " focused" : "")} data-group={gr.id} onPointerDownCapture={() => p && focusedPanel.value !== p && (focusedPanel.value = p)}>
      <div class="dock-tabs" data-tabbar={gr.id}>
        {gr.panels.map((x) => (
          <div
            key={x}
            class={"dock-tab" + (x === p ? " active" : "") + (d?.panel === x ? " dragging" : "")}
            data-panel={x}
            title={`${PANEL_TITLES[x]}: drag to dock elsewhere, onto an edge to split, or outside to float`}
            onPointerDown={(e) => startTabDrag(e, x, gr)}
            onAuxClick={(e) => e.button === 1 && setLayout((l) => closePanel(l, x))}
          >
            {PANEL_TITLES[x]}
            <span
              class="dock-x"
              title="Close panel"
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation();
                setLayout((l) => closePanel(l, x));
              }}
            >
              ×
            </span>
          </div>
        ))}
        <span class="dock-tabs-fill" onPointerDown={onHeaderDown} onDblClick={() => !floating && (maximized.value = maximized.value === gr.id ? null : gr.id)} />
        <GroupMenu gr={gr} floating={floating} />
      </div>
      <div class="dock-body">{Body ? <Body /> : <div class="empty">Empty group: drag a panel here or use Window.</div>}</div>
    </div>
  );
}

/** Live-resizes the two neighbouring cells, then commits the sizes on release. */
function Divider({ s, i, box }: { s: Split; i: number; box: { current: HTMLDivElement | null } }) {
  const onDown = (e: PointerEvent) => {
    const cont = box.current;
    if (!cont) return;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    const cells = [...cont.children].filter((c) => c.classList.contains("dock-cell")) as HTMLElement[];
    const row = s.dir === "row";
    const px = cells.map((c) => (row ? c.getBoundingClientRect().width : c.getBoundingClientRect().height));
    // fit-to-content cells (0) become fixed pixels once dragged
    const fixed = (k: number) => s.sizes[k] > 1 || s.sizes[k] === 0;
    const fracPx = px.reduce((a, b, k) => a + (fixed(k) ? 0 : b), 0) || 1;
    const fracSum = s.sizes.reduce((a, b, k) => a + (fixed(k) ? 0 : b), 0) || 1;
    const start = row ? e.clientX : e.clientY;
    const min = (k: number) => (fixed(k) ? 30 : 90);
    let next = s.sizes.slice();
    const move = (ev: PointerEvent) => {
      let dlt = (row ? ev.clientX : ev.clientY) - start;
      dlt = Math.max(min(i) - px[i], Math.min(px[i + 1] - min(i + 1), dlt));
      next = s.sizes.slice();
      for (const [k, v] of [
        [i, px[i] + dlt],
        [i + 1, px[i + 1] - dlt],
      ] as [number, number][]) {
        next[k] = fixed(k) ? Math.max(2, Math.round(v)) : (v / fracPx) * fracSum;
        cells[k].style.flex = next[k] > 1 ? `0 0 ${next[k]}px` : `${next[k]} 1 0px`;
      }
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      setLayout((l) => resize(l, s.id, next));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return <div class={"dock-divider dock-" + s.dir} onPointerDown={onDown} />;
}

function SplitView({ s }: { s: Split }) {
  const box = useRef<HTMLDivElement>(null);
  const out: any[] = [];
  s.kids.forEach((k, i) => {
    const sz = s.sizes[i] ?? 1;
    out.push(
      <div class={"dock-cell" + (sz === 0 ? " fit" : "")} key={k.id} style={{ flex: sz === 0 ? "0 0 auto" : sz > 1 ? `0 0 ${sz}px` : `${sz} 1 0px` }}>
        <NodeView n={k} />
      </div>,
    );
    if (i < s.kids.length - 1) out.push(<Divider key={"d" + k.id} s={s} i={i} box={box} />);
  });
  return (
    <div class={"dock-split dock-" + s.dir} ref={box}>
      {out}
    </div>
  );
}

function NodeView({ n }: { n: Node }) {
  return n.t === "group" ? <GroupView gr={n} /> : <SplitView s={n} />;
}

function FloatView({ f }: { f: Floating }) {
  const ref = useRef<HTMLDivElement>(null);
  const track = (e: PointerEvent, kind: "move" | "size") => {
    if (e.button !== 0) return;
    e.preventDefault();
    const el = ref.current!;
    const x0 = e.clientX;
    const y0 = e.clientY;
    const r0 = { x: f.x, y: f.y, w: f.w, h: f.h };
    let r = r0;
    setLayout((l) => activate(l, f.group.active || f.group.panels[0]));
    const move = (ev: PointerEvent) => {
      const dx = ev.clientX - x0;
      const dy = ev.clientY - y0;
      const b = dockRect();
      r = kind === "move" ? { ...r0, x: Math.max(-r0.w + 80, Math.min(b.width - 80, r0.x + dx)), y: Math.max(0, Math.min(b.height - 30, r0.y + dy)) } : { ...r0, w: Math.max(220, r0.w + dx), h: Math.max(140, r0.h + dy) };
      Object.assign(el.style, { left: r.x + "px", top: r.y + "px", width: r.w + "px", height: r.h + "px" });
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      setLayout((l) => moveFloat(l, f.group.id, r));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div class="dock-float" ref={ref} style={{ left: f.x + "px", top: f.y + "px", width: f.w + "px", height: f.h + "px" }}>
      <GroupView gr={f.group} floating onHeaderDown={(e) => track(e, "move")} />
      <div class="dock-resize" onPointerDown={(e) => track(e, "size")} />
    </div>
  );
}

function DragOverlay() {
  const d = drag.value;
  if (!d) return null;
  const t = d.target;
  return (
    <>
      {t && <div class="dock-drop" style={{ left: t.rect.x + "px", top: t.rect.y + "px", width: t.rect.w + "px", height: t.rect.h + "px" }} />}
      <div class="dock-ghost" style={{ left: d.x + 12 + "px", top: d.y + 10 + "px" }}>
        {PANEL_TITLES[d.panel]}
        <small>{t ? (t.zone === "center" ? "add as tab" : `dock ${t.zone}`) : "float"}</small>
      </div>
    </>
  );
}

export function Dock() {
  const l = shown.value;
  const mx = maximized.value ? groups(l).find((x) => x.id === maximized.value) : undefined;
  return (
    <div class={"dock" + (drag.value ? " dragging" : "")}>
      {mx ? <GroupView gr={mx} /> : <NodeView n={l.root} />}
      {!mx && l.floats.map((f) => <FloatView f={f} key={f.group.id} />)}
      <DragOverlay />
    </div>
  );
}
