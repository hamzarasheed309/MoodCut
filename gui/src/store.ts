import { computed, effect, signal } from "@preact/signals";
import type { Catalog, Comp, Item, Job, LogEntry, Media, Project, Track } from "./types";

export const project = signal<Project | null>(null);
export const rev = signal(0);
export const activeId = signal("s1");
export const projectPath = signal<string | null>(null);
export const dirty = signal(false);
export const undoN = signal(0);
export const redoN = signal(0);
export const ffVersion = signal("");
export const appVersion = signal("");
export const selection = signal<string[]>([]);
export const playhead = signal(0);
export const playing = signal(false);
/** The playhead while paused, -1 while playing: for views that only update when the picture is still
 * (frame loads, the transform gizmo, scopes), so playback doesn't re-render them every frame. */
export const pausedPlayhead = computed(() => (playing.value ? -1 : playhead.value));
export const zoom = signal(80); // px per second
export const scrollX = signal(0);
/** Premiere / After Effects tools (see the Tools panel). */
export type Tool = "select" | "track" | "ripple" | "roll" | "rate" | "razor" | "slip" | "slide" | "hand" | "zoom" | "type" | "shape" | "mask";
export const tool = signal<Tool>("select");
export const shapeKind = signal<"rect" | "ellipse">("rect");
export const snapping = signal(true);
/** Item copied with Ctrl+C (Ctrl+V pastes a copy, Ctrl+Alt+V pastes its attributes). */
export const clipboard = signal<string | null>(null);
/** Source monitor: media shown, its time and in/out marks. */
export const sourceMedia = signal<string | null>(null);
export const sourceT = signal(0);
export const sourceIn = signal<number | null>(null);
export const sourceOut = signal<number | null>(null);
export const monitorTab = signal<"program" | "source">("program");
export const history = signal<{ undo: string[]; redo: string[] }>({ undo: [], redo: [] });
export const catalog = signal<Catalog | null>(null);
export const logs = signal<LogEntry[]>([]);
export const jobs = signal<Job[]>([]);
export const toasts = signal<{ id: number; msg: string; err: boolean }[]>([]);
export const aiPulse = signal(0);
export const connected = signal(false);
export const showRender = signal(false);
export const showCheck = signal(false);
/** HTML graphic editor: null = closed, {} = new graphic, {id} = edit that graphic, `item` = opened from that clip. */
export const graphicEditor = signal<null | { id?: string; item?: string }>(null);
/** Preview proxies (Premiere's Toggle Proxies): status per media and the on/off switch. */
export type ProxyState = { enabled: boolean; mode: string; summary: string; media: Record<string, string>; busy: boolean };
export const proxyState = signal<ProxyState | null>(null);
/** Bumped when previews of the same revision may look different (a proxy finished, proxies toggled). */
export const viewGen = signal(0);
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
export function toast(msg: string, err = false, ms?: number) {
  const id = ++toastId;
  toasts.value = [...toasts.value, { id, msg, err }];
  setTimeout(() => (toasts.value = toasts.value.filter((t) => t.id !== id)), ms ?? (err ? 7000 : 3500));
}

const OFFLINE = "Can't reach the MoodCut engine. Is MoodCut still running?";

async function j<T>(url: string, init?: RequestInit): Promise<T> {
  let r: Response;
  try {
    r = await fetch(url, init);
  } catch {
    throw new Error(OFFLINE);
  }
  if (!r.ok && r.status !== 200) {
    let msg = r.statusText;
    try {
      msg = (await r.json()).error || msg;
    } catch {}
    throw new Error(msg);
  }
  return r.json();
}

/** Runs `fn` once at a time: calls during a run share it and trigger exactly one more run afterwards,
 * so an edit's reply and its WebSocket notice cost one state fetch, and the result is never stale. */
function coalesce(fn: () => Promise<void>): () => Promise<void> {
  let running: Promise<void> | null = null;
  let again = false;
  return () => {
    if (running) {
      again = true;
      return running;
    }
    running = (async () => {
      do {
        again = false;
        await fn();
      } while (again);
    })().finally(() => (running = null));
    return running;
  };
}

/** Revision at which the shown project was opened (or created): a new one resets the view. */
let openedAt = -1;
let noticeShown = "";

export const refresh = coalesce(async () => {
  const s = await j<any>("/api/state");
  project.value = s.project;
  rev.value = s.rev;
  activeId.value = s.active;
  projectPath.value = s.path;
  dirty.value = s.dirty;
  undoN.value = s.undo;
  redoN.value = s.redo;
  ffVersion.value = s.ffmpeg;
  appVersion.value = s.version || "";
  if (s.opened !== openedAt) {
    // another project: its ids mean other things, and it opens where it was saved
    openedAt = s.opened;
    selection.value = [];
    sourceMedia.value = null;
    setPlayhead(typeof s.restore === "number" ? s.restore : 0);
  } else {
    // drop selections that no longer exist
    const sel = selection.value.filter((id) => findItem(id));
    if (sel.length !== selection.value.length) selection.value = sel;
  }
  // e.g. the project MoodCut was started with couldn't be opened
  if (s.notice && s.notice !== noticeShown) {
    noticeShown = s.notice;
    toast(s.notice, true, 20000);
  }
});

export const refreshLogs = coalesce(async () => {
  logs.value = await j<LogEntry[]>("/api/log");
});

export const refreshHistory = coalesce(async () => {
  history.value = await j<{ undo: string[]; redo: string[] }>("/api/history");
});

export async function refreshJobs() {
  jobs.value = await j<Job[]>("/api/jobs");
}

/** Runs MoodCut Script through the same path the AI uses. */
export async function run(script: string, quiet = false): Promise<{ ok: boolean; log?: string[]; error?: string }> {
  let r: any;
  try {
    r = await j<any>("/api/edit", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ script, source: "gui" }) });
  } catch (e) {
    const error = e instanceof Error ? e.message : String(e);
    toast(error, true);
    return { ok: false, error };
  }
  if (!r.ok) toast(r.error, true);
  else if (!quiet && r.log?.length) toast(r.log.slice(0, 3).join("  ·  "));
  await refresh();
  refreshLogs();
  refreshHistory();
  return r;
}

/** Ids created by a script run (the `+id` lines of its log). */
export function createdIds(log: string[] | undefined): string[] {
  return (log || []).map((l) => /^\+([a-z]\d+)\b/.exec(l)?.[1]).filter((x): x is string => !!x);
}

export async function project_(body: any): Promise<{ ok: boolean; text: string }> {
  let r: { ok: boolean; text: string };
  try {
    r = await j<any>("/api/project", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
  } catch (e) {
    const text = e instanceof Error ? e.message : String(e);
    toast(text, true);
    return { ok: false, text };
  }
  // opening can report repairs and missing media ("! ..." lines): give those time to be read
  const warn = !r.ok || r.text.includes("\n! ");
  toast(r.text, warn, warn ? 15000 : undefined);
  await refresh().catch(() => {});
  return r;
}

/** Save (Ctrl+S) to the project's file, asking where when it has none; Save As (Ctrl+Shift+S) always asks. */
export async function saveProject(as = false): Promise<boolean> {
  if (projectPath.value && !as) return (await project_({ action: "save" })).ok;
  const f = await pickFiles("save", "project");
  if (!f[0]) return false;
  return (await project_({ action: "save", path: f[0] })).ok;
}

/** Before New / Open / Recover replace a project with unsaved work. */
export function confirmLeave(): boolean {
  if (!dirty.value) return true;
  const name = project.value?.name || "Untitled";
  if (projectPath.value) return confirm(`Some changes to "${name}" aren't saved yet. Leave it anyway?`);
  return confirm(`"${name}" hasn't been saved to a file yet.\n\nContinue anyway? You can get it back later from Home > Unsaved projects.`);
}

export async function importPaths(paths: string[]) {
  if (!paths.length) return;
  let r: any;
  try {
    r = await j<any>("/api/import", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ paths }) });
  } catch (e) {
    return toast(e instanceof Error ? e.message : String(e), true);
  }
  if (!r.ok) toast(r.error, true);
  else toast(r.log.find((l: string) => l.startsWith("making preview proxies")) ? `Imported. ${r.log[r.log.length - 1]}` : `Imported ${r.log.length} item(s)`);
  await refresh();
  refreshProxies();
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

let proxyPoll: number | undefined;
/** Reads (or changes, with a body) the proxy state; polls while proxies are being made. */
export async function refreshProxies(body?: Record<string, unknown>) {
  try {
    const r = await j<any>("/api/proxy", body ? { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) } : undefined);
    if (!r.ok) return toast(r.error, true);
    const before = proxyState.value;
    proxyState.value = r;
    if (body || (before && (before.enabled !== r.enabled || Object.keys(before.media).length !== Object.keys(r.media).length))) viewGen.value++;
    clearTimeout(proxyPoll);
    if (r.busy) proxyPoll = window.setTimeout(() => refreshProxies(), 2000);
  } catch {}
}

export async function loadCatalog() {
  catalog.value = await j<Catalog>("/api/catalog");
}

let wsOpened = false;
let socket: WebSocket | null = null;
let uiTimer: number | undefined;

/** Tells the engine what is selected and where the playhead is, so AI scripts can say `sel` and `playhead`. */
function sendUi() {
  if (uiTimer) clearTimeout(uiTimer);
  uiTimer = window.setTimeout(() => {
    if (socket?.readyState !== WebSocket.OPEN) return;
    socket.send(JSON.stringify({ kind: "ui", ui: { selection: selection.value, playhead: +playhead.value.toFixed(4), comp: activeId.value } }));
  }, 150);
}

/** The window's title bar (the native editor window shows it): "Name * - MoodCut" while there are unsaved changes. */
effect(() => {
  const name = project.value?.name;
  document.title = name ? `${name}${dirty.value ? " *" : ""} - MoodCut` : "MoodCut";
});

effect(() => {
  void selection.value;
  void playhead.value;
  void activeId.value;
  sendUi();
});

export function connectWS() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/api/ws`);
  socket = ws;
  ws.onopen = () => {
    // after a reconnect (MoodCut restarted, laptop woke up) reload everything
    if (wsOpened) {
      refresh();
      refreshLogs();
      refreshJobs();
    }
    wsOpened = true;
    connected.value = true;
    sendUi();
  };
  ws.onmessage = (ev) => {
    try {
      const e = JSON.parse(ev.data);
      if (e.kind === "changed" && e.rev !== rev.value) {
        refresh();
        refreshLogs();
        refreshHistory();
        if (e.source === "mcp") aiPulse.value++;
        // imports (by the AI too) queue proxies for heavy footage
        if (e.source !== "gui" || /import/.test(e.text || "")) refreshProxies();
      } else if (e.kind === "setup" || e.kind === "saved") {
        refresh();
      } else if (e.kind === "autosave") {
        // the project file couldn't be written (disk full, read-only, ...), or it works again
        const failed = /failed/.test(e.text || "");
        toast(e.text, failed, failed ? 20000 : undefined);
        refresh();
      } else if (e.kind === "notice") {
        noticeShown = e.text;
        toast(e.text, true, 20000);
      } else if (e.kind === "proxy") {
        viewGen.value++;
        refreshProxies();
      } else if (e.kind === "job") {
        refreshJobs();
        const t: string = e.text || "";
        if (/ done| failed|retrying/.test(t)) toast(t, t.includes("failed") && !t.includes("retrying"), t.includes("failed") ? 12000 : undefined);
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
