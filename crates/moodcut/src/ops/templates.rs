//! Motion graphics templates (`title <template> ...`). Each template is built
//! as a transparent precomp from ordinary MoodCut Script, so the result stays
//! fully editable (every inner layer has an id) and nests cleanly. Overlay
//! templates (light leak, letterbox, vignette, grain) are single layers placed
//! directly in the current comp instead.

use super::exec::{Exec, Mode, A};
use crate::model::project::*;
use crate::model::time::fmt_t;
use crate::model::value::{fmt_num, Color};
use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;

pub const TEMPLATES: &[(&str, &str, &str)] = &[
    ("lowerthird", "text= sub= color= style=bar|box|minimal|gradient side=left|right font=", "name + title lower third"),
    ("title", "text= sub= style=fade|kinetic|typewriter|cinematic|glitch|pop|neon color= font=", "big animated title card"),
    ("callout", "text= x= y= color=", "dot + line pointing at a spot, with label"),
    ("subscribe", "text= color=", "animated subscribe button"),
    ("counter", "from= to= prefix= suffix= decimals= color= size=", "number counting up"),
    ("progress", "color= height= pos=top|bottom", "progress bar filling over the duration"),
    ("quote", "text= author= color=", "quote card"),
    ("social", "text=@handle color=", "social handle pill"),
    ("chapter", "num= text= color=", "chapter / section heading"),
    ("bars", "values=40,75,60 labels=A,B,C color= suffix=", "animated bar chart"),
    ("shapewipe", "color= color2= dir=left|right", "colored shape sweep (use over a cut)"),
    ("lightleak", "color= amount=", "light leak overlay"),
    ("letterbox", "ratio=2.39", "cinematic bars overlay"),
    ("vignette", "amount=", "vignette overlay"),
    ("grain", "amount=", "film grain overlay"),
];

fn q(s: &str) -> String {
    super::lex::quote(s)
}

struct Ctx {
    cw: f64,
    ch: f64,
    s: f64,
    dur: f64,
    /// Placement for overlay templates (`V5 at=8`), which go straight into
    /// the current comp: blend modes and adjustment layers need the layers
    /// below them, which a precomp would hide.
    place: String,
}

const OVERLAYS: &[&str] = &["lightleak", "letterbox", "vignette", "grain"];

impl Ctx {
    fn n(&self, v: f64) -> String {
        fmt_num((v * 100.0).round() / 100.0)
    }
    fn px(&self, v: f64) -> String {
        self.n(v * self.s)
    }
}

impl<'a> Exec<'a> {
    pub fn c_title(&mut self, a: &mut A) -> Result<()> {
        let tpl = a.shift().ok_or_else(|| anyhow!("title <template> [params] — templates: {}", TEMPLATES.iter().map(|t| t.0).collect::<Vec<_>>().join(" ")))?;
        let tpl_l = tpl.to_ascii_lowercase().replace(['-', '_'], "");
        let canon = match tpl_l.as_str() {
            "lowerthird" | "lower" | "l3" | "nameplate" => "lowerthird",
            "title" | "titlecard" | "headline" => "title",
            "callout" | "pointer" => "callout",
            "subscribe" | "sub" | "cta" => "subscribe",
            "counter" | "count" | "number" => "counter",
            "progress" | "progressbar" => "progress",
            "quote" => "quote",
            "social" | "handle" => "social",
            "chapter" | "section" => "chapter",
            "bars" | "barchart" | "chart" => "bars",
            "shapewipe" | "wipe" => "shapewipe",
            "lightleak" | "leak" => "lightleak",
            "letterbox" | "cinemabars" => "letterbox",
            "vignette" => "vignette",
            "grain" | "filmgrain" => "grain",
            _ => {
                let hint = crate::model::schema::suggest(TEMPLATES.iter().map(|t| t.0), &tpl).map(|h| format!(" Did you mean '{h}'?")).unwrap_or_default();
                bail!("unknown template '{tpl}'.{hint} (help templates)")
            }
        };
        let at = a.take(&["at", "start"]).map(|s| self.time(&s)).transpose()?.unwrap_or(0.0);
        let track = a.take(&["track"]);
        let alias = a.take(&["as", "name"]);
        let comp_dur = self.comp().duration();
        let def_dur = match canon {
            "lowerthird" => 5.0,
            "title" => 4.0,
            "progress" | "lightleak" | "letterbox" | "vignette" | "grain" => {
                if comp_dur > at {
                    comp_dur - at
                } else {
                    10.0
                }
            }
            "shapewipe" => 1.0,
            "counter" => 3.0,
            _ => 4.0,
        };
        let dur = a.take(&["dur", "duration"]).map(|s| self.time(&s)).transpose()?.unwrap_or(def_dur);
        let pos_main = a.pos.drain(..).collect::<Vec<_>>();
        let mut p: BTreeMap<String, String> = a.rest().into_iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect();
        if !pos_main.is_empty() && !p.contains_key("text") {
            p.insert("text".into(), pos_main.join(" "));
        }
        // the brand kit's accent and font, for templates that take them
        let accepts = TEMPLATES.iter().find(|t| t.0 == canon).map(|t| t.1).unwrap_or("");
        if let (Some(c), true) = (&self.p.brand.accent, accepts.contains("color=")) {
            p.entry("color".into()).or_insert_with(|| c.clone());
        }
        if let (Some(f), true) = (&self.p.brand.font, accepts.contains("font=")) {
            p.entry("font".into()).or_insert_with(|| f.clone());
        }
        let (cw, ch, fps) = (self.comp().w as f64, self.comp().h as f64, self.comp().fps);
        let overlay = OVERLAYS.contains(&canon);
        let overlay_track = match (&track, overlay) {
            (Some(t), true) => t.clone(),
            (None, true) => self.pick_track_above(TrackKind::Video, at, at + dur),
            _ => String::new(),
        };
        let place = if overlay {
            format!("{overlay_track} at={}{}", fmt_num(at), alias.as_ref().map(|a| format!(" as={}", q(a))).unwrap_or_default())
        } else {
            "at=0".into()
        };
        let cx = Ctx { cw, ch, s: ch.min(cw) / 1080.0, dur, place };
        let script = build(canon, &mut p, &cx)?;
        if !p.is_empty() {
            bail!("unknown parameter(s) for {canon}: {} (help templates)", p.keys().cloned().collect::<Vec<_>>().join(", "));
        }
        if overlay {
            let cur = self.comp.clone();
            let ids = self.run_sub(&cur, &script, true).map_err(|e| anyhow!("template {canon}: {e}"))?;
            let iid = ids.first().cloned().unwrap_or_default();
            self.note(format!("+{iid} {canon} {overlay_track} {}-{}", fmt_t(at), fmt_t(at + dur)));
            self.created.retain(|c| c != &iid);
            self.created.push(iid);
            return Ok(());
        }
        // precomp
        let cid = self.p.next_comp_id();
        let cname = format!("{} {}", canon, self.p.comps.len());
        let mut nc = Comp::new(&cid, &cname, cw as u32, ch as u32, fps);
        nc.bg = Color::TRANSPARENT;
        nc.dur = Some(dur);
        self.p.comps.push(nc);
        self.run_sub(&cid, &script, true).map_err(|e| anyhow!("template {canon}: {e}"))?;
        let mut it = self.new_item(ItemKind::Comp, at, dur);
        it.src = Some(cid.clone());
        it.name = alias;
        let iid = it.id.clone();
        let tid = match track {
            Some(t) => t,
            None => self.pick_track_above(TrackKind::Video, at, at + dur),
        };
        let tid = self.place(it, &tid, Mode::Strict)?;
        let inner: Vec<String> = self.p.comp(&cid).unwrap().items().map(|i| i.id.clone()).collect();
        self.note(format!("+{iid} {canon} {tid} {}-{} (precomp {cid}: {})", fmt_t(at), fmt_t(at + dur), inner.join(",")));
        self.created.push(iid);
        Ok(())
    }
}

fn take(p: &mut BTreeMap<String, String>, keys: &[&str], def: &str) -> String {
    for k in keys {
        if let Some(v) = p.remove(*k) {
            return v;
        }
    }
    def.to_string()
}

fn build(t: &str, p: &mut BTreeMap<String, String>, c: &Ctx) -> Result<String> {
    let d = c.dur;
    let out_t = (d - 0.45).max(0.3);
    let mut s = String::new();
    let mut line = |l: String| {
        s.push_str(&l);
        s.push('\n');
    };
    match t {
        "lowerthird" => {
            let name = take(p, &["text", "name", "line1"], "Jane Doe");
            let sub = take(p, &["sub", "title", "line2", "role"], "");
            let color = take(p, &["color", "accent"], "#ff3b30");
            let style = take(p, &["style"], "bar");
            let side = take(p, &["side", "align"], "left");
            let font = take(p, &["font"], "Inter");
            let right = side == "right";
            let x0 = if right { c.cw * 0.93 } else { c.cw * 0.07 };
            let y = c.ch * 0.80;
            let (anchor_x, align) = if right { (1.0, "right") } else { (0.0, "left") };
            let dir = if right { "Right" } else { "Left" };
            let tx = if right { x0 - 28.0 * c.s } else { x0 + 28.0 * c.s };
            match style.as_str() {
                "bar" => {
                    line(format!("shape rect w={} h={} fill={color} radius={} pos={},{} at=0 dur={} as=bar", c.px(10.0), c.px(118.0), c.px(5.0), c.n(x0), c.n(y), c.n(d)));
                    line(format!("kf $ scale 0:1,0 0.4:1,1@outExpo {}:1,1 {}:1,0@inExpo", c.n(out_t), c.n(d - 0.05)));
                }
                "gradient" => {
                    let gx = if right { x0 - c.cw * 0.2 } else { x0 + c.cw * 0.2 };
                    line(format!("shape rect w={} h={} fill={color}cc fill2={color}00 gradAngle={} pos={},{} at=0 dur={} as=bg", c.n(c.cw * 0.42), c.px(150.0), if right { 180 } else { 0 }, c.n(gx), c.n(y), c.n(d)));
                    line(format!("anim $ stretchIn dur=0.5; anim $ fadeOut dur=0.35"));
                }
                "minimal" => {
                    let lx = if right { x0 - 110.0 * c.s } else { x0 + 110.0 * c.s };
                    line(format!("shape line from={},0 to={},0 stroke={color} strokeW={} pos={},{} at=0 dur={} as=rule", c.px(-110.0), c.px(110.0), c.px(4.0), c.n(lx), c.n(y - 78.0 * c.s), c.n(d)));
                    line(format!("kf $.s1 trimEnd 0:0 0.6:100@outExpo; anim $ fadeOut dur=0.35"));
                }
                "box" => {}
                other => bail!("lowerthird style must be bar|box|minimal|gradient, not {other}"),
            }
            let bg = if style == "box" { format!(" bg={color} bgPad={} bgRadius={}", c.px(16.0), c.px(8.0)) } else { String::new() };
            line(format!(
                "text {} font={} align={align} anchor={},1 pos={},{} size={} weight=bold at=0 dur={}{bg} as=name",
                q(&name),
                q(&font),
                anchor_x,
                c.n(tx),
                c.n(y - 2.0 * c.s),
                c.px(56.0),
                c.n(d)
            ));
            line(format!("anim $ slideIn{dir} dist={} dur=0.55 delay=0.12; anim $ fadeOut dur=0.3", c.px(40.0)));
            if !sub.is_empty() {
                let sbg = if style == "box" { format!(" bg=#000000cc bgPad={} bgRadius={}", c.px(12.0), c.px(8.0)) } else { String::new() };
                line(format!(
                    "text {} font={} align={align} anchor={},0 pos={},{} size={} weight=medium color=#ffffffd9 at=0 dur={}{sbg} as=role",
                    q(&sub),
                    q(&font),
                    anchor_x,
                    c.n(tx),
                    c.n(y + 10.0 * c.s),
                    c.px(36.0),
                    c.n(d)
                ));
                line(format!("anim $ slideIn{dir} dist={} dur=0.55 delay=0.25; anim $ fadeOut dur=0.3", c.px(40.0)));
            }
        }
        "title" => {
            let text = take(p, &["text", "title"], "Your Title");
            let sub = take(p, &["sub", "subtitle"], "");
            let style = take(p, &["style"], "fade");
            let color = take(p, &["color"], "#ffffff");
            let font = take(p, &["font"], "Inter");
            let cy = if sub.is_empty() { c.ch * 0.5 } else { c.ch * 0.46 };
            let base = format!("text {} font={} pos={},{} size={} color={color} boxW={} at=0 dur={} as=title", q(&text), q(&font), c.n(c.cw / 2.0), c.n(cy), c.px(140.0), c.n(c.cw * 0.86), c.n(d));
            match style.as_str() {
                "fade" => {
                    line(format!("{base} weight=black enter=fadeUp exit=fadeDown"));
                }
                "kinetic" => {
                    line(format!("{base} weight=black caps=1 enter=wordPop exit=popOut"));
                    line("anim $ punch at=0.6".into());
                }
                "typewriter" => {
                    line(format!("{base} weight=bold font='Inter' enter=typewriter exit=typeOut"));
                }
                "cinematic" => {
                    line(format!("{base} weight=light caps=1 tracking={} enter=trackIn", c.px(18.0)));
                    line("anim $ fadeOut dur=0.6".into());
                }
                "glitch" => {
                    line(format!("{base} weight=black caps=1"));
                    line("fx $ glitch amount=1; kf $.f1 amount 0:1 0.5:0@outCubic {o}:0 {e}:1@inCubic".replace("{o}", &c.n(out_t)).replace("{e}", &c.n(d)));
                    line("fx $ rgbsplit amount=12; kf $.f2 amount 0:30 0.5:3@outCubic".into());
                    line("anim $ fadeIn dur=0.15; anim $ fadeOut dur=0.2".into());
                }
                "pop" => {
                    line(format!("{base} weight=black enter=pop exit=popOut"));
                }
                "neon" => {
                    line(format!("{base} weight=bold style=neon shadow={color} enter=fadeChars"));
                    line("anim $ flicker amount=0.08".into());
                }
                other => bail!("title style must be fade|kinetic|typewriter|cinematic|glitch|pop|neon, not {other}"),
            }
            if !sub.is_empty() {
                line(format!(
                    "text {} font={} pos={},{} size={} weight=medium color=#ffffffcc at=0.3 dur={} enter=fadeUp exit=fadeDown as=subtitle",
                    q(&sub),
                    q(&font),
                    c.n(c.cw / 2.0),
                    c.n(c.ch * 0.58),
                    c.px(48.0),
                    c.n((d - 0.3).max(0.5))
                ));
            }
        }
        "callout" => {
            let text = take(p, &["text", "label"], "Look here");
            let color = take(p, &["color"], "#ffd60a");
            let x: f64 = take(p, &["x"], &fmt_num(c.cw * 0.5)).parse()?;
            let y: f64 = take(p, &["y"], &fmt_num(c.ch * 0.5)).parse()?;
            let dx = if x > c.cw * 0.6 { -1.0 } else { 1.0 };
            let ex = x + dx * 220.0 * c.s;
            let ey = y - 160.0 * c.s;
            line(format!("shape ellipse w={} fill={color} stroke=#ffffff strokeW={} pos={},{} at=0 dur={} as=dot", c.px(26.0), c.px(4.0), c.n(x), c.n(y), c.n(d)));
            line("anim $ popIn; anim $ pulse amount=0.12 freq=1.5; anim $ popOut".into());
            line(format!("shape path d='M0,0 L{},{} L{},{}' stroke={color} strokeW={} fill=none pos={},{} anchor=0,0 at=0.15 dur={} as=line", c.n((ex - x) * 0.6), c.n(ey - y), c.n(ex - x), c.n(ey - y), c.px(5.0), c.n(x), c.n(y), c.n(d - 0.15)));
            line(format!("kf $.s1 trimEnd 0:0 0.45:100@outCubic {}:100 {}:0@inCubic", c.n(out_t - 0.15), c.n(d - 0.2)));
            let align = if dx > 0.0 { "left" } else { "right" };
            let anchor = if dx > 0.0 { "0,0.5" } else { "1,0.5" };
            line(format!(
                "text {} align={align} anchor={anchor} pos={},{} size={} weight=bold bg=#000000cc bgPad={} at=0.45 dur={} enter=fadeUp exit=fadeDown as=label",
                q(&text),
                c.n(ex + dx * 16.0 * c.s),
                c.n(ey),
                c.px(44.0),
                c.px(14.0),
                c.n(d - 0.45)
            ));
        }
        "subscribe" => {
            let text = take(p, &["text"], "SUBSCRIBE");
            let color = take(p, &["color"], "#ff0033");
            let (x, y) = (c.cw * 0.5, c.ch * 0.82);
            line(format!("shape rect w={} h={} radius={} fill={color} pos={},{} at=0 dur={} as=button", c.px(440.0), c.px(112.0), c.px(56.0), c.n(x), c.n(y), c.n(d)));
            line(format!("anim $ popIn; anim $ punch at=1.4 amount=0.1; anim $ popOut"));
            line(format!("text {} pos={},{} size={} weight=black at=0.1 dur={} as=label", q(&text), c.n(x), c.n(y), c.px(52.0), c.n(d - 0.1)));
            line("anim $ popIn; anim $ punch at=1.4 amount=0.1; anim $ popOut".into());
        }
        "counter" => {
            let from = take(p, &["from"], "0");
            let to = take(p, &["to"], "100");
            let prefix = take(p, &["prefix"], "");
            let suffix = take(p, &["suffix"], "");
            let dec = take(p, &["decimals", "dec"], "0");
            let color = take(p, &["color"], "#ffffff");
            let size = take(p, &["size"], &c.px(180.0));
            let run = (d - 0.6).max(0.3);
            let expr = format!("{prefix}{{round(easeout(time, 0, {}, {from}, {to}), {dec})}}{suffix}", c.n(run));
            line(format!("text {} pos=center size={size} weight=black color={color} at=0 dur={} as=counter", q(&expr), c.n(d)));
            line("anim $ popIn; anim $ fadeOut dur=0.3".into());
        }
        "progress" => {
            let color = take(p, &["color"], "#ff3b30");
            let h: f64 = take(p, &["height", "h"], &c.px(12.0)).parse()?;
            let pos = take(p, &["pos"], "bottom");
            let y = if pos == "top" { h / 2.0 } else { c.ch - h / 2.0 };
            line(format!("shape rect w={} h={} fill={color} anchor=0,0.5 pos=0,{} at=0 dur={} as=bar", c.n(c.cw), c.n(h), c.n(y), c.n(d)));
            line(format!("kf $ scale 0:0,1@linear {}:1,1", c.n(d)));
        }
        "quote" => {
            let text = take(p, &["text", "quote"], "Simplicity is the ultimate sophistication.");
            let author = take(p, &["author", "by"], "");
            let color = take(p, &["color"], "#ffd60a");
            line(format!("text '\u{201C}' pos={},{} size={} weight=black color={color} at=0 dur={} as=mark", c.n(c.cw / 2.0), c.n(c.ch * 0.28), c.px(260.0), c.n(d)));
            line("anim $ popIn; anim $ fadeOut".into());
            line(format!("text {} pos=center size={} weight=semibold boxW={} lineH=1.25 at=0.2 dur={} enter=fadeUp exit=fadeDown as=quote", q(&text), c.px(64.0), c.n(c.cw * 0.7), c.n(d - 0.2)));
            if !author.is_empty() {
                line(format!("text {} pos={},{} size={} weight=medium color={color} at=0.6 dur={} enter=fadeUp exit=fadeDown as=author", q(&format!("\u{2014} {author}")), c.n(c.cw / 2.0), c.n(c.ch * 0.72), c.px(40.0), c.n(d - 0.6)));
            }
        }
        "social" => {
            let text = take(p, &["text", "handle"], "@moodcut");
            let color = take(p, &["color"], "#0a84ff");
            line(format!("text {} align=left anchor=0,0.5 pos={},{} size={} weight=bold bg={color} bgPad={} bgRadius={} at=0 dur={} as=handle", q(&text), c.n(c.cw * 0.06), c.n(c.ch * 0.1), c.px(44.0), c.px(18.0), c.px(40.0), c.n(d)));
            line("anim $ slideInLeft dur=0.5; anim $ slideOutLeft dur=0.4".into());
        }
        "chapter" => {
            let num = take(p, &["num", "number"], "01");
            let text = take(p, &["text", "title"], "Chapter");
            let color = take(p, &["color"], "#ffd60a");
            line(format!("text {} align=left anchor=0,1 pos={},{} size={} weight=black color={color} at=0 dur={} enter=slideUp exit=fadeDown as=num", q(&num), c.n(c.cw * 0.08), c.n(c.ch * 0.55), c.px(200.0), c.n(d)));
            line(format!("text {} align=left anchor=0,0 pos={},{} size={} weight=bold caps=1 tracking={} at=0.2 dur={} enter=fadeUp exit=fadeDown as=title", q(&text), c.n(c.cw * 0.085), c.n(c.ch * 0.57), c.px(64.0), c.px(4.0), c.n(d - 0.2)));
        }
        "bars" => {
            let values: Vec<f64> = take(p, &["values", "data"], "40,75,60").split(',').map(|v| v.trim().parse::<f64>()).collect::<std::result::Result<_, _>>().map_err(|_| anyhow!("values must be numbers like 40,75,60"))?;
            let labels: Vec<String> = take(p, &["labels"], "").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            let color = take(p, &["color"], "#0a84ff");
            let suffix = take(p, &["suffix"], "");
            if values.is_empty() || values.len() > 24 {
                bail!("bars needs 1-24 values");
            }
            let maxv = values.iter().cloned().fold(f64::MIN, f64::max).max(1e-9);
            let n = values.len() as f64;
            let area_w = c.cw * 0.7;
            let gap = area_w / n;
            let bw = gap * 0.62;
            let base = c.ch * 0.78;
            let maxh = c.ch * 0.5;
            for (i, v) in values.iter().enumerate() {
                let x = c.cw * 0.15 + gap * (i as f64 + 0.5);
                let h = (v / maxv) * maxh;
                let delay = 0.15 * i as f64;
                line(format!("shape rect w={} h={} radius={} fill={color} anchor=0.5,1 pos={},{} at=0 dur={}", c.n(bw), c.n(h), c.px(8.0), c.n(x), c.n(base), c.n(d)));
                line(format!("kf $ scale {}:1,0 {}:1,1@outExpo {}:1,1 {}:1,0@inExpo", c.n(delay), c.n(delay + 0.8), c.n(out_t), c.n(d)));
                let run = delay + 0.8;
                line(format!(
                    "text '{{round(easeout(time, {}, {}, 0, {}), 0)}}{}' pos={},{} size={} weight=bold at=0 dur={}",
                    c.n(delay),
                    c.n(run),
                    fmt_num(*v),
                    suffix,
                    c.n(x),
                    c.n(base - h - 36.0 * c.s),
                    c.px(40.0),
                    c.n(d)
                ));
                line(format!("anim $ fadeIn delay={} dur=0.3; anim $ fadeOut dur=0.3", c.n(delay)));
                if let Some(lb) = labels.get(i) {
                    line(format!("text {} pos={},{} size={} weight=medium color=#ffffffcc at=0 dur={} enter=fadeUp exit=fadeDown", q(lb), c.n(x), c.n(base + 40.0 * c.s), c.px(34.0), c.n(d)));
                }
            }
        }
        "shapewipe" => {
            let color = take(p, &["color"], "#ffd60a");
            let color2 = take(p, &["color2"], "#ff2d55");
            let dir = take(p, &["dir"], "left");
            let sign = if dir == "right" { -1.0 } else { 1.0 };
            let w = c.cw * 1.4;
            let (x0, x1) = (c.cw / 2.0 + sign * (c.cw / 2.0 + w / 2.0), c.cw / 2.0 - sign * (c.cw / 2.0 + w / 2.0));
            for (i, col) in [color2.as_str(), color.as_str()].iter().enumerate() {
                let delay = 0.08 * i as f64;
                line(format!("shape rect w={} h={} fill={col} rot=-12 pos={},{} at=0 dur={}", c.n(w), c.n(c.ch * 1.6), c.n(x0), c.n(c.ch / 2.0), c.n(d)));
                line(format!("kf $ pos {}:{},{} {}:{},{}@inOutCubic", c.n(delay), c.n(x0), c.n(c.ch / 2.0), c.n(d - 0.08 + delay * 0.5), c.n(x1), c.n(c.ch / 2.0)));
            }
        }
        "lightleak" => {
            let color = take(p, &["color"], "#ff7a18");
            let amount = take(p, &["amount"], "0.7");
            line(format!("solid #000000 {} dur={} blend=screen", c.place, c.n(d)));
            line(format!("fx $ lightleak color={color} amount={amount}"));
        }
        "letterbox" => {
            let ratio = take(p, &["ratio"], "2.39");
            line(format!("adjust {} dur={}", c.place, c.n(d)));
            line(format!("fx $ letterbox ratio={ratio}"));
        }
        "vignette" => {
            let amount = take(p, &["amount"], "0.5");
            line(format!("adjust {} dur={}", c.place, c.n(d)));
            line(format!("fx $ vignette amount={amount}"));
        }
        "grain" => {
            let amount = take(p, &["amount"], "0.1");
            line(format!("adjust {} dur={}", c.place, c.n(d)));
            line(format!("fx $ noise amount={amount}"));
        }
        _ => bail!("template {t} not implemented"),
    }
    Ok(s)
}
