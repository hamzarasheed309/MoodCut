import { useState } from "preact/hooks";
import { activeId, catalog, graphicEditor, importPaths, pickFiles, playhead, project, run, selection, toast } from "../store";
import type { Media } from "../types";
import { fmtNum } from "../util";

function MediaCard({ m }: { m: Media }) {
  const thumb = m.kind !== "audio" ? `url(/api/thumb?media=${m.id}&w=240)` : undefined;
  const add = () => run(`add ${m.id} at=${fmtNum(playhead.value, 4)}`);
  return (
    <div
      class="bin-item"
      draggable
      title={`${m.path}\nDouble-click: add at playhead · drag onto the timeline`}
      onDragStart={(e) => {
        e.dataTransfer!.setData("text/moodcut-media", m.id);
        e.dataTransfer!.effectAllowed = "copy";
      }}
      onDblClick={add}
    >
      <div class={"bin-thumb" + (m.kind === "audio" ? " audio-thumb" : "") + (m.gfx ? " gfx-thumb" : "")} style={{ backgroundImage: thumb }}>
        <span class="kind">{m.gfx ? "html" : m.kind}</span>
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

function ProjectTab() {
  const p = project.value;
  const [filter, setFilter] = useState("");
  const media = (p?.media || []).filter((m) => !filter || m.name.toLowerCase().includes(filter.toLowerCase()));
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
      <input class="search" placeholder="Search media…" value={filter} onInput={(e) => setFilter((e.target as HTMLInputElement).value)} onKeyDown={(e) => e.stopPropagation()} />
      {media.length === 0 ? (
        <div class="empty">No media yet.<br />Import files, or ask your AI to `media import` a folder.</div>
      ) : (
        <div class="bin-grid">
          {media.map((m) => (
            <MediaCard m={m} key={m.id} />
          ))}
        </div>
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

function LibraryTab() {
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
  const at = fmtNum(playhead.value, 4);
  const secs: Record<string, { name: string; desc: string; cat?: string; act: () => void }[]> = {
    effects: cat.effects.map((e) => ({ name: e.name, desc: e.desc, cat: e.cat, act: () => need("an effect") && run(`fx ${selection.value.join(",")} ${e.name}`) })),
    transitions: cat.transitions.map((t) => ({ name: t.name, desc: t.desc, act: () => need("a transition") && run(`tr ${target} ${t.name} 0.6`) })),
    animations: [...cat.anims, ...cat.text_anims.map((a) => ({ ...a, desc: "text: " + a.desc }))].map((a) => ({ name: a.name, desc: a.desc, act: () => need("an animation") && run(`anim ${selection.value.join(",")} ${a.name.split("|")[0]}`) })),
    titles: cat.templates.map((t) => ({ name: t.name, desc: `${t.desc} · ${t.params}`, act: () => run(`title ${t.name} at=${at}`) })),
    color: cat.colors.map((c) => ({ name: c.name, desc: c.desc, act: () => need("a grade") && run(`color ${selection.value.join(",")} ${c.name}`) })),
    audio: cat.audio_effects.map((e) => ({ name: e.name, desc: e.desc, act: () => need("an audio effect") && run(`afx ${target} ${e.name}`) })),
    generate: [
      { name: "Text", desc: "Title text at the playhead", act: () => run(`text 'Your text' at=${at} dur=3 style=title enter=fadeUp exit=fadeDown`) },
      { name: "Rectangle", desc: "Shape layer", act: () => run(`shape rect w=500 h=120 radius=20 fill=#8b7cf6 at=${at} dur=3`) },
      { name: "Circle", desc: "Shape layer", act: () => run(`shape ellipse w=240 fill=#ec4899 at=${at} dur=3 enter=popIn`) },
      { name: "Star", desc: "Shape layer", act: () => run(`shape star fill=#fbbf24 at=${at} dur=3 enter=spinIn`) },
      { name: "Color matte", desc: "Full-frame solid", act: () => run(`solid #111111 at=${at} dur=3`) },
      { name: "Gradient matte", desc: "Full-frame gradient", act: () => run(`solid #1b1464 color2=#ff5f6d at=${at} dur=3`) },
      { name: "Adjustment layer", desc: "Effects apply to everything below", act: () => run(`adjust at=${at} dur=5`) },
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

export function LeftPanel() {
  const [tab, setTab] = useState<"project" | "library">("project");
  return (
    <div class="panel left">
      <div class="tabs">
        <div class={"tab" + (tab === "project" ? " active" : "")} onClick={() => setTab("project")}>
          Project
        </div>
        <div class={"tab" + (tab === "library" ? " active" : "")} onClick={() => setTab("library")}>
          Library
        </div>
      </div>
      {tab === "project" ? <ProjectTab /> : <LibraryTab />}
    </div>
  );
}
