// The Tools panel (Premiere Pro / After Effects). Timeline tools act in the timeline,
// Type and Shape act in the Program monitor.
import { shapeKind, snapping, tool } from "../store";
import type { Tool } from "../store";

const S = { fill: "none", stroke: "currentColor", "stroke-width": 1.4, "stroke-linecap": "round", "stroke-linejoin": "round" } as const;

const TOOLS: { id: Tool; key: string; name: string; tip: string; icon: any }[] = [
  { id: "select", key: "V", name: "Selection", tip: "Select, move and trim clips; move, scale and rotate layers in the Program monitor", icon: <path d="M4 2.5v10.5l2.8-2.6 2 4.1 1.6-.8-2-4h3.9z" fill="currentColor" /> },
  { id: "track", key: "A", name: "Track Select Forward", tip: "Click a clip to select it and everything after it (Shift: this track only)", icon: <path {...S} d="M2.5 3v10M6 4.5l3.5 3.5L6 11.5M10 4.5l3.5 3.5-3.5 3.5" /> },
  { id: "ripple", key: "B", name: "Ripple Edit", tip: "Drag an edge: trims and shifts everything after it, leaving no gap", icon: <path {...S} d="M6 3H3.5v10H6M6 8h8M11.5 5.5 14 8l-2.5 2.5" /> },
  { id: "roll", key: "N", name: "Rolling Edit", tip: "Drag a cut between two clips: one gets longer, the other shorter", icon: <path {...S} d="M8 2.5v11M2 8h4M10 8h4M4 6 2 8l2 2M12 6l2 2-2 2" /> },
  { id: "rate", key: "R", name: "Rate Stretch", tip: "Drag an edge to change the clip's speed instead of its content", icon: <path {...S} d="M2.5 4v8M13.5 4v8M5 8h6M6.5 6.2 4.8 8l1.7 1.8M9.5 6.2 11.2 8l-1.7 1.8" /> },
  { id: "razor", key: "C", name: "Razor", tip: "Click a clip to cut it (Shift: cut all tracks)", icon: <path {...S} d="M4.5 13.5a1.8 1.8 0 1 0 0-.1M11.5 13.5a1.8 1.8 0 1 0 0-.1M5.6 11.9 11.8 2.5M10.4 11.9 4.2 2.5" /> },
  { id: "slip", key: "Y", name: "Slip", tip: "Drag inside a clip: change which part of the source plays, same position and length", icon: <path {...S} d="M2 5h12v6H2zM5 8h6M6.5 6.8 5 8l1.5 1.2M9.5 6.8 11 8 9.5 9.2" /> },
  { id: "slide", key: "U", name: "Slide", tip: "Drag a clip between its neighbours: they are trimmed to keep the cut lengths", icon: <path {...S} d="M5.5 5h5v6h-5zM1.5 8h2.5M12 8h2.5M3 6.5 1.5 8 3 9.5M13 6.5l1.5 1.5L13 9.5" /> },
  { id: "hand", key: "H", name: "Hand", tip: "Drag to scroll the timeline (middle mouse works with any tool)", icon: <path {...S} d="M5.5 8.5V4a1 1 0 0 1 2 0v3.5M7.5 7V3a1 1 0 0 1 2 0v4M9.5 7V3.8a1 1 0 0 1 2 0V9c0 2.8-1.7 4.5-4 4.5-1.6 0-2.6-.7-3.5-2L2.6 9.3a1 1 0 0 1 1.6-1.2l1.3 1.5" /> },
  { id: "zoom", key: "Z", name: "Zoom", tip: "Click the timeline to zoom in (Alt: out)", icon: <path {...S} d="M7 2.5a4.5 4.5 0 1 0 0 9 4.5 4.5 0 1 0 0-9M10.3 10.3 14 14M5 7h4M7 5v4" /> },
  { id: "type", key: "T", name: "Type", tip: "Click in the Program monitor to add a title there", icon: <path {...S} d="M3 3.5h10M8 3.5V13M6 13h4" /> },
  { id: "shape", key: "G", name: "Shape / Mask", tip: "Drag in the Program monitor to draw a shape layer (Alt: a mask on the selected layer)", icon: <path {...S} d="M2.5 4.5h7v7h-7zM11.5 6.5a2.6 2.6 0 1 1 0 5.2" /> },
];

export function ToolsPanel() {
  return (
    <div class="tools" role="toolbar" aria-label="Tools">
      {TOOLS.map((t) => (
        <button key={t.id} class={"tool" + (tool.value === t.id ? " on" : "")} title={`${t.name} (${t.key})\n${t.tip}`} aria-label={t.name} onClick={() => (tool.value = t.id)}>
          <svg viewBox="0 0 16 16" width="16" height="16">
            {t.icon}
          </svg>
        </button>
      ))}
      {tool.value === "shape" &&
        (["rect", "ellipse"] as const).map((k) => (
          <button key={k} class={"tool opt" + (shapeKind.value === k ? " on" : "")} title={k === "rect" ? "Rectangle" : "Ellipse"} onClick={() => (shapeKind.value = k)}>
            {k === "rect" ? "▭" : "◯"}
          </button>
        ))}
      <button class={"tool" + (snapping.value ? " on" : "")} title={`Snap in timeline (S): ${snapping.value ? "on" : "off"}`} aria-label="Snapping" onClick={() => (snapping.value = !snapping.value)}>
        <svg viewBox="0 0 16 16" width="16" height="16">
          <path {...S} d="M4 2.5v5a4 4 0 0 0 8 0v-5M4 5h2.5M9.5 5H12M6.5 2.5v5a1.5 1.5 0 0 0 3 0v-5" />
        </svg>
      </button>
    </div>
  );
}

export const toolName = (t: Tool) => TOOLS.find((x) => x.id === t)?.name || t;
