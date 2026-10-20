import { useState } from "preact/hooks";
import { showPanel } from "../dock";
import { activeId, catalog, graphicEditor, importPaths, monitorTab, pickFiles, playhead, project, proxyState, refreshProxies, run, selection, sourceIn, sourceMedia, sourceOut, sourceT, toast } from "../store";
import type { Media } from "../types";
import { fmtNum } from "../util";

function MediaCard({ m }: { m: Media }) {
  const add = () => run(`add ${m.id} at=${fmtNum(playhead.value, 4)}`);
  return (
    <div
      draggable
      class={"bin-item" + (sourceMedia.value === m.id ? " open" : "")}
      title={`${m.path}${m.note ? "\n" + m.note : ""}${m.tags?.length ? "\n#" + m.tags.join(" #") : ""}\nClick: open in the Source monitor · double-click: add at playhead · drag onto the timeline`}
      onClick={() => {
        if (sourceMedia.value !== m.id) {
          sourceMedia.value = m.id;
          sourceT.value = 0;
          sourceIn.value = null;
          sourceOut.value = null;
        }
        monitorTab.value = "source";
        showPanel("source");
      }}
      onDragStart={(e) => {
        e.dataTransfer!.setData("text/moodcut-media", m.id);
        e.dataTransfer!.effectAllowed = "copy";
      }}
      onDblClick={add}
    >
      <div class={"bin-thumb" + (m.kind === "audio" ? " audio-thumb" : "") + (m.gfx ? " gfx-thumb" : "")}>
        {m.kind !== "audio" && <img src={`/api/thumb?media=${m.id}&w=240`} loading="lazy" decoding="async" alt="" />}
        <span class="kind">{m.gfx ? "html" : m.kind}</span>
        <ProxyBadge id={m.id} />
        {m.gfx && (
          <button class="btn sm edit-gfx" title="Edit the HTML and re-render" onClick={(e) => (e.stopPropagation(), (graphicEditor.value = { id: m.id }))}>
            Edit
          </button>
        )}
        {m.kind === "audio" && <span style={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", fontSize: "26px", opacity: 0.6 }}>♫</span>}
        {m.dur > 0 && <span class="dur">{m.dur >= 60 ? `${Math.floor(m.dur / 60)}:${String(Math.round(m.dur % 60)).padStart(2, "0")}` : `${fmtNum(m.dur, 1)}s`}</span>}
      </div>
      <div class="bin-name">
        <small>{m.id}</small>
        {m.name}
      </div>
    </div>
  );
}

/** "P" when a preview proxy is ready, its progress while it is made. */
function ProxyBadge({ id }: { id: string }) {
  const t = proxyState.value?.media[id];
  if (!t) return null;
  const ready = t === "proxy";
  const label = ready ? "P" : t.includes("%") ? `P ${t.split(" ")[1]}` : t.includes("failed") ? "P !" : "P …";
  return (
    <span class={"proxy-badge" + (ready ? " ready" : "")} title={ready ? "Previews use a fast proxy of this clip (exports use the original)" : t}>
      {label}
    </span>
  );
}

/** Premiere-style proxy actions for the whole library. */
function ProxyMenu() {
  const px = proxyState.value;
  if (!px) return null;
  return (
    <select
      class="proxy-menu"
      value=""
      title={px.summary}
      onChange={(e) => {
        const v = (e.target as HTMLSelectElement).value;
        (e.target as HTMLSelectElement).value = "";
        if (v === "create") refreshProxies({ action: "create" }).then(() => toast("Making proxies in the background"));
        else if (v === "delete") refreshProxies({ action: "delete" });
        else if (v === "cancel") refreshProxies({ action: "cancel" });
        else if (v) refreshProxies({ action: "mode", mode: v });
      }}
    >
      <option value="">Proxies{px.busy ? " …" : ""}</option>
      <option value="create">Make proxies for all video</option>
      {px.busy && <option value="cancel">Stop making proxies</option>}
      <option value="delete">Delete proxies</option>
      <option disabled>── new imports ──</option>
      <option value="auto">{px.mode === "auto" ? "✓ " : ""}Proxies for heavy footage (4K, HEVC…)</option>
      <option value="all">{px.mode === "all" ? "✓ " : ""}Proxies for every video</option>
      <option value="off">{px.mode === "off" ? "✓ " : ""}No proxies</option>
    </select>
  );
}

const PAGE = 240;
const KINDS = ["all", "video", "audio", "image", "html"] as const;

/** Project panel: the media bin and compositions. */
export function ProjectPanel() {
  const p = project.value;
  const [filter, setFilter] = useState("");
  const [kind, setKind] = useState<(typeof KINDS)[number]>("all");
  const [shown, setShown] = useState(PAGE);
  const all = p?.media || [];
  const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
  const media = all.filter((m) => {
    if (kind === "html" ? !m.gfx : kind !== "all" && (m.kind !== kind || m.gfx)) return false;
    const text = `${m.id} ${m.name} ${m.note || ""} ${(m.tags || []).join(" ")}`.toLowerCase();
    return words.every((w) => text.includes(w));
  });
  return (
    <div class="scroll">
      <div style={{ display: "flex", gap: "6px", padding: "8px 8px 0" }}>
        <button class="btn primary" style={{ flex: 1, justifyContent: "center" }} onClick={async () => importPaths(await pickFiles("import"))}>
          ＋ Import media
        </button>
        <button class="btn" title="Import a whole folder" onClick={async () => importPaths(await pickFiles("folder"))}>
          Folder
        </button>
        <button class="btn" title="Design a motion graphic in HTML/CSS/JS" onClick={() => (graphicEditor.value = {})}>
          HTML
        </button>
      </div>
      <input class="search" placeholder="Search names, tags, notes…" value={filter} onInput={(e) => setFilter((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
      {all.length > 0 && (
        <div class="kind-chips">
          {KINDS.map((k) => (
            <button key={k} class={"btn sm" + (kind === k ? " on" : "")} onClick={() => setKind(k)}>
              {k}
            </button>
          ))}
          <span class="count">{media.length === all.length ? `${all.length}` : `${media.length}/${all.length}`}</span>
          {all.some((m) => m.kind === "video") && <ProxyMenu />}
        </div>
      )}
      {media.length === 0 ? (
        <div class="empty">No media yet.<br />Import files, or ask your AI to `media import` a folder.</div>
      ) : (
        <>
          <div class="bin-grid">
            {media.slice(0, shown).map((m) => (
              <MediaCard m={m} key={m.id} />
            ))}
          </div>
          {media.length > shown && (
            <button class="btn sm show-more" onClick={() => setShown(shown + PAGE)}>
              Show {Math.min(PAGE, media.length - shown)} more of {media.length - shown}
            </button>
          )}
        </>
      )}
      <div class="section-title">Compositions</div>
      {p?.comps.map((c) => (
        <div class={"comp-row" + (c.id === activeId.value ? " active" : "")} key={c.id} onClick={() => run(`comp use ${c.id}`, true)} onDblClick={() => run(`add ${c.id} at=${fmtNum(playhead.value, 4)}`)}>
          <span>▤</span>
          <span>
            {c.name} <small style={{ color: "var(--faint)" }}>{c.id}</small>
          </span>
          <span class="badge">
            {c.w}×{c.h}
            {c.id === p.main ? " ★" : ""}
          </span>
        </div>
      ))}
      <div style={{ display: "flex", gap: "6px", padding: "8px" }}>
        <button class="btn sm" onClick={() => run("comp new 'Comp' 1920x1080 30")}>
          + 16:9
        </button>
        <button class="btn sm" onClick={() => run("comp new 'Vertical' 1080x1920 30")}>
          + 9:16
        </button>
        <button class="btn sm" onClick={() => run("reframe 9:16 mode=blur")} title="New vertical comp nesting the current one">
          Reframe 9:16
        </button>
      </div>
    </div>
  );
}

/** Effects & Presets: effects, transitions, animations, titles, looks, audio effects, sound FX, generators. */
export function EffectsPanel() {
  const cat = catalog.value;
  const [q, setQ] = useState("");
  const [sec, setSec] = useState("effects");
  if (!cat) return <div class="empty">Loading…</div>;
  const target = selection.value[selection.value.length - 1];
  const need = (what: string) => {
    if (!target) {
      toast(`Select a clip first to apply ${what}`, true);
      return false;
    }
    return true;
  };
  const match = (s: string) => !q || s.toLowerCase().includes(q.toLowerCase());
  const at = () => fmtNum(playhead.peek(), 4);
  const secs: Record<string, { name: string; desc: string; cat?: string; act: () => void }[]> = {
    effects: cat.effects.map((e) => ({ name: e.name, desc: e.desc, cat: e.cat, act: () => need("an effect") && run(`fx ${selection.value.join(",")} ${e.name}`) })),
    transitions: cat.transitions.map((t) => ({ name: t.name, desc: t.desc, act: () => need("a transition") && run(`tr ${target} ${t.name} 0.6`) })),
    animations: [...cat.anims, ...cat.text_anims.map((a) => ({ ...a, desc: "text: " + a.desc }))].map((a) => ({ name: a.name, desc: a.desc, act: () => need("an animation") && run(`anim ${selection.value.join(",")} ${a.name.split("|")[0]}`) })),
    titles: cat.templates.map((t) => ({ name: t.name, desc: `${t.desc} · ${t.params}`, act: () => run(`title ${t.name} at=${at()}`) })),
    color: cat.colors.map((c) => ({ name: c.name, desc: c.desc, act: () => need("a grade") && run(`color ${selection.value.join(",")} ${c.name}`) })),
    audio: cat.audio_effects.map((e) => ({ name: e.name, desc: e.desc, act: () => need("an audio effect") && run(`afx ${target} ${e.name}`) })),
    "sound fx": cat.sfx.map((x) => ({ name: x.name, desc: `${x.desc} · lands ${x.sync <= 0 ? "on" : x.sync >= 1 ? "ending at" : "peaking at"} the playhead`, act: () => run(`sfx ${x.name} at=${at()}`) })),
    generate: [
      { name: "Text", desc: "Title text at the playhead", act: () => run(`text 'Your text' at=${at()} dur=3 style=title enter=fadeUp exit=fadeDown`) },
      { name: "Rectangle", desc: "Shape layer", act: () => run(`shape rect w=500 h=120 radius=20 fill=#e76c33 at=${at()} dur=3`) },
      { name: "Circle", desc: "Shape layer", act: () => run(`shape ellipse w=240 fill=#ffffff at=${at()} dur=3 enter=popIn`) },
      { name: "Star", desc: "Shape layer", act: () => run(`shape star fill=#fbbf24 at=${at()} dur=3 enter=spinIn`) },
      { name: "Color matte", desc: "Full-frame solid", act: () => run(`solid #111111 at=${at()} dur=3`) },
      { name: "Gradient matte", desc: "Full-frame gradient", act: () => run(`solid #1b1464 color2=#ff5f6d at=${at()} dur=3`) },
      { name: "Adjustment layer", desc: "Effects apply to everything below", act: () => run(`adjust at=${at()} dur=5`) },
      { name: "Null object", desc: "Invisible layer to parent the selection to (Ctrl+Alt+Shift+Y)", act: () => run(`null at=${at()} dur=5${selection.value.length ? ` children=${selection.value.join(",")}` : ""}`) },
      { name: "Camera", desc: "3D camera: layers with z / rx / ry or 3d=1 are seen through it", act: () => run(`camera at=${at()} dur=5`) },
      { name: "Camera push-in", desc: "Slow dolly in; the selection becomes 3D for parallax", act: () => run(`camera at=${at()} dur=5 move=pushin amount=300${selection.value.length ? ` layers=${selection.value.join(",")}` : ""}`) },
      { name: "Camera orbit", desc: "Orbit around the point of interest", act: () => run(`camera at=${at()} dur=5 move=orbit amount=30${selection.value.length ? ` layers=${selection.value.join(",")}` : ""}`) },
    ],
  };
  const list = secs[sec].filter((x) => match(x.name) || match(x.desc));
  let lastCat = "";
  return (
    <div class="scroll">
      <input class="search" placeholder="Search library…" value={q} onInput={(e) => setQ((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
      <div style={{ display: "flex", flexWrap: "wrap", gap: "4px", padding: "0 8px 6px" }}>
        {Object.keys(secs).map((k) => (
          <button key={k} class={"btn sm" + (sec === k ? " on" : "")} onClick={() => setSec(k)}>
            {k}
          </button>
        ))}
      </div>
      {list.map((x) => {
        const head = x.cat && x.cat !== lastCat ? x.cat : null;
        if (x.cat) lastCat = x.cat;
        return (
          <div key={x.name}>
            {head && <div class="section-title">{head}</div>}
            <div class="lib-item" onClick={x.act} title="Click to apply">
              <b>{x.name}</b>
              <span>{x.desc}</span>
            </div>
          </div>
        );
      })}
    </div>
  );
}
