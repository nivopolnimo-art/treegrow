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
    pub transparent: bool, // прозрачный фон вместо градиента
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
            transparent: true,
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
