use crate::engine::{Engine, Settings};
use crate::media::{cmd, Media};
use crate::render::Renderer;
use crate::scan::{is_media, Node};
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tiny_skia::Pixmap;

pub struct Job {
    pub out: PathBuf,
    pub w: u32,
    pub h: u32,
    pub fps: u32,
    pub crf: u32,
    pub alpha: bool, // MOV ProRes 4444 с прозрачностью
}

/// phase: 0 подготовка, 1 рендер, 2 готово, 3 ошибка, 4 отменено
pub struct ExportState {
    pub phase: AtomicUsize,
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
    pub error: Mutex<String>,
}

impl ExportState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            phase: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
            total: AtomicUsize::new(1),
            cancel: AtomicBool::new(false),
            error: Mutex::new(String::new()),
        })
    }
}

pub fn run(nodes: Arc<Vec<Node>>, settings: Settings, font: Vec<u8>, job: Job, st: Arc<ExportState>) {
    if let Err(e) = run_inner(nodes, settings, font, job, &st) {
        *st.error.lock().unwrap() = e;
        st.phase.store(3, Ordering::SeqCst);
    }
}

fn run_inner(nodes: Arc<Vec<Node>>, settings: Settings, font: Vec<u8>, job: Job, st: &Arc<ExportState>) -> Result<(), String> {
    let media = Media::new();
    let ff = media
        .ffmpeg
        .clone()
        .ok_or_else(|| "ffmpeg не найден: положи ffmpeg.exe рядом с программой".to_string())?;

    let mut eng = Engine::new(nodes.clone(), settings.clone());

    // Превью грузим заранее, чтобы кадры были детерминированными
    if settings.thumbs {
        let ids: Vec<usize> = eng
            .order
            .iter()
            .copied()
            .filter(|&i| is_media(nodes[i].kind))
            .take(settings.max_media)
            .collect();
        st.total.store(ids.len().max(1), Ordering::SeqCst);
        st.done.store(0, Ordering::SeqCst);
        media.preload(&nodes, &ids, &st.done);
    }
    media.set_budget(0);

    let fps = job.fps.max(1);
    let ticks = (60 / fps).max(1);
    let frames = (eng.total_time() * fps as f32).ceil() as usize + 1;
    st.total.store(frames, Ordering::SeqCst);
    st.done.store(0, Ordering::SeqCst);
    st.phase.store(1, Ordering::SeqCst);

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-s".into(),
        format!("{}x{}", job.w, job.h),
        "-r".into(),
        fps.to_string(),
        "-i".into(),
        "-".into(),
    ];
    if job.alpha {
        for a in ["-c:v", "prores_ks", "-profile:v", "4444", "-pix_fmt", "yuva444p10le", "-alpha_bits", "16"] {
            args.push(a.into());
        }
    } else {
        for a in ["-c:v", "libx264", "-preset", "medium", "-crf"] {
            args.push(a.into());
        }
        args.push(job.crf.to_string());
        for a in ["-pix_fmt", "yuv420p", "-movflags", "+faststart"] {
            args.push(a.into());
        }
    }
    let mut child = cmd(&ff)
        .args(&args)
        .arg(&job.out)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("не удалось запустить ffmpeg: {e}"))?;
    let mut stdin = child.stdin.take().ok_or_else(|| "нет stdin у ffmpeg".to_string())?;

    let renderer = Renderer::new(font);
    let mut pm = Pixmap::new(job.w, job.h).ok_or_else(|| "неверный размер кадра".to_string())?;
    let mut buf: Vec<u8> = Vec::new();

    for f in 0..frames {
        if st.cancel.load(Ordering::SeqCst) {
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
            st.phase.store(4, Ordering::SeqCst);
            return Ok(());
        }
        for _ in 0..ticks {
            eng.step(&media);
        }
        renderer.render(&mut pm, &eng, &media);
        let res = if job.alpha {
            unpremultiply(pm.data(), &mut buf);
            stdin.write_all(&buf)
        } else {
            stdin.write_all(pm.data())
        };
        res.map_err(|e| format!("ffmpeg закрылся досрочно: {e}"))?;
        st.done.store(f + 1, Ordering::Relaxed);
    }
    drop(stdin);
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("ffmpeg завершился с ошибкой".to_string());
    }
    st.phase.store(2, Ordering::SeqCst);
    Ok(())
}

/// tiny-skia хранит цвет с премультипликацией, ffmpeg ждёт обычный (straight) альфа-канал.
fn unpremultiply(src: &[u8], buf: &mut Vec<u8>) {
    buf.clear();
    buf.extend_from_slice(src);
    for px in buf.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in px.iter_mut().take(3) {
                *c = ((*c as u32 * 255) / a).min(255) as u8;
            }
        }
    }
}
