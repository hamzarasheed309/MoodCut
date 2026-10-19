// Dockable workspace layout (Premiere Pro / After Effects style): a tree of splits whose
// leaves are tab groups of panels, plus floating windows. Workspaces are presets the
// person can rearrange; each workspace remembers its arrangement in localStorage.
import { batch, computed, effect, signal } from "@preact/signals";

export type PanelId =
  | "project"
  | "effects"
  | "source"
  | "program"
  | "scopes"
  | "controls"
  | "lumetri"
  | "essential"
  | "mixer"
  | "markers"
  | "history"
  | "ai"
  | "timeline"
  | "tools"
  | "align"
  | "character"
  | "brand";

export const PANEL_TITLES: Record<PanelId, string> = {
  project: "Project",
  effects: "Effects & Presets",
  source: "Source",
  program: "Program",
  scopes: "Lumetri Scopes",
  controls: "Effect Controls",
  lumetri: "Lumetri Color",
  essential: "Essential Sound",
  mixer: "Audio Track Mixer",
  markers: "Markers",
  history: "History",
  ai: "AI & Script",
  timeline: "Timeline",
  tools: "Tools",
  align: "Align",
  character: "Character",
  brand: "Brand Kit",
};

export const ALL_PANELS = Object.keys(PANEL_TITLES) as PanelId[];

export interface Group {
  t: "group";
  id: string;
  panels: PanelId[];
  active: PanelId | null;
}

export interface Split {
  t: "split";
  id: string;
  dir: "row" | "col";
  kids: Node[];
  /** Per child: a fraction of the free space (0 < x <= 1), fixed pixels (> 1), or 0 to fit its content. */
  sizes: number[];
}

export type Node = Group | Split;

export interface Floating {
  group: Group;
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Layout {
  root: Node;
  floats: Floating[];
}

let uid = 0;
const nid = (p: string) => `${p}${Date.now().toString(36)}${(uid++).toString(36)}`;

export const g = (panels: PanelId[], active?: PanelId): Group => ({ t: "group", id: nid("g"), panels, active: active ?? panels[0] ?? null });
const row = (kids: Node[], sizes: number[]): Split => ({ t: "split", id: nid("s"), dir: "row", kids, sizes });
const col = (kids: Node[], sizes: number[]): Split => ({ t: "split", id: nid("s"), dir: "col", kids, sizes });

/** Bottom of most workspaces: Tools | Timeline. */
const editBottom = (left: PanelId[] | null) => row(left ? [g(left), g(["tools"]), g(["timeline"])] : [g(["tools"]), g(["timeline"])], left ? [0.26, 0, 0.74] : [0, 1]);

export const WORKSPACES: { id: string; name: string; desc: string; build: () => Layout }[] = [
  {
    id: "assembly",
    name: "Assembly",
    desc: "Big Project panel for browsing and logging footage",
    build: () => ({ root: col([row([g(["project", "effects"]), g(["source", "controls"]), g(["program"])], [0.4, 0.3, 0.3]), editBottom(null)], [0.6, 0.4]), floats: [] }),
  },
  {
    id: "editing",
    name: "Editing",
    desc: "Premiere Pro's default: Source and Program monitors over the timeline",
    build: () => ({
      root: col([row([g(["source", "controls", "lumetri", "essential", "mixer"], "controls"), g(["program"])], [0.5, 0.5]), editBottom(["project", "effects", "markers", "history", "ai"])], [0.56, 0.44]),
      floats: [],
    }),
  },
  {
    id: "color",
    name: "Color",
    desc: "Lumetri Color and Scopes around a large Program monitor",
    build: () => ({
      root: col([row([g(["scopes", "source"]), g(["program"]), g(["lumetri", "controls"])], [0.28, 0.44, 0.28]), editBottom(["project", "effects", "history"])], [0.6, 0.4]),
      floats: [],
    }),
  },
  {
    id: "effects",
    name: "Effects",
    desc: "Effect Controls and Effects & Presets beside the Program monitor",
    build: () => ({
      root: col([row([g(["controls", "source", "lumetri"]), g(["program"]), g(["effects", "markers"])], [0.3, 0.45, 0.25]), editBottom(["project", "history", "ai"])], [0.58, 0.42]),
      floats: [],
    }),
  },
  {
    id: "audio",
    name: "Audio",
    desc: "Audio Track Mixer and Essential Sound",
    build: () => ({
      root: col([row([g(["mixer", "source"]), g(["program"]), g(["essential", "controls"])], [0.38, 0.37, 0.25]), editBottom(["project", "effects", "markers"])], [0.56, 0.44]),
      floats: [],
    }),
  },
  {
    id: "motion",
    name: "Motion",
    desc: "After Effects Standard: Project, Composition, Effect Controls, Align, Character",
    build: () => ({
      root: col(
        [
          row([g(["project", "effects"]), g(["program"]), col([g(["controls", "character"]), g(["align", "brand", "history"])], [0.6, 0.4])], [0.22, 0.53, 0.25]),
          row([g(["tools"]), g(["timeline"])], [0, 1]),
        ],
        [0.6, 0.4],
      ),
      floats: [],
    }),
  },
  {
    id: "graphics",
    name: "Graphics",
    desc: "Titles and captions: Character, Brand Kit and Align with the Program monitor",
    build: () => ({
      root: col([row([g(["character", "brand", "align"]), g(["program"]), g(["controls", "effects"])], [0.25, 0.5, 0.25]), editBottom(["project", "markers"])], [0.6, 0.4]),
      floats: [],
    }),
  },
  {
    id: "ai",
    name: "AI",
    desc: "The AI console beside the Program monitor, with History to review its edits",
    build: () => ({
      root: col([row([g(["ai", "project"]), g(["program"]), g(["controls", "history", "markers"])], [0.3, 0.45, 0.25]), editBottom(null)], [0.62, 0.38]),
      floats: [],
    }),
  },
];

/** Narrow windows (phones, split screens): one column of tabs. */
function compactLayout(): Layout {
  return {
    root: col([g(["program", "source", "scopes"]), g(["timeline"]), g(["controls", "project", "effects", "lumetri", "essential", "mixer", "ai", "markers", "history", "character", "brand", "align", "tools"])], [0.42, 0.33, 0.25]),
    floats: [],
  };
}

// ---------------------------------------------------------------- tree operations

function walk(n: Node, f: (n: Node, parent: Split | null, i: number) => void, parent: Split | null = null, i = 0) {
  f(n, parent, i);
  if (n.t === "split") n.kids.forEach((k, j) => walk(k, f, n, j));
}

export function groups(l: Layout): Group[] {
  const out: Group[] = [];
  walk(l.root, (n) => n.t === "group" && out.push(n));
  l.floats.forEach((f) => out.push(f.group));
  return out;
}

export function groupOf(l: Layout, p: PanelId): Group | undefined {
  return groups(l).find((gr) => gr.panels.includes(p));
}

function clone(l: Layout): Layout {
  return JSON.parse(JSON.stringify(l));
}

/** Drops empty groups and single-child splits, merges nested splits of the same direction. */
function normalize(l: Layout): Layout {
  const fix = (n: Node): Node | null => {
    if (n.t === "group") return n.panels.length ? n : null;
    const kids: Node[] = [];
    const sizes: number[] = [];
    n.kids.forEach((k, i) => {
      const f = fix(k);
      if (!f) return;
      if (f.t === "split" && f.dir === n.dir) {
        // flatten: share this child's size among its children
        const frac = n.sizes[i] > 0 && n.sizes[i] <= 1 ? n.sizes[i] : 0;
        const tot = f.sizes.filter((x) => x > 0 && x <= 1).reduce((a, b) => a + b, 0) || 1;
        f.kids.forEach((kk, j) => {
          kids.push(kk);
          sizes.push(f.sizes[j] > 1 || f.sizes[j] === 0 ? f.sizes[j] : (f.sizes[j] / tot) * (frac || 0.5));
        });
      } else {
        kids.push(f);
        sizes.push(n.sizes[i]);
      }
    });
    if (!kids.length) return null;
    if (kids.length === 1) return kids[0];
    // fixed-size children only: let the last one fill
    if (sizes.every((s) => s > 1 || s === 0)) sizes[sizes.length - 1] = 1;
    return { ...n, kids, sizes };
  };
  const root = fix(l.root) || g([]);
  const floats = l.floats.filter((f) => f.group.panels.length);
  return { root, floats };
}

function detach(l: Layout, p: PanelId): Layout {
  const n = clone(l);
  for (const gr of groups(n)) {
    const i = gr.panels.indexOf(p);
    if (i < 0) continue;
    gr.panels.splice(i, 1);
    if (gr.active === p) gr.active = gr.panels[Math.min(i, gr.panels.length - 1)] ?? null;
  }
  return n;
}

export type Zone = "center" | "left" | "right" | "top" | "bottom";

/** Moves panel `p` into group `target`: as a tab (center, at `index`) or into a new split beside it. */
export function dock(l: Layout, p: PanelId, target: string, zone: Zone, index?: number): Layout {
  if (zone === "center") {
    const src = groupOf(l, p);
    if (src?.id === target && src.panels.length === 1) return l;
  }
  const n = detach(l, p);
  const tgt = groups(n).find((x) => x.id === target);
  if (!tgt) return l;
  if (zone === "center") {
    const at = index === undefined ? tgt.panels.length : Math.max(0, Math.min(index, tgt.panels.length));
    tgt.panels.splice(at, 0, p);
    tgt.active = p;
    return normalize(n);
  }
  const fresh = g([p]);
  const dir = zone === "left" || zone === "right" ? "row" : "col";
  const first = zone === "left" || zone === "top";
  // a docked target splits in place; a floating one docks the panel into a new split inside the window
  const replace = (node: Node): Node => {
    if (node.t === "group" && node.id === target) return { t: "split", id: nid("s"), dir, kids: first ? [fresh, node] : [node, fresh], sizes: first ? [0.35, 0.65] : [0.65, 0.35] };
    if (node.t === "split") return { ...node, kids: node.kids.map(replace) };
    return node;
  };
  const fl = n.floats.find((f) => f.group.id === target);
  if (fl) {
    // floating windows hold one group: add as a tab instead
    fl.group.panels.push(p);
    fl.group.active = p;
    return normalize(n);
  }
  n.root = replace(n.root);
  return normalize(n);
}

export function float(l: Layout, p: PanelId, x: number, y: number, w = 420, h = 360): Layout {
  const n = detach(l, p);
  n.floats.push({ group: g([p]), x: Math.max(0, x), y: Math.max(0, y), w, h });
  return normalize(n);
}

export function closePanel(l: Layout, p: PanelId): Layout {
  return normalize(detach(l, p));
}

export function closeGroup(l: Layout, id: string): Layout {
  const n = clone(l);
  for (const gr of groups(n)) if (gr.id === id) gr.panels = [];
  return normalize(n);
}

export function activate(l: Layout, p: PanelId): Layout {
  const n = clone(l);
  const gr = groupOf(n, p);
  if (gr) gr.active = p;
  // bring a floating window to the front
  const fi = n.floats.findIndex((f) => f.group.panels.includes(p));
  if (fi >= 0 && fi < n.floats.length - 1) n.floats.push(n.floats.splice(fi, 1)[0]);
  return n;
}

/** Where a closed panel comes back: next to a related panel, else floating. */
const HOMES: Partial<Record<PanelId, PanelId[]>> = {
  scopes: ["lumetri", "source", "controls"],
  lumetri: ["controls", "source"],
  essential: ["mixer", "controls", "source"],
  mixer: ["essential", "source", "controls"],
  controls: ["source", "lumetri"],
  source: ["controls", "program"],
  program: ["source"],
  project: ["effects", "markers"],
  effects: ["project", "controls"],
  markers: ["project", "history"],
  history: ["project", "markers"],
  ai: ["project", "history"],
  align: ["character", "controls"],
  character: ["controls", "align"],
  brand: ["character", "align", "controls"],
  timeline: [],
  tools: [],
};

export function show(l: Layout, p: PanelId): Layout {
  if (groupOf(l, p)) return activate(l, p);
  for (const near of HOMES[p] || []) {
    const gr = groupOf(l, near);
    if (gr) return dock(l, p, gr.id, "center");
  }
  return float(l, p, 120 + l.floats.length * 24, 80 + l.floats.length * 24);
}

export function moveFloat(l: Layout, id: string, rect: Partial<Floating>): Layout {
  return { ...l, floats: l.floats.map((f) => (f.group.id === id ? { ...f, ...rect } : f)) };
}

export function resize(l: Layout, splitId: string, sizes: number[]): Layout {
  const n = clone(l);
  walk(n.root, (x) => {
    if (x.t === "split" && x.id === splitId) x.sizes = sizes;
  });
  return n;
}

// ---------------------------------------------------------------- state + persistence

const KEY = "moodcut.dock.v1";

function load(): { ws: string; saved: Record<string, Layout> } {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) || "null");
    if (raw && typeof raw.ws === "string" && raw.saved) return raw;
  } catch {}
  return { ws: "editing", saved: {} };
}

/** Drops unknown or duplicated panels from a stored layout. */
function sanitize(l: Layout): Layout | null {
  try {
    const seen = new Set<PanelId>();
    for (const gr of groups(l)) {
      gr.panels = gr.panels.filter((p) => ALL_PANELS.includes(p) && !seen.has(p) && seen.add(p));
      if (!gr.active || !gr.panels.includes(gr.active)) gr.active = gr.panels[0] ?? null;
    }
    return normalize(l);
  } catch {
    return null;
  }
}

const initial = load();
export const workspace = signal(WORKSPACES.some((w) => w.id === initial.ws) ? initial.ws : "editing");
const saved: Record<string, Layout> = initial.saved;
const preset = (id: string) => (WORKSPACES.find((w) => w.id === id) || WORKSPACES[1]).build();

export const layout = signal<Layout>((saved[workspace.value] && sanitize(saved[workspace.value])) || preset(workspace.value));
/** Group shown alone (the ` key, like Premiere's maximize panel). */
export const maximized = signal<string | null>(null);
/** Panel last clicked: keyboard shortcuts act on it (I/O mark the Source monitor when it has focus). */
export const focusedPanel = signal<PanelId | null>(null);
export const narrow = signal(typeof window !== "undefined" && window.innerWidth < 900);
if (typeof window !== "undefined") window.addEventListener("resize", () => (narrow.value = window.innerWidth < 900));

/** The one-column layout used on narrow windows (tabs switch, nothing is saved). */
const compact = signal<Layout>(compactLayout());
/** What is on screen: the workspace, or the one-column layout on narrow windows. */
export const shown = computed<Layout>(() => (narrow.value ? compact.value : layout.value));

let saveTimer: number | undefined;
effect(() => {
  const l = layout.value;
  const ws = workspace.value;
  saved[ws] = l;
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    try {
      localStorage.setItem(KEY, JSON.stringify({ ws, saved }));
    } catch {}
  }, 250);
});

export function setLayout(f: (l: Layout) => Layout) {
  if (narrow.value) compact.value = f(compact.value);
  else layout.value = f(layout.value);
}

export function switchWorkspace(id: string) {
  if (id === workspace.value) return;
  saved[workspace.value] = layout.value;
  const next = (saved[id] && sanitize(saved[id])) || preset(id);
  // one update, so the auto-save never files the old layout under the new name
  batch(() => {
    workspace.value = id;
    maximized.value = null;
    layout.value = next;
  });
}

export function resetWorkspace() {
  delete saved[workspace.value];
  maximized.value = null;
  layout.value = preset(workspace.value);
}

/** Opens a panel (or brings it to the front) in the current workspace. */
export function showPanel(p: PanelId) {
  setLayout((l) => show(l, p));
}

export function isOpen(p: PanelId): boolean {
  return !!groupOf(shown.value, p);
}
