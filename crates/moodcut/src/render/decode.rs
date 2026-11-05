//! Frame sources: FFmpeg-backed video readers (with background prefetch and
//! chunked reverse playback), plus cached image and SVG rasterization.

use super::ffmpeg::Ff;
use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use resvg::tiny_skia::{IntSize, Pixmap};
use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
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
    /// Seconds the video starts after the file's start (camcorder .MTS and other
    /// files whose audio starts first). Source time t is time since the file's
    /// start, the clock the audio uses; reads before the video begins show its
    /// first frame. Decoders never start inside that gap: FFmpeg rebases the first
    /// frame after a seek to time 0, which would shift every later frame by the gap
    /// (MPEG-TS seeks stay within a frame or two; MP4/MOV are exact).
    pub start: f64,
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
    /// FFmpeg decoding threads (0: FFmpeg's choice, one per core).
    threads: usize,
    /// Held while this reader may decode on the GPU.
    _gpu: Option<GpuSlot>,
}

/// One of the GPU decoders allowed at a time; given back when its reader closes.
struct GpuSlot(Arc<AtomicUsize>);

impl Drop for GpuSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl VideoReader {
    pub fn new(ff: Arc<Ff>, spec: ReaderSpec) -> VideoReader {
        VideoReader { spec, ff, proc: None, next: 0, last: None, eof_at: None, chunk: None, last_req: None, used: Instant::now(), threads: 0, _gpu: None }
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
            c.args(self.ff.hwdec_args());
        }
        if self.threads > 0 {
            c.args(["-threads", &self.threads.to_string()]);
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
        let idx = ((t * rate + 1e-6).floor() as i64).max(self.first_index());
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
        // playing forward again (or seeking): the reverse buffer can go
        self.chunk = None;
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

    /// The frame index where the video starts (0 unless it starts after the file).
    fn first_index(&self) -> i64 {
        let max_t = (self.spec.src_dur - 0.5 / self.spec.rate).max(0.0);
        (self.spec.start.min(max_t) * self.spec.rate + 1e-6).floor() as i64
    }

    fn reverse_chunk(&mut self, idx: i64) -> Result<FrameRef> {
        let fb = self.frame_bytes().max(1);
        let n = ((256usize << 20) / fb).clamp(4, 60) as i64;
        let first = (idx - n + 1).max(self.first_index());
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
    /// When video decodes on the GPU.
    pub hwdec: super::gpu::HwDec,
    /// GPU decoders open now (capped: each holds video memory).
    gpu_live: Arc<AtomicUsize>,
    /// Preview proxies (the editor's frame server only; exports never use them).
    pub proxies: Option<Arc<super::proxy::Proxies>>,
    /// Video start offsets by path.
    starts: Mutex<HashMap<String, f64>>,
}

/// Open decoders above this are closed when idle (each is an FFmpeg process with frames buffered).
const MAX_READERS: usize = 32;
/// A decoder used this recently is never closed for the cap: every layer of a big comp keeps its own.
const READER_IDLE_SECS: f64 = 2.0;

/// GPU decoders at a time (`MOODCUT_HWDEC_MAX`, default 6); more decode on the CPU.
fn max_gpu_decoders() -> usize {
    std::env::var("MOODCUT_HWDEC_MAX").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(6)
}

/// Decoding threads per reader: the cores shared among the open decoders. FFmpeg
/// keeps a frame in flight per thread, so this bounds memory when many clips play at once.
fn decode_threads(open: usize) -> usize {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    (cores / open.max(1)).clamp(2, 16)
}

/// Memory for cached stills, images and graphics (`MOODCUT_IMAGE_CACHE_MB`, default 768).
fn still_budget() -> usize {
    std::env::var("MOODCUT_IMAGE_CACHE_MB").ok().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(768) << 20
}

impl FrameServer {
    /// `hw`: whether video may decode on the GPU at all (then as `MOODCUT_HWDEC` says).
    pub fn new(ff: Arc<Ff>, hw: bool) -> FrameServer {
        let hwdec = if hw { super::gpu::HwDec::from_env() } else { super::gpu::HwDec::Off };
        FrameServer { ff, readers: Mutex::new(HashMap::new()), stills: Mutex::new(HashMap::new()), hwdec, gpu_live: Arc::new(AtomicUsize::new(0)), proxies: None, starts: Mutex::new(HashMap::new()) }
    }

    /// Whether `m` decodes on the GPU here.
    pub fn hw_for(&self, m: &crate::model::project::Media) -> bool {
        self.ff.gpu_decode(m, self.hwdec)
    }

    /// Where a file's video starts after the file's start (measured once per file).
    pub fn video_start(&self, path: &str) -> f64 {
        if let Some(v) = self.starts.lock().get(path) {
            return *v;
        }
        let v = self.ff.video_start_offset(path);
        self.starts.lock().insert(path.to_string(), v);
        v
    }

    /// A frame server for previews: proxies, and heavy footage without one decodes
    /// on a discrete GPU (its video engine is separate from the one drawing the editor).
    pub fn for_preview(ff: Arc<Ff>, proxies: Arc<super::proxy::Proxies>) -> FrameServer {
        FrameServer { proxies: Some(proxies), ..FrameServer::new(ff, true) }
    }

    /// `owner` scopes a reader to one timeline item so parallel clips of the
    /// same file get independent sequential decoders.
    pub fn video(&self, owner: &str, spec: &ReaderSpec, t: f64) -> Result<FrameRef> {
        let key = format!("{owner}|{}", spec.key());
        let r = {
            let mut g = self.readers.lock();
            let open = g.get(&key).cloned();
            if let Some(r) = open {
                r
            } else {
                let mut vr = VideoReader::new(self.ff.clone(), spec.clone());
                if vr.spec.hw {
                    if self.gpu_live.fetch_add(1, Ordering::Relaxed) < max_gpu_decoders() {
                        vr._gpu = Some(GpuSlot(self.gpu_live.clone()));
                    } else {
                        self.gpu_live.fetch_sub(1, Ordering::Relaxed);
                        vr.spec.hw = false;
                    }
                }
                vr.threads = decode_threads(g.len() + 1);
                let r = Arc::new(Mutex::new(vr));
                g.insert(key.clone(), r.clone());
                close_idle_readers(&mut g, &key);
                r
            }
        };
        let mut r = r.lock();
        r.frame_at(t)
    }

    /// A single still frame (freeze frames / hold), cached.
    pub fn video_still(&self, spec: &ReaderSpec, t: f64) -> Result<FrameRef> {
        let key = format!("still|{}|{:.4}", spec.key(), t);
        if let Some(f) = self.get_still(&key) {
            return Ok(f);
        }
        let mut spec = spec.clone();
        spec.hw = false; // one frame: not worth starting a GPU decoder
        let mut r = VideoReader::new(self.ff.clone(), spec);
        let f = r.frame_at(t)?;
        self.put_still(key, f.clone());
        Ok(f)
    }

    fn get_still(&self, key: &str) -> Option<FrameRef> {
        let mut g = self.stills.lock();
        let e = g.get_mut(key)?;
        e.1 = Instant::now();
        Some(e.0.clone())
    }

    /// Caches a still, dropping the least recently used ones to stay within the
    /// memory budget (a 4K frame is 33 MB, a 24-megapixel photo 96 MB).
    fn put_still(&self, key: String, f: FrameRef) {
        let budget = still_budget();
        let mut g = self.stills.lock();
        g.remove(&key);
        let mut total: usize = g.values().map(|(s, _)| s.data().len()).sum::<usize>() + f.data().len();
        if total > budget || g.len() >= 256 {
            let mut v: Vec<(String, Instant, usize)> = g.iter().map(|(k, (s, t))| (k.clone(), *t, s.data().len())).collect();
            v.sort_by_key(|x| x.1);
            for (k, _, n) in v {
                if total <= budget && g.len() < 256 {
                    break;
                }
                g.remove(&k);
                total = total.saturating_sub(n);
            }
        }
        g.insert(key, (f, Instant::now()));
    }

    /// Decoded image scaled to (w, h).
    pub fn image(&self, path: &str, w: u32, h: u32) -> Result<FrameRef> {
        let key = format!("img|{path}|{w}x{h}");
        if let Some(f) = self.get_still(&key) {
            return Ok(f);
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
        if let Some(f) = self.get_still(&key) {
            return Ok(f);
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
        if let Some(f) = self.get_still(&key) {
            return Ok(f);
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

/// Above [`MAX_READERS`], closes the decoders idle longest (not `keep`, the one just
/// opened, nor any used in the last [`READER_IDLE_SECS`]).
fn close_idle_readers(g: &mut HashMap<String, Arc<Mutex<VideoReader>>>, keep: &str) {
    if g.len() <= MAX_READERS {
        return;
    }
    let mut idle: Vec<(String, Instant)> = g
        .iter()
        .filter(|(k, _)| k.as_str() != keep)
        .filter_map(|(k, r)| r.try_lock().map(|r| (k.clone(), r.used)))
        .filter(|(_, used)| used.elapsed().as_secs_f64() > READER_IDLE_SECS)
        .collect();
    idle.sort_by_key(|x| x.1);
    let excess = g.len() - MAX_READERS;
    for (k, _) in idle.into_iter().take(excess) {
        g.remove(&k);
    }
}

/// Resizes a premultiplied pixmap (area-averaging for downscale, bilinear for upscale).
pub fn resize_pixmap(src: &Pixmap, w: u32, h: u32) -> Pixmap {
    let img = image::RgbaImage::from_raw(src.width(), src.height(), src.data().to_vec()).unwrap();
    let filter = if w < src.width() / 2 { image::imageops::FilterType::Triangle } else { image::imageops::FilterType::CatmullRom };
    let out = image::imageops::resize(&img, w.max(1), h.max(1), filter);
    Pixmap::from_vec(out.into_raw(), IntSize::from_wh(w.max(1), h.max(1)).unwrap()).unwrap()
}
