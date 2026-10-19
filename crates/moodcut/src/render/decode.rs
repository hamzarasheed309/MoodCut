//! Frame sources: FFmpeg-backed video readers (with background prefetch and
//! chunked reverse playback), plus cached image and SVG rasterization.

use super::ffmpeg::Ff;
use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use resvg::tiny_skia::{IntSize, Pixmap};
use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Stdio};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

pub type FrameRef = Arc<Pixmap>;

#[derive(Clone, Debug, PartialEq)]
pub struct ReaderSpec {
    pub path: String,
    pub w: u32,
    pub h: u32,
    /// Frames delivered per source second.
    pub rate: f64,
    /// FFmpeg filters applied before scaling (decode-time effects).
    pub vf: String,
    pub alpha: bool,
    pub hw: bool,
    pub src_dur: f64,
    /// "none" | "blend" | "flow" (slow-motion interpolation).
    pub interp: String,
}

impl ReaderSpec {
    pub fn key(&self) -> String {
        format!("{}|{}x{}|{:.5}|{}|{}", self.path, self.w, self.h, self.rate, self.vf, self.interp)
    }
}

pub fn premultiply(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 255 {
            continue;
        }
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            continue;
        }
        px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
        px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
        px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
    }
}

pub fn pixmap_from_rgba(mut data: Vec<u8>, w: u32, h: u32, premul: bool) -> Option<Pixmap> {
    if premul {
        premultiply(&mut data);
    }
    Pixmap::from_vec(data, IntSize::from_wh(w, h)?)
}

struct Proc {
    child: Child,
    rx: Receiver<Option<Vec<u8>>>,
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct VideoReader {
    spec: ReaderSpec,
    ff: Arc<Ff>,
    proc: Option<Proc>,
    /// Global frame index of the next frame the process will deliver.
    next: i64,
    last: Option<(i64, FrameRef)>,
    eof_at: Option<i64>,
    /// Reverse playback buffer: (first global index, frames).
    chunk: Option<(i64, Vec<FrameRef>)>,
    last_req: Option<i64>,
    pub used: Instant,
}

impl VideoReader {
    pub fn new(ff: Arc<Ff>, spec: ReaderSpec) -> VideoReader {
        VideoReader { spec, ff, proc: None, next: 0, last: None, eof_at: None, chunk: None, last_req: None, used: Instant::now() }
    }

    fn frame_bytes(&self) -> usize {
        self.spec.w as usize * self.spec.h as usize * 4
    }

    fn start(&mut self, idx: i64) -> Result<()> {
        self.proc = None;
        let t = idx as f64 / self.spec.rate;
        let mut vf = vec!["setpts=PTS-STARTPTS".to_string()];
        match self.spec.interp.as_str() {
            "flow" => vf.push(format!("minterpolate=fps={:.6}:mi_mode=mci:mc_mode=aobmc:vsbmc=1", self.spec.rate)),
            "blend" => vf.push(format!("framerate=fps={:.6}", self.spec.rate)),
            _ => {}
        }
        if !self.spec.vf.is_empty() {
            vf.push(self.spec.vf.clone());
        }
        vf.push(format!("fps={:.6}", self.spec.rate));
        vf.push(format!("scale={}:{}:flags=bicubic", self.spec.w, self.spec.h));
        vf.push("format=rgba".into());
        self.ff.need()?;
        let mut c = self.ff.cmd();
        if self.spec.hw {
            c.args(["-hwaccel", "auto"]);
        }
        if t > 0.0 {
            c.args(["-ss", &format!("{:.6}", t)]);
        }
        c.args(["-i", &self.spec.path, "-an", "-sn", "-dn", "-vf", &vf.join(","), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"]);
        c.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
        let mut child = c.spawn().map_err(|e| anyhow!("failed to start ffmpeg decoder: {e}"))?;
        let mut out = child.stdout.take().unwrap();
        let n = self.frame_bytes();
        let (w, h, alpha) = (self.spec.w, self.spec.h, self.spec.alpha);
        let (tx, rx) = sync_channel::<Option<Vec<u8>>>(3);
        std::thread::spawn(move || loop {
            let mut buf = vec![0u8; n];
            let mut got = 0;
            while got < n {
                match out.read(&mut buf[got..]) {
                    Ok(0) => break,
                    Ok(k) => got += k,
                    Err(_) => break,
                }
            }
            if got < n {
                let _ = tx.send(None);
                return;
            }
            if alpha {
                premultiply(&mut buf);
            }
            let _ = (w, h);
            if tx.send(Some(buf)).is_err() {
                return;
            }
        });
        self.proc = Some(Proc { child, rx });
        self.next = idx;
        self.eof_at = None;
        Ok(())
    }

    fn read_one(&mut self) -> Result<Option<FrameRef>> {
        let Some(p) = self.proc.as_mut() else { return Ok(None) };
        match p.rx.recv() {
            Ok(Some(buf)) => {
                let pm = Pixmap::from_vec(buf, IntSize::from_wh(self.spec.w, self.spec.h).unwrap()).ok_or_else(|| anyhow!("bad frame"))?;
                self.next += 1;
                Ok(Some(Arc::new(pm)))
            }
            _ => {
                self.eof_at = Some(self.next);
                self.proc = None;
                Ok(None)
            }
        }
    }

    /// Returns the frame showing source time `t` (seconds).
    pub fn frame_at(&mut self, t: f64) -> Result<FrameRef> {
        self.used = Instant::now();
        let rate = self.spec.rate;
        let max_t = (self.spec.src_dur - 0.5 / rate).max(0.0);
        let t = t.clamp(0.0, max_t);
        let idx = (t * rate + 1e-6).floor() as i64;
        if let Some((li, f)) = &self.last {
            if *li == idx {
                return Ok(f.clone());
            }
        }
        if let Some((first, frames)) = &self.chunk {
            if idx >= *first && idx < first + frames.len() as i64 {
                let f = frames[(idx - first) as usize].clone();
                self.last = Some((idx, f.clone()));
                self.last_req = Some(idx);
                return Ok(f);
            }
        }
        let backwards = self.last_req.map(|l| idx < l && l - idx <= (rate * 0.5).ceil() as i64 + 1).unwrap_or(false);
        self.last_req = Some(idx);
        if backwards {
            return self.reverse_chunk(idx);
        }
        if let Some(e) = self.eof_at {
            if idx >= e {
                if let Some((_, f)) = &self.last {
                    return Ok(f.clone());
                }
            }
        }
        let ahead = idx - self.next;
        if self.proc.is_none() || ahead < 0 || ahead as f64 > rate * 3.0 {
            self.start(idx)?;
        }
        let mut cur: Option<FrameRef> = None;
        while self.next <= idx {
            match self.read_one()? {
                Some(f) => cur = Some(f),
                None => break,
            }
        }
        let f = match cur {
            Some(f) => f,
            None => match &self.last {
                Some((_, f)) => f.clone(),
                None => {
                    // seek landed past the last decodable frame: back off and retry once
                    if idx > 0 {
                        self.start((idx - (rate as i64).max(1)).max(0))?;
                        let mut c2 = None;
                        while let Some(f) = self.read_one()? {
                            c2 = Some(f);
                            if self.next > idx {
                                break;
                            }
                        }
                        c2.ok_or_else(|| anyhow!("no frames decoded from {}", self.spec.path))?
                    } else {
                        bail!("no frames decoded from {}", self.spec.path)
                    }
                }
            },
        };
        self.last = Some((idx, f.clone()));
        Ok(f)
    }

    fn reverse_chunk(&mut self, idx: i64) -> Result<FrameRef> {
        let fb = self.frame_bytes().max(1);
        let n = ((512usize << 20) / fb).clamp(4, 60) as i64;
        let first = (idx - n + 1).max(0);
        self.start(first)?;
        let mut frames = Vec::with_capacity(n as usize);
        while self.next <= idx {
            match self.read_one()? {
                Some(f) => frames.push(f),
                None => break,
            }
        }
        self.proc = None;
        if frames.is_empty() {
            bail!("no frames decoded (reverse) from {}", self.spec.path);
        }
        let f = frames[((idx - first) as usize).min(frames.len() - 1)].clone();
        self.chunk = Some((first, frames));
        self.last = Some((idx, f.clone()));
        Ok(f)
    }
}

/// Shared frame/image cache used by the compositor.
pub struct FrameServer {
    pub ff: Arc<Ff>,
    readers: Mutex<HashMap<String, Arc<Mutex<VideoReader>>>>,
    stills: Mutex<HashMap<String, (FrameRef, Instant)>>,
    pub hw: bool,
    /// Preview proxies (the editor's frame server only; exports never use them).
    pub proxies: Option<Arc<super::proxy::Proxies>>,
}

impl FrameServer {
    pub fn new(ff: Arc<Ff>, hw: bool) -> FrameServer {
        FrameServer { ff, readers: Mutex::new(HashMap::new()), stills: Mutex::new(HashMap::new()), hw, proxies: None }
    }

    /// A frame server for previews: proxies, and hardware decoding only when asked
    /// for with MOODCUT_HWDEC=1 (CPU decoding keeps the GPU free by default).
    pub fn for_preview(ff: Arc<Ff>, proxies: Arc<super::proxy::Proxies>) -> FrameServer {
        let hw = std::env::var("MOODCUT_HWDEC").is_ok_and(|v| matches!(v.as_str(), "1" | "on" | "auto" | "true"));
        FrameServer { proxies: Some(proxies), ..FrameServer::new(ff, hw) }
    }

    /// `owner` scopes a reader to one timeline item so parallel clips of the
    /// same file get independent sequential decoders.
    pub fn video(&self, owner: &str, spec: &ReaderSpec, t: f64) -> Result<FrameRef> {
        let key = format!("{owner}|{}", spec.key());
        let r = {
            let mut g = self.readers.lock();
            g.entry(key).or_insert_with(|| Arc::new(Mutex::new(VideoReader::new(self.ff.clone(), spec.clone())))).clone()
        };
        let mut r = r.lock();
        r.frame_at(t)
    }

    /// A single still frame (freeze frames / hold), cached.
    pub fn video_still(&self, spec: &ReaderSpec, t: f64) -> Result<FrameRef> {
        let key = format!("still|{}|{:.4}", spec.key(), t);
        if let Some((f, _)) = self.stills.lock().get(&key) {
            return Ok(f.clone());
        }
        let mut r = VideoReader::new(self.ff.clone(), spec.clone());
        let f = r.frame_at(t)?;
        self.put_still(key, f.clone());
        Ok(f)
    }

    fn put_still(&self, key: String, f: FrameRef) {
        let mut g = self.stills.lock();
        if g.len() > 256 {
            let mut v: Vec<(String, Instant)> = g.iter().map(|(k, (_, t))| (k.clone(), *t)).collect();
            v.sort_by_key(|x| x.1);
            for (k, _) in v.into_iter().take(64) {
                g.remove(&k);
            }
        }
        g.insert(key, (f, Instant::now()));
    }

    /// Decoded image scaled to (w, h).
    pub fn image(&self, path: &str, w: u32, h: u32) -> Result<FrameRef> {
        let key = format!("img|{path}|{w}x{h}");
        if let Some((f, _)) = self.stills.lock().get(&key) {
            return Ok(f.clone());
        }
        let native = self.image_native(path)?;
        let f = if native.width() == w && native.height() == h {
            native
        } else {
            Arc::new(resize_pixmap(&native, w, h))
        };
        self.put_still(key, f.clone());
        Ok(f)
    }

    fn image_native(&self, path: &str) -> Result<FrameRef> {
        let key = format!("imgn|{path}");
        if let Some((f, _)) = self.stills.lock().get(&key) {
            return Ok(f.clone());
        }
        let pm = match image::open(path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                pixmap_from_rgba(rgba.into_raw(), w, h, true).ok_or_else(|| anyhow!("bad image {path}"))?
            }
            Err(_) => {
                // exotic formats: let ffmpeg decode one frame
                let (out, _) = self.ff.run_capture(&["-i".into(), path.into(), "-frames:v".into(), "1".into(), "-f".into(), "image2pipe".into(), "-vcodec".into(), "png".into(), "-".into()])?;
                let img = image::load_from_memory(&out)?;
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                pixmap_from_rgba(rgba.into_raw(), w, h, true).ok_or_else(|| anyhow!("bad image {path}"))?
            }
        };
        let f = Arc::new(pm);
        self.put_still(key, f.clone());
        Ok(f)
    }

    /// SVG rasterized at (w, h).
    pub fn svg(&self, path: &str, w: u32, h: u32) -> Result<FrameRef> {
        let key = format!("svg|{path}|{w}x{h}");
        if let Some((f, _)) = self.stills.lock().get(&key) {
            return Ok(f.clone());
        }
        let data = std::fs::read(path)?;
        let mut opt = resvg::usvg::Options::default();
        opt.fontdb = super::text::fontdb_arc();
        let tree = resvg::usvg::Tree::from_data(&data, &opt).map_err(|e| anyhow!("bad svg {path}: {e}"))?;
        let sz = tree.size();
        let mut pm = Pixmap::new(w.max(1), h.max(1)).ok_or_else(|| anyhow!("bad svg size"))?;
        let ts = resvg::tiny_skia::Transform::from_scale(w as f32 / sz.width(), h as f32 / sz.height());
        resvg::render(&tree, ts, &mut pm.as_mut());
        let f = Arc::new(pm);
        self.put_still(key, f.clone());
        Ok(f)
    }

    /// Drops readers idle for longer than `secs`.
    pub fn gc(&self, secs: f64) {
        let mut g = self.readers.lock();
        g.retain(|_, r| r.try_lock().map(|r| r.used.elapsed().as_secs_f64() < secs).unwrap_or(true));
    }

    pub fn clear(&self) {
        self.readers.lock().clear();
        self.stills.lock().clear();
    }
}

/// Resizes a premultiplied pixmap (area-averaging for downscale, bilinear for upscale).
pub fn resize_pixmap(src: &Pixmap, w: u32, h: u32) -> Pixmap {
    let img = image::RgbaImage::from_raw(src.width(), src.height(), src.data().to_vec()).unwrap();
    let filter = if w < src.width() / 2 { image::imageops::FilterType::Triangle } else { image::imageops::FilterType::CatmullRom };
    let out = image::imageops::resize(&img, w.max(1), h.max(1), filter);
    Pixmap::from_vec(out.into_raw(), IntSize::from_wh(w.max(1), h.max(1)).unwrap()).unwrap()
}
