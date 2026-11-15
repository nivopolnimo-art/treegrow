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
