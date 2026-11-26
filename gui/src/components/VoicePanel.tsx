// AI Voice: clone a voice from a few seconds of audio and speak scripts with it, using
// OmniVoice (k2-fsa/OmniVoice) on this computer. The engine finds the model by itself; when it
// can't, this panel offers to locate it or to download it to a drive the person picks. Speech
// is imported into the Project panel (tag "voice") and can be placed at the playhead. AI apps
// drive the same setup, voices and tasks through the `voice` MCP tool.
import { useEffect, useMemo, useState } from "preact/hooks";
import { showPanel } from "../dock";
import { api } from "../gfx";
import { mediaById, monitorTab, pickFiles, playhead, project, sourceIn, sourceMedia, sourceOut, sourceT, toast, voiceTick } from "../store";
import { fmtNum } from "../util";

type Install = {
  running: boolean;
  root: string;
  step: string;
  status: string;
  pct: number | null;
  log: string[];
  error: string | null;
  hint: string | null;
  detail: string | null;
  done: boolean;
};
type Task = {
  id: number;
  kind: "clone" | "generate" | "retranscribe";
  label: string;
  state: "queued" | "running" | "done" | "error";
  status: string;
  pct: number | null;
  error: string | null;
  hint: string | null;
  fix: "retry" | "repair" | null;
  detail: string | null;
  result: {
    media?: string[];
    voice?: string;
    text?: string;
    ref_text?: string;
  } | null;
  secs: number;
};
type Voice = {
  id: string;
  name: string;
  ref_text: string;
  source: string;
  dur: number;
  start?: number;
  auto?: boolean;
  typed?: boolean;
  denoise?: boolean;
  created: number;
};
type Status = {
  state: "missing" | "runtime" | "model" | "ready";
  root: string | null;
  model: string | null;
  python: string | null;
  runtime: {
    torch?: string;
    device?: string;
    gpu?: string | null;
    omnivoice?: string;
    vram_gb?: number;
  } | null;
  gpu: string | null;
  worker: {
    loaded: boolean;
    device?: string;
    busy?: boolean;
    vram?: { used_gb?: number; total_gb?: number; peak_gb?: number };
  };
  install: Install;
  tasks: Task[];
  voices: Voice[];
  repo: string;
  subdir: string;
  sizes: { model: number; runtime: number; whisper: number };
};
type Drive = {
  root: string;
  label: string;
  free: number | null;
  total: number | null;
  removable: boolean;
};
type Reload = (b?: Record<string, unknown>) => Promise<boolean>;

const GB = 1e9;
const gb = (b: number) => `${(b / GB).toFixed(b >= 10 * GB ? 0 : 1)} GB`;
const clock = (s: number) => (s >= 60 ? `${Math.floor(s / 60)}:${String(Math.round(s % 60)).padStart(2, "0")}` : `${Math.round(s)} s`);

const LANGUAGES = [
  "English",
  "Spanish",
  "French",
  "German",
  "Italian",
  "Portuguese",
  "Dutch",
  "Polish",
  "Russian",
  "Turkish",
  "Arabic",
  "Hindi",
  "Urdu",
  "Bengali",
  "Chinese",
  "Japanese",
  "Korean",
  "Vietnamese",
  "Indonesian",
  "Thai",
];
const QUALITY: [string, string][] = [
  ["fast", "Fast (16 steps)"],
  ["standard", "Standard (32 steps)"],
  ["best", "Best (48 steps, slower)"],
];

function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {}
}
function recall(key: string, fallback: string): string {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}

async function voiceApi(body?: Record<string, unknown>) {
  return api<Status>("/api/voice", body ?? { action: "status" });
}

function openInSource(id: string) {
  sourceMedia.value = id;
  sourceT.value = 0;
  sourceIn.value = null;
  sourceOut.value = null;
  monitorTab.value = "source";
  showPanel("source");
}

export function VoicePanel() {
  const [st, setSt] = useState<Status | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [setup, setSetup] = useState(false);
  const [dismissed, setDismissed] = useState<number | null>(null);

  const load: Reload = async (body) => {
    const r = await voiceApi(body);
    if (!r.ok) {
      if (body) toast(r.error, true);
      else setErr(r.error);
      return false;
    }
    setErr(null);
    setSt(r);
    return true;
  };

  useEffect(() => {
    load();
  }, [voiceTick.value]);

  // follow progress while something runs (events also arrive; this keeps the bars moving)
  const busy = !!st && (st.install.running || st.tasks.some((t) => t.state === "queued" || t.state === "running"));
  useEffect(() => {
    if (!busy) return;
    const h = setInterval(() => load(), 700);
    return () => clearInterval(h);
  }, [busy]);

  if (!st)
    return (
      <div class="scroll voice-panel">
        <div class="hint" style={{ padding: "10px" }}>
          {err || "Looking for OmniVoice…"}
        </div>
      </div>
    );

  const showSetup = st.install.running || setup || (st.install.error && st.state !== "ready");
  // the task to show at the top: the one running, else the latest result until it is dismissed
  const running = st.tasks.find((t) => t.state === "running");
  const last = st.tasks[st.tasks.length - 1];
  const current = running || (last && last.state !== "queued" && last.id !== dismissed ? last : undefined);
  return (
    <div class="scroll voice-panel">
      <Header st={st} onUnload={() => load({ action: "unload" })} />
      {st.state === "ready" && !st.install.running ? (
        <>
          <SetupNote st={st} reload={load} />
          {current && (
            <Current t={current} queued={st.tasks.filter((t) => t.state === "queued").length} reload={load} onDismiss={current.state === "running" ? undefined : () => setDismissed(current.id)} />
          )}
          <CloneForm st={st} onSent={() => load()} />
          <Voices st={st} reload={load} />
          <GenerateForm st={st} onSent={() => load()} />
          <Tasks st={st} reload={load} skip={current?.id} />
        </>
      ) : showSetup ? (
        <SetupForm st={st} reload={load} onClose={() => setSetup(false)} />
      ) : (
        <Missing st={st} reload={load} onSetup={() => setSetup(true)} />
      )}
    </div>
  );
}

function Header({ st, onUnload }: { st: Status; onUnload: () => void }) {
  const dev = st.worker.loaded ? st.worker.device || st.runtime?.device : st.runtime?.device;
  const onGpu = !!dev && dev !== "cpu";
  const vram = st.worker.vram;
  const chip =
    st.state === "ready" ? (
      <span
        class={"vc-chip " + (onGpu ? "ok" : "warn")}
        title={[st.runtime?.gpu || dev || "", vram?.used_gb != null ? `GPU memory in use: ${vram.used_gb} of ${vram.total_gb} GB` : ""].filter(Boolean).join("\n")}
      >
        {onGpu ? `GPU${st.runtime?.gpu ? ` · ${st.runtime.gpu.replace(/^NVIDIA (GeForce )?/, "")}` : ""}` : "CPU (slower)"}
        {st.worker.loaded && !st.worker.busy ? " · loaded" : ""}
      </span>
    ) : (
      <span class="vc-chip">not set up</span>
    );
  return (
    <div class="vc-head">
      <div>
        <b>AI Voice</b> <span class="vc-faint">OmniVoice · voice cloning</span>
      </div>
      <div class="vc-head-r">
        {chip}
        {st.worker.loaded && !st.worker.busy && (
          <button class="btn sm ghost" title="Unload the model and free its GPU memory (it loads again on the next request)" onClick={onUnload}>
            Unload
          </button>
        )}
        {st.root && (
          <button class="btn sm ghost" title={`Open ${st.root}`} onClick={() => api("/api/reveal", { path: st.root })}>
            Folder
          </button>
        )}
      </div>
    </div>
  );
}

/** A problem with the setup while voices are usable (a repair that failed). */
function SetupNote({ st, reload }: { st: Status; reload: Reload }) {
  const inst = st.install;
  if (!inst.error || inst.running) return null;
  return (
    <div class="group-b">
      <Problem message={inst.error} hint={inst.hint} detail={inst.detail}>
        <button class="btn sm" onClick={() => reload({ action: "repair" })}>
          Repair again
        </button>
      </Problem>
    </div>
  );
}

/** An error people can act on: what happened, what to do, buttons, and the technical details. */
function Problem({ message, hint, detail, children }: { message: string; hint?: string | null; detail?: string | null; children?: preact.ComponentChildren }) {
  return (
    <div class="vc-problem">
      <div class="vc-err-text">{message}</div>
      {hint && <div class="vc-hint">{hint}</div>}
      {children && <div class="btn-row">{children}</div>}
      {detail && detail !== message && (
        <details class="vc-log">
          <summary>Details</summary>
          <pre>{detail}</pre>
        </details>
      )}
    </div>
  );
}

/** Not found: say where it looked, offer Locate / Download. */
function Missing({ st, reload, onSetup }: { st: Status; reload: Reload; onSetup: () => void }) {
  const locate = async () => {
    const [dir] = await pickFiles("folder");
    if (!dir) return;
    if (await reload({ action: "locate", path: dir })) toast("OmniVoice found");
  };
  return (
    <div class="group-b">
      {st.state === "runtime" ? (
        <>
          <div class="vc-msg">
            <b>Model found</b> at <code>{st.model}</code>.
            <br />
            Its Python runtime (PyTorch and omnivoice, about {gb(st.sizes.runtime)}) isn't set up yet.
          </div>
          <div class="btn-row">
            <button class="btn sm primary" onClick={onSetup}>
              Set up runtime…
            </button>
            <button class="btn sm" onClick={locate}>
              Locate…
            </button>
          </div>
        </>
      ) : (
        <>
          <div class="vc-msg">
            <b>OmniVoice wasn't found on this computer.</b>
            <br />
            MoodCut looked in its saved location, <code>{st.subdir}</code> on every drive, the usual model folders and the Hugging Face cache.
          </div>
          <div class="btn-row">
            <button class="btn sm primary" onClick={onSetup}>
              Download…
            </button>
            <button class="btn sm" title="Pick the folder that holds config.json, model.safetensors and audio_tokenizer" onClick={locate}>
              Locate model folder…
            </button>
            <button class="btn sm ghost" onClick={() => reload({ action: "scan" })}>
              Search again
            </button>
          </div>
        </>
      )}
      <div class="hint">
        OmniVoice ({st.repo}) clones a voice from 3 to 10 seconds of speech and runs entirely on this computer. An NVIDIA GPU makes it fast; it also works on the CPU, more slowly.
      </div>
    </div>
  );
}

/** Pick a drive (or a folder), then download and set up; shows progress while it runs. */
function SetupForm({ st, reload, onClose }: { st: Status; reload: Reload; onClose: () => void }) {
  const [drives, setDrives] = useState<Drive[] | null>(null);
  const [need, setNeed] = useState(0);
  const [drive, setDrive] = useState<string>("");
  const [folder, setFolder] = useState("");
  const inst = st.install;

  useEffect(() => {
    if (inst.running) return;
    (async () => {
      const r = await api<{
        drives: Drive[];
        need: number;
        suggested: string | null;
      }>("/api/voice", { action: "drives" });
      if (!r.ok) return toast(r.error, true);
      setDrives(r.drives);
      setNeed(r.need);
      if (r.suggested) setFolder(r.suggested);
      // the roomiest drive that isn't the system drive, else the roomiest
      const fits = r.drives.filter((d) => d.free == null || d.free > r.need + 2 * GB);
      const pick = fits.find((d) => !/^C:/i.test(d.root) && !d.removable) || fits[0] || r.drives[0];
      if (pick) setDrive(pick.root);
    })();
  }, [inst.running]);

  if (inst.running) {
    return (
      <div class="group-b">
        <div class="vc-msg">
          <b>{inst.step === "Repairing" ? "Repairing" : "Setting up"} OmniVoice</b> in <code>{inst.root}</code>
        </div>
        <div class="vc-step">{inst.step}</div>
        <Bar pct={inst.pct == null ? null : inst.pct / 100} />
        <div class="vc-status" title={inst.status}>
          {inst.status}
        </div>
        <details class="vc-log">
          <summary>Details</summary>
          <pre>{inst.log.join("\n")}</pre>
        </details>
        <div class="btn-row">
          <button class="btn sm" onClick={() => reload({ action: "cancel_install" })}>
            Cancel
          </button>
        </div>
        <div class="hint">You can keep editing meanwhile. A cancelled or interrupted download continues where it stopped.</div>
      </div>
    );
  }

  const target = folder.trim() || (drive ? `${drive}${st.subdir}` : "");
  const sel = drives?.find((d) => target.toUpperCase().startsWith(d.root.toUpperCase()));
  const short = !!sel && sel.free != null && sel.free < need + 2 * GB;
  const install = async () => {
    if (!target) return toast("Pick a drive", true);
    await reload({ action: "install", folder: target });
  };
  return (
    <div class="group-b">
      {inst.error && <Problem message={inst.error} hint={inst.hint} detail={inst.detail} />}
      <div class="vc-msg">
        <b>Where should OmniVoice go?</b> It needs about {gb(need || st.sizes.model + st.sizes.runtime)}
        {st.state === "missing" ? ` (model ${gb(st.sizes.model)}, Python with PyTorch ${gb(st.sizes.runtime)})` : " (Python with PyTorch)"}, plus {gb(st.sizes.whisper)} later if you clone voices
        without typing their transcript.
      </div>
      <div class="vc-drives">
        {drives == null && <div class="hint">Reading drives…</div>}
        {drives?.map((d) => {
          const fits = d.free == null || d.free > need + 2 * GB;
          const used = d.total && d.free != null ? 1 - d.free / d.total : 0;
          return (
            <label key={d.root} class={"vc-drive" + (drive === d.root && !folder.trim() ? " on" : "") + (fits ? "" : " full")}>
              <input type="radio" name="vc-drive" checked={drive === d.root && !folder.trim()} disabled={!fits} onChange={() => (setDrive(d.root), setFolder(""))} />
              <span class="vc-drive-name">
                <b>{d.root.replace(/\\$/, "")}</b> {d.label || (d.removable ? "Removable" : "Local disk")}
              </span>
              <span class="vc-drive-bar">
                <i style={{ width: `${Math.round(used * 100)}%` }} />
              </span>
              <span class="vc-faint">{d.free != null ? `${gb(d.free)} free` : ""}</span>
            </label>
          );
        })}
      </div>
      <div class="row">
        <label>Folder</label>
        <div class="ctl">
          <input value={target} placeholder={`D:\\${st.subdir}`} onInput={(e) => setFolder((e.target as HTMLInputElement).value)} />
          <button
            class="btn sm"
            style={{ flex: "0 0 auto" }}
            onClick={async () => {
              const [dir] = await pickFiles("folder");
              if (dir) setFolder(dir);
            }}
          >
            Browse…
          </button>
        </div>
        <span />
      </div>
      {short && <div class="vc-error">Not enough free space on {sel?.root}: pick another drive.</div>}
      <div class="btn-row">
        <button class="btn sm primary" disabled={!target || short} onClick={install}>
          {inst.error ? "Try again" : st.state === "runtime" ? "Set up runtime" : "Download and set up"}
        </button>
        <button class="btn sm ghost" onClick={onClose}>
          Back
        </button>
      </div>
      <div class="hint">
        MoodCut downloads the model from huggingface.co/{st.repo}, and Python with PyTorch{st.gpu ? ` (CUDA, for ${st.gpu})` : " (CPU)"} through uv, all into this folder: nothing is installed
        system-wide and the system drive stays free. The model weights are licensed CC BY-NC 4.0 (non-commercial use) and its audio tokenizer under the Boson Higgs Audio 2 Community License.
      </div>
    </div>
  );
}

/** A progress bar: a fraction, or a moving stripe while the amount is unknown. */
function Bar({ pct }: { pct: number | null }) {
  return (
    <div class={"progress vc-bar" + (pct == null ? " indet" : "")}>
      <div
        style={{
          width: pct == null ? "30%" : `${Math.max(2, Math.round(pct * 100))}%`,
        }}
      />
    </div>
  );
}

/** Clone: a project clip (Source monitor In/Out) or a file, an optional transcript, consent. */
function CloneForm({ st, onSent }: { st: Status; onSent: () => void }) {
  const audible = (project.value?.media || []).filter((m) => m.has_audio);
  const [name, setName] = useState("");
  const [from, setFrom] = useState<"clip" | "file">("clip");
  const [media, setMedia] = useState("");
  const [file, setFile] = useState("");
  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");
  const [text, setText] = useState("");
  const [denoise, setDenoise] = useState(false);
  const [consent, setConsent] = useState(false);
  const [open, setOpen] = useState(st.voices.length === 0);

  // follow the Source monitor: its clip and In/Out are the natural reference
  useEffect(() => {
    const m = sourceMedia.value && mediaById.value.get(sourceMedia.value);
    if (m && m.has_audio) {
      setMedia(m.id);
      setFrom("clip");
    }
  }, [sourceMedia.value]);
  useEffect(() => {
    if (from === "clip" && media && media === sourceMedia.value) {
      setStart(sourceIn.value != null ? fmtNum(sourceIn.value, 2) : "");
      setEnd(sourceOut.value != null ? fmtNum(sourceOut.value, 2) : "");
    }
  }, [sourceIn.value, sourceOut.value, media, from]);

  const s = parseFloat(start) || 0;
  const e = parseFloat(end);
  const len = isFinite(e) && e > s ? e - s : null;
  const send = async () => {
    if (!name.trim()) return toast("Give the voice a name", true);
    if (from === "clip" && !media) return toast("Pick the clip to clone from", true);
    if (from === "file" && !file) return toast("Pick an audio or video file", true);
    if (!consent) return toast("Confirm you have the speaker's permission", true);
    const r = await api("/api/voice", {
      action: "clone",
      name: name.trim(),
      media: from === "clip" ? media : undefined,
      path: from === "file" ? file : undefined,
      start: s,
      end: isFinite(e) ? e : undefined,
      transcript: text.trim() || undefined,
      denoise,
      consent,
    });
    if (!r.ok) return toast(r.error, true);
    toast(`Cloning "${name.trim()}"…`);
    setName("");
    setText("");
    setConsent(false);
    onSent();
  };

  return (
    <>
      <div class="section-title vc-toggle" onClick={() => setOpen(!open)}>
        {open ? "▾" : "▸"} Clone a voice
      </div>
      {open && (
        <div class="group-b">
          <div class="row">
            <label>Name</label>
            <div class="ctl">
              <input value={name} placeholder="Narrator" onInput={(ev) => setName((ev.target as HTMLInputElement).value)} />
            </div>
            <span />
          </div>
          <div class="row">
            <label>Reference</label>
            <div class="ctl">
              <select value={from} onChange={(ev) => setFrom((ev.target as HTMLSelectElement).value as "clip" | "file")}>
                <option value="clip">Clip in the project</option>
                <option value="file">Audio or video file</option>
              </select>
            </div>
            <span />
          </div>
          {from === "clip" ? (
            <div class="row">
              <label>Clip</label>
              <div class="ctl">
                <select value={media} onChange={(ev) => setMedia((ev.target as HTMLSelectElement).value)}>
                  <option value="">{audible.length ? "Pick a clip with speech" : "Import a clip with speech first"}</option>
                  {audible.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.id} {m.name}
                    </option>
                  ))}
                </select>
              </div>
              <span />
            </div>
          ) : (
            <div class="row">
              <label>File</label>
              <div class="ctl">
                <input value={file} placeholder="C:\\voice.wav" onInput={(ev) => setFile((ev.target as HTMLInputElement).value)} />
                <button
                  class="btn sm"
                  style={{ flex: "0 0 auto" }}
                  onClick={async () => {
                    const [f] = await pickFiles("open");
                    if (f) setFile(f);
                  }}
                >
                  Browse…
                </button>
              </div>
              <span />
            </div>
          )}
          <div class="row">
            <label>From / to (s)</label>
            <div class="ctl">
              <input type="number" min="0" step="0.1" value={start} placeholder="0" onInput={(ev) => setStart((ev.target as HTMLInputElement).value)} />
              <input type="number" min="0" step="0.1" value={end} placeholder="auto" onInput={(ev) => setEnd((ev.target as HTMLInputElement).value)} />
            </div>
            <span />
          </div>
          <div class="hint">
            {len != null
              ? `${fmtNum(len, 1)} s of reference. `
              : text.trim()
                ? "Set From and To to exactly the words you typed. "
                : "Without an end, MoodCut picks the best 3-12 s of speech in the first 30 s, cut at pauses. "}
            One person speaking clearly without music works best; mark In and Out in the Source monitor to choose the part.
          </div>
          <textarea
            rows={2}
            value={text}
            placeholder="What is said (optional: left empty, Whisper writes it and you can correct it later)"
            onInput={(ev) => setText((ev.target as HTMLTextAreaElement).value)}
          />
          <label class="vc-check" title="Lowers steady background noise (fans, hiss, street). Leave it off for clean studio audio: it can dull the voice.">
            <input type="checkbox" checked={denoise} onChange={(ev) => setDenoise((ev.target as HTMLInputElement).checked)} />
            Reduce background noise (phone and room recordings)
          </label>
          <label class="vc-check">
            <input type="checkbox" checked={consent} onChange={(ev) => setConsent((ev.target as HTMLInputElement).checked)} />
            This is my voice, or I have the speaker's permission to clone it.
          </label>
          <div class="btn-row">
            <button class="btn sm primary" disabled={!consent} onClick={send}>
              Clone voice
            </button>
          </div>
        </div>
      )}
    </>
  );
}

function Voices({ st, reload }: { st: Status; reload: Reload }) {
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  if (!st.voices.length) return null;
  const fix = async (v: Voice) => {
    if (!draft.trim()) return toast("Type the words said in the reference", true);
    const r = await api("/api/voice", {
      action: "retranscribe",
      voice: v.id,
      transcript: draft.trim(),
    });
    if (!r.ok) return toast(r.error, true);
    setEditing(null);
    reload();
  };
  return (
    <>
      <div class="section-title">Voices</div>
      <div class="vc-voices">
        {st.voices.map((v) => (
          <div class="vc-voice" key={v.id}>
            <div class="vc-voice-top">
              <div class="vc-voice-main" title={`${v.source}${v.start != null ? ` from ${fmtNum(v.start, 1)} s` : ""}\n${fmtNum(v.dur, 1)} s reference${v.denoise ? ", noise reduced" : ""}`}>
                <b>{v.name}</b>
                <span class="vc-faint">
                  {fmtNum(v.dur, 1)} s from {v.source}
                  {v.auto ? " (best part picked)" : ""}
                </span>
              </div>
              <button
                class="btn sm ghost"
                title="Rename"
                onClick={() => {
                  const n = prompt("Voice name", v.name);
                  if (n && n.trim() && n.trim() !== v.name)
                    reload({
                      action: "rename_voice",
                      id: v.id,
                      name: n.trim(),
                    });
                }}
              >
                ✎
              </button>
              <button
                class="btn sm ghost"
                title="Delete this voice (clips already generated stay)"
                onClick={() => confirm(`Delete the voice "${v.name}"?`) && reload({ action: "delete_voice", id: v.id })}
              >
                ✕
              </button>
            </div>
            {editing === v.id ? (
              <div class="vc-fix">
                <textarea rows={2} value={draft} onInput={(ev) => setDraft((ev.target as HTMLTextAreaElement).value)} onKeyDown={(ev) => ev.stopPropagation()} />
                <div class="btn-row">
                  <button class="btn sm primary" onClick={() => fix(v)}>
                    Encode again
                  </button>
                  <button class="btn sm ghost" onClick={() => setEditing(null)}>
                    Cancel
                  </button>
                </div>
                <div class="hint">The words must match the reference exactly; a correct transcript gives a more faithful voice.</div>
              </div>
            ) : (
              <div class="vc-transcript" title="What the reference says. A wrong word here makes the clone less faithful.">
                “{v.ref_text || "…"}”{" "}
                <button class="btn sm ghost vc-link" onClick={() => (setEditing(v.id), setDraft(v.ref_text || ""))}>
                  Fix transcript
                </button>
              </div>
            )}
          </div>
        ))}
      </div>
    </>
  );
}

function GenerateForm({ st, onSent }: { st: Status; onSent: () => void }) {
  const [voice, setVoice] = useState(() => recall("mc.voice.last", ""));
  const [text, setText] = useState("");
  const [language, setLanguage] = useState(() => recall("mc.voice.lang", "auto"));
  const [speed, setSpeed] = useState(1);
  const [quality, setQuality] = useState(() => recall("mc.voice.quality", "standard"));
  const [takes, setTakes] = useState(1);
  const [split, setSplit] = useState(true);
  const [place, setPlace] = useState(false);
  const [numbers, setNumbers] = useState(true);
  const [even, setEven] = useState(true);
  const paragraphs = useMemo(() => text.split(/\n\s*\n/).filter((p) => p.trim()).length, [text]);

  // a voice that was deleted (or none chosen yet): the newest one
  const vid = st.voices.some((v) => v.id === voice) ? voice : st.voices[0]?.id || "";
  if (!st.voices.length) {
    return (
      <>
        <div class="section-title">Generate speech</div>
        <div class="group-b hint">Clone a voice first, then type a script here to hear it in that voice.</div>
      </>
    );
  }
  const clips = (split ? Math.max(1, paragraphs) : 1) * takes;
  const send = async () => {
    if (!text.trim()) return toast("Write the script to speak", true);
    store("mc.voice.last", vid);
    store("mc.voice.lang", language);
    store("mc.voice.quality", quality);
    const r = await api("/api/voice", {
      action: "generate",
      voice: vid,
      text,
      language,
      speed,
      quality,
      takes,
      split,
      numbers,
      loudness: even,
      at: place ? playhead.value : undefined,
    });
    if (!r.ok) return toast(r.error, true);
    toast("Generating speech…");
    onSent();
  };
  return (
    <>
      <div class="section-title">Generate speech</div>
      <div class="group-b">
        <div class="row">
          <label>Voice</label>
          <div class="ctl">
            <select value={vid} onChange={(ev) => setVoice((ev.target as HTMLSelectElement).value)}>
              {st.voices.map((v) => (
                <option key={v.id} value={v.id}>
                  {v.name}
                </option>
              ))}
            </select>
          </div>
          <span />
        </div>
        <textarea
          rows={6}
          value={text}
          placeholder={"Type the script.\n\nA blank line starts a new clip. Punctuation shapes the reading: commas pause, question marks rise."}
          onInput={(ev) => setText((ev.target as HTMLTextAreaElement).value)}
          onKeyDown={(ev) => {
            // Ctrl+Enter generates; keep the editor's shortcuts out of the text box
            ev.stopPropagation();
            if (ev.key === "Enter" && (ev.ctrlKey || ev.metaKey)) send();
          }}
        />
        <div class="vc-faint vc-count">{text.trim() ? `${text.trim().split(/\s+/).length} words · ${clips} clip${clips === 1 ? "" : "s"}` : ""}</div>
        <div class="row">
          <label>Language</label>
          <div class="ctl">
            <select value={language} onChange={(ev) => setLanguage((ev.target as HTMLSelectElement).value)}>
              <option value="auto">Detect from the text</option>
              {LANGUAGES.map((l) => (
                <option key={l}>{l}</option>
              ))}
            </select>
          </div>
          <span />
        </div>
        <div class="row">
          <label>Speed</label>
          <div class="ctl">
            <input type="range" min="0.7" max="1.4" step="0.05" value={speed} onInput={(ev) => setSpeed(parseFloat((ev.target as HTMLInputElement).value))} />
          </div>
          <span class="vc-faint" style={{ cursor: "pointer" }} title="Reset" onClick={() => setSpeed(1)}>
            {fmtNum(speed, 2)}×
          </span>
        </div>
        <div class="row">
          <label>Quality</label>
          <div class="ctl">
            <select value={quality} onChange={(ev) => setQuality((ev.target as HTMLSelectElement).value)}>
              {QUALITY.map(([k, l]) => (
                <option key={k} value={k}>
                  {l}
                </option>
              ))}
            </select>
          </div>
          <span />
        </div>
        <div class="row">
          <label title="Each take is a different reading; pick the best one in the Project panel">Takes</label>
          <div class="ctl">
            <select value={takes} onChange={(ev) => setTakes(parseInt((ev.target as HTMLSelectElement).value))}>
              <option value={1}>1</option>
              <option value={2}>2 to choose from</option>
              <option value={3}>3 to choose from</option>
            </select>
          </div>
          <span />
        </div>
        <label class="vc-check">
          <input type="checkbox" checked={split} onChange={(ev) => setSplit((ev.target as HTMLInputElement).checked)} />
          One clip per paragraph
        </label>
        <label class="vc-check" title="2026 is read 'twenty twenty-six', 40% 'forty percent'">
          <input type="checkbox" checked={numbers} onChange={(ev) => setNumbers((ev.target as HTMLInputElement).checked)} />
          Read numbers as words
        </label>
        <label class="vc-check" title="Every clip at the same level (about -16 LUFS), whatever the reference's volume">
          <input type="checkbox" checked={even} onChange={(ev) => setEven((ev.target as HTMLInputElement).checked)} />
          Even loudness
        </label>
        <label class="vc-check">
          <input type="checkbox" checked={place} onChange={(ev) => setPlace((ev.target as HTMLInputElement).checked)} />
          Also add to the timeline at the playhead
          {takes > 1 ? " (first take)" : ""}
        </label>
        <div class="btn-row">
          <button class="btn sm primary" disabled={!text.trim()} onClick={send} title="Ctrl+Enter">
            Generate
          </button>
        </div>
        <div class="hint">Clips go into the Project panel, tagged “voice”, with the script as their note.</div>
      </div>
    </>
  );
}

/** The task running now (or the latest result), pinned under the header so progress and results
 * are in view without scrolling to the Activity list. */
function Current({ t, queued, reload, onDismiss }: { t: Task; queued: number; reload: Reload; onDismiss?: () => void }) {
  return (
    <div class="vc-current">
      <TaskView t={t} reload={reload} onDismiss={onDismiss} />
      {queued > 0 && <div class="vc-faint vc-queued">{queued} more waiting</div>}
    </div>
  );
}

function Tasks({ st, reload, skip }: { st: Status; reload: Reload; skip?: number }) {
  const list = st.tasks.filter((t) => t.id !== skip);
  if (!list.length) return null;
  const finished = list.some((t) => t.state === "done" || t.state === "error");
  return (
    <>
      <div class="section-title vc-tasks-head">
        Activity
        {finished && (
          <button class="btn sm ghost" onClick={() => reload({ action: "clear" })}>
            Clear
          </button>
        )}
      </div>
      <div class="vc-tasks">
        {[...list].reverse().map((t) => (
          <TaskView key={t.id} t={t} reload={reload} />
        ))}
      </div>
    </>
  );
}

/** One clone or speech task: its progress while it runs, then its result or its explained error. */
function TaskView({ t, reload, onDismiss }: { t: Task; reload: Reload; onDismiss?: () => void }) {
  return (
    <div class={"vc-task " + t.state}>
      <div class="vc-task-top">
        <span class="vc-dot" />
        <span class="vc-task-label" title={t.label}>
          {t.label}
        </span>
        {(t.state === "queued" || t.state === "running") && (
          <button class="btn sm ghost" title="Cancel" onClick={() => reload({ action: "cancel", id: t.id })}>
            ✕
          </button>
        )}
        {onDismiss && (
          <button class="btn sm ghost" title="Hide (it stays in Activity)" onClick={onDismiss}>
            ✕
          </button>
        )}
      </div>
      {t.state === "running" && (
        <div class="vc-task-body">
          <Bar pct={t.pct} />
          <div class="vc-task-status">
            <span>{t.status}…</span>
            <span class="vc-faint">
              {t.pct != null ? `${Math.round(t.pct * 100)}% · ` : ""}
              {clock(t.secs)}
            </span>
          </div>
        </div>
      )}
      {t.state === "queued" && <div class="vc-task-status">{t.status}</div>}
      {t.state === "done" && (
        <div class="vc-task-status">
          {t.status}
          {t.secs > 0 ? <span class="vc-faint"> · {clock(t.secs)}</span> : null}
        </div>
      )}
      {t.state === "error" && (
        <div class="vc-task-body">
          {t.error === "Cancelled." ? (
            <div class="vc-task-status">Cancelled.</div>
          ) : (
            <Problem message={t.error || "Failed."} hint={t.hint} detail={t.detail}>
              {t.fix === "repair" && (
                <button class="btn sm primary" onClick={() => reload({ action: "repair" })}>
                  Repair
                </button>
              )}
              <button class={"btn sm" + (t.fix === "retry" ? " primary" : "")} onClick={() => reload({ action: "retry", id: t.id })}>
                Retry
              </button>
            </Problem>
          )}
        </div>
      )}
      {t.result?.media?.length ? (
        <div class="vc-task-media">
          {t.result.media.map((id) => (
            <button key={id} class="btn sm" title="Open in the Source monitor to listen" onClick={() => openInSource(id)}>
              ▶ {mediaById.value.get(id)?.name || id}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
