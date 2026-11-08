//! Play mode: a champion walking a map the way the game moves units: right-click to move,
//! pathfinding on the map's navigation grid (walls, and structures by their collision radius,
//! block), height from the grid's sampled heights, brush hides the champion.

use crate::formats::{mob::ObjectType, navgrid::{NavGrid, flags}};
use crate::project::{Doc, MapProject};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

pub struct World {
    pub grid: NavGrid,
    blocked: Vec<bool>,
    pub spawn: [f32; 2],
    pub bounds: [f32; 4],
    /// Structures for the HUD and the minimap: (name, team, x, z).
    pub structures: Vec<(String, u8, f32, f32)>,
    /// Jungle monsters (they block like units; shown as camps on the minimap).
    pub monsters: Vec<crate::jungle::Monster>,
}

impl World {
    pub fn from_project(p: &MapProject) -> Option<Self> {
        let grid = p.navgrid()?.clone();
        let mut blocked: Vec<bool> = (0..grid.cell_count()).map(|k| grid.flags(k) & flags::WALL != 0).collect();
        let mut structures = Vec::new();
        let mut spawn = None;
        let block = |x: f32, z: f32, r: f32, blocked: &mut Vec<bool>| {
            let n = (r / grid.cell_size).ceil() as i64;
            let Some((ci, cj)) = grid.cell_of(x, z) else { return };
            for dj in -n..=n {
                for di in -n..=n {
                    let (i, j) = (ci as i64 + di, cj as i64 + dj);
                    if i < 0 || j < 0 || i as usize >= grid.nx || j as usize >= grid.ny {
                        continue;
                    }
                    let c = grid.cell_center(i as usize, j as usize);
                    if (c[0] - x).hypot(c[1] - z) <= r {
                        blocked[j as usize * grid.nx + i as usize] = true;
                    }
                }
            }
        };
        let team_of = |n: &str| if n.contains("T1") || n.contains("Order") || n.contains("order") { 1 } else { 2 };
        if let Some(m) = p.mob() {
            for o in &m.objects {
                let (x, z) = (o.position[0], o.position[2]);
                match o.kind {
                    ObjectType::Turret => {
                        block(x, z, 88.0, &mut blocked);
                        structures.push((o.name.clone(), team_of(&o.name), x, z));
                    }
                    ObjectType::Inhibitor => {
                        block(x, z, 190.0, &mut blocked);
                        structures.push((o.name.clone(), team_of(&o.name), x, z));
                    }
                    ObjectType::Nexus => {
                        block(x, z, 350.0, &mut blocked);
                        structures.push((o.name.clone(), team_of(&o.name), x, z));
                    }
                    ObjectType::SpawnPoint if o.name.contains("T1") || spawn.is_none() => spawn = Some([x, z]),
                    _ => {}
                }
            }
        }
        // Map1-style structures are room.dsc meshes (central point = position).
        for e in p.dsc_entries() {
            let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0).to_string();
            let r = if stem.starts_with("HQ") { 350.0 } else if stem.starts_with("Barracks") { 190.0 } else if stem.starts_with("Turret") { 88.0 } else { continue };
            let c = p.files.iter().filter(|f| f.stem().eq_ignore_ascii_case(&stem)).find_map(|f| match &f.doc {
                Doc::Scb(s) => Some(s.central),
                Doc::Sco(s) => Some(s.central),
                _ => None,
            });
            if let Some(c) = c {
                block(c[0], c[2], r, &mut blocked);
                structures.push((stem.clone(), team_of(&stem), c[0], c[2]));
            }
        }
        let monsters = crate::jungle::monsters(p);
        for m in &monsters {
            // Big monsters (Dragon, Baron) take more room.
            let r = if matches!(m.name.as_str(), "Dragon" | "Worm") { 150.0 } else { 45.0 };
            block(m.pos[0], m.pos[2], r, &mut blocked);
        }
        let bounds = [grid.min[0], grid.min[2], grid.max[0], grid.max[2]];
        let mut w = Self { grid, blocked, spawn: spawn.unwrap_or([0.0, 0.0]), bounds, structures, monsters };
        w.spawn = w.nearest_walkable(w.spawn).unwrap_or(w.spawn);
        Some(w)
    }

    /// Ground height (the grid's sampled heights, as the game uses for units).
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let g = &self.grid;
        if g.hx < 2 || g.hy < 2 || g.hdx <= 0.0 {
            return crate::create::grid_height(g, x, z);
        }
        let fx = ((x - g.min[0]) / g.hdx).clamp(0.0, (g.hx - 1) as f32 - 0.001);
        let fz = ((z - g.min[2]) / g.hdy).clamp(0.0, (g.hy - 1) as f32 - 0.001);
        let (i, j) = (fx as usize, fz as usize);
        let (tx, tz) = (fx - i as f32, fz - j as f32);
        let h = |i: usize, j: usize| g.sampled_height(i, j);
        (h(i, j) * (1.0 - tx) + h(i + 1, j) * tx) * (1.0 - tz) + (h(i, j + 1) * (1.0 - tx) + h(i + 1, j + 1) * tx) * tz
    }

    fn cell(&self, x: f32, z: f32) -> Option<usize> {
        self.grid.cell_of(x, z).map(|(i, j)| j * self.grid.nx + i)
    }

    pub fn walkable(&self, x: f32, z: f32) -> bool {
        self.cell(x, z).is_some_and(|k| !self.blocked[k])
    }

    pub fn in_brush(&self, x: f32, z: f32) -> bool {
        self.cell(x, z).is_some_and(|k| self.grid.flags(k) & flags::BRUSH != 0)
    }

    pub fn nearest_walkable(&self, p: [f32; 2]) -> Option<[f32; 2]> {
        let g = &self.grid;
        let (ci, cj) = g.cell_of(p[0], p[1]).unwrap_or((
            ((p[0] - g.min[0]) / g.cell_size).clamp(0.0, g.nx as f32 - 1.0) as usize,
            ((p[1] - g.min[2]) / g.cell_size).clamp(0.0, g.ny as f32 - 1.0) as usize,
        ));
        if !self.blocked[cj * g.nx + ci] && g.cell_of(p[0], p[1]).is_some() {
            return Some(p);
        }
        for r in 1..200i64 {
            let mut best: Option<(f32, [f32; 2])> = None;
            for dj in -r..=r {
                for di in -r..=r {
                    if di.abs() != r && dj.abs() != r {
                        continue;
                    }
                    let (i, j) = (ci as i64 + di, cj as i64 + dj);
                    if i < 0 || j < 0 || i as usize >= g.nx || j as usize >= g.ny || self.blocked[j as usize * g.nx + i as usize] {
                        continue;
                    }
                    let c = g.cell_center(i as usize, j as usize);
                    let d = (c[0] - p[0]).hypot(c[1] - p[1]);
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, c));
                    }
                }
            }
            if let Some(b) = best {
                return Some(b.1);
            }
        }
        None
    }

    /// Straight walkable line between two points (sampled every half cell).
    pub fn line_clear(&self, a: [f32; 2], b: [f32; 2]) -> bool {
        let d = (b[0] - a[0]).hypot(b[1] - a[1]);
        let n = (d / (self.grid.cell_size * 0.4)).ceil().max(1.0) as usize;
        (0..=n).all(|k| {
            let t = k as f32 / n as f32;
            self.walkable(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t)
        })
    }

    /// A* over the grid (8 directions, no corner cutting), then shortened by line of sight.
    pub fn find_path(&self, from: [f32; 2], to: [f32; 2]) -> Vec<[f32; 2]> {
        let Some(to) = self.nearest_walkable(to) else { return Vec::new() };
        if self.line_clear(from, to) {
            return vec![to];
        }
        let g = &self.grid;
        let (Some(s), Some(e)) = (self.nearest_walkable(from).and_then(|p| self.cell(p[0], p[1])), self.cell(to[0], to[1])) else { return Vec::new() };
        let nx = g.nx;
        let n = g.cell_count();
        let mut cost = vec![f32::INFINITY; n];
        let mut prev = vec![u32::MAX; n];
        let h = |k: usize| {
            let (dx, dz) = (((k % nx) as f32 - (e % nx) as f32).abs(), ((k / nx) as f32 - (e / nx) as f32).abs());
            dx.max(dz) + (std::f32::consts::SQRT_2 - 1.0) * dx.min(dz)
        };
        #[derive(PartialEq, PartialOrd)]
        struct F(f32);
        impl Eq for F {}
        impl Ord for F {
            fn cmp(&self, o: &Self) -> std::cmp::Ordering {
                self.0.partial_cmp(&o.0).unwrap_or(std::cmp::Ordering::Equal)
            }
        }
        let mut open = BinaryHeap::new();
        cost[s] = 0.0;
        open.push(Reverse((F(h(s)), s)));
        let steps: [(i64, i64, f32); 8] = [(1, 0, 1.0), (-1, 0, 1.0), (0, 1, 1.0), (0, -1, 1.0), (1, 1, 1.414), (1, -1, 1.414), (-1, 1, 1.414), (-1, -1, 1.414)];
        let mut found = false;
        let mut expanded = 0;
        while let Some(Reverse((_, k))) = open.pop() {
            if k == e {
                found = true;
                break;
            }
            expanded += 1;
            if expanded > 400_000 {
                break;
            }
            let (i, j) = ((k % nx) as i64, (k / nx) as i64);
            for (di, dj, c) in steps {
                let (ii, jj) = (i + di, j + dj);
                if ii < 0 || jj < 0 || ii as usize >= nx || jj as usize >= g.ny {
                    continue;
                }
                let kk = jj as usize * nx + ii as usize;
                if self.blocked[kk] || (di != 0 && dj != 0 && (self.blocked[j as usize * nx + ii as usize] || self.blocked[jj as usize * nx + i as usize])) {
                    continue;
                }
                let nc = cost[k] + c;
                if nc < cost[kk] {
                    cost[kk] = nc;
                    prev[kk] = k as u32;
                    open.push(Reverse((F(nc + h(kk)), kk)));
                }
            }
        }
        if !found {
            return Vec::new();
        }
        let mut cells = vec![e];
        while let Some(&k) = cells.last() {
            if k == s || prev[k] == u32::MAX {
                break;
            }
            cells.push(prev[k] as usize);
        }
        cells.reverse();
        let pts: Vec<[f32; 2]> = cells.iter().map(|&k| g.cell_center(k % nx, k / nx)).collect();
        // String pulling: keep only the corners that line of sight needs.
        let mut out = Vec::new();
        let mut cur = from;
        let mut i = 0;
        while i < pts.len() {
            let mut far = i;
            for k in (i..pts.len()).rev() {
                if self.line_clear(cur, pts[k]) {
                    far = k;
                    break;
                }
            }
            cur = pts[far];
            out.push(cur);
            i = far + 1;
        }
        if let Some(last) = out.last_mut() {
            *last = to;
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Idle,
    Run,
    /// A one-shot animation (spell, attack, emote) and the time it started.
    Once(String, f32),
    /// Recall channel: teleports to the spawn when finished.
    Recall(f32),
}

pub struct Hero {
    pub pos: [f32; 2],
    pub facing: f32,
    pub path: Vec<[f32; 2]>,
    pub action: Action,
    pub anim_time: f32,
    pub speed: f32,
}

pub const RECALL_TIME: f32 = 8.0;

impl Hero {
    pub fn new(pos: [f32; 2], speed: f32) -> Self {
        Self { pos, facing: 0.0, path: Vec::new(), action: Action::Idle, anim_time: 0.0, speed }
    }

    pub fn move_to(&mut self, w: &World, target: [f32; 2]) {
        self.path = w.find_path(self.pos, target);
        if !self.path.is_empty() {
            if matches!(self.action, Action::Idle | Action::Recall(_) | Action::Once(..)) {
                self.action = Action::Run;
                self.anim_time = 0.0;
            }
        }
    }

    pub fn stop(&mut self) {
        self.path.clear();
        if self.action == Action::Run {
            self.action = Action::Idle;
            self.anim_time = 0.0;
        }
    }

    pub fn play(&mut self, anim: String) {
        self.path.clear();
        self.action = Action::Once(anim, 0.0);
        self.anim_time = 0.0;
    }

    /// Advances the hero; `once_len` gives one-shot animation lengths.
    pub fn update(&mut self, dt: f32, w: &World, once_len: impl Fn(&str) -> f32) {
        self.anim_time += dt;
        match &mut self.action {
            Action::Recall(t) => {
                *t += dt;
                if *t >= RECALL_TIME {
                    self.pos = w.spawn;
                    self.action = Action::Idle;
                    self.anim_time = 0.0;
                }
                return;
            }
            Action::Once(name, t) => {
                *t += dt;
                if *t >= once_len(name) {
                    self.action = if self.path.is_empty() { Action::Idle } else { Action::Run };
                    self.anim_time = 0.0;
                }
                return;
            }
            _ => {}
        }
        let mut left = self.speed * dt;
        while left > 0.0 {
            let Some(&next) = self.path.first() else { break };
            let (dx, dz) = (next[0] - self.pos[0], next[1] - self.pos[1]);
            let d = dx.hypot(dz);
            if d > 1e-3 {
                let target = dx.atan2(dz);
                // Turn quickly toward the direction of travel.
                let mut diff = target - self.facing;
                while diff > std::f32::consts::PI {
                    diff -= std::f32::consts::TAU;
                }
                while diff < -std::f32::consts::PI {
                    diff += std::f32::consts::TAU;
                }
                self.facing += diff * (dt * 18.0).min(1.0);
            }
            if d <= left {
                self.pos = next;
                self.path.remove(0);
                left -= d;
            } else {
                self.pos = [self.pos[0] + dx / d * left, self.pos[1] + dz / d * left];
                left = 0.0;
            }
        }
        if self.path.is_empty() && self.action == Action::Run {
            self.action = Action::Idle;
            self.anim_time = 0.0;
        }
    }
}
