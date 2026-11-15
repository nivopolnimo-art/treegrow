use crate::scan::{Kind, Node};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tiny_skia::Pixmap;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const TS: u32 = 128; // размер превью

pub fn cmd(path: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(path);
    #[cfg(windows)]
    c.creation_flags(0x0800_0000); // без чёрного окна консоли
    c
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            let c = dir.join(exe);
            if c.exists() {
                return Some(c);
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for p in std::env::split_paths(&paths) {
            let c = p.join(exe);
            if c.exists() {
                return Some(c);
            }
        }
    }
    None
}

pub struct Media {
    thumbs: Mutex<HashMap<usize, Arc<Pixmap>>>,
    waves: Mutex<HashMap<usize, Arc<Vec<f32>>>>,
    pending: Mutex<HashSet<usize>>,
    pub ffmpeg: Option<PathBuf>,
    budget: AtomicUsize,
}

impl Media {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            thumbs: Mutex::new(HashMap::new()),
            waves: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashSet::new()),
            ffmpeg: find_ffmpeg(),
            budget: AtomicUsize::new(0),
        })
    }

    pub fn set_budget(&self, n: usize) {
        self.budget.store(n, Ordering::Relaxed);
    }

    pub fn clear(&self) {
        self.thumbs.lock().unwrap().clear();
        self.waves.lock().unwrap().clear();
        self.pending.lock().unwrap().clear();
    }

    pub fn thumb(&self, id: usize) -> Option<Arc<Pixmap>> {
        self.thumbs.lock().unwrap().get(&id).cloned()
    }

    pub fn wave(&self, id: usize) -> Option<Arc<Vec<f32>>> {
        self.waves.lock().unwrap().get(&id).cloned()
    }

    /// Запросить загрузку превью/волны в фоне (не блокирует).
    pub fn want(self: &Arc<Self>, nodes: &Arc<Vec<Node>>, id: usize) {
        if !matches!(nodes[id].kind, Kind::Image | Kind::Video | Kind::Audio) {
            return;
        }
        if self.thumbs.lock().unwrap().contains_key(&id) || self.waves.lock().unwrap().contains_key(&id) {
            return;
        }
        {
            let mut p = self.pending.lock().unwrap();
            if p.contains(&id) {
                return;
            }
            if self.budget.load(Ordering::Relaxed) == 0 {
                return;
            }
            self.budget.fetch_sub(1, Ordering::Relaxed);
            p.insert(id);
        }
        let me = Arc::clone(self);
        let nodes = Arc::clone(nodes);
        rayon::spawn(move || {
            me.load_now(&nodes, id);
            me.pending.lock().unwrap().remove(&id);
        });
    }

    /// Синхронная загрузка (для экспорта).
    pub fn preload(self: &Arc<Self>, nodes: &Arc<Vec<Node>>, ids: &[usize], progress: &AtomicUsize) {
        ids.par_iter().for_each(|&id| {
            self.load_now(nodes, id);
            progress.fetch_add(1, Ordering::Relaxed);
        });
    }

    pub fn load_now(&self, nodes: &[Node], id: usize) {
        let nd = &nodes[id];
        match nd.kind {
            Kind::Image => {
                if let Some(p) = load_image_thumb(&nd.path) {
                    self.thumbs.lock().unwrap().insert(id, Arc::new(p));
                }
            }
            Kind::Video => {
                if let Some(ff) = &self.ffmpeg {
                    if let Some(p) = video_thumb(ff, &nd.path) {
                        self.thumbs.lock().unwrap().insert(id, Arc::new(p));
                    }
                }
            }
            Kind::Audio => {
                let w = audio_wave(&nd.path).unwrap_or_else(|| fake_wave(&nd.path));
                self.waves.lock().unwrap().insert(id, Arc::new(w));
            }
            _ => {}
        }
    }
}

fn rgba_to_pixmap(img: image::RgbaImage) -> Option<Pixmap> {
    let (w, h) = img.dimensions();
    let mut pm = Pixmap::new(w, h)?;
    for (dst, src) in pm.data_mut().chunks_exact_mut(4).zip(img.as_raw().chunks_exact(4)) {
        let a = src[3] as u32;
        dst[0] = (src[0] as u32 * a / 255) as u8;
        dst[1] = (src[1] as u32 * a / 255) as u8;
        dst[2] = (src[2] as u32 * a / 255) as u8;
        dst[3] = src[3];
    }
    Some(pm)
}

fn load_image_thumb(path: &Path) -> Option<Pixmap> {
    let md = std::fs::metadata(path).ok()?;
    if md.len() > 80_000_000 {
        return None;
    }
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let sq = img.resize_to_fill(TS, TS, image::imageops::FilterType::Triangle);
    rgba_to_pixmap(sq.to_rgba8())
}

fn video_thumb(ff: &Path, path: &Path) -> Option<Pixmap> {
    let vf = format!("scale={TS}:{TS}:force_original_aspect_ratio=increase,crop={TS}:{TS}");
    for ss in ["1", "0"] {
        let out = cmd(ff)
            .args(["-hide_banner", "-loglevel", "error", "-ss", ss, "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-vf", vf.as_str(), "-f", "image2pipe", "-vcodec", "png", "-"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if out.stdout.len() > 100 {
            if let Ok(img) = image::load_from_memory(&out.stdout) {
                return rgba_to_pixmap(img.to_rgba8());
            }
        }
    }
    None
}

const BARS: usize = 48;

/// Реальная форма волны через symphonia (mp3, wav, flac, ogg/vorbis, m4a/aac).
fn audio_wave(path: &Path) -> Option<Vec<f32>> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .ok()?;
    let mut format = probed.format;
    let track = format.default_track()?;
    let tid = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .ok()?;

    let mut peaks: Vec<f32> = Vec::new();
    while peaks.len() < 200_000 {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };
        if packet.track_id() != tid {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = *buf.spec();
                let mut sb = SampleBuffer::<f32>::new(buf.capacity() as u64, spec);
                sb.copy_interleaved_ref(buf);
                let m = sb.samples().iter().fold(0.0f32, |a, v| a.max(v.abs()));
                peaks.push(m);
            }
            Err(_) => continue,
        }
    }
    if peaks.len() < 4 {
        return None;
    }
    let len = peaks.len();
    let mut out = Vec::with_capacity(BARS);
    for b in 0..BARS {
        let a = b * len / BARS;
        let z = ((b + 1) * len / BARS).max(a + 1).min(len);
        out.push(peaks[a..z].iter().cloned().fold(0.0f32, f32::max));
    }
    let mx = out.iter().cloned().fold(0.0001f32, f32::max);
    for v in out.iter_mut() {
        *v = (*v / mx).clamp(0.0, 1.0);
    }
    Some(out)
}

/// Запасной вариант (например .opus, который symphonia не умеет): красивая псевдо-волна из имени файла.
fn fake_wave(path: &Path) -> Vec<f32> {
    let mut h: u64 = 1469598103934665603;
    for b in path.to_string_lossy().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    let mut v = Vec::with_capacity(BARS);
    let mut prev = 0.5f32;
    for _ in 0..BARS {
        h ^= h << 13;
        h ^= h >> 7;
        h ^= h << 17;
        let r = ((h >> 40) as f32) / 16_777_216.0;
        prev = (prev * 0.45 + r * 0.55).clamp(0.08, 1.0);
        v.push(prev);
    }
    v
}
