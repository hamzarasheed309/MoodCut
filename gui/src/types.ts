// Mirrors the Rust project model (crates/moodcut/src/model/project.rs).

export type Val = number | [number, number] | string | boolean;
export type Key = [number, Val, string?];
export type AnimObj = { v?: Val; k?: Key[]; x?: string; loop?: string };
export type Anim = Val | AnimObj;
export type Props = Record<string, Anim>;

export interface Effect {
  id: string;
  kind: string;
  on?: boolean;
  props?: Props;
}

export interface Transition {
  kind: string;
  dur: number;
  props?: Record<string, Val>;
}

export type ItemKind = "clip" | "text" | "shape" | "solid" | "adjust" | "comp" | "null" | "camera";

export interface Item {
  id: string;
  kind: ItemKind;
  name?: string;
  src?: string;
  start: number;
  dur: number;
  in?: number;
  text?: string;
  props?: Props;
  fx?: Effect[];
  afx?: Effect[];
  masks?: Effect[];
  shapes?: Effect[];
  anims?: Effect[];
  tin?: Transition;
  tout?: Transition;
  parent?: string;
  matte?: { src: string; mode: string };
  off?: boolean;
}

export interface Track {
  id: string;
  kind: "video" | "audio";
  name?: string;
  items: Item[];
  hidden?: boolean;
  locked?: boolean;
  muted?: boolean;
  solo?: boolean;
  vol?: number;
  pan?: number;
  duck?: { by: string; amount: number };
  afx?: Effect[];
}

export interface Marker {
  t: number;
  name: string;
  color?: string;
  dur?: number;
  note?: string;
}

export interface Comp {
  id: string;
  name: string;
  w: number;
  h: number;
  fps: number;
  dur?: number;
  bg: string;
  tracks: Track[];
  markers?: Marker[];
  master?: { vol?: number; lufs?: number; limiter?: boolean; afx?: Effect[] };
  /** work area (sequence in/out), seconds */
  work?: [number, number];
}

export interface Media {
  id: string;
  path: string;
  name: string;
  kind: "video" | "audio" | "image" | "svg";
  dur: number;
  w: number;
  h: number;
  fps: number;
  has_video: boolean;
  has_audio: boolean;
  alpha?: boolean;
  gfx?: { html: string; w: number; h: number; fps: number; dur: number; alpha: boolean; vars?: Record<string, string> };
  tags?: string[];
  note?: string;
}

export interface Project {
  v: number;
  name: string;
  main: string;
  media: Media[];
  comps: Comp[];
  brand?: Brand;
}

export interface PDef {
  name: string;
  type: "num" | "vec2" | "color" | "str" | "bool" | string[];
  def: Val;
  min: number | null;
  max: number | null;
  anim: boolean;
  desc: string;
}

export interface FxDef {
  name: string;
  cat?: string;
  desc: string;
  ff?: boolean;
  params: PDef[];
}

export interface Catalog {
  effects: FxDef[];
  audio_effects: FxDef[];
  transitions: FxDef[];
  anims: { name: string; desc: string }[];
  text_anims: { name: string; desc: string }[];
  templates: { name: string; params: string; desc: string }[];
  presets: { name: string; desc: string }[];
  colors: { name: string; desc: string }[];
  props: Record<string, PDef[]>;
  shape_kinds: { name: string; params: PDef[] }[];
  scopes: string[];
  sfx: { name: string; desc: string; dur: number; sync: number; vol: number }[];
  brands: { name: string; use: string; font: string; weight: string; primary: string; accent: string; text: string; dark: string }[];
}

export interface Brand {
  font?: string;
  weight?: string;
  primary?: string;
  accent?: string;
  text?: string;
  dark?: string;
}

/** On-screen layer geometry from /api/layer (comp px). */
export interface LayerGeom {
  quad: [number, number][];
  anchor: [number, number];
}

export interface LogEntry {
  rev: number;
  source: string;
  script: string;
  result: string;
  ok: boolean;
}

export interface Job {
  id: string;
  state: "running" | "done" | "error";
  pct?: number;
  frames?: string;
  eta_s?: number;
  preset: string;
  out: string;
  error?: string;
  bytes?: number;
  secs?: number;
  render_fps?: number;
  /** set on caption / chapter / XML / EDL exports */
  note?: string;
}
