// AI Voice: speech in a cloned or library voice through ElevenLabs, with the person's own API
// key. Pick a voice (yours first, then ElevenLabs' library, with previews), write the script
// (a blank line starts a new clip), shape the delivery, and the speech is imported into the
// Project panel (tag "voice") and optionally placed at the playhead. Clone a voice from a clip's
// In/Out or a file, with the speaker's permission. AI apps drive the same voices and tasks
// through the `voice` MCP tool.
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { showPanel } from "../dock";
import { api } from "../gfx";
import { mediaById, monitorTab, pickFiles, playhead, project, servicesTick, showServices, sourceIn, sourceMedia, sourceOut, sourceT, toast, voiceTick } from "../store";
import { fmtNum } from "../util";
import { Section, Slider } from "./Controls";
import { AiTask, ConnectCard, Problem, Service, TaskList } from "./aiui";

type Voice = {
  id: string;
  name: string;
  category: string;
  mine: boolean;
  description: string;
  accent?: string | null;
  gender?: string | null;
  age?: string | null;
  use?: string | null;
  language?: string | null;
  preview?: string | null;
};
type Account = { tier: string; used: number; limit: number; reset: number | null; can_clone: boolean | null; voices_used: number | null; voice_limit: number | null } | null;
type Status = {
  provider: Service | null;
  account: Account;
  voices: Voice[];
  voices_error: { message: string; hint?: string; fix?: AiTask["fix"]; link?: string } | null;
  models: { id: string; label: string; max_chars: number; language: boolean }[];
  default_model: string;
  languages: { name: string; code: string }[];
  tasks: AiTask[];
};
type Reload = (b?: Record<string, unknown>) => Promise<boolean>;

const num = (n: number) => n.toLocaleString("en-US");

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

function openInSource(id: string) {
  sourceMedia.value = id;
  sourceT.value = 0;
  sourceIn.value = null;
  sourceOut.value = null;
  monitorTab.value = "source";
  showPanel("source");
}

// one preview at a time
let previewEl: HTMLAudioElement | null = null;

export function VoicePanel() {
  const [st, setSt] = useState<Status | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [voice, setVoice] = useState(() => recall("mc.el.voice", ""));

  const load: Reload = async (body) => {
    const r = await api<Status>("/api/voice", body ?? { action: "status" });
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
  }, [voiceTick.value, servicesTick.value]);
  const busy = !!st && st.tasks.some((t) => t.state === "queued" || t.state === "running");
  useEffect(() => {
    if (!busy) return;
    const h = setInterval(() => load(), 800);
    return () => clearInterval(h);
  }, [busy]);

  if (!st)
    return (
      <div class="scroll ai-panel">
        <div class="ai-faint" style={{ padding: "10px" }}>
          {err || "Loading AI Voice…"}
        </div>
      </div>
    );
  const connected = !!st.provider?.connected;
  const vid = st.voices.some((v) => v.id === voice) ? voice : st.voices[0]?.id || "";
  const pick = (id: string) => {
    setVoice(id);
    store("mc.el.voice", id);
  };
  return (
    <div class="scroll ai-panel">
      <Header st={st} />
      {!connected ? (
        <ConnectCard
          id="elevenlabs"
          title="Connect ElevenLabs to use AI Voice"
          lines={[
            "Speech in thousands of ready voices and 70+ languages, made in seconds.",
            "Clone a voice from a minute or two of a clip (paid ElevenLabs plans), with the speaker's permission.",
            "Usage counts against your ElevenLabs plan; the key stays encrypted on this computer.",
          ]}
        />
      ) : (
        <>
          <TaskList
            tasks={st.tasks}
            onCancel={(id) => load({ action: "cancel", id })}
            onRetry={(id) => load({ action: "retry", id })}
            onClear={() => load({ action: "clear" })}
            render={(t) =>
              t.result?.media?.length ? (
                <div class="ai-results">
                  {(t.result.media as string[]).map((id) => (
                    <button key={id} class="btn sm" title="Open in the Source monitor to listen" onClick={() => openInSource(id)}>
                      ▶ {mediaById.value.get(id)?.name || id}
                    </button>
                  ))}
                </div>
              ) : t.result?.voice ? (
                <div class="ai-results">
                  <button class="btn sm" onClick={() => pick(t.result.voice)}>
                    Use “{t.result.name}”
                  </button>
                </div>
              ) : null
            }
          />
          {st.voices_error && <Problem message={st.voices_error.message} hint={st.voices_error.hint} fix={st.voices_error.fix} link={st.voices_error.link} provider="elevenlabs" onRetry={() => load({ action: "voices" })} />}
          <Voices st={st} selected={vid} onPick={pick} reload={load} />
          <Speak st={st} voice={vid} onSent={() => load()} />
          <Clone st={st} onSent={() => load()} />
        </>
      )}
    </div>
  );
}

function Header({ st }: { st: Status }) {
  const a = st.account;
  const left = a ? Math.max(0, a.limit - a.used) : null;
  const reset = a?.reset ? new Date(a.reset * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" }) : null;
  return (
    <div class="ai-head">
      <div class="ai-head-t">
        <b>AI Voice</b>
        <span class="ai-faint">ElevenLabs</span>
      </div>
      {a && left != null && a.limit > 0 && (
        <div class="ai-quota" title={`${a.tier} plan: ${num(a.used)} of ${num(a.limit)} characters used${reset ? `; resets ${reset}` : ""}`}>
          <div class="ai-quota-bar">
            <i style={{ width: `${Math.min(100, (a.used / a.limit) * 100)}%` }} class={left < a.limit * 0.1 ? "low" : ""} />
          </div>
          <span>
            {num(left)} characters left{reset ? ` · resets ${reset}` : ""}
          </span>
        </div>
      )}
      <button class="btn sm ghost" title="AI Services: API keys" onClick={() => (showServices.value = "elevenlabs")}>
        ⚙
      </button>
    </div>
  );
}

function traits(v: Voice) {
  return [v.gender, v.age, v.accent, v.use].filter((x) => x && String(x).trim()).join(" · ");
}

function Voices({ st, selected, onPick, reload }: { st: Status; selected: string; onPick: (id: string) => void; reload: Reload }) {
  const [q, setQ] = useState("");
  const [which, setWhich] = useState<"all" | "mine" | "library">(() => (recall("mc.el.which", "all") as any) || "all");
  const [playing, setPlaying] = useState<string | null>(null);
  const mine = st.voices.filter((v) => v.mine).length;
  const list = useMemo(() => {
    const s = q.trim().toLowerCase();
    return st.voices.filter((v) => (which === "mine" ? v.mine : which === "library" ? !v.mine : true)).filter((v) => !s || `${v.name} ${v.description} ${traits(v)} ${v.language || ""}`.toLowerCase().includes(s));
  }, [st.voices, q, which]);
  const preview = (v: Voice) => {
    if (previewEl) {
      previewEl.pause();
      previewEl = null;
    }
    if (playing === v.id || !v.preview) return setPlaying(null);
    const a = new Audio(v.preview);
    a.onended = () => setPlaying(null);
    a.onerror = () => (setPlaying(null), toast("The preview couldn't be played", true));
    a.play().catch(() => setPlaying(null));
    previewEl = a;
    setPlaying(v.id);
  };
  useEffect(() => () => previewEl?.pause(), []);
  const sel = st.voices.find((v) => v.id === selected);
  return (
    <Section
      title={`Voice${sel ? `: ${sel.name}` : ""}`}
      acts={
        <button class="btn sm ghost" title="Refresh the list (voices added on elevenlabs.io appear)" onClick={() => reload({ action: "voices" })}>
          ⟳
        </button>
      }
    >
      <div class="ai-filter">
        <input class="search" placeholder="Search voices: warm, british, narration…" value={q} onInput={(e) => setQ((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
        <div class="seg">
          {(
            [
              ["all", `All ${st.voices.length}`],
              ["mine", `Yours ${mine}`],
              ["library", "Library"],
            ] as const
          ).map(([k, l]) => (
            <button key={k} class={"btn sm" + (which === k ? " on" : "")} onClick={() => (setWhich(k), store("mc.el.which", k))}>
              {l}
            </button>
          ))}
        </div>
      </div>
      <div class="ai-voices" role="listbox" aria-label="Voices">
        {!list.length && <div class="ai-faint" style={{ padding: "8px" }}>{st.voices.length ? "No voice matches." : "No voices yet."}</div>}
        {list.map((v) => (
          <div key={v.id} role="option" aria-selected={v.id === selected} class={"ai-voice" + (v.id === selected ? " on" : "")} onClick={() => onPick(v.id)} title={v.description || v.name}>
            <button class={"ai-play" + (playing === v.id ? " on" : "")} disabled={!v.preview} title={v.preview ? "Listen to a sample" : "No sample"} onClick={(e) => (e.stopPropagation(), preview(v))}>
              {playing === v.id ? "■" : "▶"}
            </button>
            <div class="ai-voice-main">
              <div>
                <b>{v.name}</b> {v.mine && <span class="ai-chip">{v.category === "cloned" ? "your clone" : "yours"}</span>}
              </div>
              <div class="ai-faint ai-ellipsis">{traits(v) || v.description || v.category}</div>
            </div>
            {v.mine && (
              <span class="ai-voice-acts" onClick={(e) => e.stopPropagation()}>
                <button
                  class="btn sm ghost icon"
                  title="Rename"
                  onClick={() => {
                    const n = prompt("Voice name", v.name);
                    if (n && n.trim() && n.trim() !== v.name) reload({ action: "rename_voice", id: v.id, name: n.trim() });
                  }}
                >
                  ✎
                </button>
                <button class="btn sm ghost icon" title="Delete this voice from ElevenLabs (clips already made stay)" onClick={() => confirm(`Delete the voice "${v.name}" from your ElevenLabs account?`) && reload({ action: "delete_voice", id: v.id })}>
                  ✕
                </button>
              </span>
            )}
          </div>
        ))}
      </div>
    </Section>
  );
}

const DELIVERY = { stability: 0.5, similarity: 0.75, style: 0, speed: 1 };

function Speak({ st, voice, onSent }: { st: Status; voice: string; onSent: () => void }) {
  const [text, setText] = useState("");
  const [model, setModel] = useState(() => recall("mc.el.model", ""));
  const [language, setLanguage] = useState(() => recall("mc.el.lang", "auto"));
  const [d, setD] = useState<typeof DELIVERY>(() => {
    try {
      return { ...DELIVERY, ...JSON.parse(recall("mc.el.delivery", "{}")) };
    } catch {
      return DELIVERY;
    }
  });
  const [boost, setBoost] = useState(true);
  const [takes, setTakes] = useState(1);
  const [split, setSplit] = useState(true);
  const [numbers, setNumbers] = useState(true);
  const [even, setEven] = useState(true);
  const [place, setPlace] = useState(false);
  const [sending, setSending] = useState(false);
  const area = useRef<HTMLTextAreaElement>(null);
  const m = st.models.find((x) => x.id === model) || st.models.find((x) => x.id === st.default_model) || st.models[0];
  const paragraphs = useMemo(() => (split ? text.split(/\n\s*\n/).filter((p) => p.trim()) : text.trim() ? [text] : []), [text, split]);
  const chars = text.replace(/\s+/g, " ").trim().length;
  const longest = paragraphs.reduce((n, p) => Math.max(n, p.replace(/\s+/g, " ").trim().length), 0);
  const clips = Math.max(1, paragraphs.length) * takes;
  const a = st.account;
  const left = a ? Math.max(0, a.limit - a.used) : null;
  const cost = chars * takes;
  const setDel = (k: keyof typeof DELIVERY, v: number) => {
    const n = { ...d, [k]: v };
    setD(n);
    store("mc.el.delivery", JSON.stringify(n));
  };
  const send = async () => {
    if (!voice) return toast("Pick a voice first", true);
    if (!text.trim()) return toast("Write the script to speak", true);
    if (m && longest > m.max_chars) return toast(`A paragraph has ${num(longest)} characters; ${m.id} takes ${num(m.max_chars)} per clip. Split it with a blank line.`, true);
    store("mc.el.model", m?.id || "");
    store("mc.el.lang", language);
    setSending(true);
    const r = await api("/api/voice", {
      action: "speak",
      voice,
      text,
      model: m?.id,
      language,
      stability: d.stability,
      similarity: d.similarity,
      style: d.style,
      speed: d.speed,
      boost,
      takes,
      split,
      numbers,
      loudness: even,
      at: place ? playhead.value : undefined,
    });
    setSending(false);
    if (!r.ok) return toast(r.error, true);
    toast("Generating speech…");
    onSent();
  };
  return (
    <Section title="Speech">
      <textarea
        ref={area}
        class="ai-script"
        rows={6}
        value={text}
        placeholder={"Type the script.\n\nA blank line starts a new clip. Punctuation shapes the reading: commas pause, question marks rise." + (m?.id === "eleven_v3" ? " Tags like [whispers] or [laughs] direct the delivery." : "")}
        onInput={(e) => setText((e.target as HTMLTextAreaElement).value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) send();
        }}
      />
      <div class="ai-count">
        <span>{chars ? `${num(chars)} characters · ${clips} clip${clips === 1 ? "" : "s"}` : ""}</span>
        <span class={left != null && cost > left ? "ai-warn" : "ai-faint"}>{chars && left != null ? `uses about ${num(cost)} of ${num(left)} left` : ""}</span>
      </div>
      <div class="row">
        <label>Model</label>
        <div class="ctl">
          <select value={m?.id} onChange={(e) => setModel((e.target as HTMLSelectElement).value)} title={m?.label}>
            {st.models.map((x) => (
              <option key={x.id} value={x.id}>
                {x.label.split(":")[0]}
              </option>
            ))}
          </select>
        </div>
        <span />
      </div>
      {m && <div class="ai-faint ai-desc">{m.label.split(":").slice(1).join(":").trim()}</div>}
      <div class="row">
        <label>Language</label>
        <div class="ctl">
          <select value={language} disabled={m && !m.language} onChange={(e) => setLanguage((e.target as HTMLSelectElement).value)} title={m && !m.language ? "This model detects the language from the text" : "The language the script is in"}>
            <option value="auto">Detect from the text</option>
            {st.languages.map((l) => (
              <option key={l.code} value={l.code}>
                {l.name}
              </option>
            ))}
          </select>
        </div>
        <span />
      </div>
      <div class="ai-sub">Delivery</div>
      <Slider label="Stability" value={d.stability} min={0} max={1} step={0.05} def={0.5} title="Low: more expressive and varied. High: even and consistent." onCommit={(v) => setDel("stability", v)} />
      <Slider label="Similarity" value={d.similarity} min={0} max={1} step={0.05} def={0.75} title="How closely it keeps to the voice's own sound" onCommit={(v) => setDel("similarity", v)} />
      <Slider label="Style" value={d.style} min={0} max={1} step={0.05} def={0} title="Exaggerates the speaker's style (slower to make). 0 for neutral narration." onCommit={(v) => setDel("style", v)} />
      <Slider label="Speed" value={d.speed} min={0.7} max={1.2} step={0.01} def={1} title="0.7 slow, 1 natural, 1.2 fast" onCommit={(v) => setDel("speed", v)} />
      <div class="ai-checks">
        <label class="ai-check" title="Keeps the voice closer to the speaker (a little slower)">
          <input type="checkbox" checked={boost} onChange={(e) => setBoost((e.target as HTMLInputElement).checked)} />
          Speaker boost
        </label>
        <label class="ai-check">
          <input type="checkbox" checked={split} onChange={(e) => setSplit((e.target as HTMLInputElement).checked)} />
          One clip per paragraph
        </label>
        <label class="ai-check" title="2026 is read 'twenty twenty-six', 40% 'forty percent'">
          <input type="checkbox" checked={numbers} onChange={(e) => setNumbers((e.target as HTMLInputElement).checked)} />
          Read numbers naturally
        </label>
        <label class="ai-check" title="Every clip at about -16 LUFS">
          <input type="checkbox" checked={even} onChange={(e) => setEven((e.target as HTMLInputElement).checked)} />
          Even loudness
        </label>
        <label class="ai-check">
          <input type="checkbox" checked={place} onChange={(e) => setPlace((e.target as HTMLInputElement).checked)} />
          Add to the timeline at the playhead{takes > 1 ? " (first take)" : ""}
        </label>
      </div>
      <div class="ai-go">
        <select value={takes} title="Each take is a different reading; pick the best in the Project panel" onChange={(e) => setTakes(parseInt((e.target as HTMLSelectElement).value))}>
          <option value={1}>1 take</option>
          <option value={2}>2 takes</option>
          <option value={3}>3 takes</option>
        </select>
        <button class="btn primary" disabled={!text.trim() || !voice || sending} onClick={send} title="Ctrl+Enter">
          {sending ? "Sending…" : "Generate speech"}
        </button>
      </div>
      <div class="ai-faint ai-desc">Clips go into the Project panel tagged “voice”, with the script as their note.</div>
    </Section>
  );
}

/** Clone: a project clip (its Source monitor In/Out) or a file, with the speaker's consent. */
function Clone({ st, onSent }: { st: Status; onSent: () => void }) {
  const audible = (project.value?.media || []).filter((m) => m.has_audio);
  const [name, setName] = useState("");
  const [from, setFrom] = useState<"clip" | "file">("clip");
  const [media, setMedia] = useState("");
  const [file, setFile] = useState("");
  const [start, setStart] = useState("");
  const [end, setEnd] = useState("");
  const [denoise, setDenoise] = useState(false);
  const [consent, setConsent] = useState(false);
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
  const srcDur = from === "clip" ? mediaById.value.get(media)?.dur || 0 : 0;
  const len = isFinite(e) && e > s ? e - s : srcDur ? Math.min(120, srcDur - s) : null;
  const a = st.account;
  const cannot = a?.can_clone === false;
  const send = async () => {
    if (!name.trim()) return toast("Give the voice a name", true);
    if (from === "clip" && !media) return toast("Pick the clip to clone from", true);
    if (from === "file" && !file) return toast("Pick an audio or video file", true);
    if (!consent) return toast("Confirm you have the speaker's permission", true);
    const r = await api("/api/voice", { action: "clone", name: name.trim(), media: from === "clip" ? media : undefined, path: from === "file" ? file : undefined, start: s, end: isFinite(e) ? e : undefined, denoise, consent });
    if (!r.ok) return toast(r.error, true);
    toast(`Cloning "${name.trim()}"…`);
    setName("");
    setConsent(false);
    onSent();
  };
  return (
    <Section title="Clone a voice" open={!st.voices.some((v) => v.mine)}>
      {cannot && <div class="ai-note err">Your {a?.tier} plan can't clone voices: library voices work, or upgrade on elevenlabs.io.</div>}
      {a?.voice_limit != null && a.voices_used != null && (
        <div class="ai-faint ai-desc">
          {a.voices_used} of {a.voice_limit} voice slots used.
        </div>
      )}
      <div class="row">
        <label>Name</label>
        <div class="ctl">
          <input value={name} placeholder="Narrator" onInput={(ev) => setName((ev.target as HTMLInputElement).value)} onKeyDown={(ev) => ev.stopPropagation()} />
        </div>
        <span />
      </div>
      <div class="row">
        <label>Reference</label>
        <div class="ctl">
          <div class="seg">
            <button class={"btn sm" + (from === "clip" ? " on" : "")} onClick={() => setFrom("clip")}>
              Project clip
            </button>
            <button class={"btn sm" + (from === "file" ? " on" : "")} onClick={() => setFrom("file")}>
              File
            </button>
          </div>
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
            <input value={file} placeholder="C:\\voice.wav" onInput={(ev) => setFile((ev.target as HTMLInputElement).value)} onKeyDown={(ev) => ev.stopPropagation()} />
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
          <input type="number" min="0" step="0.1" value={start} placeholder="0" onInput={(ev) => setStart((ev.target as HTMLInputElement).value)} onKeyDown={(ev) => ev.stopPropagation()} />
          <input type="number" min="0" step="0.1" value={end} placeholder="2 min" onInput={(ev) => setEnd((ev.target as HTMLInputElement).value)} onKeyDown={(ev) => ev.stopPropagation()} />
        </div>
        <span />
      </div>
      <div class="ai-faint ai-desc">
        {len != null ? `${fmtNum(len, 1)} s of reference${len < 30 ? ": more (1-2 minutes) gives a closer likeness" : ""}. ` : "Without an end, up to 2 minutes are used. "}
        One person speaking clearly, without music; mark In and Out in the Source monitor to choose the part.
      </div>
      <label class="ai-check" title="Removes background noise with ElevenLabs' audio isolation. Leave off for clean recordings: it can dull the voice.">
        <input type="checkbox" checked={denoise} onChange={(ev) => setDenoise((ev.target as HTMLInputElement).checked)} />
        Remove background noise (phone and room recordings)
      </label>
      <label class="ai-check consent">
        <input type="checkbox" checked={consent} onChange={(ev) => setConsent((ev.target as HTMLInputElement).checked)} />
        This is my voice, or I have the speaker's permission to clone it.
      </label>
      <div class="ai-go">
        <button class="btn primary" disabled={!consent || cannot} onClick={send}>
          Clone voice
        </button>
      </div>
    </Section>
  );
}
