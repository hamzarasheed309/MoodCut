// The menu bar, laid out like Premiere Pro's (File, Edit, Clip, Sequence, Markers, Graphics, View,
// Window, Help, plus AI): every command with its shortcut, so features are found by browsing
// and the keys are learnt by reading. Click a title to open it; while one is open, hovering
// another switches to it; Escape or a click elsewhere closes it.
import { signal } from "@preact/signals";
import { useEffect, useRef, useState } from "preact/hooks";
import { easeSelected, fitTimeline, gotoEdit, gotoMarker, liftOrExtract, markWork, moveToTrack, pasteClipboard, rippleTrimTo, splitAtPlayhead, toggleEnabled, copySelection } from "../actions";
import { ALL_PANELS, closePanel, isOpen, maximized, PANEL_TITLES, resetWorkspace, setLayout, showPanel, switchWorkspace, workspace, WORKSPACES } from "../dock";
import { comp, confirmLeave, graphicEditor, importPaths, pickFiles, playhead, project_, run, saveProject, selection, showCheck, showRender, showServices, snapping, tool, zoom } from "../store";
import { fmtNum } from "../util";
import { showHome } from "./Home";

export const showShortcuts = signal(false);

type Item = { label: string; kbd?: string; act?: () => void; check?: boolean; disabled?: boolean; hint?: string } | "sep" | { head: string };

const T = (t: number) => fmtNum(t, 4);
const sel = () => selection.value;
const needSel = (f: (ids: string[]) => void) => () => (sel().length ? f(sel()) : undefined);

function menus(onConnect: () => void): { name: string; items: () => Item[] }[] {
  return [
    {
      name: "File",
      items: () => [
        {
          label: "New Project…",
          kbd: "Ctrl+Alt+N",
          act: () => {
            if (!confirmLeave()) return;
            const name = prompt("Project name", "Untitled");
            if (name) project_({ action: "new", name, size: "1920x1080", fps: 30 });
          },
        },
        {
          label: "Open Project…",
          kbd: "Ctrl+O",
          act: async () => {
            const f = await pickFiles("open", "project");
            if (f[0] && confirmLeave()) project_({ action: "open", path: f[0] });
          },
        },
        { label: "Home: recent projects and setup", act: () => (showHome.value = true) },
        "sep",
        { label: "Save", kbd: "Ctrl+S", act: () => saveProject() },
        { label: "Save As…", kbd: "Ctrl+Shift+S", act: () => saveProject(true) },
        "sep",
        {
          label: "Import…",
          kbd: "Ctrl+I",
          act: async () => {
            const f = await pickFiles("import");
            if (f.length) importPaths(f);
          },
        },
        { label: "Export Media…", kbd: "Ctrl+M", act: () => (showRender.value = true) },
        "sep",
        { label: "Connect an AI app (Claude, Cursor…)…", act: onConnect },
      ],
    },
    {
      name: "Edit",
      items: () => [
        { label: "Undo", kbd: "Ctrl+Z", act: () => run("undo", true) },
        { label: "Redo", kbd: "Ctrl+Shift+Z", act: () => run("redo", true) },
        "sep",
        { label: "Copy", kbd: "Ctrl+C", disabled: !sel().length, act: copySelection },
        { label: "Paste", kbd: "Ctrl+V", act: () => pasteClipboard(false) },
        { label: "Paste Attributes", kbd: "Ctrl+Alt+V", act: () => pasteClipboard(true) },
        { label: "Duplicate", kbd: "Ctrl+D", disabled: !sel().length, act: needSel((s) => run(`dup ${s[s.length - 1]}`)) },
        { label: "Clear", kbd: "Delete", disabled: !sel().length, act: needSel((s) => run(`del ${s.join(",")}`, true)) },
        { label: "Ripple Delete", kbd: "Shift+Delete", disabled: !sel().length, act: needSel((s) => run(`del ${s.join(",")} ripple`, true)) },
        "sep",
        { label: "Select All", kbd: "Ctrl+A", act: () => (selection.value = (comp.value?.tracks || []).flatMap((t) => t.items.map((i) => i.id))) },
        { label: "Deselect All", kbd: "Esc", act: () => (selection.value = []) },
        "sep",
        { label: "AI Services (API keys)…", act: () => (showServices.value = "") },
      ],
    },
    {
      name: "Clip",
      items: () => [
        { label: "Enable", kbd: "Shift+E", disabled: !sel().length, act: toggleEnabled },
        { label: "Nest…", kbd: "Ctrl+Shift+C", disabled: !sel().length, act: needSel((s) => run(`precomp ${s.join(",")} name='Nested'`)) },
        "sep",
        { head: "Keyframe easing" },
        { label: "Easy Ease", kbd: "F9", act: () => easeSelected("easy") },
        { label: "Ease In", kbd: "Shift+F9", act: () => easeSelected("easeIn") },
        { label: "Ease Out", kbd: "Ctrl+Shift+F9", act: () => easeSelected("easeOut") },
        "sep",
        { label: "Move to Track Above", kbd: "Alt+↑", disabled: !sel().length, act: () => moveToTrack(1) },
        { label: "Move to Track Below", kbd: "Alt+↓", disabled: !sel().length, act: () => moveToTrack(-1) },
        "sep",
        { label: "Effect Controls", act: () => showPanel("controls") },
        { label: "Lumetri Color", act: () => showPanel("lumetri") },
        { label: "Essential Sound", act: () => showPanel("essential") },
      ],
    },
    {
      name: "Sequence",
      items: () => [
        { label: "Add Edit", kbd: "Ctrl+K", act: splitAtPlayhead },
        { label: "Ripple Trim Previous Edit to Playhead", kbd: "Q", act: () => rippleTrimTo("start") },
        { label: "Ripple Trim Next Edit to Playhead", kbd: "W", act: () => rippleTrimTo("end") },
        "sep",
        { label: "Mark In", kbd: "I", act: () => markWork("in") },
        { label: "Mark Out", kbd: "O", act: () => markWork("out") },
        { label: "Clear In and Out", kbd: "Ctrl+Shift+X", act: () => run("workarea clear", true) },
        { label: "Lift", kbd: ";", act: () => liftOrExtract("lift") },
        { label: "Extract", kbd: "'", act: () => liftOrExtract("extract") },
        "sep",
        { label: "Go to Next Edit Point", kbd: "↓", act: () => gotoEdit(1) },
        { label: "Go to Previous Edit Point", kbd: "↑", act: () => gotoEdit(-1) },
        "sep",
        { label: "Add Video Track", act: () => run("track add V", true) },
        { label: "Add Audio Track", act: () => run("track add A", true) },
        "sep",
        { label: "Snap in Timeline", kbd: "S", check: snapping.value, act: () => (snapping.value = !snapping.value) },
        { label: "Zoom In", kbd: "=", act: () => (zoom.value = Math.min(3000, zoom.value * 1.25)) },
        { label: "Zoom Out", kbd: "-", act: () => (zoom.value = Math.max(2, zoom.value / 1.25)) },
        { label: "Zoom to Sequence", kbd: "\\", act: fitTimeline },
        "sep",
        { label: "Check the Edit…", hint: "gaps, title-safe, contrast, loudness", act: () => (showCheck.value = true) },
      ],
    },
    {
      name: "Markers",
      items: () => [
        { label: "Add Marker", kbd: "M", act: () => run(`marker add ${T(playhead.value)}`, true) },
        { label: "Go to Next Marker", kbd: "Shift+M", act: () => gotoMarker(1) },
        { label: "Go to Previous Marker", kbd: "Ctrl+Shift+M", act: () => gotoMarker(-1) },
        "sep",
        { label: "Markers Panel", act: () => showPanel("markers") },
      ],
    },
    {
      name: "Graphics",
      items: () => [
        { label: "New Text Layer", kbd: "T", hint: "click in the Program monitor", act: () => (tool.value = "type") },
        { label: "New Shape Layer", kbd: "G", hint: "drag in the Program monitor", act: () => (tool.value = "shape") },
        { label: "New HTML Motion Graphic…", act: () => (graphicEditor.value = {}) },
        "sep",
        { label: "Character", act: () => showPanel("character") },
        { label: "Align", act: () => showPanel("align") },
        { label: "Brand Kit", act: () => showPanel("brand") },
        { label: "Tracker", act: () => showPanel("tracker") },
      ],
    },
    {
      name: "AI",
      items: () => [
        { label: "AI Video: generate a shot…", act: () => showPanel("aivideo") },
        { label: "AI Voice: speech and voice cloning…", act: () => showPanel("voice") },
        { label: "AI Console and Script", act: () => showPanel("ai") },
        "sep",
        { label: "AI Services (API keys)…", act: () => (showServices.value = "") },
        { label: "Connect an AI app (Claude, Cursor…)…", act: onConnect },
        "sep",
        { label: "AI Workspace", act: () => switchWorkspace("ai") },
      ],
    },
    {
      name: "View",
      items: () => [
        { label: "Maximize Panel Under Pointer", kbd: "`", check: !!maximized.value, act: () => (maximized.value = null) },
        { label: "Program Monitor", act: () => showPanel("program") },
        { label: "Source Monitor", act: () => showPanel("source") },
        { label: "Lumetri Scopes", act: () => showPanel("scopes") },
        { label: "Audio Meters", act: () => showPanel("meters") },
        "sep",
        { label: "Zoom to Sequence", kbd: "\\", act: fitTimeline },
      ],
    },
    {
      name: "Window",
      items: () => [
        { head: "Workspaces" },
        ...WORKSPACES.map((w): Item => ({ label: w.name, check: workspace.value === w.id, hint: w.desc, act: () => switchWorkspace(w.id) })),
        { label: "Reset to Saved Layout", act: resetWorkspace },
        "sep",
        { head: "Panels" },
        ...ALL_PANELS.slice()
          .sort((a, b) => PANEL_TITLES[a].localeCompare(PANEL_TITLES[b]))
          .map((p): Item => {
            const on = isOpen(p);
            return { label: PANEL_TITLES[p], check: on, act: () => (on ? setLayout((l) => closePanel(l, p)) : showPanel(p)) };
          }),
      ],
    },
    {
      name: "Help",
      items: () => [
        { label: "Keyboard Shortcuts", kbd: "F1", act: () => (showShortcuts.value = true) },
        { label: "MoodCut Home", act: () => (showHome.value = true) },
        "sep",
        { label: "AI apps: help('workflow') over MCP", disabled: true },
      ],
    },
  ];
}

export function MenuBar({ onConnect }: { onConnect: () => void }) {
  const [open, setOpen] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const off = (e: PointerEvent) => !ref.current?.contains(e.target as Node) && setOpen(null);
    const key = (e: KeyboardEvent) => e.key === "Escape" && (e.stopPropagation(), setOpen(null));
    window.addEventListener("pointerdown", off, true);
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("pointerdown", off, true);
      window.removeEventListener("keydown", key, true);
    };
  }, [open]);
  useEffect(() => {
    const f1 = (e: KeyboardEvent) => {
      if (e.key === "F1") {
        e.preventDefault();
        showShortcuts.value = !showShortcuts.value;
      }
    };
    window.addEventListener("keydown", f1);
    return () => window.removeEventListener("keydown", f1);
  }, []);
  const all = menus(onConnect);
  return (
    <nav class="menubar" ref={ref} aria-label="Menu bar">
      {all.map((m) => (
        <div key={m.name} class="menubar-item">
          <button
            class={"menubar-title" + (open === m.name ? " on" : "")}
            aria-haspopup="menu"
            aria-expanded={open === m.name}
            onPointerDown={(e) => {
              e.preventDefault();
              setOpen(open === m.name ? null : m.name);
            }}
            onPointerEnter={() => open && open !== m.name && setOpen(m.name)}
          >
            {m.name}
          </button>
          {open === m.name && (
            <div class="menu-pop menubar-pop" role="menu">
              {m.items().map((it, i) =>
                it === "sep" ? (
                  <div key={i} class="menu-sep" />
                ) : "head" in it ? (
                  <div key={i} class="menu-head">
                    {it.head}
                  </div>
                ) : (
                  <div
                    key={i}
                    role="menuitem"
                    class={"menu-item" + (it.disabled ? " disabled" : "")}
                    title={it.hint}
                    onClick={() => {
                      if (it.disabled) return;
                      setOpen(null);
                      it.act?.();
                    }}
                  >
                    <span class="check">{it.check ? "✓" : ""}</span>
                    <span class="menu-label">{it.label}</span>
                    {it.kbd && <span class="kbd">{it.kbd}</span>}
                  </div>
                ),
              )}
            </div>
          )}
        </div>
      ))}
    </nav>
  );
}

const SHORTCUTS: [string, [string, string][]][] = [
  ["Playback", [["Space", "Play / stop"], ["← / →", "One frame back / forward (Shift: 5)"], ["↑ / ↓", "Previous / next edit point"], ["Home / End", "Start / end of the sequence"], ["Shift+I / Shift+O", "Go to In / Out"]]],
  ["Editing", [["Ctrl+K", "Add edit (split) at the playhead"], ["Q / W", "Ripple trim previous / next edit to the playhead"], ["Delete / Shift+Delete", "Clear / ripple delete"], [", / .", "Insert / overwrite from the Source monitor"], ["; / '", "Lift / extract the In-Out range"], ["Alt+← / →", "Nudge clips one frame (Shift: 5)"], ["Alt+↑ / ↓", "Move clips to the track above / below"], ["Ctrl+C / Ctrl+V", "Copy / paste at the playhead"], ["Ctrl+Alt+V", "Paste attributes"], ["Ctrl+D", "Duplicate"], ["Shift+E", "Enable / disable clips"]]],
  ["Marking", [["I / O", "Mark In / Out"], ["X", "Mark the selected clip"], ["Ctrl+Shift+X", "Clear In and Out"], ["M", "Add marker"], ["Shift+M / Ctrl+Shift+M", "Next / previous marker"]]],
  ["Tools", [["V", "Selection"], ["A", "Track select forward"], ["B", "Ripple edit"], ["N", "Rolling edit"], ["R", "Rate stretch"], ["C", "Razor"], ["Y / U", "Slip / slide"], ["H / Z", "Hand / zoom"], ["T / G / P", "Type / shape / mask pen"], ["S", "Snapping on / off"]]],
  ["Keyframes", [["J / K", "Previous / next keyframe"], ["F9", "Easy ease (Shift: ease in, Ctrl+Shift: ease out)"], ["[ / ]", "Move the layer's in / out point to the playhead"], ["Alt+[ / ]", "Trim the layer's in / out point to the playhead"]]],
  ["View", [["= / -", "Zoom the timeline in / out"], ["\\", "Zoom to the whole sequence"], ["`", "Maximize the panel with focus"], ["Ctrl+wheel / Alt+wheel", "Zoom the timeline at the pointer"], ["Shift+wheel", "Scroll the timeline sideways"]]],
  ["Values (hot text)", [["Drag a blue value", "Scrub it (Shift: 10× faster, Alt: finer)"], ["Click it", "Type a value; Enter applies, Esc cancels"], ["↑ / ↓ on a value", "Step it (Shift: 10×)"]]],
  ["Project", [["Ctrl+S / Ctrl+Shift+S", "Save / Save As"], ["Ctrl+I", "Import"], ["Ctrl+M", "Export media"], ["Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"], ["F1", "This list"]]],
];

export function ShortcutsModal() {
  if (!showShortcuts.value) return null;
  const close = () => (showShortcuts.value = false);
  return (
    <div class="modal-bg" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div class="modal shortcuts" role="dialog" aria-label="Keyboard shortcuts">
        <div class="modal-h">
          Keyboard Shortcuts
          <span class="x" onClick={close}>
            ✕
          </span>
        </div>
        <div class="modal-b shortcuts-grid">
          {SHORTCUTS.map(([group, list]) => (
            <div key={group} class="shortcuts-group">
              <div class="menu-head">{group}</div>
              {list.map(([k, d]) => (
                <div key={k} class="shortcut">
                  <span class="kbd">{k}</span>
                  <span>{d}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
