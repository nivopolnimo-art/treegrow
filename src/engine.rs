use crate::media::Media;
use crate::scan::{birth_order, Node};
use crate::sim::Sim;
use std::sync::Arc;

/// Шаг симуляции: всегда 1/60 с, чтобы превью и экспорт выглядели одинаково.
pub const DT: f32 = 1.0 / 60.0;

#[derive(Clone)]
pub struct Settings {
    pub nps: f32, // узлов в секунду
    pub chrono: bool,
    pub thumbs: bool,
    pub max_media: usize,
    pub labels: bool,
    pub glow: bool,
    pub theme: usize,
    pub tail: f32, // секунд тишины в конце
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            nps: 120.0,
            chrono: false,
            thumbs: true,
            max_media: 800,
            labels: true,
            glow: true,
            theme: 0,
            tail: 3.0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Camera {
    pub cx: f32,
    pub cy: f32,
    pub hw: f32,
    pub hh: f32,
}

impl Camera {
    pub fn zoom(&self, w: f32, h: f32) -> f32 {
        (w / (2.0 * self.hw)).min(h / (2.0 * self.hh)) * 0.92
    }
}

pub struct Engine {
    pub nodes: Arc<Vec<Node>>,
    pub order: Vec<usize>,
    pub birth: Vec<f32>,
    pub sim: Sim,
    pub next: usize,
    pub time: f32,
    pub cam: Camera,
    pub auto_cam: bool,
    pub settings: Settings,
}

impl Engine {
    pub fn new(nodes: Arc<Vec<Node>>, settings: Settings) -> Self {
        let order = birth_order(&nodes, settings.chrono);
        let mut birth = vec![0.0f32; nodes.len()];
        for (k, &id) in order.iter().enumerate() {
            birth[id] = k as f32 / settings.nps;
        }
        let sim = Sim::new(&nodes);
        Self {
            nodes,
            order,
            birth,
            sim,
            next: 0,
            time: 0.0,
            cam: Camera { cx: 0.0, cy: 0.0, hw: 200.0, hh: 120.0 },
            auto_cam: true,
            settings,
        }
    }

    /// Смена скорости на лету: пересчитываем времена ещё не появившихся узлов.
    pub fn set_nps(&mut self, nps: f32) {
        self.settings.nps = nps;
        for k in self.next..self.order.len() {
            let id = self.order[k];
            self.birth[id] = self.time + (k - self.next) as f32 / nps;
        }
    }

    pub fn total_time(&self) -> f32 {
        self.order.last().map(|&id| self.birth[id]).unwrap_or(0.0) + self.settings.tail
    }

    pub fn finished(&self) -> bool {
        self.next >= self.order.len()
    }

    pub fn step(&mut self, media: &Arc<Media>) {
        self.time += DT;
        while self.next < self.order.len() && self.birth[self.order[self.next]] <= self.time {
            let id = self.order[self.next];
            self.sim.spawn(id, self.nodes[id].parent);
            self.next += 1;
            if self.settings.thumbs {
                media.want(&self.nodes, id);
            }
        }
        self.sim.step(&self.nodes);

        if self.auto_cam && !self.sim.alive.is_empty() {
            let b = self.sim.bbox;
            let tcx = (b[0] + b[2]) * 0.5;
            let tcy = (b[1] + b[3]) * 0.5;
            let thw = ((b[2] - b[0]) * 0.5 + 60.0).max(150.0);
            let thh = ((b[3] - b[1]) * 0.5 + 60.0).max(100.0);
            let k = 0.05;
            self.cam.cx += (tcx - self.cam.cx) * k;
            self.cam.cy += (tcy - self.cam.cy) * k;
            self.cam.hw += (thw - self.cam.hw) * k;
            self.cam.hh += (thh - self.cam.hh) * k;
        }
    }
}

use crate::scan::Node;

const R: f32 = 64.0; // радиус отталкивания

/// Силовая раскладка графа (пружины + отталкивание через сетку + гравитация).
pub struct Sim {
    pub pos: Vec<[f32; 2]>,
    vel: Vec<[f32; 2]>,
    pub alive: Vec<usize>,
    pub is_alive: Vec<bool>,
    pub bbox: [f32; 4],
    rest: Vec<f32>,
    head: Vec<u32>,
    next: Vec<u32>,
    cell: Vec<u32>,
    rng: u64,
}

impl Sim {
    pub fn new(nodes: &[Node]) -> Self {
        let n = nodes.len();
        let rest = nodes
            .iter()
            .map(|nd| match nd.parent {
                Some(p) => 24.0 + (nodes[p].children as f32).sqrt() * 4.0,
                None => 0.0,
            })
            .collect();
        Self {
            pos: vec![[0.0; 2]; n],
            vel: vec![[0.0; 2]; n],
            alive: Vec::new(),
            is_alive: vec![false; n],
            bbox: [-100.0, -100.0, 100.0, 100.0],
            rest,
            head: Vec::new(),
            next: vec![u32::MAX; n],
            cell: vec![0; n],
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        ((x >> 40) as f32) / 16_777_216.0
    }

    pub fn spawn(&mut self, id: usize, parent: Option<usize>) {
        let base = parent.map(|p| self.pos[p]).unwrap_or([0.0, 0.0]);
        let a = self.rand() * std::f32::consts::TAU;
        let r = 3.0 + self.rand() * 4.0;
        self.pos[id] = [base[0] + a.cos() * r, base[1] + a.sin() * r];
        self.vel[id] = [0.0, 0.0];
        self.is_alive[id] = true;
        self.alive.push(id);
    }

    pub fn step(&mut self, nodes: &[Node]) {
        if self.alive.is_empty() {
            return;
        }

        // Пружины по рёбрам родитель-ребёнок
        for &i in &self.alive {
            let Some(p) = nodes[i].parent else { continue };
            if !self.is_alive[p] {
                continue;
            }
            let dx = self.pos[i][0] - self.pos[p][0];
            let dy = self.pos[i][1] - self.pos[p][1];
            let d = (dx * dx + dy * dy).sqrt().max(0.01);
            let f = (d - self.rest[i]) * 0.035;
            let fx = dx / d * f;
            let fy = dy / d * f;
            let dp = (nodes[p].children as f32 + if nodes[p].parent.is_some() { 1.0 } else { 0.0 }).max(1.0);
            let di = nodes[i].children as f32 + 1.0;
            let b = dp / (dp + di);
            self.vel[i][0] -= fx * b;
            self.vel[i][1] -= fy * b;
            self.vel[p][0] += fx * (1.0 - b);
            self.vel[p][1] += fy * (1.0 - b);
        }

        // Границы для сетки
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &i in &self.alive {
            let p = self.pos[i];
            minx = minx.min(p[0]);
            miny = miny.min(p[1]);
            maxx = maxx.max(p[0]);
            maxy = maxy.max(p[1]);
        }
        let gw = (((maxx - minx) / R) as usize) + 1;
        let gh = (((maxy - miny) / R) as usize) + 1;

        if gw * gh <= 4_000_000 {
            self.head.clear();
            self.head.resize(gw * gh, u32::MAX);
            for &i in &self.alive {
                let p = self.pos[i];
                let cx = (((p[0] - minx) / R) as usize).min(gw - 1);
                let cy = (((p[1] - miny) / R) as usize).min(gh - 1);
                let c = cy * gw + cx;
                self.cell[i] = c as u32;
                self.next[i] = self.head[c];
                self.head[c] = i as u32;
            }
            for &i in &self.alive {
                let c = self.cell[i] as usize;
                let (cx, cy) = (c % gw, c / gw);
                for oy in cy.saturating_sub(1)..=(cy + 1).min(gh - 1) {
                    for ox in cx.saturating_sub(1)..=(cx + 1).min(gw - 1) {
                        let mut j = self.head[oy * gw + ox];
                        while j != u32::MAX {
                            let jj = j as usize;
                            if jj > i {
                                let dx = self.pos[i][0] - self.pos[jj][0];
                                let dy = self.pos[i][1] - self.pos[jj][1];
                                let d2 = dx * dx + dy * dy;
                                if d2 < R * R {
                                    let (ux, uy, d) = if d2 < 1e-6 {
                                        (1.0, 0.0, 0.001)
                                    } else {
                                        let d = d2.sqrt();
                                        (dx / d, dy / d, d)
                                    };
                                    let push = (R - d) / R * 0.6;
                                    self.vel[i][0] += ux * push;
                                    self.vel[i][1] += uy * push;
                                    self.vel[jj][0] -= ux * push;
                                    self.vel[jj][1] -= uy * push;
                                }
                            }
                            j = self.next[jj];
                        }
                    }
                }
            }
        }

        // Интегрирование
        let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &i in &self.alive {
            let p = self.pos[i];
            let v = &mut self.vel[i];
            v[0] -= p[0] * 0.0006;
            v[1] -= p[1] * 0.0006;
            v[0] *= 0.80;
            v[1] *= 0.80;
            let sp = (v[0] * v[0] + v[1] * v[1]).sqrt();
            if sp > 12.0 {
                v[0] *= 12.0 / sp;
                v[1] *= 12.0 / sp;
            }
            let np = [p[0] + v[0], p[1] + v[1]];
            self.pos[i] = np;
            bx0 = bx0.min(np[0]);
            by0 = by0.min(np[1]);
            bx1 = bx1.max(np[0]);
            by1 = by1.max(np[1]);
        }
        self.bbox = [bx0, by0, bx1, by1];
    }
}

#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

mod engine;
mod export;
mod media;
mod render;
mod scan;
mod sim;

use eframe::egui;
use engine::{Engine, Settings, DT};
use export::{ExportState, Job};
use media::Media;
use render::{Renderer, THEMES};
use scan::{Node, ScanOpts, ScanShared};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tiny_skia::Pixmap;

const RES: [(&str, u32, u32); 4] = [
    ("HD 1280×720", 1280, 720),
    ("Full HD 1920×1080", 1920, 1080),
    ("2K 2560×1440", 2560, 1440),
    ("4K 3840×2160", 3840, 2160),
];

#[cfg(windows)]
fn all_drives() -> Vec<PathBuf> {
    (b'A'..=b'Z')
        .map(|c| PathBuf::from(format!("{}:\\", c as char)))
        .filter(|p| p.exists())
        .collect()
}

#[cfg(not(windows))]
fn all_drives() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

struct App {
    settings: Settings,
    path: String,
    whole_pc: bool,
    only_media: bool,
    skip_junk: bool,
    max_nodes: usize,
    scan: Option<Arc<ScanShared>>,
    nodes: Option<Arc<Vec<Node>>>,
    engine: Option<Engine>,
    media: Arc<Media>,
    renderer: Renderer,
    font: Vec<u8>,
    tex: Option<egui::TextureHandle>,
    pm: Option<Pixmap>,
    playing: bool,
    accum: f32,
    preview_scale: f32,
    export: Option<Arc<ExportState>>,
    exp_res: usize,
    exp_fps: u32,
    msg: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut v = egui::Visuals::dark();
        v.panel_fill = egui::Color32::from_rgb(20, 18, 30);
        v.window_fill = egui::Color32::from_rgb(24, 22, 36);
        v.selection.bg_fill = egui::Color32::from_rgb(124, 92, 235);
        v.widgets.hovered.bg_fill = egui::Color32::from_rgb(70, 58, 120);
        v.widgets.active.bg_fill = egui::Color32::from_rgb(124, 92, 235);
        cc.egui_ctx.set_visuals(v);

        let font = egui::FontDefinitions::default()
            .font_data
            .get("Ubuntu-Light")
            .map(|d| d.font.to_vec())
            .expect("встроенный шрифт не найден");

        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();

        Self {
            settings: Settings::default(),
            path: home,
            whole_pc: false,
            only_media: false,
            skip_junk: true,
            max_nodes: 15000,
            scan: None,
            nodes: None,
            engine: None,
            media: Media::new(),
            renderer: Renderer::new(font.clone()),
            font,
            tex: None,
            pm: None,
            playing: false,
            accum: 0.0,
            preview_scale: 0.75,
            export: None,
            exp_res: 1,
            exp_fps: 30,
            msg: String::new(),
        }
    }

    fn start_scan(&mut self) {
        let roots = if self.whole_pc {
            all_drives()
        } else {
            vec![PathBuf::from(self.path.trim())]
        };
        if roots.is_empty() || (!self.whole_pc && !roots[0].is_dir()) {
            self.msg = "Папка не найдена".into();
            return;
        }
        let opts = ScanOpts {
            roots,
            only_media: self.only_media,
            skip_junk: self.skip_junk,
            max_nodes: self.max_nodes,
        };
        let sh = ScanShared::new();
        let sh2 = sh.clone();
        std::thread::spawn(move || {
            let r = scan::scan(&opts, &sh2.count, &sh2.cancel);
            *sh2.result.lock().unwrap() = Some(r);
            sh2.done.store(true, Ordering::SeqCst);
        });
        self.scan = Some(sh);
        self.msg.clear();
    }

    fn start_engine(&mut self) {
        if let Some(n) = &self.nodes {
            self.media.clear();
            self.media.set_budget(self.settings.max_media);
            self.engine = Some(Engine::new(n.clone(), self.settings.clone()));
            self.playing = true;
            self.accum = 0.0;
        }
    }

    fn poll_scan(&mut self, ctx: &egui::Context) {
        let mut finished = None;
        if let Some(sc) = &self.scan {
            if sc.done.load(Ordering::SeqCst) {
                finished = sc.result.lock().unwrap().take();
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let Some(v) = finished {
            self.scan = None;
            if v.is_empty() {
                self.msg = "Ничего не найдено".into();
            } else {
                self.nodes = Some(Arc::new(v));
                self.start_engine();
            }
        }
    }

    fn advance(&mut self, ctx: &egui::Context) {
        if !self.playing {
            return;
        }
        let Some(eng) = self.engine.as_mut() else { return };
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        self.accum += dt;
        let mut n = 0;
        while self.accum >= DT && n < 8 {
            eng.step(&self.media);
            self.accum -= DT;
            n += 1;
        }
        if n == 8 {
            self.accum = 0.0;
        }
        ctx.request_repaint();
    }

    fn sync_engine(&mut self) {
        let mut restart = false;
        if let Some(eng) = self.engine.as_mut() {
            if (eng.settings.nps - self.settings.nps).abs() > 0.01 {
                eng.set_nps(self.settings.nps);
            }
            eng.settings.labels = self.settings.labels;
            eng.settings.glow = self.settings.glow;
            eng.settings.theme = self.settings.theme;
            eng.settings.thumbs = self.settings.thumbs;
            eng.settings.tail = self.settings.tail;
            if eng.settings.chrono != self.settings.chrono {
                restart = true;
            }
        }
        if restart {
            self.start_engine();
        }
    }

    fn start_export(&mut self, out: PathBuf) {
        let Some(nodes) = self.nodes.clone() else { return };
        let (_, w, h) = RES[self.exp_res];
        let st = ExportState::new();
        let job = Job { out, w, h, fps: self.exp_fps, crf: 16 };
        let settings = self.settings.clone();
        let font = self.font.clone();
        let st2 = st.clone();
        std::thread::spawn(move || export::run(nodes, settings, font, job, st2));
        self.export = Some(st);
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("side").exact_width(330.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("TreeGrow").size(26.0).strong());
                ui.label(egui::RichText::new("рост файловой вселенной").weak());
                ui.separator();

                ui.label(egui::RichText::new("Источник").strong());
                ui.checkbox(&mut self.whole_pc, "Весь компьютер (все диски)");
                ui.add_enabled_ui(!self.whole_pc, |ui| {
                    ui.text_edit_singleline(&mut self.path);
                    if ui.button("Выбрать папку…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.path = p.to_string_lossy().into_owned();
                        }
                    }
                });
                ui.checkbox(&mut self.only_media, "Только папки и медиа (фото, видео, аудио)");
                ui.checkbox(&mut self.skip_junk, "Пропускать мусор (node_modules, .git, Windows…)");
                ui.add(egui::Slider::new(&mut self.max_nodes, 1000..=60000).text("макс. узлов"));

                let scanning = self.scan.is_some();
                if scanning {
                    let c = self.scan.as_ref().map(|s| s.count.load(Ordering::Relaxed)).unwrap_or(0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Сканирую… {c}"));
                    });
                    if ui.button("Отмена").clicked() {
                        if let Some(s) = &self.scan {
                            s.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                } else if ui
                    .add_sized([ui.available_width(), 34.0], egui::Button::new(egui::RichText::new("Построить").strong()))
                    .clicked()
                {
                    self.start_scan();
                }
                if !self.msg.is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(250, 120, 120), &self.msg);
                }
                ui.separator();

                ui.label(egui::RichText::new("Анимация").strong());
                ui.add(
                    egui::Slider::new(&mut self.settings.nps, 5.0..=3000.0)
                        .logarithmic(true)
                        .text("скорость, узлов/с"),
                );
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.settings.chrono, false, "Рост от корня");
                    ui.radio_value(&mut self.settings.chrono, true, "По дате файлов");
                });
                ui.checkbox(&mut self.settings.thumbs, "Фото и видео в узлах");
                ui.add(egui::Slider::new(&mut self.settings.max_media, 0..=3000).text("макс. превью"));
                ui.checkbox(&mut self.settings.labels, "Подписи папок");
                ui.checkbox(&mut self.settings.glow, "Свечение");
                if let Some(eng) = self.engine.as_mut() {
                    ui.checkbox(&mut eng.auto_cam, "Авто-камера (иначе: тяни и крути колесо)");
                }
                egui::ComboBox::from_label("тема")
                    .selected_text(THEMES[self.settings.theme].name)
                    .show_ui(ui, |ui| {
                        for (i, t) in THEMES.iter().enumerate() {
                            ui.selectable_value(&mut self.settings.theme, i, t.name);
                        }
                    });
                ui.add(egui::Slider::new(&mut self.preview_scale, 0.4..=1.0).text("качество превью"));

                ui.horizontal(|ui| {
                    let label = if self.playing { "Пауза" } else { "Играть" };
                    if ui.button(label).clicked() {
                        self.playing = !self.playing;
                    }
                    if ui.button("Заново").clicked() {
                        self.start_engine();
                    }
                });
                if let Some(eng) = &self.engine {
                    let f = eng.next as f32 / eng.order.len().max(1) as f32;
                    ui.add(egui::ProgressBar::new(f).text(format!("{} / {}", eng.next, eng.order.len())));
                    ui.label(format!("Длительность ролика ≈ {:.0} с", eng.total_time()));
                }
                ui.separator();

                ui.label(egui::RichText::new("Сохранить видео").strong());
                egui::ComboBox::from_label("разрешение")
                    .selected_text(RES[self.exp_res].0)
                    .show_ui(ui, |ui| {
                        for (i, r) in RES.iter().enumerate() {
                            ui.selectable_value(&mut self.exp_res, i, r.0);
                        }
                    });
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.exp_fps, 30, "30 к/с");
                    ui.radio_value(&mut self.exp_fps, 60, "60 к/с");
                });

                let exporting = self.export.as_ref().map(|s| s.phase.load(Ordering::SeqCst) < 2).unwrap_or(false);
                if let Some(st) = self.export.clone() {
                    let ph = st.phase.load(Ordering::SeqCst);
                    let done = st.done.load(Ordering::Relaxed);
                    let total = st.total.load(Ordering::Relaxed).max(1);
                    match ph {
                        0 => {
                            ui.add(egui::ProgressBar::new(done as f32 / total as f32).text("готовлю превью…"));
                            ui.ctx().request_repaint_after(Duration::from_millis(100));
                        }
                        1 => {
                            ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(format!("кадр {done} / {total}")));
                            ui.ctx().request_repaint_after(Duration::from_millis(100));
                        }
                        2 => {
                            ui.colored_label(egui::Color32::from_rgb(120, 220, 160), "Готово: видео сохранено");
                        }
                        3 => {
                            ui.colored_label(egui::Color32::from_rgb(250, 120, 120), st.error.lock().unwrap().clone());
                        }
                        _ => {
                            ui.label("Экспорт отменён");
                        }
                    }
                    if exporting && ui.button("Отменить экспорт").clicked() {
                        st.cancel.store(true, Ordering::SeqCst);
                    }
                }
                let can = self.nodes.is_some() && !exporting;
                if ui
                    .add_enabled(can, egui::Button::new("Сохранить видео…").min_size(egui::vec2(ui.available_width(), 34.0)))
                    .clicked()
                {
                    if let Some(out) = rfd::FileDialog::new()
                        .add_filter("MP4 видео", &["mp4"])
                        .set_file_name("treegrow.mp4")
                        .save_file()
                    {
                        self.start_export(out);
                    }
                }
                if self.media.ffmpeg.is_none() {
                    ui.small("ffmpeg.exe не найден рядом с программой: экспорт видео и превью видео не будут работать");
                }
            });
        });
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(8, 8, 12)))
            .show(ctx, |ui| {
                let avail = ui.available_size();
                let (rect, resp) = ui.allocate_exact_size(avail, egui::Sense::click_and_drag());

                let Some(eng) = self.engine.as_mut() else {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Выбери папку (или весь компьютер) и нажми «Построить»",
                        egui::FontId::proportional(22.0),
                        egui::Color32::from_gray(140),
                    );
                    return;
                };

                if !eng.auto_cam {
                    let z = eng.cam.zoom(rect.width(), rect.height());
                    if resp.dragged() {
                        let d = resp.drag_delta();
                        eng.cam.cx -= d.x / z;
                        eng.cam.cy -= d.y / z;
                    }
                    let sc = ui.input(|i| i.smooth_scroll_delta.y);
                    if resp.hovered() && sc != 0.0 {
                        let f = (-sc * 0.002).exp();
                        eng.cam.hw *= f;
                        eng.cam.hh *= f;
                    }
                }

                let ppp = ui.ctx().pixels_per_point();
                let mut pw = (rect.width() * ppp * self.preview_scale).max(64.0);
                let mut ph = (rect.height() * ppp * self.preview_scale).max(64.0);
                let s = (1920.0 / pw).min(1080.0 / ph).min(1.0);
                pw *= s;
                ph *= s;
                let (pw, ph) = (pw as u32, ph as u32);

                if self.pm.as_ref().map(|p| (p.width(), p.height())) != Some((pw, ph)) {
                    self.pm = Pixmap::new(pw, ph);
                }
                let Some(pm) = self.pm.as_mut() else { return };
                self.renderer.render(pm, &*eng, &self.media);

                let img = egui::ColorImage::from_rgba_premultiplied([pw as usize, ph as usize], pm.data());
                match &mut self.tex {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => self.tex = Some(ui.ctx().load_texture("preview", img, egui::TextureOptions::LINEAR)),
                }
                if let Some(t) = &self.tex {
                    ui.painter().image(
                        t.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_scan(ctx);
        self.advance(ctx);
        self.side_panel(ctx);
        self.sync_engine();
        self.central(ctx);
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1480.0, 880.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("TreeGrow — рост файловой вселенной"),
        ..Default::default()
    };
    eframe::run_native("TreeGrow", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

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

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Dir,
    Image,
    Video,
    Audio,
    Other,
}

#[derive(Clone)]
pub struct Node {
    pub parent: Option<usize>,
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    pub mtime: u64,
    pub depth: u32,
    pub children: u32,
}

pub fn kind_of(path: &Path) -> Kind {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp" | "heic" | "tif" | "tiff" => Kind::Image,
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg" => Kind::Video,
        "mp3" | "wav" | "ogg" | "oga" | "opus" | "m4a" | "flac" | "aac" | "wma" | "amr" => Kind::Audio,
        _ => Kind::Other,
    }
}

pub fn is_media(k: Kind) -> bool {
    matches!(k, Kind::Image | Kind::Video | Kind::Audio)
}

#[derive(Clone)]
pub struct ScanOpts {
    pub roots: Vec<PathBuf>,
    pub only_media: bool,
    pub skip_junk: bool,
    pub max_nodes: usize,
}

pub struct ScanShared {
    pub count: AtomicUsize,
    pub done: AtomicBool,
    pub cancel: AtomicBool,
    pub result: Mutex<Option<Vec<Node>>>,
}

impl ScanShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            count: AtomicUsize::new(0),
            done: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            result: Mutex::new(None),
        })
    }
}

fn keep(e: &walkdir::DirEntry, o: &ScanOpts) -> bool {
    if e.depth() == 0 || !o.skip_junk || !e.file_type().is_dir() {
        return true;
    }
    const JUNK: [&str; 9] = [
        "node_modules",
        "$RECYCLE.BIN",
        "System Volume Information",
        "Windows",
        "AppData",
        "$WinREAgent",
        "target",
        "__pycache__",
        "Recovery",
    ];
    let name = e.file_name().to_string_lossy();
    if name.starts_with('.') {
        return false;
    }
    !JUNK.iter().any(|j| name.eq_ignore_ascii_case(j))
}

/// Обход файлов. Дерево хранится в порядке обхода в глубину,
/// поэтому индекс родителя всегда меньше индекса ребёнка.
pub fn scan(o: &ScanOpts, count: &AtomicUsize, cancel: &AtomicBool) -> Vec<Node> {
    let mut nodes: Vec<Node> = Vec::new();
    let virt = o.roots.len() > 1;
    if virt {
        nodes.push(Node {
            parent: None,
            name: "Мой компьютер".into(),
            path: PathBuf::new(),
            kind: Kind::Dir,
            mtime: 0,
            depth: 0,
            children: 0,
        });
    }
    let per = (o.max_nodes / o.roots.len().max(1)).max(1);

    for root in &o.roots {
        let start = nodes.len();
        let mut stack: Vec<usize> = Vec::new();
        let walker = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| keep(e, o));
        for entry in walker {
            if cancel.load(Ordering::Relaxed) || nodes.len() - start >= per {
                break;
            }
            let Ok(entry) = entry else { continue };
            let d = entry.depth();
            let is_dir = entry.file_type().is_dir();
            let kind = if is_dir { Kind::Dir } else { kind_of(entry.path()) };
            if o.only_media && kind == Kind::Other {
                continue;
            }
            let parent = if d == 0 {
                if virt {
                    Some(0)
                } else {
                    None
                }
            } else {
                match stack.get(d - 1) {
                    Some(&p) => Some(p),
                    None => continue,
                }
            };
            let name = if d == 0 {
                root.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.to_string_lossy().into_owned())
            } else {
                entry.file_name().to_string_lossy().into_owned()
            };
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let id = nodes.len();
            if let Some(p) = parent {
                nodes[p].children += 1;
            }
            nodes.push(Node {
                parent,
                name,
                path: entry.path().to_path_buf(),
                kind,
                mtime,
                depth: d as u32 + virt as u32,
                children: 0,
            });
            if is_dir {
                stack.truncate(d);
                stack.push(id);
            }
            if id % 256 == 0 {
                count.store(nodes.len(), Ordering::Relaxed);
            }
        }
    }
    count.store(nodes.len(), Ordering::Relaxed);
    nodes
}

/// Порядок появления узлов. chrono=false: рост от корня кольцами.
/// chrono=true: по времени изменения файлов (родитель всегда раньше детей).
pub fn birth_order(nodes: &[Node], chrono: bool) -> Vec<usize> {
    let n = nodes.len();
    let mut ids: Vec<usize> = (0..n).collect();
    if chrono {
        let mut eff = vec![0u64; n];
        for i in 0..n {
            let m = nodes[i].mtime;
            eff[i] = match nodes[i].parent {
                Some(p) => m.max(eff[p]),
                None => m,
            };
        }
        ids.sort_by_key(|&i| (eff[i], nodes[i].depth, i));
    } else {
        ids.sort_by_key(|&i| (nodes[i].depth, i));
    }
    ids
}

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

use crate::engine::Engine;
use crate::media::Media;
use crate::scan::{Kind, Node};
use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use tiny_skia::*;

pub struct Theme {
    pub name: &'static str,
    pub bg: [[u8; 3]; 2],
    pub dir: [u8; 3],
    pub image: [u8; 3],
    pub video: [u8; 3],
    pub audio: [u8; 3],
    pub other: [u8; 3],
    pub edge: [u8; 3],
}

pub const THEMES: [Theme; 3] = [
    Theme {
        name: "Obsidian",
        bg: [[26, 23, 40], [9, 8, 15]],
        dir: [167, 139, 250],
        image: [96, 165, 250],
        video: [244, 114, 182],
        audio: [52, 211, 153],
        other: [148, 148, 175],
        edge: [140, 128, 190],
    },
    Theme {
        name: "Неон",
        bg: [[8, 14, 32], [2, 3, 10]],
        dir: [0, 255, 200],
        image: [255, 230, 0],
        video: [255, 60, 140],
        audio: [130, 110, 255],
        other: [110, 140, 175],
        edge: [0, 180, 210],
    },
    Theme {
        name: "Тёплый",
        bg: [[38, 24, 22], [12, 8, 8]],
        dir: [251, 146, 60],
        image: [250, 204, 21],
        video: [248, 113, 113],
        audio: [163, 230, 53],
        other: [170, 152, 142],
        edge: [205, 145, 105],
    },
];

pub const AUDIO_W: f32 = 46.0;
pub const AUDIO_H: f32 = 15.0;

pub fn base_radius(nd: &Node, thumb: bool) -> f32 {
    match nd.kind {
        Kind::Dir => (5.0 + (nd.children as f32).sqrt() * 1.7).min(34.0),
        Kind::Image | Kind::Video => {
            if thumb {
                13.0
            } else {
                5.0
            }
        }
        Kind::Audio => 4.0,
        Kind::Other => 2.6,
    }
}

fn ease_out_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x).powi(3)
}

fn ease_out_back(x: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (x - 1.0).powi(3) + c1 * (x - 1.0).powi(2)
}

fn paint(c: [u8; 3], a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], a);
    p.anti_alias = true;
    p
}

fn stroke(w: f32, round: bool) -> Stroke {
    let mut s = Stroke::default();
    s.width = w;
    if round {
        s.line_cap = LineCap::Round;
    }
    s
}

fn rgb(c: [u8; 3]) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], 255)
}

fn kind_color(t: &Theme, k: Kind) -> [u8; 3] {
    match k {
        Kind::Dir => t.dir,
        Kind::Image => t.image,
        Kind::Video => t.video,
        Kind::Audio => t.audio,
        Kind::Other => t.other,
    }
}

fn kind_idx(k: Kind) -> usize {
    match k {
        Kind::Dir => 0,
        Kind::Image => 1,
        Kind::Video => 2,
        Kind::Audio => 3,
        Kind::Other => 4,
    }
}

fn blend(pm: &mut Pixmap, x: i32, y: i32, c: [u8; 3], a: f32) {
    let w = pm.width() as i32;
    let h = pm.height() as i32;
    if x < 0 || y < 0 || x >= w || y >= h || a <= 0.0 {
        return;
    }
    let i = ((y * w + x) * 4) as usize;
    let d = pm.data_mut();
    let ia = 1.0 - a;
    d[i] = (c[0] as f32 * a + d[i] as f32 * ia) as u8;
    d[i + 1] = (c[1] as f32 * a + d[i + 1] as f32 * ia) as u8;
    d[i + 2] = (c[2] as f32 * a + d[i + 2] as f32 * ia) as u8;
    d[i + 3] = (255.0 * a + d[i + 3] as f32 * ia) as u8;
}

pub struct Renderer {
    font: FontVec,
}

impl Renderer {
    pub fn new(font_bytes: Vec<u8>) -> Self {
        Self { font: FontVec::try_from_vec(font_bytes).expect("font") }
    }

    fn text_width(&self, text: &str, size: f32) -> f32 {
        let sc = self.font.as_scaled(PxScale::from(size));
        text.chars().map(|c| sc.h_advance(sc.glyph_id(c))).sum()
    }

    fn draw_text(&self, pm: &mut Pixmap, text: &str, x: f32, y: f32, size: f32, col: [u8; 3], alpha: f32) {
        let scale = PxScale::from(size);
        let sc = self.font.as_scaled(scale);
        let mut caret = x;
        for ch in text.chars() {
            let id = sc.glyph_id(ch);
            let g = id.with_scale_and_position(scale, point(caret, y));
            caret += sc.h_advance(id);
            if let Some(og) = self.font.outline_glyph(g) {
                let b = og.px_bounds();
                let (bx, by) = (b.min.x as i32, b.min.y as i32);
                og.draw(|gx, gy, cov| {
                    blend(pm, bx + gx as i32, by + gy as i32, col, cov * alpha);
                });
            }
        }
    }

    pub fn render(&self, pm: &mut Pixmap, e: &Engine, media: &Media) {
        let tm = &THEMES[e.settings.theme.min(THEMES.len() - 1)];
        let (w, h) = (pm.width() as f32, pm.height() as f32);
        let ui = (h / 1080.0).max(0.6);

        // Фон
        let mut bgp = Paint::default();
        bgp.shader = LinearGradient::new(
            Point::from_xy(0.0, 0.0),
            Point::from_xy(0.0, h),
            vec![GradientStop::new(0.0, rgb(tm.bg[0])), GradientStop::new(1.0, rgb(tm.bg[1]))],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .unwrap_or(Shader::SolidColor(rgb(tm.bg[1])));
        if let Some(r) = Rect::from_xywh(0.0, 0.0, w, h) {
            pm.fill_rect(r, &bgp, Transform::identity(), None);
        }

        let cam = e.cam;
        let z = cam.zoom(w, h);
        let sx = |x: f32| (x - cam.cx) * z + w * 0.5;
        let sy = |y: f32| (y - cam.cy) * z + h * 0.5;
        let t = e.time;
        let nodes = &e.nodes;
        let pos = &e.sim.pos;

        // Рёбра (одним путём — быстро)
        let mut old = PathBuilder::new();
        let mut young = PathBuilder::new();
        for &i in &e.sim.alive {
            let Some(p) = nodes[i].parent else { continue };
            if !e.sim.is_alive[p] {
                continue;
            }
            let age = t - e.birth[i];
            let g = ease_out_cubic((age / 0.5).clamp(0.0, 1.0));
            let (ax, ay) = (sx(pos[p][0]), sy(pos[p][1]));
            let (bx, by) = (sx(pos[i][0]), sy(pos[i][1]));
            if (ax < -50.0 && bx < -50.0)
                || (ax > w + 50.0 && bx > w + 50.0)
                || (ay < -50.0 && by < -50.0)
                || (ay > h + 50.0 && by > h + 50.0)
            {
                continue;
            }
            let (ex, ey) = (ax + (bx - ax) * g, ay + (by - ay) * g);
            let b = if age < 1.5 { &mut young } else { &mut old };
            b.move_to(ax, ay);
            b.line_to(ex, ey);
        }
        if let Some(path) = old.finish() {
            pm.stroke_path(&path, &paint(tm.edge, 85), &stroke((1.0 * ui).max(0.8), false), Transform::identity(), None);
        }
        if let Some(path) = young.finish() {
            let c = [
                tm.edge[0].saturating_add(70),
                tm.edge[1].saturating_add(70),
                tm.edge[2].saturating_add(70),
            ];
            pm.stroke_path(&path, &paint(c, 210), &stroke((1.6 * ui).max(1.0), false), Transform::identity(), None);
        }

        // Узлы
        let mut dots: [PathBuilder; 5] = std::array::from_fn(|_| PathBuilder::new());
        let mut cards: Vec<usize> = Vec::new();
        let mut labels: Vec<(f32, f32, f32, usize, f32)> = Vec::new();

        for &i in &e.sim.alive {
            let nd = &nodes[i];
            let age = t - e.birth[i];
            let k = ease_out_back((age / 0.6).clamp(0.0, 1.0));
            let (cx, cy) = (sx(pos[i][0]), sy(pos[i][1]));
            if cx < -80.0 || cx > w + 80.0 || cy < -80.0 || cy > h + 80.0 {
                continue;
            }
            let thumb = if e.settings.thumbs && matches!(nd.kind, Kind::Image | Kind::Video) {
                media.thumb(i)
            } else {
                None
            };
            let rw = base_radius(nd, thumb.is_some()) * k.max(0.0);
            let r = rw * z;
            let idx = kind_idx(nd.kind);

            if nd.kind == Kind::Audio {
                if AUDIO_W * z * k < 9.0 {
                    dots[idx].push_circle(cx, cy, (r.max(1.0 * ui)).max(0.5));
                } else {
                    cards.push(i);
                }
                continue;
            }
            if r < 4.0 {
                dots[idx].push_circle(cx, cy, r.max(0.9 * ui));
                continue;
            }

            let col = kind_color(tm, nd.kind);
            let fade = ((age * 5.0).clamp(0.0, 1.0) * 255.0) as u8;

            if e.settings.glow {
                let ga = if age < 1.2 {
                    (1.0 - age / 1.2).max(0.0) * 0.9
                } else if nd.kind == Kind::Dir {
                    0.22
                } else {
                    0.0
                };
                if ga > 0.02 {
                    let gr = r * 3.5;
                    let mut gp = Paint::default();
                    if let Some(sh) = RadialGradient::new(
                        Point::from_xy(cx, cy),
                        Point::from_xy(cx, cy),
                        gr,
                        vec![
                            GradientStop::new(0.0, Color::from_rgba8(col[0], col[1], col[2], (ga * 170.0) as u8)),
                            GradientStop::new(1.0, Color::from_rgba8(col[0], col[1], col[2], 0)),
                        ],
                        SpreadMode::Pad,
                        Transform::identity(),
                    ) {
                        gp.shader = sh;
                        if let Some(path) = PathBuilder::from_circle(cx, cy, gr) {
                            pm.fill_path(&path, &gp, FillRule::Winding, Transform::identity(), None);
                        }
                    }
                }
            }

            if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
                match &thumb {
                    Some(tp) if r >= 5.0 => {
                        if let Some(pr) = PixmapRef::from_bytes(tp.data(), tp.width(), tp.height()) {
                            let s = 2.0 * r / tp.width() as f32;
                            let mut p = Paint::default();
                            p.shader = Pattern::new(
                                pr,
                                SpreadMode::Pad,
                                FilterQuality::Bilinear,
                                fade as f32 / 255.0,
                                Transform::from_row(s, 0.0, 0.0, s, cx - r, cy - r),
                            );
                            p.anti_alias = true;
                            pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
                        }
                        pm.stroke_path(&path, &paint(col, fade), &stroke((1.8 * ui).max(1.0), false), Transform::identity(), None);
                        if nd.kind == Kind::Video && r > 9.0 {
                            if let Some(bg) = PathBuilder::from_circle(cx, cy, r * 0.5) {
                                pm.fill_path(&bg, &paint([0, 0, 0], 150), FillRule::Winding, Transform::identity(), None);
                            }
                            let mut tb = PathBuilder::new();
                            tb.move_to(cx - r * 0.16, cy - r * 0.26);
                            tb.line_to(cx - r * 0.16, cy + r * 0.26);
                            tb.line_to(cx + r * 0.28, cy);
                            tb.close();
                            if let Some(tri) = tb.finish() {
                                pm.fill_path(&tri, &paint([255, 255, 255], 230), FillRule::Winding, Transform::identity(), None);
                            }
                        }
                    }
                    _ => {
                        pm.fill_path(&path, &paint(col, fade), FillRule::Winding, Transform::identity(), None);
                    }
                }
            }

            if e.settings.labels && (nd.kind == Kind::Dir && r > 7.0 || r > 16.0) {
                labels.push((cx, cy + r, r, i, (fade as f32) / 255.0));
            }
        }

        for (idx, b) in dots.into_iter().enumerate() {
            let col = match idx {
                0 => tm.dir,
                1 => tm.image,
                2 => tm.video,
                3 => tm.audio,
                _ => tm.other,
            };
            if let Some(path) = b.finish() {
                pm.fill_path(&path, &paint(col, 235), FillRule::Winding, Transform::identity(), None);
            }
        }

        // Голосовые: «пилюля» с делениями-столбиками, которые заполняются слева направо
        for &i in &cards {
            let age = t - e.birth[i];
            let k = ease_out_back((age / 0.6).clamp(0.0, 1.0)).max(0.0);
            let (cx, cy) = (sx(pos[i][0]), sy(pos[i][1]));
            let cw = AUDIO_W * z * k;
            let ch = AUDIO_H * z * k;
            let (x0, x1) = (cx - cw * 0.5 + ch * 0.5, cx + cw * 0.5 - ch * 0.5);
            let mut line = PathBuilder::new();
            line.move_to(x0, cy);
            line.line_to(x1, cy);
            if let Some(path) = line.finish() {
                pm.stroke_path(&path, &paint(tm.audio, 170), &stroke(ch + 2.0 * ui, true), Transform::identity(), None);
                pm.stroke_path(&path, &paint([16, 18, 26], 235), &stroke(ch, true), Transform::identity(), None);
            }
            let wave = media.wave(i);
            let n = 32usize;
            let pad = ch * 0.55;
            let avail = (cw - pad * 2.0).max(1.0);
            let spacing = avail / n as f32;
            let bw = (spacing * 0.55).max(0.8);
            let reveal = ((age - 0.15) / 1.4).clamp(0.0, 1.0);
            let mut on = PathBuilder::new();
            let mut off = PathBuilder::new();
            for bi in 0..n {
                let amp = match &wave {
                    Some(wv) => wv[(bi * wv.len() / n).min(wv.len() - 1)],
                    None => 0.2,
                };
                let bh = (0.18 + 0.82 * amp) * ch * 0.62;
                let bx = cx - cw * 0.5 + pad + spacing * (bi as f32 + 0.5);
                let target = if (bi as f32 / n as f32) <= reveal { &mut on } else { &mut off };
                target.move_to(bx, cy - bh * 0.5);
                target.line_to(bx, cy + bh * 0.5);
            }
            if let Some(path) = off.finish() {
                pm.stroke_path(&path, &paint(tm.audio, 60), &stroke(bw, true), Transform::identity(), None);
            }
            if let Some(path) = on.finish() {
                pm.stroke_path(&path, &paint(tm.audio, 255), &stroke(bw, true), Transform::identity(), None);
            }
        }

        // Подписи
        labels.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        for &(cx, by, r, id, a) in labels.iter().take(500) {
            let name = &nodes[id].name;
            let shown: String = if name.chars().count() > 20 {
                let mut s: String = name.chars().take(19).collect();
                s.push('…');
                s
            } else {
                name.clone()
            };
            let size = (r * 0.5).clamp(10.0 * ui, 20.0 * ui);
            let tw = self.text_width(&shown, size);
            self.draw_text(pm, &shown, cx - tw * 0.5, by + size + 2.0 * ui, size, [235, 232, 250], 0.85 * a);
        }

        // Счётчик
        let info = format!("{} / {}", e.sim.alive.len(), nodes.len());
        self.draw_text(pm, &info, 32.0 * ui, h - 32.0 * ui, 26.0 * ui, [255, 255, 255], 0.6);
    }
}

[package]
name = "treegrow"
version = "0.1.0"
edition = "2021"

[dependencies]
eframe = { version = "0.29", default-features = false, features = ["glow", "default_fonts"] }
walkdir = "2"
rayon = "1"
tiny-skia = "0.11"
ab_glyph = "0.2"
image = { version = "0.25", default-features = false, features = ["jpeg", "png", "webp", "gif", "bmp"] }
symphonia = { version = "0.5", default-features = false, features = ["mp3", "aac", "isomp4", "ogg", "vorbis", "flac", "wav", "pcm", "alac"] }
rfd = "0.15"

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true

# TreeGrow

Приложение для Windows (Rust): сканирует папку или весь компьютер и показывает, как файловое дерево
«разрастается» в стиле графа Obsidian: узлы вылетают от родителя, пружинят, фото и видео появляются
миниатюрами, голосовые и аудио — «пилюлями» с делениями-волной. Анимацию можно сохранить в MP4 (до 4K).

## Как получить .exe (без установки Rust)

1. Создай пустой репозиторий на GitHub и залей туда содержимое этой папки.
2. Открой вкладку **Actions → build-windows → Run workflow**.
3. Через 5–10 минут скачай артефакт **TreeGrow-windows**: внутри `treegrow.exe` и `ffmpeg.exe`.
   Держи их в одной папке (ffmpeg нужен для видео-превью и сохранения MP4).

## Как собрать локально на Windows

```
rustup default stable
cargo build --release
```
Готовый файл: `target\release\treegrow.exe`. Положи рядом `ffmpeg.exe`.

## Как пользоваться

- «Весь компьютер» — обойдёт все диски (лимит узлов регулируется ползунком).
- «Скорость» — сколько узлов появляется в секунду; меняется прямо во время показа.
- «Рост от корня» — дерево растёт кольцами; «По дате файлов» — файлы появляются в порядке даты изменения.
- Экспорт: выбери 1080p / 2K / 4K и 30 или 60 к/с. Рендер кадров идёт на CPU, 4K занимает заметно дольше превью.

## Заметки

- Волна голосовых — настоящая для mp3, wav, flac, ogg (vorbis), m4a/aac. Для .opus (например, голосовые из Telegram)
  рисуется стилизованная волна, потому что декодер Opus в этой сборке не подключён.
- Превью HEIC/TIFF не поддерживаются, такие файлы рисуются цветным узлом.

name: build-windows

on:
  push:
    branches: [main]
  workflow_dispatch: {}

jobs:
  build:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Build
        run: cargo build --release
      - name: Pack exe + ffmpeg
        shell: pwsh
        run: |
          Invoke-WebRequest -Uri "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip" -OutFile ffmpeg.zip
          Expand-Archive ffmpeg.zip -DestinationPath ff
          New-Item -ItemType Directory -Force dist | Out-Null
          Copy-Item target/release/treegrow.exe dist/
          Copy-Item (Get-ChildItem -Recurse ff -Filter ffmpeg.exe | Select-Object -First 1).FullName dist/
      - uses: actions/upload-artifact@v4
        with:
          name: TreeGrow-windows
          path: dist
