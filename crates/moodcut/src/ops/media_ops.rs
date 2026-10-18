//! Library organisation commands: tag, untag, note, relink, unimport.

use super::exec::{Exec, A};
use crate::library::select_media;
use anyhow::{anyhow, bail, Result};

impl<'a> Exec<'a> {
    /// tag <media> <tag...>  — media: m3, m1..m40, all, tag:x, kind:video, q:text
    pub fn c_tag(&mut self, a: &mut A, add: bool) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("{} <media> <tag...>  (media: m3, m1..m40, all, tag:x, kind:video)", if add { "tag" } else { "untag" }))?;
        let tags: Vec<String> = a.pos.drain(..).flat_map(|t| t.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>()).filter(|t| !t.is_empty()).collect();
        if tags.is_empty() && add {
            bail!("tag needs at least one tag");
        }
        let ids = select_media(self.p, &spec)?;
        for id in &ids {
            let m = self.p.media.iter_mut().find(|m| &m.id == id).expect("selected");
            if add {
                for t in &tags {
                    if !m.tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                        m.tags.push(t.clone());
                    }
                }
            } else if tags.is_empty() || tags.iter().any(|t| t == "all") {
                m.tags.clear();
            } else {
                m.tags.retain(|x| !tags.iter().any(|t| t.eq_ignore_ascii_case(x)));
            }
        }
        let shown = if ids.len() > 3 { format!("{} media", ids.len()) } else { ids.join(",") };
        self.note(format!("~{shown} {} {}", if add { "tagged" } else { "untagged" }, if tags.is_empty() { "all".into() } else { tags.join(",") }));
        Ok(())
    }

    /// note <media> 'what is in the shot'  (empty text clears)
    pub fn c_media_note(&mut self, a: &mut A) -> Result<()> {
        let spec = a.shift().ok_or_else(|| anyhow!("note <media> 'description'"))?;
        let text = a.pos.drain(..).collect::<Vec<_>>().join(" ");
        let ids = select_media(self.p, &spec)?;
        for id in &ids {
            self.p.media.iter_mut().find(|m| &m.id == id).expect("selected").note = text.trim().to_string();
        }
        self.note(format!("~{} note", ids.join(",")));
        Ok(())
    }

    /// relink <media> <path>  — point media at a moved/replaced file (keeps id and every clip).
    pub fn c_relink(&mut self, a: &mut A) -> Result<()> {
        let id = a.shift().ok_or_else(|| anyhow!("relink <media> <new path>"))?;
        let path = a.shift().ok_or_else(|| anyhow!("relink needs the new file path"))?;
        let mid = self.p.media(&id).map(|m| m.id.clone()).ok_or_else(|| anyhow!("unknown media {id}"))?;
        let mut m = self.env.probe(&path)?;
        let old = self.p.media(&mid).cloned().expect("exists");
        m.id = mid.clone();
        m.name = if old.gfx.is_some() { old.name.clone() } else { m.name };
        m.tags = old.tags;
        m.note = old.note;
        m.gfx = old.gfx;
        if old.dur > 0.0 && m.dur + 0.05 < old.dur {
            self.note(format!("!{mid} new file is shorter ({}s < {}s); clips past its end will freeze", crate::model::time::fmt_t(m.dur), crate::model::time::fmt_t(old.dur)));
        }
        let slot = self.p.media.iter_mut().find(|x| x.id == mid).expect("exists");
        *slot = m;
        self.note(format!("~{mid} relinked to {path}"));
        Ok(())
    }

    /// unimport <media> [force]  — remove media from the library (force also deletes its clips).
    pub fn c_unimport(&mut self, a: &mut A) -> Result<()> {
        let force = a.flag("force");
        let spec = a.shift().ok_or_else(|| anyhow!("unimport <media> [force]"))?;
        let ids = select_media(self.p, &spec)?;
        let users: Vec<(String, String)> = self.p.comps.iter().flat_map(|c| c.items().filter(|i| i.src.as_deref().is_some_and(|s| ids.iter().any(|x| x == s))).map(|i| (i.id.clone(), i.src.clone().unwrap_or_default())).collect::<Vec<_>>()).collect();
        if !users.is_empty() && !force {
            bail!("{} clip(s) use this media ({}); add `force` to remove them too", users.len(), users.iter().take(6).map(|u| u.0.as_str()).collect::<Vec<_>>().join(","));
        }
        for c in self.p.comps.iter_mut() {
            for t in c.tracks.iter_mut() {
                t.items.retain(|i| !i.src.as_deref().is_some_and(|s| ids.iter().any(|x| x == s)));
            }
        }
        self.p.media.retain(|m| !ids.contains(&m.id));
        let shown = if ids.len() > 3 { format!("{} media", ids.len()) } else { ids.join(",") };
        self.note(format!("-{shown} unimported{}", if users.is_empty() { String::new() } else { format!(" with {} clip(s)", users.len()) }));
        Ok(())
    }
}
