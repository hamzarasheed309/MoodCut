//! Deliverables besides the video: caption files (SRT, WebVTT), YouTube
//! chapters from markers, and interchange timelines for finishing in other
//! editors (FCP7 XML for Premiere Pro / DaVinci Resolve, CMX3600 EDL).

use crate::model::project::{Comp, Item, ItemKind, MediaKind, Project, TrackKind};
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Sidecar presets handled here instead of the video encoder: (preset, ext).
pub fn sidecar(preset: &str) -> Option<(&'static str, &'static str)> {
    match preset.to_ascii_lowercase().as_str() {
        "srt" | "subtitles" | "captions" => Some(("srt", "srt")),
        "vtt" | "webvtt" => Some(("vtt", "vtt")),
        "chapters" | "youtube-chapters" => Some(("chapters", "txt")),
        "xml" | "fcpxml" | "fcp7" | "premiere" | "resolve" => Some(("xml", "xml")),
        "edl" | "cmx3600" => Some(("edl", "edl")),
        _ => None,
    }
}

/// Writes a sidecar deliverable; returns a one-line summary.
pub fn write(p: &Project, comp_id: &str, kind: &str, out: &std::path::Path) -> Result<String> {
    let c = p.comp(comp_id).ok_or_else(|| anyhow::anyhow!("no comp {comp_id}"))?;
    let (text, summary) = match kind {
        "srt" | "vtt" => {
            let cues = caption_cues(c);
            if cues.is_empty() {
                bail!("there are no captions to export (add them with captions <clip>)");
            }
            let s = if kind == "srt" { srt(&cues) } else { vtt(&cues) };
            (s, format!("{} caption cues", cues.len()))
        }
        "chapters" => {
            let (s, n, short) = chapters(c)?;
            (s, format!("{n} chapters (paste into the YouTube description){}", if short { "; some are under 10 s apart, which YouTube rejects: move or drop markers" } else { "" }))
        }
        "xml" => {
            let (s, skipped) = fcp7_xml(p, c);
            (s, if skipped > 0 { format!("{skipped} non-media layer(s) (text, shapes, solids, precomps) not included: render them as an overlay with preset=prores4444") } else { "all layers included".into() })
        }
        "edl" => {
            let (s, n) = edl(p, c);
            (s, format!("{n} events from V1"))
        }
        _ => bail!("unknown export {kind}"),
    };
    if let Some(d) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(out, text)?;
    Ok(summary)
}

/// Caption items: text on a track named Captions, or with a karaoke highlight.
pub fn caption_items(c: &Comp) -> Vec<&Item> {
    let mut v: Vec<&Item> = c
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video && !t.hidden)
        .flat_map(|t| t.items.iter().filter(move |i| i.kind == ItemKind::Text && !i.off && (t.name.as_deref() == Some("Captions") || i.anims.iter().any(|a| a.kind == "hl"))))
        .collect();
    v.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    v
}

fn caption_cues(c: &Comp) -> Vec<(f64, f64, String)> {
    caption_items(c).into_iter().map(|i| (i.start, i.end(), i.text.clone().unwrap_or_default().replace("\\n", "\n"))).filter(|c| !c.2.trim().is_empty()).collect()
}

fn ts(t: f64, sep: char) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}{sep}{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000)
}

pub fn srt(cues: &[(f64, f64, String)]) -> String {
    let mut s = String::new();
    for (i, (a, b, t)) in cues.iter().enumerate() {
        let _ = write!(s, "{}\n{} --> {}\n{}\n\n", i + 1, ts(*a, ','), ts(*b, ','), t.trim());
    }
    s
}

pub fn vtt(cues: &[(f64, f64, String)]) -> String {
    let mut s = String::from("WEBVTT\n\n");
    for (a, b, t) in cues {
        let _ = write!(s, "{} --> {}\n{}\n\n", ts(*a, '.'), ts(*b, '.'), t.trim());
    }
    s
}

/// YouTube chapter list from markers (first chapter must be 0:00, each ≥ 10 s).
/// Returns (text, count, some chapters shorter than YouTube's 10 s minimum).
pub fn chapters(c: &Comp) -> Result<(String, usize, bool)> {
    let mut ms: Vec<(f64, String)> = c.markers.iter().map(|m| (m.t, m.name.clone())).collect();
    ms.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if ms.is_empty() {
        bail!("no markers: add one per chapter (marker add 0 'Intro', marker add 42 'Setup', ...)");
    }
    if ms[0].0 > 0.5 {
        ms.insert(0, (0.0, "Intro".into()));
    }
    let fmt = |t: f64| {
        let s = t.round() as u64;
        if s >= 3600 {
            format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
        } else {
            format!("{}:{:02}", s / 60, s % 60)
        }
    };
    let mut out = String::new();
    for (i, (t, n)) in ms.iter().enumerate() {
        let t = if i == 0 { 0.0 } else { *t };
        let name = if n.trim().is_empty() { format!("Chapter {}", i + 1) } else { n.trim().to_string() };
        let _ = writeln!(out, "{} {name}", fmt(t));
    }
    let short = ms.windows(2).any(|w| w[1].0 - w[0].0 < 10.0);
    Ok((out, ms.len(), short))
}

fn xml_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn pathurl(path: &str) -> String {
    let p = path.replace('\\', "/");
    let enc: String = p.chars().map(|c| match c {
        ' ' => "%20".to_string(),
        '#' => "%23".to_string(),
        '%' => "%25".to_string(),
        c => c.to_string(),
    }).collect();
    if enc.starts_with('/') {
        format!("file://localhost{enc}")
    } else {
        format!("file://localhost/{enc}")
    }
}

/// (timebase, ntsc) for a frame rate.
fn timebase(fps: f64) -> (u32, bool) {
    let r = fps.round();
    if (fps - r).abs() > 0.01 && (fps * 1.001 - r).abs() < 0.02 {
        (r as u32, true)
    } else {
        (r.max(1.0) as u32, false)
    }
}

/// FCP7 XML (xmeml v5): Premiere Pro (File > Import) and DaVinci Resolve
/// (File > Import Timeline) rebuild the cut with the original media.
pub fn fcp7_xml(p: &Project, c: &Comp) -> (String, usize) {
    let (tb, ntsc) = timebase(c.fps);
    let fps_eff = if ntsc { tb as f64 / 1.001 } else { tb as f64 };
    let fr = |t: f64| (t * fps_eff).round() as i64;
    let rate = format!("<rate><timebase>{tb}</timebase><ntsc>{}</ntsc></rate>", if ntsc { "TRUE" } else { "FALSE" });
    let mut files: BTreeMap<String, usize> = BTreeMap::new();
    let mut file_def = |mid: &str, out: &mut String| {
        let n = files.len() + 1;
        let known = files.get(mid).copied();
        match known {
            Some(k) => {
                let _ = write!(out, "<file id=\"file-{k}\"/>");
            }
            None => {
                files.insert(mid.to_string(), n);
                let m = p.media(mid).unwrap();
                let (mtb, mntsc) = timebase(if m.fps > 0.0 { m.fps } else { c.fps });
                let mfps = if mntsc { mtb as f64 / 1.001 } else { mtb as f64 };
                // stills have no length of their own; give them an hour
                let dur = if m.is_still() { (3600.0 * mfps).round() as i64 } else { (m.dur * mfps).round() as i64 };
                let _ = write!(
                    out,
                    "<file id=\"file-{n}\"><name>{}</name><pathurl>{}</pathurl><rate><timebase>{mtb}</timebase><ntsc>{}</ntsc></rate><duration>{dur}</duration><media>",
                    xml_esc(&m.name),
                    xml_esc(&pathurl(&m.path)),
                    if mntsc { "TRUE" } else { "FALSE" }
                );
                if m.is_visual() {
                    let _ = write!(out, "<video><samplecharacteristics><width>{}</width><height>{}</height></samplecharacteristics></video>", m.w, m.h);
                }
                if m.has_audio {
                    let _ = write!(out, "<audio><channelcount>{}</channelcount></audio>", m.channels.max(1));
                }
                out.push_str("</media></file>");
            }
        }
    };
    let mut skipped = 0;
    let mut n_clip = 0;
    let mut video = String::new();
    let mut audio = String::new();
    let mut embedded: Vec<Vec<&Item>> = vec![];
    for t in c.vtracks() {
        let mut tr = String::from("<track>");
        let mut emb = vec![];
        for i in t.items.iter().filter(|i| !i.off) {
            let media = i.src.as_deref().and_then(|s| p.media(s)).filter(|_| i.kind == ItemKind::Clip);
            let Some(m) = media else {
                skipped += 1;
                continue;
            };
            n_clip += 1;
            let src_in = i.src_in;
            let src_out = i.src_in + i.dur * i.speed();
            let _ = write!(tr, "<clipitem id=\"clipitem-{n_clip}\"><name>{}</name><enabled>TRUE</enabled>{rate}<start>{}</start><end>{}</end><in>{}</in><out>{}</out>", xml_esc(&m.name), fr(i.start), fr(i.end()), fr(src_in), fr(src_out));
            file_def(&m.id, &mut tr);
            tr.push_str("</clipitem>");
            if m.has_audio && m.kind == MediaKind::Video && !i.props.get("mute").is_some_and(|a| a.v.bool()) {
                emb.push(i);
            }
        }
        tr.push_str("</track>");
        video.push_str(&tr);
        if !emb.is_empty() {
            embedded.push(emb);
        }
    }
    let audio_tracks: Vec<Vec<&Item>> = c.atracks().map(|t| t.items.iter().filter(|i| !i.off && i.kind == ItemKind::Clip).collect()).chain(embedded).collect();
    for items in audio_tracks {
        audio.push_str("<track>");
        for i in items {
            let Some(m) = i.src.as_deref().and_then(|s| p.media(s)) else { continue };
            n_clip += 1;
            let src_out = i.src_in + i.dur * i.speed();
            let _ = write!(audio, "<clipitem id=\"clipitem-{n_clip}\"><name>{}</name><enabled>TRUE</enabled>{rate}<start>{}</start><end>{}</end><in>{}</in><out>{}</out>", xml_esc(&m.name), fr(i.start), fr(i.end()), fr(i.src_in), fr(src_out));
            file_def(&m.id, &mut audio);
            audio.push_str("<sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex></sourcetrack></clipitem>");
        }
        audio.push_str("</track>");
    }
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xmeml>\n<xmeml version=\"5\">");
    let _ = write!(
        x,
        "<sequence id=\"sequence-1\"><name>{}</name><duration>{}</duration>{rate}<timecode>{rate}<string>00:00:00:00</string><frame>0</frame><displayformat>NDF</displayformat></timecode><media><video><format><samplecharacteristics>{rate}<width>{}</width><height>{}</height><pixelaspectratio>square</pixelaspectratio></samplecharacteristics></format>{video}</video><audio>{audio}</audio></media>",
        xml_esc(&c.name),
        fr(c.duration()),
        c.w,
        c.h
    );
    if !c.markers.is_empty() {
        for m in &c.markers {
            let _ = write!(x, "<marker><name>{}</name><in>{}</in><out>-1</out></marker>", xml_esc(&m.name), fr(m.t));
        }
    }
    x.push_str("</sequence></xmeml>\n");
    (x, skipped)
}

/// CMX3600 EDL of the V1 cut with source timecodes (reel AX, clip names as comments).
pub fn edl(p: &Project, c: &Comp) -> (String, usize) {
    let (tb, _) = timebase(c.fps);
    let tc = |t: f64| {
        let f = (t.max(0.0) * tb as f64).round() as u64;
        let fps = tb as u64;
        format!("{:02}:{:02}:{:02}:{:02}", f / (3600 * fps), (f / (60 * fps)) % 60, (f / fps) % 60, f % fps)
    };
    let mut s = format!("TITLE: {}\nFCM: NON-DROP FRAME\n\n", c.name);
    let mut n = 0;
    if let Some(t) = c.vtracks().next() {
        for i in t.items.iter().filter(|i| !i.off && i.kind == ItemKind::Clip) {
            let Some(m) = i.src.as_deref().and_then(|x| p.media(x)) else { continue };
            n += 1;
            let src_out = i.src_in + i.dur * i.speed();
            let chan = if m.has_audio { "AA/V" } else { "V   " };
            let _ = write!(s, "{n:03}  AX       {chan}  C        {} {} {} {}\n* FROM CLIP NAME: {}\n\n", tc(i.src_in), tc(src_out), tc(i.start), tc(i.end()), m.name);
        }
    }
    (s, n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_formats_and_timecodes() {
        let cues = vec![(0.5, 2.25, "Hello world".to_string()), (61.0, 62.5, "Second\nline".to_string())];
        assert_eq!(srt(&cues), "1\n00:00:00,500 --> 00:00:02,250\nHello world\n\n2\n00:01:01,000 --> 00:01:02,500\nSecond\nline\n\n");
        assert!(vtt(&cues).starts_with("WEBVTT\n\n00:00:00.500 --> 00:00:02.250\nHello world"));
        assert_eq!(timebase(29.97), (30, true));
        assert_eq!(timebase(25.0), (25, false));
        assert_eq!(pathurl(r"C:\Videos\My Trip\a.mp4"), "file://localhost/C:/Videos/My%20Trip/a.mp4");
        assert_eq!(pathurl("/home/u/a.mp4"), "file://localhost/home/u/a.mp4");
    }
}
