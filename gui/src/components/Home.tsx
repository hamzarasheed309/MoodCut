// Home screen, like the one Premiere Pro and After Effects open with: start a project from a preset,
// reopen a recent one, and finish setting up (FFmpeg, AI apps).
import { effect, signal } from "@preact/signals";
import { useEffect, useState } from "preact/hooks";
import { LogoMark, TAGLINE, Wordmark } from "../brand";
import { appVersion, ffVersion, pickFiles, project, project_, projectPath, refresh, toast } from "../store";

const RECENT_KEY = "mc.recent";
const HOME_KEY = "mc.home";

function load<T>(k: string, d: T): T {
  try {
    const v = localStorage.getItem(k);
    return v === null ? d : (JSON.parse(v) as T);
  } catch {
    return d;
  }
}
function save(k: string, v: unknown) {
  try {
    localStorage.setItem(k, JSON.stringify(v));
  } catch {}
}

export const recent = signal<string[]>(load(RECENT_KEY, []));
export const showHome = signal(false);

// remember every project that is opened or saved
effect(() => {
  const p = projectPath.value;
  if (!p) return;
  const list = [p, ...recent.peek().filter((x) => x !== p)].slice(0, 8);
  if (list.join("\n") !== recent.peek().join("\n")) {
    recent.value = list;
    save(RECENT_KEY, list);
  }
});

/** Opens Home at launch when there's nothing to edit yet (unless turned off). */
export function maybeShowHome() {
  const p = project.value;
  const empty = !p || (p.media.length === 0 && p.comps.every((c) => c.tracks.every((t) => t.items.length === 0)));
  if (empty && load(HOME_KEY, true)) showHome.value = true;
}

const PRESETS = [
  { name: "YouTube", sub: "16:9 · 1920×1080 · 30 fps", size: "1920x1080", fps: 30, w: 16, h: 9 },
  { name: "Reel / TikTok / Short", sub: "9:16 · 1080×1920 · 30 fps", size: "1080x1920", fps: 30, w: 9, h: 16 },
  { name: "Square post", sub: "1:1 · 1080×1080 · 30 fps", size: "1080x1080", fps: 30, w: 1, h: 1 },
  { name: "4K UHD", sub: "16:9 · 3840×2160 · 24 fps", size: "3840x2160", fps: 24, w: 16, h: 9 },
];

const baseName = (p: string) => p.split(/[\\/]/).pop() || p;

function Setup({ onConnect }: { onConnect: () => void }) {
  const [ai, setAi] = useState<{ found: number; on: number } | null>(null);
  const [ff, setFf] = useState<{ running: boolean; status: string; error: string | null } | null>(null);
  const missing = ffVersion.value === "not installed";
  useEffect(() => {
    fetch("/api/ai/clients")
      .then((r) => r.json())
      .then((r) => r.ok && setAi({ found: r.clients.filter((c: any) => c.installed).length, on: r.clients.filter((c: any) => c.registered && !c.stale).length }))
      .catch(() => {});
  }, []);
  useEffect(() => {
    if (!ff?.running) return;
    const h = setInterval(async () => {
      const s = await fetch("/api/setup/ffmpeg", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ action: "status" }) }).then((r) => r.json());
      setFf(s);
      if (s.installed) {
        toast(s.status || "FFmpeg ready");
        refresh();
      }
    }, 700);
    return () => clearInterval(h);
  }, [ff?.running]);
  const installFf = async () => setFf(await fetch("/api/setup/ffmpeg", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ action: "start" }) }).then((r) => r.json()));
  return (
    <div class="home-setup">
      <div class={"home-check" + (missing ? "" : " done")}>
        <span class="tick">{missing ? "1" : "✓"}</span>
        <div>
          <b>FFmpeg</b>
          <small>{missing ? (ff?.running ? `${ff.status || "starting"}…` : ff?.error || "Needed to import, play and export video (about 100 MB, no admin rights).") : `Version ${ffVersion.value} is ready.`}</small>
        </div>
        {missing && (
          <button class="btn sm primary" disabled={!!ff?.running} onClick={installFf}>
            {ff?.running ? "Installing…" : "Install"}
          </button>
        )}
      </div>
      <div class={"home-check" + (ai && ai.on > 0 ? " done" : "")}>
        <span class="tick">{ai && ai.on > 0 ? "✓" : "2"}</span>
        <div>
          <b>AI apps</b>
          <small>{!ai ? "Checking…" : ai.on > 0 ? `${ai.on} connected. Ask your assistant to "use MoodCut".` : ai.found > 0 ? `${ai.found} found on this computer: connect them in one click.` : "Connect Claude, ChatGPT, Cursor, VS Code…"}</small>
        </div>
        <button class={"btn sm" + (ai && ai.on > 0 ? "" : " primary")} onClick={onConnect}>
          {ai && ai.on > 0 ? "Manage" : "Connect AI"}
        </button>
      </div>
    </div>
  );
}

export function Home({ onConnect }: { onConnect: () => void }) {
  const [name, setName] = useState("Untitled");
  const [startup, setStartup] = useState(load(HOME_KEY, true));
  if (!showHome.value) return null;
  const close = () => (showHome.value = false);
  const create = async (size: string, fps: number) => {
    close();
    await project_({ action: "new", name: name.trim() || "Untitled", size, fps });
  };
  const open = async (path?: string) => {
    const p = path || (await pickFiles("open", "project"))[0];
    if (!p) return;
    close();
    await project_({ action: "open", path: p });
  };
  return (
    <div class="modal-bg home-bg" onClick={close}>
      <div class="home" onClick={(e) => e.stopPropagation()} role="dialog" aria-label="MoodCut Home">
        <div class="home-brand">
          <LogoMark size={96} />
          <Wordmark class="home-word" />
          <div class="home-tag">{TAGLINE}</div>
          <div class="home-ver">Version {appVersion.value}</div>
          <div class="home-tips">
            <b>Try asking your AI</b>
            <span>"Cut D:\Footage\Trip into a 30 s Reel with captions and music"</span>
            <span>"Grade every shot warm and cinematic, then check it"</span>
          </div>
        </div>
        <div class="home-main">
          <div class="home-h">
            New project
            <input class="home-name" value={name} onInput={(e) => setName((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} aria-label="Project name" />
          </div>
          <div class="home-presets">
            {PRESETS.map((p) => (
              <button key={p.name} class="home-preset" onClick={() => create(p.size, p.fps)}>
                <span class="frame" style={{ aspectRatio: `${p.w} / ${p.h}` }} />
                <b>{p.name}</b>
                <small>{p.sub}</small>
              </button>
            ))}
          </div>
          <div class="home-h">
            Recent
            <button class="btn sm" onClick={() => open()}>
              Open project…
            </button>
          </div>
          <div class="home-recent">
            {recent.value.length === 0 && <div class="ai-dim">Projects you open or save show up here.</div>}
            {recent.value.slice(0, 5).map((p) => (
              <button key={p} class="home-file" title={p} onClick={() => open(p)}>
                <b>{baseName(p).replace(/\.mcut$/i, "")}</b>
                <small>{p}</small>
              </button>
            ))}
          </div>
          <div class="home-h">Get set up</div>
          <Setup onConnect={() => (close(), onConnect())} />
          <div class="home-foot">
            <label class="ai-dim">
              <input
                type="checkbox"
                checked={startup}
                onChange={(e) => {
                  const v = (e.target as HTMLInputElement).checked;
                  setStartup(v);
                  save(HOME_KEY, v);
                }}
              />{" "}
              Show Home when MoodCut starts
            </label>
            <button class="btn" onClick={close}>
              Continue to editor
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
