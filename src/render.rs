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

        if e.settings.transparent {
            pm.fill(Color::TRANSPARENT);
        } else {
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
