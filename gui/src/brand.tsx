// The MoodCut logo: four interlocking reels, ember and white on black (see crates/moodcut/src/brand.rs).

const REELS: [number, number, string][] = [
  [648.5, 608.5, "#E76C33"],
  [1129.5, 608.5, "#FFFFFF"],
  [648.5, 1169.5, "#FFFFFF"],
  [1129.5, 1169.5, "#E76C33"],
];

/** The mark. `bold` thickens rings and hubs for sizes up to ~32 px. */
export function LogoMark({ size = 24, bold = size <= 32, class: cls }: { size?: number; bold?: boolean; class?: string }) {
  const stroke = bold ? 118 : 80;
  const hub = bold ? 118 : 100;
  return (
    <svg class={cls} width={size} height={size} viewBox="0 0 1778 1778" fill="none" aria-hidden="true">
      <rect width="1778" height="1778" rx="889" fill="#010101" />
      {REELS.map(([x, y, c]) => (
        <circle key={`r${x}${y}`} cx={x} cy={y} r="240.5" stroke={c} stroke-width={stroke} />
      ))}
      {REELS.map(([x, y, c]) => (
        <circle key={`h${x}${y}`} cx={x} cy={y} r={hub} fill={c} />
      ))}
    </svg>
  );
}

/** "Mood" in the text color, "Cut" in ember. */
export function Wordmark({ class: cls }: { class?: string }) {
  return (
    <span class={"wordmark" + (cls ? " " + cls : "")}>
      Mood<span class="cut">Cut</span>
    </span>
  );
}

export const TAGLINE = "The AI-native video editor";
