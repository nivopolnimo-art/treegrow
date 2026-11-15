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

    let args: Vec<String> = vec![
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
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "medium".into(),
        "-crf".into(),
        job.crf.to_string(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-movflags".into(),
        "+faststart".into(),
    ];
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
        stdin
            .write_all(pm.data())
            .map_err(|e| format!("ffmpeg закрылся досрочно: {e}"))?;
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
