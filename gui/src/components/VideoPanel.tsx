// AI Video: generate shots with Runway (Gen-4.5, Veo 3.1, Kling, Seedance, Gemini Omni) and
// fal.ai (Veo 3.1, Kling, Seedance, Hailuo, Wan, any fal endpoint) on the person's own keys.
// Text to video, or from a first frame (the Program monitor at the playhead, a project still or
// video frame, a file) and for most models a last frame too. Each job shows its stage and
// progress (the provider's percentage, or ~ an estimate from the model's usual time), then the
// clip lands in the Project panel (tag "ai-video") and, if asked, on the timeline. AI apps use
// the same models and jobs through the `video` MCP tool.
import { useEffect, useMemo, useState } from "preact/hooks";
import { showPanel } from "../dock";
import { api } from "../gfx";
import { comp, mediaById, monitorTab, pickFiles, playhead, project, rev, servicesTick, showServices, sourceIn, sourceMedia, sourceOut, sourceT, toast, videoTick } from "../store";
import { fmtNum, fmtTC } from "../util";
import { Section } from "./Controls";
import { AiTask, clock, ConnectCard, ExtLink, Service, TaskList } from "./aiui";

type Model = {
  id: string;
  provider: string;
  label: string;
  maker: string;
  blurb: string;
  t2v: boolean;
  i2v: boolean;
  custom: boolean;
  durs: number[];
  dur_default: number;
  sizes: { aspect: string; res: string }[];
  sound: "param" | "always" | "none";
  negative: boolean;
  seed: boolean;
  last_frame: boolean;
  i2v_auto: boolean;
  max_prompt: number;
  secs: number;
  connected: boolean;
};
type Status = { providers: Service[]; credits: { runway: number | null }; models: Model[]; tasks: AiTask[] };
type Src = { kind: "none" | "frame" | "media" | "file"; media?: string; t?: number; file?: string };

function recall(key: string, fallback: string): string {
  try {
    return localStorage.getItem(key) ?? fallback;
  } catch {
    return fallback;
  }
}
function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {}
}

function openInSource(id: string) {
  sourceMedia.value = id;
  sourceT.value = 0;
  sourceIn.value = null;
  sourceOut.value = null;
  monitorTab.value = "source";
  showPanel("source");
}

const GROUP: Record<string, string> = { runway: "Runway", fal: "fal.ai", higgsfield: "Higgsfield" };
const ASPECT_ICON: Record<string, string> = { "16:9": "▭", "9:16": "▯", "1:1": "□", "21:9": "▬", "4:3": "▭" };

export function VideoPanel() {
  const [st, setSt] = useState<Status | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const load = async (body?: Record<string, unknown>) => {
    const r = await api<Status>("/api/video", body ?? { action: "status" });
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
  }, [videoTick.value, servicesTick.value]);
  const busy = !!st && st.tasks.some((t) => t.state === "queued" || t.state === "running");
  useEffect(() => {
    if (!busy) return;
    const h = setInterval(() => load(), 1000);
    return () => clearInterval(h);
  }, [busy]);

  if (!st)
    return (
      <div class="scroll ai-panel">
        <div class="ai-faint" style={{ padding: "10px" }}>
          {err || "Loading AI Video…"}
        </div>
      </div>
    );
  const connected = st.providers.filter((p) => p.connected);
  return (
    <div class="scroll ai-panel">
      <div class="ai-head">
        <div class="ai-head-t">
          <b>AI Video</b>
          <span class="ai-faint">{connected.length ? connected.map((p) => p.name).join(" · ") : "not connected"}</span>
        </div>
        {st.credits.runway != null && (
          <span class="ai-chip" title="Runway credits left on your account">
            {Math.round(st.credits.runway).toLocaleString("en-US")} credits
          </span>
        )}
        <button class="btn sm ghost" title="AI Services: API keys" onClick={() => (showServices.value = connected[0]?.id || "runway")}>
          ⚙
        </button>
      </div>
      {!connected.length ? (
        <>
          <ConnectCard
            id="runway"
            title="Connect Runway (or fal.ai) to generate video"
            lines={[
              "Runway: Gen-4.5, Veo 3.1, Kling 3 and 4, Seedance 2.5 and Gemini Omni with one key.",
              "Shots from a text prompt, or continuing from the Program monitor's frame or a still.",
              "Each clip costs credits on your account; the key stays encrypted on this computer.",
            ]}
          />
          <div class="ai-faint ai-desc">
            Prefer fal.ai (Veo, Kling, Seedance, Hailuo, Wan) or Higgsfield (Cinema Studio 4, Kling, Seedance)?{" "}
            <button class="btn sm ghost" onClick={() => (showServices.value = "fal")}>
              Add a fal.ai key
            </button>
            <button class="btn sm ghost" onClick={() => (showServices.value = "higgsfield")}>
              Add a Higgsfield key
            </button>
          </div>
        </>
      ) : (
        <>
          <TaskList
            tasks={st.tasks}
            onCancel={(id) => load({ action: "cancel", id })}
            onRetry={(id) => load({ action: "retry", id })}
            onClear={() => load({ action: "clear" })}
            render={(t) =>
              t.result?.media ? (
                <div class="ai-results">
                  <img class="ai-thumb" src={`/api/thumb?media=${t.result.media}&t=0.5&w=160`} alt="" onClick={() => openInSource(t.result.media)} title="Open in the Source monitor" />
                  <button class="btn sm" onClick={() => openInSource(t.result.media)}>
                    ▶ Open
                  </button>
                  {t.result.clip && <span class="ai-faint">on the timeline as {t.result.clip}</span>}
                  {t.result.seed != null && <span class="ai-faint">seed {t.result.seed}</span>}
                </div>
              ) : null
            }
          />
          <Generate st={st} onSent={() => load()} />
        </>
      )}
    </div>
  );
}

/** A still for the model: the Program monitor's frame, a project still or video frame, or a file. */
function FramePick({ label, value, onChange, optional }: { label: string; value: Src; onChange: (s: Src) => void; optional?: boolean }) {
  void rev.value;
  const visual = (project.value?.media || []).filter((m) => m.kind === "image" || m.kind === "video");
  const m = value.media ? mediaById.value.get(value.media) : undefined;
  const thumb =
    value.kind === "frame"
      ? `/api/frame?t=${(value.t ?? playhead.value).toFixed(3)}&w=240&r=${rev.value}`
      : value.kind === "media" && m
        ? `/api/thumb?media=${m.id}&t=${m.kind === "video" ? fmtNum(value.t ?? 0, 2) : 0}&w=240`
        : null;
  return (
    <div class="ai-frame">
      <div class="row">
        <label>{label}</label>
        <div class="ctl">
          <select
            value={value.kind}
            onChange={(e) => {
              const k = (e.target as HTMLSelectElement).value as Src["kind"];
              onChange(k === "frame" ? { kind: "frame", t: undefined } : k === "media" ? { kind: "media", media: visual[0]?.id, t: 0 } : { kind: k });
            }}
          >
            {optional && <option value="none">None</option>}
            <option value="frame">Program monitor at the playhead</option>
            <option value="media">A still or video in the project</option>
            <option value="file">A file</option>
          </select>
        </div>
        <span />
      </div>
      {value.kind === "frame" && (
        <div class="ai-faint ai-desc">
          The frame at {fmtTC(value.t ?? playhead.value, comp.value?.fps || 30)}{" "}
          {value.t == null ? (
            <button class="btn sm ghost" title="Keep this time even if the playhead moves" onClick={() => onChange({ kind: "frame", t: playhead.value })}>
              Lock this time
            </button>
          ) : (
            <button class="btn sm ghost" onClick={() => onChange({ kind: "frame", t: undefined })}>
              Follow the playhead
            </button>
          )}
        </div>
      )}
      {value.kind === "media" && (
        <div class="row">
          <label />
          <div class="ctl">
            <select value={value.media || ""} onChange={(e) => onChange({ kind: "media", media: (e.target as HTMLSelectElement).value, t: 0 })}>
              {!visual.length && <option value="">Import a still or video first</option>}
              {visual.map((x) => (
                <option key={x.id} value={x.id}>
                  {x.id} {x.name}
                </option>
              ))}
            </select>
            {m?.kind === "video" && (
              <input type="number" min={0} max={m.dur} step={0.1} value={value.t ?? 0} title="Seconds into the video" style={{ maxWidth: "70px" }} onInput={(e) => onChange({ ...value, t: parseFloat((e.target as HTMLInputElement).value) || 0 })} onKeyDown={(e) => e.stopPropagation()} />
            )}
          </div>
          <span />
        </div>
      )}
      {value.kind === "file" && (
        <div class="row">
          <label />
          <div class="ctl">
            <input value={value.file || ""} placeholder="C:\\still.png" onInput={(e) => onChange({ kind: "file", file: (e.target as HTMLInputElement).value })} onKeyDown={(e) => e.stopPropagation()} />
            <button
              class="btn sm"
              style={{ flex: "0 0 auto" }}
              onClick={async () => {
                const [f] = await pickFiles("open");
                if (f) onChange({ kind: "file", file: f });
              }}
            >
              Browse…
            </button>
          </div>
          <span />
        </div>
      )}
      {thumb && <img class="ai-frame-thumb" src={thumb} alt="" />}
    </div>
  );
}

/** What the engine takes for a frame: frame / a media id / a path (and its time). */
function srcArgs(s: Src, key: "image" | "end_image"): Record<string, unknown> {
  if (s.kind === "frame") return { [key]: "frame", [`${key}_t`]: s.t ?? playhead.value };
  if (s.kind === "media" && s.media) return { [key]: s.media, [`${key}_t`]: s.t ?? 0 };
  if (s.kind === "file" && s.file) return { [key]: s.file };
  return {};
}

function Generate({ st, onSent }: { st: Status; onSent: () => void }) {
  const usable = st.models.filter((m) => m.connected);
  const [modelId, setModelId] = useState(() => recall("mc.vid.model", ""));
  const m = usable.find((x) => x.id === modelId) || usable[0];
  const [mode, setMode] = useState<"text" | "image">(() => (recall("mc.vid.mode", "text") === "image" ? "image" : "text"));
  const [first, setFirst] = useState<Src>({ kind: "frame" });
  const [last, setLast] = useState<Src>({ kind: "none" });
  const [prompt, setPrompt] = useState("");
  const [negative, setNegative] = useState("");
  const [dur, setDur] = useState<number | null>(null);
  const [aspect, setAspect] = useState(() => recall("mc.vid.aspect", "16:9"));
  const [res, setRes] = useState<string | null>(null);
  const [audio, setAudio] = useState(true);
  const [seed, setSeed] = useState("");
  const [endpoint, setEndpoint] = useState(() => recall("mc.vid.endpoint", ""));
  const [params, setParams] = useState("");
  const [place, setPlace] = useState(true);
  const [track, setTrack] = useState("");
  const [sending, setSending] = useState(false);

  const aspects = useMemo(() => [...new Set((m?.sizes || []).map((s) => s.aspect))], [m]);
  const a = aspects.includes(aspect) ? aspect : aspects[0] || "16:9";
  const resList = useMemo(() => [...new Set((m?.sizes || []).filter((s) => s.aspect === a).map((s) => s.res))], [m, a]);
  const r = res && resList.includes(res) ? res : resList[0] || "";
  const d = dur != null && m?.durs.includes(dur) ? dur : m?.dur_default || 5;
  const image = mode === "image" && !!m?.i2v;
  const followsImage = image && m?.i2v_auto;
  const videoTracks = (comp.value?.tracks || []).filter((t) => t.kind === "video").map((t) => t.id).sort((x, y) => parseInt(y.slice(1)) - parseInt(x.slice(1)));
  if (!m) return <div class="ai-faint ai-desc">No model is available for the connected services.</div>;
  const n = prompt.trim().length;
  const typical = m.secs * Math.min(2.5, Math.max(0.6, Math.sqrt(d / Math.max(1, m.dur_default)))) * (r === "4k" ? 2 : r === "1080p" || r === "2k" ? 1.25 : 1);
  const send = async () => {
    if (!prompt.trim()) return toast("Describe the shot in the prompt", true);
    if (n > m.max_prompt) return toast(`The prompt has ${n} characters; ${m.label} takes up to ${m.max_prompt}`, true);
    let extra: Record<string, unknown> | undefined;
    if (params.trim()) {
      try {
        extra = JSON.parse(params);
      } catch {
        return toast("Advanced parameters must be JSON, like {\"cfg_scale\": 0.6}", true);
      }
    }
    store("mc.vid.model", m.id);
    store("mc.vid.mode", mode);
    store("mc.vid.aspect", a);
    if (m.custom) store("mc.vid.endpoint", endpoint);
    setSending(true);
    const body: Record<string, unknown> = {
      action: "generate",
      model: m.id,
      prompt: prompt.trim(),
      duration: d,
      aspect: m.sizes.length ? a : undefined,
      resolution: r || undefined,
      audio: m.sound === "param" ? audio : undefined,
      negative: m.negative && negative.trim() ? negative.trim() : undefined,
      seed: m.seed && seed.trim() ? parseInt(seed) : undefined,
      endpoint: m.custom ? endpoint.trim() : undefined,
      params: extra,
      at: place ? playhead.value : undefined,
      track: place && track ? track : undefined,
      ...(image ? srcArgs(first, "image") : {}),
      ...(image && m.last_frame && last.kind !== "none" ? srcArgs(last, "end_image") : {}),
    };
    const res2 = await api("/api/video", body);
    setSending(false);
    if (!res2.ok) return toast(res2.error, true);
    toast(`${m.label} is generating: about ${clock(typical)}`);
    onSent();
  };
  const groups = ["runway", "fal", "higgsfield"].map((p) => ({ p, list: usable.filter((x) => x.provider === p) })).filter((g) => g.list.length);
  return (
    <>
      <Section title="Model">
        <div class="row">
          <label>Model</label>
          <div class="ctl">
            <select value={m.id} onChange={(e) => (setModelId((e.target as HTMLSelectElement).value), setDur(null), setRes(null))}>
              {groups.map((g) => (
                <optgroup key={g.p} label={GROUP[g.p] || g.p}>
                  {g.list.map((x) => (
                    <option key={x.id} value={x.id}>
                      {x.label} · {x.maker}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
          </div>
          <span />
        </div>
        <div class="ai-model-card">
          <div>{m.blurb}</div>
          <div class="ai-badges">
            <span class="ai-chip">{m.durs.length > 1 ? `${m.durs[0]}-${m.durs[m.durs.length - 1]} s` : `${m.durs[0]} s`}</span>
            {[...new Set(m.sizes.map((s) => s.res))].map((x) => (
              <span key={x} class="ai-chip">
                {x}
              </span>
            ))}
            <span class={"ai-chip" + (m.sound === "none" ? " dim" : " ok")}>{m.sound === "none" ? "silent" : "sound"}</span>
            {m.i2v && <span class="ai-chip">first frame</span>}
            {m.last_frame && <span class="ai-chip">last frame</span>}
            <span class="ai-chip dim" title="Usual time for a clip like this">≈ {clock(typical)}</span>
          </div>
        </div>
        {m.custom && (
          <div class="row">
            <label>Endpoint</label>
            <div class="ctl">
              <input value={endpoint} placeholder="fal-ai/kling-video/v3/pro/text-to-video" onInput={(e) => setEndpoint((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
            </div>
            <span />
          </div>
        )}
        {m.custom && (
          <div class="ai-faint ai-desc">
            Any video model on <ExtLink href="https://fal.ai/models?categories=text-to-video">fal.ai/models</ExtLink>: its id, then its own settings under Advanced.
          </div>
        )}
      </Section>
      <Section title="Shot">
        <div class="seg ai-mode">
          <button class={"btn sm" + (mode === "text" ? " on" : "")} disabled={!m.t2v} onClick={() => setMode("text")}>
            Text to video
          </button>
          <button class={"btn sm" + (mode === "image" ? " on" : "")} disabled={!m.i2v} title={m.i2v ? "Start from a frame" : `${m.label} makes video from text only`} onClick={() => setMode("image")}>
            Image to video
          </button>
        </div>
        {image && <FramePick label="First frame" value={first} onChange={setFirst} />}
        {image && m.last_frame && <FramePick label="Last frame" value={last} onChange={setLast} optional />}
        <textarea
          class="ai-script"
          rows={5}
          value={prompt}
          placeholder={image ? "Describe the motion: what moves, how the camera moves, what you hear. The frame already shows the scene." : "Subject and action, setting and light, camera move and lens, mood. For models with sound: dialogue in quotes and the ambience."}
          onInput={(e) => setPrompt((e.target as HTMLTextAreaElement).value)}
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) send();
          }}
        />
        <div class="ai-count">
          <span class={n > m.max_prompt ? "ai-warn" : "ai-faint"}>{n ? `${n} / ${m.max_prompt} characters` : ""}</span>
          <span class="ai-faint">Ctrl+Enter generates</span>
        </div>
        {m.negative && (
          <div class="row">
            <label title="What should not appear">Avoid</label>
            <div class="ctl">
              <input value={negative} placeholder="text, logos, blur" onInput={(e) => setNegative((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
            </div>
            <span />
          </div>
        )}
      </Section>
      <Section title="Format">
        {m.durs.length > 1 && (
          <div class="row">
            <label>Duration</label>
            <div class="ctl">
              {m.durs.length <= 6 ? (
                <div class="seg">
                  {m.durs.map((x) => (
                    <button key={x} class={"btn sm" + (x === d ? " on" : "")} onClick={() => setDur(x)}>
                      {x} s
                    </button>
                  ))}
                </div>
              ) : (
                <>
                  <input type="range" min={m.durs[0]} max={m.durs[m.durs.length - 1]} step={1} value={d} onInput={(e) => setDur(parseInt((e.target as HTMLInputElement).value))} onKeyDown={(e) => e.stopPropagation()} />
                  <span class="ai-val">{d} s</span>
                </>
              )}
            </div>
            <span />
          </div>
        )}
        {aspects.length > 0 && !followsImage && (
          <div class="row">
            <label>Aspect</label>
            <div class="ctl">
              <div class="seg">
                {aspects.map((x) => (
                  <button key={x} class={"btn sm" + (x === a ? " on" : "")} title={x === "9:16" ? "Vertical: Reels, TikTok, Shorts" : x === "1:1" ? "Square" : x === "16:9" ? "Widescreen" : x} onClick={() => (setAspect(x), setRes(null))}>
                    <span class="ai-asp">{ASPECT_ICON[x] || "▭"}</span> {x}
                  </button>
                ))}
              </div>
            </div>
            <span />
          </div>
        )}
        {followsImage && <div class="ai-faint ai-desc">The video keeps the first frame's framing.</div>}
        {resList.length > 1 && (
          <div class="row">
            <label>Resolution</label>
            <div class="ctl">
              <div class="seg">
                {resList.map((x) => (
                  <button key={x} class={"btn sm" + (x === r ? " on" : "")} onClick={() => setRes(x)}>
                    {x}
                  </button>
                ))}
              </div>
            </div>
            <span />
          </div>
        )}
        {m.sound === "param" && (
          <label class="ai-check" title="Native sound: dialogue, effects and ambience from the prompt (costs more on some models)">
            <input type="checkbox" checked={audio} onChange={(e) => setAudio((e.target as HTMLInputElement).checked)} />
            Generate sound
          </label>
        )}
        {m.sound === "always" && <div class="ai-faint ai-desc">This model always makes sound.</div>}
        <details class="ai-adv">
          <summary>Advanced</summary>
          {m.seed && (
            <div class="row">
              <label title="The same seed and prompt give a similar take">Seed</label>
              <div class="ctl">
                <input value={seed} placeholder="random" inputMode="numeric" onInput={(e) => setSeed((e.target as HTMLInputElement).value.replace(/\D/g, ""))} onKeyDown={(e) => e.stopPropagation()} />
              </div>
              <span />
            </div>
          )}
          <div class="ai-faint ai-desc">Extra fields for the provider's API as JSON (they override MoodCut's), e.g. {m.provider === "runway" ? '{"contentModeration": {"publicFigureThreshold": "low"}}' : '{"cfg_scale": 0.6}'}</div>
          <textarea rows={2} value={params} placeholder="{}" onInput={(e) => setParams((e.target as HTMLTextAreaElement).value)} onKeyDown={(e) => e.stopPropagation()} />
        </details>
      </Section>
      <div class="ai-place">
        <label class="ai-check">
          <input type="checkbox" checked={place} onChange={(e) => setPlace((e.target as HTMLInputElement).checked)} />
          Add to the timeline at {fmtTC(playhead.value, comp.value?.fps || 30)}
        </label>
        {place && (
          <select value={track} title="Video track" onChange={(e) => setTrack((e.target as HTMLSelectElement).value)}>
            <option value="">first free track</option>
            {videoTracks.map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </select>
        )}
      </div>
      <div class="ai-go">
        <span class="ai-faint">
          {d} s · {m.sizes.length ? `${followsImage ? "image framing" : a} ${r}` : "custom"} · {m.label}
        </span>
        <button class="btn primary" disabled={!prompt.trim() || sending || (m.custom && !endpoint.trim())} onClick={send} title="Ctrl+Enter">
          {sending ? "Sending…" : "Generate video"}
        </button>
      </div>
      <div class="ai-faint ai-desc">
        Clips cost credits on your {GROUP[m.provider] || m.provider} account and land in the Project panel tagged “ai-video” with the prompt as their note. Jobs keep running if you close MoodCut and come back on the next start.
      </div>
    </>
  );
}
