//! The MoodCut brand: the logo (four interlocking reels, ember and white on
//! black) and a generator for every asset made from it. `moodcut brand <repo>`
//! rewrites the SVG logos, the app and web icons, the Windows .ico and the
//! installer artwork, so they always match this file.

use anyhow::{anyhow, Context, Result};
use resvg::tiny_skia::{Pixmap, Transform};
use std::path::Path;

/// Ember: the brand orange.
pub const EMBER: &str = "#E76C33";
pub const INK: &str = "#010101";
pub const TAGLINE: &str = "The AI-native video editor";

/// The mark, drawn on a 1778 px canvas: a black disc holding four reels in a
/// 2x2 grid (ember top-left and bottom-right, white the other two), each a
/// ring with a hub. Ring centerlines touch, so the reels interlock.
fn mark_shapes(stroke: f64, hub: f64) -> String {
    let (a, b) = (648.5, 1129.5);
    let (y1, y2) = (608.5, 1169.5);
    let r = 240.5;
    let ring = |x: f64, y: f64, c: &str| format!(r#"<circle cx="{x}" cy="{y}" r="{r}" fill="none" stroke="{c}" stroke-width="{stroke}"/>"#);
    let dot = |x: f64, y: f64, c: &str| format!(r#"<circle cx="{x}" cy="{y}" r="{hub}" fill="{c}"/>"#);
    format!(
        r#"<rect width="1778" height="1778" rx="889" fill="{INK}"/>{}{}{}{}{}{}{}{}"#,
        ring(a, y1, EMBER),
        ring(b, y1, "#FFFFFF"),
        ring(a, y2, "#FFFFFF"),
        ring(b, y2, EMBER),
        dot(a, y1, EMBER),
        dot(a, y2, "#FFFFFF"),
        dot(b, y1, "#FFFFFF"),
        dot(b, y2, EMBER),
    )
}

/// The logo mark as a standalone SVG. `small` thickens rings and hubs so the
/// mark stays legible at 16-32 px (favicons, small icons).
pub fn mark_svg(small: bool) -> String {
    let inner = if small { mark_shapes(118.0, 118.0) } else { mark_shapes(80.0, 100.0) };
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="1778" height="1778" viewBox="0 0 1778 1778" fill="none">{inner}</svg>"#)
}

fn mark_group(x: f64, y: f64, size: f64, small: bool) -> String {
    let inner = if small { mark_shapes(118.0, 118.0) } else { mark_shapes(80.0, 100.0) };
    format!(r#"<g transform="translate({x} {y}) scale({})">{inner}</g>"#, size / 1778.0)
}

fn wordmark_width(size: f32) -> f32 {
    let st = crate::render::text::TextStyle { family: "Inter".into(), weight: 900.0, italic: false, size, align: 0, line_h: 1.0, tracking: -0.02 * size, box_w: 0.0, caps: false };
    crate::render::text::layout("MoodCut", &st).w
}

/// "Mood" in the text color, "Cut" in ember.
fn wordmark(x: f64, baseline: f64, size: f64, text: &str, anchor: &str) -> String {
    format!(
        r#"<text x="{x}" y="{baseline}" font-family="Inter" font-weight="900" font-size="{size}" letter-spacing="{}" text-anchor="{anchor}"><tspan fill="{text}">Mood</tspan><tspan fill="{EMBER}">Cut</tspan></text>"#,
        -0.02 * size
    )
}

/// Horizontal logo: mark + wordmark. `text` is the wordmark color for the
/// background it sits on (#FFFFFF on dark, #111111 on light).
pub fn logo_svg(text: &str) -> String {
    let h = 200.0;
    let size = 132.0;
    let tw = wordmark_width(size as f32) as f64;
    let w = (h + 44.0 + tw + 8.0).ceil();
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" fill="none">{}{}</svg>"#,
        mark_group(0.0, 0.0, h, false),
        wordmark(h + 44.0, 100.0 + size * 0.36, size, text, "start")
    )
}

/// Installer welcome-page artwork (164x314 at 100%).
fn wizard_svg() -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="164" height="314" viewBox="0 0 164 314">
<defs><radialGradient id="glow" cx="50%" cy="34%" r="60%"><stop offset="0" stop-color="{EMBER}" stop-opacity="0.28"/><stop offset="1" stop-color="{EMBER}" stop-opacity="0"/></radialGradient></defs>
<rect width="164" height="314" fill="#0b0b0b"/><rect width="164" height="314" fill="url(#glow)"/>
{}
{}
<text x="82" y="214" font-family="Inter" font-weight="400" font-size="9.5" fill="#9b9b9b" text-anchor="middle">{TAGLINE}</text>
<rect x="0" y="308" width="164" height="6" fill="{EMBER}"/>
</svg>"##,
        mark_group(30.0, 52.0, 104.0, false),
        wordmark(82.0, 194.0, 25.0, "#FFFFFF", "middle")
    )
}

/// Installer header artwork (55x58 at 100%), on the wizard's white header.
fn wizard_small_svg() -> String {
    format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="55" height="58" viewBox="0 0 55 58"><rect width="55" height="58" fill="#FFFFFF"/>{}</svg>"##, mark_group(3.5, 5.0, 48.0, true))
}

fn options() -> resvg::usvg::Options<'static> {
    let mut opt = resvg::usvg::Options::default();
    opt.fontdb = crate::render::text::fontdb_arc();
    opt
}

fn tree(svg: &str) -> Result<resvg::usvg::Tree> {
    resvg::usvg::Tree::from_str(svg, &options()).map_err(|e| anyhow!("bad brand svg: {e}"))
}

/// Rasterizes an SVG to `w`x`h` (premultiplied RGBA).
pub fn render(svg: &str, w: u32, h: u32) -> Result<Pixmap> {
    let t = tree(svg)?;
    let mut pm = Pixmap::new(w, h).ok_or_else(|| anyhow!("bad size"))?;
    let s = t.size();
    resvg::render(&t, Transform::from_scale(w as f32 / s.width(), h as f32 / s.height()), &mut pm.as_mut());
    Ok(pm)
}

/// Same SVG with its text turned into outlines (renders without Inter installed).
fn outlined(svg: &str) -> Result<String> {
    let opt = resvg::usvg::WriteOptions { coordinates_precision: 3, transforms_precision: 5, ..Default::default() };
    Ok(tree(svg)?.to_string(&opt))
}

fn png(pm: &Pixmap) -> Result<Vec<u8>> {
    pm.encode_png().map_err(|e| anyhow!("png: {e}"))
}

/// Straight (non-premultiplied) RGBA of a pixmap.
fn rgba(pm: &Pixmap) -> Vec<u8> {
    pm.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect()
}

/// A Windows .ico: 32-bit DIB entries below 256 px (what every Windows tool
/// reads) and a PNG entry at 256 px.
pub fn ico(images: &[Pixmap]) -> Result<Vec<u8>> {
    let mut entries: Vec<(u32, Vec<u8>)> = vec![];
    for pm in images {
        let s = pm.width();
        let data = if s >= 256 {
            png(pm)?
        } else {
            let px = rgba(pm);
            let mut d = Vec::new();
            let mask_row = s.div_ceil(32) * 4;
            for v in [40u32, s, s * 2] {
                d.extend_from_slice(&v.to_le_bytes());
            }
            d.extend_from_slice(&1u16.to_le_bytes());
            d.extend_from_slice(&32u16.to_le_bytes());
            for v in [0u32, s * s * 4 + mask_row * s, 0, 0, 0, 0] {
                d.extend_from_slice(&v.to_le_bytes());
            }
            for y in (0..s).rev() {
                for x in 0..s {
                    let o = ((y * s + x) * 4) as usize;
                    d.extend_from_slice(&[px[o + 2], px[o + 1], px[o], px[o + 3]]);
                }
            }
            d.extend(std::iter::repeat_n(0u8, (mask_row * s) as usize));
            d
        };
        entries.push((s, data));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * entries.len() as u32;
    for (s, d) in &entries {
        let b = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[b, b, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += d.len() as u32;
    }
    for (_, d) in entries {
        out.extend_from_slice(&d);
    }
    Ok(out)
}

/// 24-bit BMP over an opaque background (the installer's wizard images).
fn bmp(pm: &Pixmap) -> Result<Vec<u8>> {
    let px = rgba(pm);
    let rgb: Vec<u8> = px.chunks_exact(4).flat_map(|c| [c[0], c[1], c[2]]).collect();
    let img = image::RgbImage::from_raw(pm.width(), pm.height(), rgb).ok_or_else(|| anyhow!("bmp size"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Bmp)?;
    Ok(out.into_inner())
}

fn write(root: &Path, rel: &str, data: &[u8], done: &mut Vec<String>) -> Result<()> {
    let p = root.join(rel);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, data).with_context(|| format!("writing {}", p.display()))?;
    done.push(format!("{rel} ({} KB)", data.len().div_ceil(1024)));
    Ok(())
}

/// Writes every brand asset into the repository at `root`.
pub fn generate(root: &Path) -> Result<Vec<String>> {
    let mut done = vec![];
    let (mark, small) = (mark_svg(false), mark_svg(true));
    write(root, "assets/brand/moodcut-mark.svg", mark.as_bytes(), &mut done)?;
    write(root, "assets/brand/moodcut-mark-small.svg", small.as_bytes(), &mut done)?;
    write(root, "assets/brand/moodcut-logo-dark.svg", outlined(&logo_svg("#FFFFFF"))?.as_bytes(), &mut done)?;
    write(root, "assets/brand/moodcut-logo-light.svg", outlined(&logo_svg("#111111"))?.as_bytes(), &mut done)?;
    write(root, "assets/brand/moodcut-mark-1024.png", &png(&render(&mark, 1024, 1024)?)?, &mut done)?;
    write(root, "gui/public/favicon.svg", small.as_bytes(), &mut done)?;
    for (rel, s) in [("gui/public/icon-192.png", 192), ("gui/public/icon-512.png", 512), ("gui/public/apple-touch-icon.png", 180)] {
        write(root, rel, &png(&render(&mark, s, s)?)?, &mut done)?;
    }
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 96, 128, 256];
    let imgs: Vec<Pixmap> = sizes.iter().map(|&s| render(if s <= 32 { &small } else { &mark }, s, s)).collect::<Result<_>>()?;
    write(root, "crates/moodcut/assets/moodcut.ico", &ico(&imgs)?, &mut done)?;
    let wz = wizard_svg();
    write(root, "installer/wizard.bmp", &bmp(&render(&wz, 164, 314)?)?, &mut done)?;
    write(root, "installer/wizard-2x.bmp", &bmp(&render(&wz, 328, 628)?)?, &mut done)?;
    let ws = wizard_small_svg();
    write(root, "installer/wizard-small.bmp", &bmp(&render(&ws, 55, 58)?)?, &mut done)?;
    write(root, "installer/wizard-small-2x.bmp", &bmp(&render(&ws, 110, 116)?)?, &mut done)?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_renders_with_its_colors_and_the_icon_is_valid() {
        let pm = render(&mark_svg(false), 128, 128).unwrap();
        let at = |x: u32, y: u32| pm.pixel(x, y).unwrap().demultiply();
        // corner outside the disc is transparent, the center is black, the top-left hub is ember
        assert_eq!(at(1, 1).alpha(), 0);
        let c = at(64, 64);
        assert!(c.red() < 20 && c.alpha() == 255);
        let hub = at((648.5 / 1778.0 * 128.0) as u32, (608.5 / 1778.0 * 128.0) as u32);
        assert!(hub.red() > 200 && hub.green() > 80 && hub.green() < 130 && hub.blue() < 80, "{hub:?}");
        let ico = ico(&[render(&mark_svg(true), 16, 16).unwrap(), render(&mark_svg(false), 256, 256).unwrap()]).unwrap();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        let off = u32::from_le_bytes(ico[6 + 16 + 12..6 + 16 + 16].try_into().unwrap()) as usize;
        assert_eq!(&ico[off..off + 4], b"\x89PNG");
        // the wordmark is outlined (no font needed to show it)
        let svg = outlined(&logo_svg("#FFFFFF")).unwrap();
        assert!(!svg.contains("<text") && svg.contains("<path"), "{}", &svg[..200.min(svg.len())]);
    }
}
