//! `AIPath.aimesh_ngrid`: the navigation grid (`NavGrid::LoadBinary`, versions 2-5).
//!
//! Layout: version byte, flags (v3+), min/max corner, cell size, X/Y cell counts, 56-byte cells,
//! region tags (v4+), region tag table (v5), sampled heights, then 900 pathfinding hint nodes.
//! The bytes are kept as they are and edited in place, so untouched data round-trips exactly.

use super::bin::{Reader, get_f32, get_u16, put_f32, put_u16};
use anyhow::{Result, bail};
use rayon::prelude::*;

pub const CELL_BYTES: usize = 0x38;
pub const HINT_NODES: usize = 900;
pub const HINT_SIDE: usize = 30;
pub const HINT_BYTES: usize = 0xe14;
pub const REGION_TABLE_BYTES: usize = 0x210;

// NavGridCell field offsets.
const CELL_HEIGHT: usize = 0x00;
const CELL_SESSION: usize = 0x04;
const CELL_LOCATOR: usize = 0x18;
const CELL_GOOD_SESSION: usize = 0x28;
const CELL_HINT_WEIGHT: usize = 0x2c;
const CELL_ARRIVAL_DIR: usize = 0x30;
const CELL_FLAGS: usize = 0x32;
const CELL_HINT_NODES: usize = 0x34;

pub mod flags {
    pub const BRUSH: u16 = 0x01;
    pub const WALL: u16 = 0x02;
    pub const SEE_THROUGH: u16 = 0x40;
    pub const FLAG_80: u16 = 0x80;
    pub const EXPOSED: u16 = 0x100;
}

#[derive(Clone, Debug)]
pub struct NavGrid {
    pub data: Vec<u8>,
    pub version: u8,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub cell_size: f32,
    pub nx: usize,
    pub ny: usize,
    cells_at: usize,
    region_tags_at: Option<usize>,
    heights_at: usize,
    pub hx: usize,
    pub hy: usize,
    pub hdx: f32,
    pub hdy: f32,
    hints_at: usize,
}

impl NavGrid {
    pub fn parse(data: Vec<u8>) -> Result<Self> {
        let mut r = Reader::new(&data);
        let version = r.u8()?;
        if !(2..=5).contains(&version) {
            bail!("navigation grid version {version}; the game reads versions 2 to 5");
        }
        if version >= 3 {
            r.u16()?;
        }
        let min = r.vec3()?;
        let max = r.vec3()?;
        let cell_size = r.f32()?;
        let nx = r.u32()? as usize;
        let ny = r.u32()? as usize;
        if nx == 0 || ny == 0 || nx > 4096 || ny > 4096 || !(cell_size > 0.0) {
            bail!("navigation grid has an invalid size ({nx} x {ny} cells of {cell_size})");
        }
        let cells_at = r.pos;
        r.skip(nx * ny * CELL_BYTES)?;
        let region_tags_at = if version >= 4 {
            let at = r.pos;
            r.skip(nx * ny * 2)?;
            Some(at)
        } else {
            None
        };
        if version >= 5 {
            r.skip(REGION_TABLE_BYTES)?;
        }
        let hx = r.u32()? as usize;
        let hy = r.u32()? as usize;
        let hdx = r.f32()?;
        let hdy = r.f32()?;
        let heights_at = r.pos;
        r.skip(hx * hy * 4)?;
        let hints_at = r.pos;
        r.skip(HINT_NODES * HINT_BYTES)?;
        if r.remaining() != 0 {
            bail!("{} unexpected bytes at the end of the navigation grid", r.remaining());
        }
        Ok(Self { data, version, min, max, cell_size, nx, ny, cells_at, region_tags_at, heights_at, hx, hy, hdx, hdy, hints_at })
    }

    pub fn write(&self) -> Vec<u8> {
        self.data.clone()
    }

    fn cell_at(&self, i: usize) -> usize {
        self.cells_at + i * CELL_BYTES
    }

    pub fn cell_count(&self) -> usize {
        self.nx * self.ny
    }

    pub fn flags(&self, i: usize) -> u16 {
        get_u16(&self.data, self.cell_at(i) + CELL_FLAGS)
    }

    pub fn set_flags(&mut self, i: usize, f: u16) {
        let at = self.cell_at(i) + CELL_FLAGS;
        put_u16(&mut self.data, at, f);
    }

    pub fn height(&self, i: usize) -> f32 {
        get_f32(&self.data, self.cell_at(i) + CELL_HEIGHT)
    }

    pub fn set_height(&mut self, i: usize, h: f32) {
        let at = self.cell_at(i) + CELL_HEIGHT;
        put_f32(&mut self.data, at, h);
    }

    pub fn region_tag(&self, i: usize) -> Option<u16> {
        self.region_tags_at.map(|at| get_u16(&self.data, at + i * 2))
    }

    pub fn sampled_height(&self, i: usize, j: usize) -> f32 {
        get_f32(&self.data, self.heights_at + (j * self.hx + i) * 4)
    }

    pub fn set_sampled_height(&mut self, i: usize, j: usize, h: f32) {
        let at = self.heights_at + (j * self.hx + i) * 4;
        put_f32(&mut self.data, at, h);
    }

    /// The cell containing world position (x, z), if inside the grid.
    pub fn cell_of(&self, x: f32, z: f32) -> Option<(usize, usize)> {
        let i = ((x - self.min[0]) / self.cell_size).floor();
        let j = ((z - self.min[2]) / self.cell_size).floor();
        if i < 0.0 || j < 0.0 || i as usize >= self.nx || j as usize >= self.ny {
            return None;
        }
        Some((i as usize, j as usize))
    }

    pub fn cell_center(&self, i: usize, j: usize) -> [f32; 2] {
        [
            self.min[0] + (i as f32 + 0.5) * self.cell_size,
            self.min[2] + (j as f32 + 0.5) * self.cell_size,
        ]
    }

    pub fn walkable(&self) -> Vec<bool> {
        (0..self.cell_count()).map(|i| self.flags(i) & flags::WALL == 0).collect()
    }

    pub fn hint_node_cell(&self, n: usize) -> (i16, i16) {
        let at = self.hints_at + n * HINT_BYTES + HINT_NODES * 4;
        (get_u16(&self.data, at) as i16, get_u16(&self.data, at + 2) as i16)
    }

    /// A new, blank grid in the given version (3 like Map1, or 5 like map11).
    pub fn new_blank(version: u8, min: [f32; 3], max: [f32; 3], cell_size: f32, nx: usize, ny: usize) -> Self {
        let hx = 2 * nx + 1;
        let hy = 2 * ny + 1;
        let mut size = 1 + if version >= 3 { 2 } else { 0 } + 36 + nx * ny * CELL_BYTES + 16 + hx * hy * 4 + HINT_NODES * HINT_BYTES;
        if version >= 4 {
            size += nx * ny * 2;
        }
        if version >= 5 {
            size += REGION_TABLE_BYTES;
        }
        let mut d = Vec::with_capacity(size);
        d.push(version);
        if version >= 3 {
            d.extend_from_slice(&1u16.to_le_bytes());
        }
        for v in min.iter().chain(max.iter()) {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&cell_size.to_le_bytes());
        d.extend_from_slice(&(nx as u32).to_le_bytes());
        d.extend_from_slice(&(ny as u32).to_le_bytes());
        for j in 0..ny {
            for i in 0..nx {
                let mut c = [0u8; CELL_BYTES];
                c[CELL_SESSION..CELL_SESSION + 4].copy_from_slice(&(-1i32).to_le_bytes());
                c[CELL_LOCATOR..CELL_LOCATOR + 2].copy_from_slice(&(i as i16).to_le_bytes());
                c[CELL_LOCATOR + 2..CELL_LOCATOR + 4].copy_from_slice(&(j as i16).to_le_bytes());
                c[CELL_GOOD_SESSION..CELL_GOOD_SESSION + 4].copy_from_slice(&(-1i32).to_le_bytes());
                c[CELL_HINT_WEIGHT..CELL_HINT_WEIGHT + 4].copy_from_slice(&1.0f32.to_le_bytes());
                c[CELL_ARRIVAL_DIR] = 9;
                c[CELL_HINT_NODES..CELL_HINT_NODES + 4].copy_from_slice(&(-1i32).to_le_bytes());
                d.extend_from_slice(&c);
            }
        }
        if version >= 4 {
            d.resize(d.len() + nx * ny * 2, 0);
        }
        if version >= 5 {
            d.resize(d.len() + REGION_TABLE_BYTES, 0);
        }
        let sx = (max[0] - min[0]) / (hx - 1) as f32;
        let sz = (max[2] - min[2]) / (hy - 1) as f32;
        d.extend_from_slice(&(hx as u32).to_le_bytes());
        d.extend_from_slice(&(hy as u32).to_le_bytes());
        d.extend_from_slice(&sx.to_le_bytes());
        d.extend_from_slice(&sz.to_le_bytes());
        d.resize(d.len() + hx * hy * 4 + HINT_NODES * HINT_BYTES, 0);
        debug_assert_eq!(d.len(), size);
        Self::parse(d).expect("a blank grid is valid")
    }

    /// Recomputes the pathfinding hints (900 nodes on a 30x30 layout, their walking distances,
    /// and each cell's two nearest nodes), laid out like the shipped maps' grids.
    /// Run this after changing which cells are walkable.
    pub fn rebuild_hints(&mut self) {
        let (nx, ny, cs) = (self.nx, self.ny, self.cell_size);
        let walk = self.walkable();
        let tx = nx as f64 / HINT_SIDE as f64;
        let tz = ny as f64 / HINT_SIDE as f64;
        let mut nodes: Vec<Option<(usize, usize)>> = vec![None; HINT_NODES];
        for tj in 0..HINT_SIDE {
            for ti in 0..HINT_SIDE {
                let (ccx, ccz) = ((ti as f64 + 0.5) * tx, (tj as f64 + 0.5) * tz);
                let mut best = None;
                let mut bd = f64::INFINITY;
                let j0 = (tj as f64 * tz).ceil() as usize;
                let i0 = (ti as f64 * tx).ceil() as usize;
                let mut j = j0;
                while j < ny && (j as f64) < (tj + 1) as f64 * tz {
                    let mut i = i0;
                    while i < nx && (i as f64) < (ti + 1) as f64 * tx {
                        if walk[j * nx + i] {
                            let d = (i as f64 + 0.5 - ccx).powi(2) + (j as f64 + 0.5 - ccz).powi(2);
                            if d < bd {
                                bd = d;
                                best = Some((i, j));
                            }
                        }
                        i += 1;
                    }
                    j += 1;
                }
                nodes[tj * HINT_SIDE + ti] = best;
            }
        }
        let fields: Vec<Option<Vec<f32>>> = nodes
            .par_iter()
            .map(|n| n.map(|start| walking_distances(&walk, nx, ny, start)))
            .collect();
        for k in 0..nx * ny {
            let at = self.cell_at(k);
            let (mut a, mut da, mut b, mut db) = (-1i32, f32::INFINITY, -1i32, f32::INFINITY);
            if walk[k] {
                for (id, f) in fields.iter().enumerate() {
                    let Some(f) = f else { continue };
                    let d = f[k];
                    if d < da {
                        b = a;
                        db = da;
                        a = id as i32;
                        da = d;
                    } else if d < db {
                        b = id as i32;
                        db = d;
                    }
                }
            }
            let mut weight = 1.0f32;
            if da.is_finite() {
                if b < 0 {
                    b = a;
                    db = da;
                }
                let w = if da + db > 0.0 { db / (da + db) } else { 1.0 };
                weight = if w.is_finite() { w } else { 1.0 };
            } else {
                a = -1;
                b = -1;
            }
            put_f32(&mut self.data, at + CELL_HINT_WEIGHT, weight);
            put_u16(&mut self.data, at + CELL_HINT_NODES, a as i16 as u16);
            put_u16(&mut self.data, at + CELL_HINT_NODES + 2, b as i16 as u16);
        }
        for n in 0..HINT_NODES {
            let at = self.hints_at + n * HINT_BYTES;
            for m in 0..HINT_NODES {
                let d = match (&fields[n], nodes[m]) {
                    (Some(f), Some((i, j))) => f[j * nx + i],
                    _ => f32::INFINITY,
                };
                let v = if d.is_finite() { (d * cs / 10.0).round() * 10.0 } else { 0.0 };
                put_f32(&mut self.data, at + m * 4, v);
            }
            let (i, j) = nodes[n].map_or((0, 0), |(i, j)| (i as i16, j as i16));
            put_u16(&mut self.data, at + HINT_NODES * 4, i as u16);
            put_u16(&mut self.data, at + HINT_NODES * 4 + 2, j as u16);
        }
    }
}

/// 8-way walking distances (in cells) from `start` over walkable cells; diagonal moves need both
/// side cells walkable.
pub fn walking_distances(walk: &[bool], nx: usize, ny: usize, start: (usize, usize)) -> Vec<f32> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let mut dist = vec![f32::INFINITY; walk.len()];
    let s = start.1 * nx + start.0;
    if !walk[s] {
        return dist;
    }
    #[derive(PartialEq, PartialOrd)]
    struct D(f32);
    impl Eq for D {}
    impl Ord for D {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            self.0.partial_cmp(&o.0).unwrap()
        }
    }
    let mut heap = BinaryHeap::new();
    dist[s] = 0.0;
    heap.push(Reverse((D(0.0), s)));
    const SQ2: f32 = std::f32::consts::SQRT_2;
    let steps: [(i32, i32, f32); 8] = [(-1, 0, 1.0), (1, 0, 1.0), (0, -1, 1.0), (0, 1, 1.0), (-1, -1, SQ2), (-1, 1, SQ2), (1, -1, SQ2), (1, 1, SQ2)];
    while let Some(Reverse((D(d), k))) = heap.pop() {
        if d > dist[k] {
            continue;
        }
        let (z, x) = ((k / nx) as i32, (k % nx) as i32);
        for (dz, dx, c) in steps {
            let (zz, xx) = (z + dz, x + dx);
            if zz < 0 || xx < 0 || zz >= ny as i32 || xx >= nx as i32 {
                continue;
            }
            let kk = zz as usize * nx + xx as usize;
            if !walk[kk] {
                continue;
            }
            if dz != 0 && dx != 0 && !(walk[z as usize * nx + xx as usize] && walk[zz as usize * nx + x as usize]) {
                continue;
            }
            let nd = d + c;
            if nd < dist[kk] {
                dist[kk] = nd;
                heap.push(Reverse((D(nd), kk)));
            }
        }
    }
    dist
}
