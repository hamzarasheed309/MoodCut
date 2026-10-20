# MoodCut brand

<p><img src="../assets/brand/moodcut-mark.svg" width="96" alt="MoodCut mark"></p>

## The mark

Four reels in a 2×2 grid on a black disc: ember top-left and bottom-right, white top-right and bottom-left, each a ring
with a hub. The rings' centerlines touch, so the reels interlock, a nod to film reels and to cutting between shots.

| File | Use |
| --- | --- |
| `assets/brand/moodcut-mark.svg` | the mark, 48 px and up |
| `assets/brand/moodcut-mark-small.svg` | thicker rings and hubs for 16-32 px (favicon, small icons) |
| `assets/brand/moodcut-logo-dark.svg` | mark + wordmark on dark backgrounds |
| `assets/brand/moodcut-logo-light.svg` | mark + wordmark on light backgrounds |
| `assets/brand/moodcut-mark-1024.png` | raster for stores, social cards and slides |
| `crates/moodcut/assets/moodcut.ico` | the Windows app icon (16-256 px) |
| `installer/wizard*.bmp` | installer artwork |

All of these are generated from `crates/moodcut/src/brand.rs`: change the definition there and run
`moodcut brand .` from the repository root (or `cargo run --release -- brand .`).

## Wordmark

**Mood**<span style="color:#E76C33">**Cut**</span> in Inter Black with -2% tracking: "Mood" in the text color, "Cut" in
ember. Logo SVGs carry it as outlines, so they look the same without the font.

## Color

| Name | Hex | Use |
| --- | --- | --- |
| Ember | `#E76C33` | the brand color: focus, selection, primary actions, the playhead |
| Ember light | `#F2884F` | hover, gradients |
| Ember deep | `#C9541F` | primary button fill (keeps white labels readable) |
| Ink | `#010101` | the mark's disc, splash and installer backgrounds |
| White | `#FFFFFF` | the mark's other reels, text on ink |

The editor's interface uses neutral greys (`#111111` to `#3A3A3A`) with no color cast, so footage reads true while
grading; ember is kept for things that need attention.

## Voice

Tagline: *The AI-native video editor.* Write like an editor talking to an editor: plain words, the outcome first, no
hype. Name things by what they do ("Connect AI", "Make proxies"), not by technology.
