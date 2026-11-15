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
