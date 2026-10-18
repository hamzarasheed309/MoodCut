import { computed, signal } from "@preact/signals";
import type { Catalog, Comp, Item, Job, LogEntry, Media, Project, Track } from "./types";

export const project = signal<Project | null>(null);
export const rev = signal(0);
export const activeId = signal("s1");
export const projectPath = signal<string | null>(null);
export const dirty = signal(false);
export const undoN = signal(0);
export const redoN = signal(0);
export const ffVersion = signal("");
export const selection = signal<string[]>([]);
export const playhead = signal(0);
export const playing = signal(false);
export const zoom = signal(80); // px per second
export const scrollX = signal(0);
export const tool = signal<"select" | "razor">("select");
export const catalog = signal<Catalog | null>(null);
export const logs = signal<LogEntry[]>([]);
export const jobs = signal<Job[]>([]);
export const toasts = signal<{ id: number; msg: string; err: boolean }[]>([]);
export const aiPulse = signal(0);
export const connected = signal(false);
export const showRender = signal(false);
export const showCheck = signal(false);
/** HTML graphic editor: null = closed, {} = new graphic, {id} = edit that graphic. */
export const graphicEditor = signal<null | { id?: string }>(null);
export const fileBrowser = signal<null | { kind: "open" | "save" | "import" | "folder"; filter: string; resolve: (p: string[]) => void }>(null);

export const comp = computed<Comp | null>(() => {
  const p = project.value;
  if (!p) return null;
  return p.comps.find((c) => c.id === activeId.value) || p.comps.find((c) => c.id === p.main) || null;
});

export const mediaById = computed(() => {
  const m = new Map<string, Media>();
  for (const x of project.value?.media || []) m.set(x.id, x);
  return m;
});

export function findItem(id: string): { item: Item; track: Track; comp: Comp } | null {
  const p = project.value;
  if (!p) return null;
  for (const c of p.comps)
    for (const t of c.tracks)
      for (const i of t.items) if (i.id === id) return { item: i, track: t, comp: c };
  return null;
}

export const selectedItem = computed(() => {
  const id = selection.value[selection.value.length - 1];
  return id ? findItem(id) : null;
});

let toastId = 0;
export function toast(msg: string, err = false) {
  const id = ++toastId;
  toasts.value = [...toasts.value, { id, msg, err }];
  setTimeout(() => (toasts.value = toasts.value.filter((t) => t.id !== id)), err ? 7000 : 3500);
}

async function j<T>(url: string, init?: RequestInit): Promise<T> {
  const r = await fetch(url, init);
  if (!r.ok && r.status !== 200) {
    let msg = r.statusText;
    try {
      msg = (await r.json()).error || msg;
    } catch {}
    throw new Error(msg);
  }
  return r.json();
}

export async function refresh() {
  const s = await j<any>("/api/state");
  project.value = s.project;
  rev.value = s.rev;
  activeId.value = s.active;
  projectPath.value = s.path;
  dirty.value = s.dirty;
  undoN.value = s.undo;
  redoN.value = s.redo;
  ffVersion.value = s.ffmpeg;
  // drop selections that no longer exist
  const sel = selection.value.filter((id) => findItem(id));
  if (sel.length !== selection.value.length) selection.value = sel;
}

export async function refreshLogs() {
  logs.value = await j<LogEntry[]>("/api/log");
}

export async function refreshJobs() {
  jobs.value = await j<Job[]>("/api/jobs");
}

/** Runs MoodCut Script through the same path the AI uses. */
export async function run(script: string, quiet = false): Promise<{ ok: boolean; log?: string[]; error?: string }> {
  const r = await j<any>("/api/edit", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ script, source: "gui" }) });
  if (!r.ok) toast(r.error, true);
  else if (!quiet && r.log?.length) toast(r.log.slice(0, 3).join("  ·  "));
  await refresh();
  refreshLogs();
  return r;
}

export async function project_(body: any) {
  const r = await j<any>("/api/project", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  toast(r.text, !r.ok);
  await refresh();
  return r;
}

export async function importPaths(paths: string[]) {
  if (!paths.length) return;
  const r = await j<any>("/api/import", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ paths }) });
  if (!r.ok) toast(r.error, true);
  else toast(`Imported ${r.log.length} item(s)`);
  await refresh();
}

/** Native OS dialog, falling back to the in-app browser. */
export async function pickFiles(kind: "open" | "save" | "import" | "folder", filter = "media"): Promise<string[]> {
  try {
    const r = await j<any>("/api/dialog", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ kind: kind === "import" ? "open" : kind, filter }) });
    if (r.ok) return r.paths || []; // empty = cancelled
  } catch {}
  // no native dialog available (e.g. Linux without zenity): use the in-app browser
  return new Promise((resolve) => (fileBrowser.value = { kind, filter, resolve }));
}

export async function loadCatalog() {
  catalog.value = await j<Catalog>("/api/catalog");
}

let wsOpened = false;

export function connectWS() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/api/ws`);
  ws.onopen = () => {
    // after a reconnect (MoodCut restarted, laptop woke up) reload everything
    if (wsOpened) {
      refresh();
      refreshLogs();
      refreshJobs();
    }
    wsOpened = true;
    connected.value = true;
  };
  ws.onmessage = (ev) => {
    try {
      const e = JSON.parse(ev.data);
      if (e.kind === "changed" && e.rev !== rev.value) {
        refresh();
        refreshLogs();
        if (e.source === "mcp") aiPulse.value++;
      } else if (e.kind === "setup") {
        refresh();
      } else if (e.kind === "job") {
        refreshJobs();
        if (e.text?.includes("done") || e.text?.includes("failed")) toast(e.text, e.text.includes("failed"));
      }
    } catch {}
  };
  ws.onclose = () => {
    connected.value = false;
    setTimeout(connectWS, 1500);
  };
}

export const fps = computed(() => comp.value?.fps || 30);

export function setPlayhead(t: number) {
  const c = comp.value;
  const f = c ? 1 / c.fps : 1 / 30;
  playhead.value = Math.max(0, Math.round(t / f) * f);
}
