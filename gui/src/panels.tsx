// Panel registry: what each dock panel renders.
import type { FunctionComponent } from "preact";
import { AudioMixer, EssentialSound } from "./components/AudioPanel";
import { BrandPanel } from "./components/BrandPanel";
import { ColorPanel } from "./components/ColorPanel";
import { Console } from "./components/Console";
import { HistoryPanel } from "./components/HistoryPanel";
import { AlignPanel, CharacterPanel, Inspector } from "./components/Inspector";
import { EffectsPanel, ProjectPanel } from "./components/LeftPanel";
import { MarkersPanel } from "./components/MarkersPanel";
import { ProgramMonitor, ScopesPanel, SourceMonitor } from "./components/Monitor";
import { TimelinePanel } from "./components/TimelinePanel";
import { ToolsPanel } from "./components/Tools";
import type { PanelId } from "./dock";

export const PANELS: Record<PanelId, FunctionComponent> = {
  project: ProjectPanel,
  effects: EffectsPanel,
  source: SourceMonitor,
  program: ProgramMonitor,
  scopes: ScopesPanel,
  controls: Inspector,
  lumetri: ColorPanel,
  essential: EssentialSound,
  mixer: AudioMixer,
  markers: MarkersPanel,
  history: HistoryPanel,
  ai: Console,
  timeline: TimelinePanel,
  tools: ToolsPanel,
  align: AlignPanel,
  character: CharacterPanel,
  brand: BrandPanel,
};
