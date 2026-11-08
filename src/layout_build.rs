//! Builds a complete map folder from a painted `Layout`:
//! - terrain: a height field with smooth, noisy cliffs where walls are painted, lowered river beds;
//! - ground: one four-blend material (four texture layers mixed by a painted mask, like
//!   Summoner's Rift's lanes and base), cliffs with their own rock material, grass in brush;
//! - water: a Lake01 object over rivers and deep water;
//! - navigation grid with pathfinding hints, AI mesh, minimap;
//! - structures as map objects with ObjectCFG stats, the turret script (CreateLevelProps.lua),
//!   lane points, fountain spawns and shops, and the sun in terrain.inibin.
//!
//! Generic gameplay rules (items, experience, minion waves) come from a rules map (Classic), or
//! minimal generated scripts (Sandbox).

use crate::formats::{
    aimesh::AiMesh,
    dds::{self, Image},
    inibin::{Inibin, Value, hash_key},
    mob::{MapObject, MapObjects, ObjectType},
    navgrid::{NavGrid, flags},
    nvr::{Channel, Geometry, Material, Nvr, material_type},
    scb::Scb,
};
use crate::layout::{CELL, Layout, MAX_LAYERS, ObjKind, Objective, Rules, Surface, Theme, TurretTier};
use crate::project::{MapFile, MapProject, write_dsc, DscEntry};
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct BuildOptions {
    /// A map folder whose generic rules files (Classic rules) are copied.
    pub rules_source: Option<PathBuf>,
    /// Geometry only (for the live 3D preview): no hints, AI mesh, minimap or rules.
    pub preview: bool,
    /// Scenery props placed on the map and used for jungle walls.
    pub props: Option<std::sync::Arc<crate::props::PropLibrary>>,
}

/// A stable pseudo-random number in 0..1.
pub fn hash01(a: u32, b: u32, c: u32) -> f32 {
    hash2(a as i32, b as i32, c)
}

// ---------------------------------------------------------------- noise and textures

fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6b343) ^ (y as u32).wrapping_mul(0xd8163841) ^ seed.wrapping_mul(0xcb1ab31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1e995);
    h ^= h >> 15;
    (h & 0xffffff) as f32 / 16777215.0
}

/// Smooth value noise, 4 octaves, 0..1.
pub fn noise2(x: f32, y: f32, seed: u32) -> f32 {
    let (mut t, mut amp, mut f, mut norm) = (0.0, 0.5, 1.0, 0.0);
    for o in 0..4 {
        let (px, py) = (x * f, y * f);
        let (ix, iy) = (px.floor(), py.floor());
        let (fx, fy) = (px - ix, py - iy);
        let s = |dx: i32, dy: i32| hash2(ix as i32 + dx, iy as i32 + dy, seed + o);
        let sm = |t: f32| t * t * (3.0 - 2.0 * t);
        let (ux, uy) = (sm(fx), sm(fy));
        let v = (s(0, 0) * (1.0 - ux) + s(1, 0) * ux) * (1.0 - uy) + (s(0, 1) * (1.0 - ux) + s(1, 1) * ux) * uy;
        t += v * amp;
        norm += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    t / norm
}

/// Tileable noise on a 0..1 square (periodic in both axes).
fn tile_noise(u: f32, v: f32, freq: f32, seed: u32) -> f32 {
    let (a, b) = (u * std::f32::consts::TAU, v * std::f32::consts::TAU);
    let r = freq / std::f32::consts::TAU;
    noise2(a.cos() * r + 20.0 + b.cos() * r * 0.7, a.sin() * r + 20.0 + b.sin() * r * 0.7, seed)
}

/// Built-in tileable textures for themes that do not use game textures.
pub fn generated_texture(kind: &str) -> Image {
    let n = 512usize;
    let mut px = vec![0u8; n * n * 4];
    px.par_chunks_mut(n * 4).enumerate().for_each(|(y, row)| {
        for x in 0..n {
            let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
            let a = tile_noise(u, v, 6.0, 1);
            let b = tile_noise(u, v, 18.0, 2);
            let c = tile_noise(u, v, 48.0, 3);
            let (r, g, bl, al): (f32, f32, f32, f32) = match kind {
                "grass" => {
                    let t = a * 0.5 + b * 0.3 + c * 0.2;
                    let blade = (c - 0.5).abs() * 2.0;
                    (52.0 + 46.0 * t - 10.0 * blade, 92.0 + 64.0 * t - 6.0 * blade, 34.0 + 22.0 * t, 255.0)
                }
                "dirt" => {
                    let t = a * 0.5 + b * 0.35 + c * 0.15;
                    let pebble = if c > 0.72 { 18.0 } else { 0.0 };
                    (104.0 + 50.0 * t + pebble, 80.0 + 38.0 * t + pebble, 54.0 + 26.0 * t + pebble, 255.0)
                }
                "path" => {
                    // Cobblestones: cells of a jittered grid with dark joints.
                    let (gx, gy) = (u * 8.0, v * 8.0 + if (u * 8.0).floor() as i32 % 2 == 0 { 0.0 } else { 0.5 });
                    let (fx, fy) = (gx.fract(), gy.fract());
                    let edge = fx.min(1.0 - fx).min(fy.min(1.0 - fy));
                    let joint = (edge * 9.0).clamp(0.0, 1.0);
                    let stone = hash2(gx.floor() as i32 & 7, gy.floor() as i32 & 7, 9);
                    let t = 0.55 + 0.25 * stone + 0.2 * c;
                    let s = 150.0 * t * (0.35 + 0.65 * joint);
                    (s * 1.02, s * 0.98, s * 0.9, 255.0)
                }
                "sand" => {
                    let t = a * 0.4 + b * 0.3 + c * 0.3;
                    (190.0 + 30.0 * t, 166.0 + 28.0 * t, 118.0 + 24.0 * t, 255.0)
                }
                "snow" => {
                    let t = a * 0.5 + b * 0.3 + c * 0.2;
                    (150.0 + 35.0 * t, 160.0 + 32.0 * t, 175.0 + 28.0 * t, 255.0)
                }
                "lava" => {
                    // Glowing molten veins between dark crust plates.
                    // Dark cooled crust with thin glowing cracks and a few hot pools.
                    let cells = ((a * 2.0 - 1.0).abs() * 0.6 + (b * 2.0 - 1.0).abs() * 0.4).min(1.0);
                    let pool = ((c - 0.62) / 0.2).clamp(0.0, 1.0);
                    let vein = (1.0 - cells * 5.0).clamp(0.0, 1.0).max(pool * 0.8);
                    let heat = 0.5 + 0.5 * c;
                    let crust = 22.0 + 22.0 * b;
                    (crust + vein * 225.0 * heat, crust * 0.5 + vein * 120.0 * heat * heat, crust * 0.35 + vein * 25.0, 255.0)
                }
                "ash" => {
                    let t = a * 0.5 + b * 0.3 + c * 0.2;
                    let ember = if c > 0.82 { 90.0 * (c - 0.82) / 0.18 } else { 0.0 };
                    (45.0 + 40.0 * t + ember * 1.6, 38.0 + 30.0 * t + ember * 0.5, 36.0 + 26.0 * t, 255.0)
                }
                "rock" => {
                    let strata = ((v * 22.0 + a * 5.0) * std::f32::consts::PI).sin() * 0.5 + 0.5;
                    let t = 0.45 * a + 0.3 * b + 0.25 * strata;
                    let s = 70.0 + 80.0 * t;
                    (s * 1.06, s * 0.98, s * 0.86, 255.0)
                }
                "brush" => {
                    // Grass blades rising from the bottom, transparent between them.
                    let col = (u * 26.0 + tile_noise(u, 0.0, 12.0, 5) * 3.0).fract();
                    let height = 0.45 + 0.55 * hash2((u * 26.0) as i32, 0, 7);
                    let up = 1.0 - v;
                    let width = 0.42 * (1.0 - up / height).max(0.0);
                    let on = up < height && (col - 0.5).abs() < width;
                    let t = 0.5 + 0.5 * up;
                    (30.0 + 40.0 * t, 80.0 + 90.0 * t, 26.0 + 30.0 * t, if on { 255.0 } else { 0.0 })
                }
                _ => (128.0, 128.0, 128.0, 255.0),
            };
            let o = x * 4;
            row[o] = r.clamp(0.0, 255.0) as u8;
            row[o + 1] = g.clamp(0.0, 255.0) as u8;
            row[o + 2] = bl.clamp(0.0, 255.0) as u8;
            row[o + 3] = al as u8;
        }
    });
    Image { width: n, height: n, pixels: px }
}

/// A theme texture reference ("gen:grass" or a file path) as map file name and DDS bytes.
/// Heights units can stand on above the ground: the upward-facing tops of walk-through props
/// (podiums, steps, plinths), on the layout's cells. NaN where there is none. Parts more than
/// 180 above the ground (pillars, statues on a podium) are left out.
pub fn walk_surfaces(l: &Layout, lib: Option<&crate::props::PropLibrary>, ground: &dyn Fn(f32, f32) -> f32) -> Vec<f32> {
    let (nx, ny) = (l.nx, l.ny);
    let mut out = vec![f32::NAN; nx * ny];
    let Some(lib) = lib else { return out };
    for q in l.props.iter().filter(|q| q.walk_through) {
        let Some(def) = lib.get(&q.prop) else { continue };
        let g0 = ground(q.pos[0], q.pos[1]) + q.lift;
        for part in &def.parts {
            let w: Vec<[f32; 3]> = part.geo.positions.iter().map(|v| {
                let (o, _) = crate::props::place_vertex(*v, [0.0, 1.0, 0.0], q.yaw, q.scale, q.tilt, q.stretch);
                [q.pos[0] + o[0], g0 + o[1], q.pos[1] + o[2]]
            }).collect();
            for t in part.geo.indices.chunks_exact(3) {
                let (a, b, c) = (w[t[0] as usize], w[t[1] as usize], w[t[2] as usize]);
                let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
                let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                // Walkable faces only: mostly upward (either winding).
                if len < 1e-6 || n[1].abs() / len < 0.75 {
                    continue;
                }
                let (x0, x1) = (a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]));
                let (z0, z1) = (a[2].min(b[2]).min(c[2]), a[2].max(b[2]).max(c[2]));
                let (i0, i1) = (((x0 / CELL).floor().max(0.0)) as usize, ((x1 / CELL).ceil() as usize).min(nx));
                let (j0, j1) = (((z0 / CELL).floor().max(0.0)) as usize, ((z1 / CELL).ceil() as usize).min(ny));
                let det = (b[0] - a[0]) * (c[2] - a[2]) - (c[0] - a[0]) * (b[2] - a[2]);
                if det.abs() < 1e-6 {
                    continue;
                }
                for j in j0..j1 {
                    for i in i0..i1 {
                        let (px, pz) = ((i as f32 + 0.5) * CELL, (j as f32 + 0.5) * CELL);
                        let l1 = ((b[0] - px) * (c[2] - pz) - (c[0] - px) * (b[2] - pz)) / det;
                        let l2 = ((c[0] - px) * (a[2] - pz) - (a[0] - px) * (c[2] - pz)) / det;
                        let l3 = 1.0 - l1 - l2;
                        if l1 < -0.02 || l2 < -0.02 || l3 < -0.02 {
                            continue;
                        }
                        let y = l1 * a[1] + l2 * b[1] + l3 * c[1];
                        let gy = ground(px, pz);
                        if y - gy > 180.0 || y < gy {
                            continue;
                        }
                        let k = j * nx + i;
                        if out[k].is_nan() || y > out[k] {
                            out[k] = y;
                        }
                    }
                }
            }
        }
    }
    out
}

/// Quads over the deep-water cells (2 × 2 cells each) at a height, tiled every 600 units, fully lit.
fn lava_geometry(l: &Layout, y: f32) -> Geometry {
    let mut g = Geometry::default();
    let step = 2usize;
    for j in (0..l.ny).step_by(step) {
        for i in (0..l.nx).step_by(step) {
            let hot = (j..(j + step).min(l.ny)).any(|jj| (i..(i + step).min(l.nx)).any(|ii| l.surface_at(ii, jj) == Surface::DeepWater && l.theme.lava_at(ii as f32 * CELL)));
            if !hot {
                continue;
            }
            let (x0, z0) = ((i as f32 - 0.5) * CELL, (j as f32 - 0.5) * CELL);
            let (x1, z1) = (((i + step) as f32 + 0.5) * CELL, ((j + step) as f32 + 0.5) * CELL);
            let b = g.positions.len() as u32;
            for (x, z) in [(x0, z0), (x1, z0), (x1, z1), (x0, z1)] {
                g.positions.push([x, y, z]);
                g.normals.push([0.0, 1.0, 0.0]);
                g.uvs.push([x / 1100.0, z / 1100.0]);
                g.uvs1.push([x / 1100.0, z / 1100.0]);
                // Some vertex light: lava glows in the dark.
                g.colors.push(0xff484848);
            }
            g.indices.extend([b, b + 2, b + 1, b, b + 3, b + 2]);
        }
    }
    g
}

/// The mound texture of League walls: the chosen one, else foliage_sr_leaves.dds next to the cliff
/// texture (classic Summoner's Rift), else the first ground layer.
pub fn mound_texture(th: &Theme) -> String {
    if !th.forest.mound_texture.is_empty() {
        return th.forest.mound_texture.clone();
    }
    let sib = Path::new(&th.cliff).with_file_name("foliage_sr_leaves.dds");
    if sib.is_file() {
        return sib.to_string_lossy().to_string();
    }
    th.ground.first().map(|g| g.texture.clone()).unwrap_or_else(|| "gen:grass".into())
}

pub fn resolve_texture(r: &str) -> Result<(String, Vec<u8>)> {
    if let Some(kind) = r.strip_prefix("gen:") {
        return Ok((format!("lms_{kind}.dds"), dds::encode_argb(&generated_texture(kind), true)));
    }
    let p = Path::new(r);
    let name = p.file_name().context("texture path has no file name")?.to_string_lossy().to_string();
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".dds") {
        return Ok((name, std::fs::read(p).with_context(|| format!("reading texture {r}"))?));
    }
    let img = image::open(p).with_context(|| format!("reading texture {r}"))?.to_rgba8();
    let (w, h) = img.dimensions();
    let stem = name.rsplit_once('.').map_or(name.as_str(), |x| x.0).to_string();
    Ok((format!("{stem}.dds"), dds::encode_argb(&Image { width: w as usize, height: h as usize, pixels: img.into_raw() }, true)))
}

// ---------------------------------------------------------------- terrain

fn blur(src: &[f32], nx: usize, ny: usize, r: usize, passes: usize) -> Vec<f32> {
    let mut a = src.to_vec();
    let mut b = vec![0.0f32; a.len()];
    for _ in 0..passes {
        for j in 0..ny {
            let mut s = 0.0;
            let mut c = 0;
            for i in 0..=r.min(nx - 1) {
                s += a[j * nx + i];
                c += 1;
            }
            for i in 0..nx {
                b[j * nx + i] = s / c as f32;
                if i + r + 1 < nx {
                    s += a[j * nx + i + r + 1];
                    c += 1;
                }
                if i >= r {
                    s -= a[j * nx + i - r];
                    c -= 1;
                }
            }
        }
        for i in 0..nx {
            let mut s = 0.0;
            let mut c = 0;
            for j in 0..=r.min(ny - 1) {
                s += b[j * nx + i];
                c += 1;
            }
            for j in 0..ny {
                a[j * nx + i] = s / c as f32;
                if j + r + 1 < ny {
                    s += b[(j + r + 1) * nx + i];
                    c += 1;
                }
                if j >= r {
                    s -= b[(j - r) * nx + i];
                    c -= 1;
                }
            }
        }
    }
    a
}

/// Heights at cell corners ((nx+1) x (ny+1)).
pub struct Terrain {
    pub nx: usize,
    pub ny: usize,
    pub h: Vec<f32>,
    pub void: Vec<bool>,
    pub water_y: f32,
    pub wall: Vec<f32>,
}

impl Terrain {
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let fx = (x / CELL).clamp(0.0, self.nx as f32 - 0.001);
        let fz = (z / CELL).clamp(0.0, self.ny as f32 - 0.001);
        let (i, j) = (fx as usize, fz as usize);
        let (tx, tz) = (fx - i as f32, fz - j as f32);
        let w = self.nx + 1;
        let h = |i: usize, j: usize| self.h[j * w + i];
        (h(i, j) * (1.0 - tx) + h(i + 1, j) * tx) * (1.0 - tz) + (h(i, j + 1) * (1.0 - tx) + h(i + 1, j + 1) * tx) * tz
    }
}

pub fn terrain(l: &Layout) -> Terrain {
    let (nx, ny) = (l.nx, l.ny);
    let mask = |s: Surface| -> Vec<f32> { l.surface.iter().map(|&v| if v == s as u8 { 1.0 } else { 0.0 }).collect() };
    // League walls rise as a rounded mound, so their mask is softened more.
    let wall = if l.theme.forest.league_walls { blur(&mask(Surface::Wall), nx, ny, 4, 3) } else { blur(&mask(Surface::Wall), nx, ny, 2, 2) };
    let mut wet: Vec<f32> = l.surface.iter().map(|&v| if v == Surface::River as u8 { 1.0 } else if v == Surface::DeepWater as u8 { 1.8 } else { 0.0 }).collect();
    wet = blur(&wet, nx, ny, 3, 2);
    let hoff = blur(&l.height, nx, ny, 1, 1);
    let th = &l.theme;
    let w = nx + 1;
    let mut h = vec![0.0f32; w * (ny + 1)];
    let mut cell_wall = vec![0.0f32; w * (ny + 1)];
    h.par_chunks_mut(w).zip(cell_wall.par_chunks_mut(w)).enumerate().for_each(|(j, (row, wrow))| {
        for i in 0..=nx {
            let (mut s_wall, mut s_wet, mut s_h, mut c) = (0.0, 0.0, 0.0, 0.0);
            for (di, dj) in [(0usize, 0usize), (1, 0), (0, 1), (1, 1)] {
                if i < di || j < dj || i - di >= nx || j - dj >= ny {
                    continue;
                }
                let k = (j - dj) * nx + (i - di);
                s_wall += wall[k];
                s_wet += wet[k];
                s_h += hoff[k];
                c += 1.0;
            }
            let (wv, wt, hv) = (s_wall / c, s_wet / c, s_h / c);
            let (x, z) = (i as f32 * CELL, j as f32 * CELL);
            // Cliffs: a steep ramp of the smoothed wall mask, with rocky variation on top.
            let ramp = ((wv - 0.2) / 0.5).clamp(0.0, 1.0);
            let ramp = ramp * ramp * (3.0 - 2.0 * ramp);
            let rough = 0.75 + 0.5 * noise2(x / 400.0, z / 400.0, 11);
            // League walls: a mossy mound, ringed with trees (classic Summoner's Rift).
            let wall_h = if th.forest.league_walls { th.forest.mound } else { th.wall_height };
            row[i] = th.base_height + hv + ramp * wall_h * rough - wt.min(1.6) * th.river_depth + (noise2(x / 900.0, z / 900.0, 4) - 0.5) * 14.0;
            wrow[i] = ramp;
        }
    });
    let void = l.surface.iter().map(|&v| v == Surface::Void as u8).collect();
    Terrain { nx, ny, h, void, water_y: th.base_height - th.river_depth * 0.35, wall: cell_wall }
}

struct Parts {
    ground: Geometry,
    cliff: Geometry,
    grass: Geometry,
}

fn terrain_geometry(l: &Layout, t: &Terrain) -> Parts {
    let (nx, ny) = (l.nx, l.ny);
    let w = nx + 1;
    let [mw, md] = l.size();
    let tiling = l.theme.tiling.max(16.0);
    let normal_at = |i: usize, j: usize| -> [f32; 3] {
        let h = |i: usize, j: usize| t.h[j.min(ny) * w + i.min(nx)];
        let dx = h(i + 1, j) - h(i.saturating_sub(1), j);
        let dz = h(i, j + 1) - h(i, j.saturating_sub(1));
        let n = [-dx, 2.0 * CELL, -dz];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        n.map(|v| v / len)
    };
    let mut ground = Geometry::default();
    let mut cliff = Geometry::default();
    let mut gidx = vec![u32::MAX; w * (ny + 1)];
    let push_ground = |g: &mut Geometry, gidx: &mut Vec<u32>, i: usize, j: usize| -> u32 {
        let k = j * w + i;
        if gidx[k] == u32::MAX {
            let (x, z) = (i as f32 * CELL, j as f32 * CELL);
            g.positions.push([x, t.h[k], z]);
            g.normals.push(normal_at(i, j));
            g.uvs.push([x / tiling, z / tiling]);
            g.uvs1.push([x / mw, z / md]);
            g.colors.push(0xff000000);
            gidx[k] = (g.positions.len() - 1) as u32;
        }
        gidx[k]
    };
    for j in 0..ny {
        for i in 0..nx {
            if t.void[j * nx + i] {
                continue;
            }
            let c = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            for tri in [[0usize, 2, 1], [0, 3, 2]] {
                let p: Vec<[f32; 3]> = tri.iter().map(|&q| {
                    let (a, b) = c[q];
                    [a as f32 * CELL, t.h[b * w + a], b as f32 * CELL]
                }).collect();
                let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                let ny_ = (n[1] / len).abs();
                if ny_ > 0.62 {
                    for &q in &tri {
                        let (a, b) = c[q];
                        let id = push_ground(&mut ground, &mut gidx, a, b);
                        ground.indices.push(id);
                    }
                } else {
                    // Cliff faces: texture wraps around the dominant horizontal axis.
                    let side = (n[0] / len).abs() > (n[2] / len).abs();
                    let base = cliff.positions.len() as u32;
                    for (k, &q) in tri.iter().enumerate() {
                        let (a, b) = c[q];
                        let v = p[k];
                        cliff.positions.push(v);
                        cliff.normals.push(normal_at(a, b));
                        let u = if side { v[2] } else { v[0] };
                        cliff.uvs.push([u / (tiling * 0.75), -v[1] / (tiling * 0.75)]);
                        cliff.uvs1.push([v[0] / mw, v[2] / md]);
                        cliff.colors.push(0xff000000);
                    }
                    cliff.indices.extend_from_slice(&[base, base + 1, base + 2]);
                }
            }
        }
    }
    // Grass: crossed blades in brush cells, a little random in size and turn.
    let mut grass = Geometry::default();
    for j in 0..ny {
        for i in 0..nx {
            if l.surface[j * nx + i] != Surface::Brush as u8 {
                continue;
            }
            for n in 0..2 {
                let r1 = hash2(i as i32, j as i32, 40 + n);
                let r2 = hash2(i as i32, j as i32, 50 + n);
                let (x, z) = ((i as f32 + r1) * CELL, (j as f32 + r2) * CELL);
                let y = t.height_at(x, z) - 4.0;
                let hgt = 120.0 + 70.0 * hash2(i as i32, j as i32, 60 + n);
                let half = 34.0 + 20.0 * r2;
                let ang = r1 * std::f32::consts::PI;
                for turn in [0.0, std::f32::consts::FRAC_PI_2] {
                    let (s, c) = (ang + turn).sin_cos();
                    let base = grass.positions.len() as u32;
                    let quad = [[x - c * half, y, z - s * half], [x + c * half, y, z + s * half], [x + c * half * 1.15, y + hgt, z + s * half * 1.15], [x - c * half * 1.15, y + hgt, z - s * half * 1.15]];
                    for (k, p) in quad.iter().enumerate() {
                        grass.positions.push(*p);
                        grass.normals.push([0.0, 1.0, 0.0]);
                        grass.uvs.push([if k == 0 || k == 3 { 0.0 } else { 1.0 }, if k < 2 { 1.0 } else { 0.0 }]);
                        grass.uvs1.push([x / mw, z / md]);
                        grass.colors.push(0xff000000);
                    }
                    grass.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
                }
            }
        }
    }
    Parts { ground, cliff, grass }
}

/// Ground split into areas (chunks); each area uses the four layers it needs most, which gives
/// one four-blend material per distinct set of layers (as Summoner's Rift's lanes, base and
/// jungle materials do).
pub const CHUNK: usize = 48;

struct GroundMaterials {
    /// Layers of each material (first = base).
    sets: Vec<Vec<usize>>,
    /// Material of each chunk.
    chunk_set: Vec<usize>,
    cx: usize,
}

fn ground_materials(l: &Layout) -> GroundMaterials {
    let (nx, ny) = (l.nx, l.ny);
    let layers = l.theme.ground.len().clamp(1, MAX_LAYERS);
    let (cx, cz) = (nx.div_ceil(CHUNK), ny.div_ceil(CHUNK));
    let mut sets: Vec<Vec<usize>> = Vec::new();
    let mut chunk_set = vec![0; cx * cz];
    for j in 0..cz {
        for i in 0..cx {
            let mut sum = vec![0u64; layers];
            let (i0, i1) = ((i * CHUNK).saturating_sub(3), ((i + 1) * CHUNK + 3).min(nx));
            let (j0, j1) = ((j * CHUNK).saturating_sub(3), ((j + 1) * CHUNK + 3).min(ny));
            for jj in j0..j1 {
                for ii in i0..i1 {
                    let w = &l.paint[jj * nx + ii];
                    for (k, s) in sum.iter_mut().enumerate() {
                        *s += w[k] as u64;
                    }
                }
            }
            let mut order: Vec<usize> = (0..layers).filter(|&k| sum[k] > 0 || k == 0).collect();
            order.sort_by(|&a, &b| sum[b].cmp(&sum[a]).then(a.cmp(&b)));
            order.truncate(4);
            let base = order[0];
            let mut rest: Vec<usize> = order[1..].to_vec();
            rest.sort();
            let mut set = vec![base];
            set.extend(rest);
            let id = match sets.iter().position(|s| *s == set) {
                Some(i) => i,
                None => {
                    sets.push(set);
                    sets.len() - 1
                }
            };
            chunk_set[j * cx + i] = id;
        }
    }
    GroundMaterials { sets, chunk_set, cx }
}

/// The four-blend mask of one layer set: r, g, b drive the shader's lerp chain toward layers
/// 1, 2, 3 of the set (rows run with +Z).
fn blend_mask(l: &Layout, set: &[usize], side: usize) -> Image {
    let (nx, ny) = (l.nx, l.ny);
    let ch: Vec<Vec<f32>> = set.iter().map(|&c| blur(&l.paint.iter().map(|w| w[c] as f32).collect::<Vec<_>>(), nx, ny, 1, 1)).collect();
    let mut px = vec![0u8; side * side * 4];
    px.par_chunks_mut(side * 4).enumerate().for_each(|(y, row)| {
        for x in 0..side {
            let (fx, fy) = ((x as f32 + 0.5) / side as f32 * nx as f32 - 0.5, (y as f32 + 0.5) / side as f32 * ny as f32 - 0.5);
            let (i, j) = (fx.floor().clamp(0.0, nx as f32 - 1.0) as usize, fy.floor().clamp(0.0, ny as f32 - 1.0) as usize);
            let (i1, j1) = ((i + 1).min(nx - 1), (j + 1).min(ny - 1));
            let (tx, ty) = ((fx - i as f32).clamp(0.0, 1.0), (fy - j as f32).clamp(0.0, 1.0));
            // Natural, slightly noisy edges between painted areas.
            let wobble = (noise2(x as f32 / 9.0, y as f32 / 9.0, 21) - 0.5) * 70.0;
            let mut w = [0.0f32; 4];
            for (c, wv) in w.iter_mut().enumerate().take(ch.len()) {
                let v = |i: usize, j: usize| ch[c][j * nx + i];
                let s = (v(i, j) * (1.0 - tx) + v(i1, j) * tx) * (1.0 - ty) + (v(i, j1) * (1.0 - tx) + v(i1, j1) * tx) * ty;
                *wv = if c > 0 && s > 2.0 && s < 253.0 { (s + wobble).max(0.0) } else { s };
            }
            let total: f32 = w.iter().sum::<f32>().max(1e-3);
            let w = w.map(|v| v / total);
            // final = lerp(lerp(lerp(L0, L1, r), L2, g), L3, b)
            let b = w[3];
            let g = if b < 0.999 { w[2] / (1.0 - b) } else { 0.0 };
            let r = if (1.0 - b) * (1.0 - g) > 1e-3 { w[1] / ((1.0 - b) * (1.0 - g)) } else { 0.0 };
            for (c, v) in [r, g, b].iter().enumerate() {
                row[x * 4 + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            row[x * 4 + 3] = 255;
        }
    });
    Image { width: side, height: side, pixels: px }
}

fn four_blend_material(name: &str, layers: &[String; 4], mask: &str) -> Material {
    let ident = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let c = 1.0 / 255.0;
    let mut m = Material::new(name, &layers[0], material_type::FOUR_BLEND, 0x11);
    m.channels = (0..8)
        .map(|i| Channel {
            color: [c; 4],
            texture: match i {
                0 => layers[0].clone(),
                1 => mask.to_string(),
                2 => layers[1].clone(),
                4 => layers[2].clone(),
                6 => layers[3].clone(),
                _ => String::new(),
            },
            transform: ident,
        })
        .collect();
    m
}

// ---------------------------------------------------------------- objects and names

pub struct Named {
    pub obj: Objective,
    pub names: Vec<(String, ObjectType)>,
}

fn team_word(team: u8) -> &'static str {
    if team == 1 { "Order" } else { "Chaos" }
}

/// Game object names for each objective (Riot's naming, so the standard rules recognise them).
pub fn object_names(l: &Layout) -> Vec<Named> {
    let mut used = std::collections::HashSet::new();
    let mut unique = |base: String| -> String {
        let mut n = base.clone();
        let mut k = 2;
        while !used.insert(n.clone()) {
            n = format!("{base}{}", (b'a' + k - 2) as char);
            k += 1;
        }
        n
    };
    let mut nth: std::collections::HashMap<ObjKind, usize> = std::collections::HashMap::new();
    l.objectives
        .iter()
        .map(|o| {
            let t = o.team;
            let n = {
                let c = nth.entry(o.kind).or_insert(0);
                *c += 1;
                *c
            };
            let lane = o.lane.letter();
            if let Some(n) = &o.name {
                let kind = match o.kind {
                    ObjKind::Nexus => ObjectType::Nexus,
                    ObjKind::Inhibitor => ObjectType::Inhibitor,
                    ObjKind::Turret | ObjKind::FountainTurret => ObjectType::Turret,
                    ObjKind::SpawnPoint | ObjKind::Fountain => ObjectType::SpawnPoint,
                    ObjKind::Shop => ObjectType::Shop,
                    ObjKind::MinionSpawn => ObjectType::Barracks,
                    k if k.is_mode_object() => ObjectType::InfoPoint,
                    _ => return Named { obj: o.clone(), names: Vec::new() },
                };
                return Named { obj: o.clone(), names: vec![(unique(n.clone()), kind)] };
            }
            let names = match o.kind {
                ObjKind::Nexus => vec![(unique(format!("HQ_T{t}")), ObjectType::Nexus)],
                ObjKind::Inhibitor => vec![(unique(format!("Barracks_T{t}_{lane}1")), ObjectType::Inhibitor)],
                ObjKind::Turret => {
                    let lane = if matches!(o.tier, TurretTier::NexusA | TurretTier::NexusB) { 'C' } else { lane };
                    vec![(unique(format!("Turret_T{t}_{lane}_{:02}", o.tier.code())), ObjectType::Turret)]
                }
                ObjKind::Fountain | ObjKind::SpawnPoint => vec![(unique(format!("__Spawn_T{t}")), ObjectType::SpawnPoint)],
                ObjKind::FountainTurret => vec![(unique(format!("Turret_{}TurretShrine", team_word(t))), ObjectType::Turret)],
                ObjKind::Shop => vec![(unique(format!("{}Shop01", if t == 1 { "order" } else { "chaos" })), ObjectType::Shop)],
                ObjKind::MinionSpawn => vec![(unique(format!("__P_{}_Spawn_Barracks__{lane}01", team_word(t))), ObjectType::Barracks)],
                // Dominion / ARAM objects are spawned by the server at Map8/Map12-style markers.
                ObjKind::CapturePoint => vec![(unique(format!("Info_Point{}", (b'A' + (n as u8 - 1).min(25)) as char)), ObjectType::InfoPoint)],
                ObjKind::SpeedShrine => vec![(unique(format!("Info_SpeedShrine{n}")), ObjectType::InfoPoint)],
                ObjKind::CenterRelic => vec![(unique(format!("Info_LargeRelic{n}")), ObjectType::InfoPoint)],
                ObjKind::HealthRelic => vec![(unique(format!("Info_HealthPack{n:02}")), ObjectType::InfoPoint)],
                ObjKind::Crystal => vec![(unique(format!("Info_Spawn_{}_Crystal", team_word(t))), ObjectType::InfoPoint)],
                // Jungle camps are scripted (NeutralMinionSpawn.lua), not map objects.
                _ => Vec::new(),
            };
            Named { obj: o.clone(), names }
        })
        .collect()
}

fn cfg_section(name: &str, kind: ObjectType, team: u8, yaw: f32) -> String {
    let side = team_word(team);
    match kind {
        ObjectType::Nexus => format!("[{name}]\r\nmMaxHP=5500.0000\r\nmBaseStaticHPRegen=20\r\nmArmor=0.0000\r\nmMaxMP=0.0000\r\nSelectionHeight=117.0000\r\nSelectionRadius=397.2222\r\nPerceptionBubbleRadius=1350.0000\r\nArmorMaterial=Metal\r\nSkinName1={side}Nexus\r\nSkinName2=DestroyedNexus\r\nCollision Radius=319.4445\r\nCollision Height=0.0000\r\nPathfindingCollisionRadius=352.7778\r\n"),
        ObjectType::Inhibitor => format!("[{name}]\r\nmMaxHP=3000.0000\r\nmBaseStaticHPRegen=15\r\nmArmor=0.0000\r\nmMaxMP=0.0000\r\nSelectionHeight=105.0000\r\nSelectionRadius=197.0000\r\nPerceptionBubbleRadius=1350.0000\r\nArmorMaterial=Wood\r\nSkinName1={side}Inhibitor\r\nSkinName2={side}Inhibitor_D\r\nPathfindingCollisionRadius=187.1500\r\n"),
        ObjectType::Barracks => format!("[{name}]\r\nSkinName1={side}Barracks_Wizard\r\nSelectionHeight=39.6218\r\nSelectionRadius=19.8094\r\nPathfindingCollisionRadius=18.8190\r\n"),
        ObjectType::Turret => format!("[{name}]\r\nMove=0.0000 0.0000 0.0000\r\nRot={:.4} 0.0000 0.0000\r\n", yaw.rem_euclid(360.0)),
        _ => String::new(),
    }
}

fn turret_skin(name: &str, o: &Objective) -> (String, u8) {
    if let Some(s) = &o.skin {
        let tier = if o.kind == ObjKind::FountainTurret { 0 } else { o.tier.code() };
        return (s.clone(), tier);
    }
    if name.contains("TurretShrine") {
        return (format!("{}TurretShrine", team_word(o.team)), 0);
    }
    (o.tier.skin(o.team).to_string(), o.tier.code())
}

// ---------------------------------------------------------------- scripts

fn create_level_props(named: &[Named]) -> String {
    let mut s = String::from("-- Generated by LoL Map Studio: creates the map's turrets.\r\n-- CreateChildTurret(name, skin, team, tier, lane): tier 3 outer, 2 inner, 1 inhibitor, 5/4 nexus, 0 fountain; lane 0 bottom, 1 mid, 2 top.\r\nfunction CreateLevelProps()\r\n");
    for n in named {
        for (name, kind) in &n.names {
            if *kind != ObjectType::Turret {
                continue;
            }
            let (skin, tier) = turret_skin(name, &n.obj);
            let team = if n.obj.team == 1 { "TEAM_ORDER" } else { "TEAM_CHAOS" };
            let lane = if tier == 0 || tier >= 4 { 1 } else { n.obj.lane.index() };
            s += &format!("\tCreateChildTurret(\"{name}\", \"{skin}\", {team}, {tier}, {lane})\r\n");
        }
    }
    s += "end\r\n";
    s
}

/// Summoner's Rift camp data (Map1 NeutralMinionSpawn.luaobj): monster formation offsets from
/// the first spawn point (blue side; point-mirrored on the red side), camp type, respawn and
/// first-spawn delay in seconds, minimap icon, timer type per side, camp level, kill events.
struct CampDef {
    offsets: &'static [[f32; 2]],
    camp_type: &'static str,
    respawn: u32,
    delay: u32,
    icon: &'static str,
    timer: Option<[&'static str; 2]>,
    level: u32,
    events: Option<(&'static str, &'static str, &'static str)>,
}

fn camp_def(k: ObjKind) -> Option<CampDef> {
    let lesser = |offsets: &'static [[f32; 2]], camp_type: &'static str| CampDef { offsets, camp_type, respawn: 50, delay: 25, icon: "LesserCamp", timer: None, level: 1, events: None };
    Some(match k {
        ObjKind::BlueBuff => CampDef { offsets: &[[0.0, 0.0], [-80.0, 199.0], [-180.0, -10.0]], camp_type: "CAMPTYPE_BLUEBUFF", respawn: 300, delay: 15, icon: "Camp", timer: Some(["Order Blue", "Chaos Blue"]), level: 1, events: None },
        ObjKind::RedBuff => CampDef { offsets: &[[0.0, 0.0], [5.0, -180.0], [-218.0, 0.0]], camp_type: "CAMPTYPE_REDBUFF", respawn: 300, delay: 15, icon: "Camp", timer: Some(["Order Red", "Chaos Red"]), level: 1, events: None },
        ObjKind::Wolves => lesser(&[[0.0, 0.0], [150.0, 0.0], [-50.0, 150.0]], "CAMPTYPE_WOLVES"),
        ObjKind::Wraiths => lesser(&[[0.0, 0.0], [222.9, 126.9], [353.8, -21.8], [282.9, -192.2]], "CAMPTYPE_WRAITHS"),
        ObjKind::Golems => lesser(&[[0.0, 0.0], [300.0, 0.0], [150.0, -50.0]], "CAMPTYPE_SMALLGOLEMS"),
        ObjKind::Wight => lesser(&[[0.0, 0.0]], "CAMPTYPE_OWLBEAR"),
        ObjKind::Dragon => CampDef { offsets: &[[0.0, 0.0], [-104.6, 48.9], [95.4, -151.1]], camp_type: "CAMPTYPE_DRAGON", respawn: 360, delay: 50, icon: "Dragon", timer: Some(["Dragon", "Dragon"]), level: 1, events: Some(("VO_INTERACTIVE_REVEAL_ENEMY_DRAGON_KILL", "EVENT_ON_KILL_DRAGON", "EVENT_ON_KILL_DRAGON_SPECTATOR")) },
        // ARAM health relics (Map12 NeutralMinionSpawn: respawn 40 s, first after 120 s, camp level 3).
        ObjKind::HealthRelic => CampDef { offsets: &[[0.0, 0.0]], camp_type: "CAMPTYPE_HEALTHRELIC", respawn: 40, delay: 120, icon: "LesserCamp", timer: None, level: 3, events: None },
        ObjKind::Baron => CampDef { offsets: &[[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]], camp_type: "CAMPTYPE_NASHOR", respawn: 420, delay: 800, icon: "Baron", timer: Some(["Baron", "Baron"]), level: 2, events: Some(("VO_INTERACTIVE_REVEAL_ENEMY_NASHOR_KILL", "EVENT_ON_KILL_WORM", "EVENT_ON_KILL_WORM_SPECTATOR")) },
        _ => return None,
    })
}

/// Whether an objective kind is written as a camp in NeutralMinionSpawn.lua.
pub(crate) fn camp_def_exists(k: ObjKind) -> bool {
    camp_def(k).is_some()
}

pub(crate) fn create_level_props_pub(named: &[Named]) -> String {
    create_level_props(named)
}

/// Monster character names of a camp kind (for previews and Play mode).
pub fn camp_monsters(k: ObjKind) -> &'static [&'static str] {
    match k {
        ObjKind::BlueBuff => &["AncientGolem", "YoungLizard", "YoungLizard"],
        ObjKind::RedBuff => &["LizardElder", "YoungLizard", "YoungLizard"],
        ObjKind::Wolves => &["GiantWolf", "Wolf", "Wolf"],
        ObjKind::Wraiths => &["Wraith", "LesserWraith", "LesserWraith", "LesserWraith"],
        ObjKind::Golems => &["SmallGolem", "Golem"],
        ObjKind::Wight => &["GreatWraith"],
        ObjKind::Dragon => &["Dragon"],
        ObjKind::Baron => &["Worm"],
        ObjKind::HealthRelic => &["HA_AP_HealthRelic"],
        _ => &[],
    }
}

/// Where each monster of a camp stands (world X/Z), Map1's formation.
pub fn camp_positions(o: &Objective) -> Vec<[f32; 2]> {
    let Some(def) = camp_def(o.kind) else { return Vec::new() };
    let flip = if o.team == 2 { -1.0 } else { 1.0 };
    camp_monsters(o.kind).iter().enumerate().map(|(i, _)| {
        let q = def.offsets[i.min(def.offsets.len() - 1)];
        [o.pos[0] + q[0] * flip, o.pos[1] + q[1] * flip]
    }).collect()
}

/// NeutralMinionSpawn.lua for the layout's camps, with the same tables and functions as
/// Summoner's Rift's script (NeutralMinionInit, InitializeNeutralMinionInfo,
/// SpawnInitialNeutralMinions, NeutralMinionDeath, CreateRespawnNeutralTimer).
pub fn neutral_script(l: &Layout, height: &dyn Fn(f32, f32) -> f32) -> String {
    let camps: Vec<(&Objective, CampDef)> = l.objectives.iter().filter_map(|o| camp_def(o.kind).map(|d| (o, d))).collect();
    if camps.is_empty() {
        return NEUTRAL_STUB.to_string();
    }
    let [w, d] = l.size();
    let pt = |x: f32, z: f32| format!("Make3DPoint({x:.4}, {:.4}, {z:.4})", height(x, z));
    let q = |v: &[&str]| v.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ");
    let mut s = String::from("-- Generated by LoL Map Studio: jungle camps, structured like Summoner's Rift's NeutralMinionSpawn.\r\n");
    s += "TEAM_UNKNOWN = 0\r\nTEAM_ORDER = 100\r\nTEAM_CHAOS = 200\r\nTEAM_NEUTRAL = 300\r\nTEAM_MAX = 400\r\nHOSTILE = 1\r\nINACTIVE = 0\r\nHERO_EXP_RADIUS = 800\r\n";
    // Everything the map spawns is preloaded (the camps' monsters and Dominion/ARAM objects).
    let mut names = vec!["Lizard", "Dragon", "Golem", "wolf", "AncientGolem", "LesserWraith", "GiantWolf", "LizardElder", "YoungLizard", "Wraith", "Worm", "SmallGolem", "GreatWraith"];
    for o in &l.objectives {
        let extra: &[&str] = match o.kind {
            ObjKind::HealthRelic => &["HA_AP_HealthRelic"],
            ObjKind::CapturePoint => &["OdinNeutralGuardian"],
            ObjKind::SpeedShrine => &["OdinSpeedShrine"],
            ObjKind::CenterRelic => &["OdinCenterRelic"],
            ObjKind::Crystal => &["Odin_SOG_Order_Crystal", "Odin_SOG_Chaos_Crystal"],
            _ => &[],
        };
        for e in extra {
            if !names.contains(e) {
                names.push(e);
            }
        }
    }
    s += &format!("NeutralMinionNames = {{ {} }}\r\n", q(&names));
    s += "DefaultNeutralMinionValues = {}\r\n";
    for n in ["SmallGolem", "Dragon", "Golem", "AncientGolem", "LesserWraith", "LizardElder", "GiantWolf", "YoungLizard", "Worm", "wolf", "Wraith", "Urf"] {
        s += &format!("DefaultNeutralMinionValues[\"{n}\"] = {{ GoldGiven = 0, ExpGiven = 0, SoulGiven = 0 }}\r\n");
    }
    s += "NeutralTimers = {}\r\nCAMPTYPE_DRAGON = 1\r\nCAMPTYPE_SMALLGOLEMS = 2\r\nCAMPTYPE_WRAITHS = 3\r\nCAMPTYPE_REDBUFF = 4\r\nCAMPTYPE_WOLVES = 5\r\nCAMPTYPE_BLUEBUFF = 6\r\nCAMPTYPE_OWLBEAR = 7\r\nCAMPTYPE_NASHOR = 8\r\nCAMPTYPE_HEALTHRELIC = 9\r\nPredefinedCamps = {}\r\n";
    for (k, ty) in [(ObjKind::Dragon, "DRAGON"), (ObjKind::Golems, "SMALLGOLEMS"), (ObjKind::Wraiths, "WRAITHS"), (ObjKind::RedBuff, "REDBUFF"), (ObjKind::Wolves, "WOLVES"), (ObjKind::BlueBuff, "BLUEBUFF"), (ObjKind::Wight, "OWLBEAR"), (ObjKind::Baron, "NASHOR"), (ObjKind::HealthRelic, "HEALTHRELIC")] {
        s += &format!("PredefinedCamps[CAMPTYPE_{ty}] = {{ {} }}\r\n", q(camp_monsters(k)));
    }
    s += "PredefinedCampKillEvents = {}\r\nPredefinedCampKillEvents[CAMPTYPE_DRAGON] = { \"EVENT_ON_KILL_DRAGON\" }\r\nPredefinedCampKillEvents[CAMPTYPE_NASHOR] = { \"EVENT_ON_KILL_WORM\" }\r\n";
    s += "PredefinedCampKillEventsForSpectator = {}\r\nPredefinedCampKillEventsForSpectator[CAMPTYPE_DRAGON] = { \"EVENT_ON_KILL_DRAGON_SPECTATOR\" }\r\nPredefinedCampKillEventsForSpectator[CAMPTYPE_NASHOR] = { \"EVENT_ON_KILL_WORM_SPECTATOR\" }\r\n";
    s += "CampSpawnPoints = {}\r\nCampFacePoints = {}\r\nNeutralMinionCamps = {}\r\n";
    for (i, (o, def)) in camps.iter().enumerate() {
        let n = i + 1;
        let flip = if o.team == 2 { -1.0 } else { 1.0 };
        let pts: Vec<String> = def.offsets.iter().map(|q| pt(o.pos[0] + q[0] * flip, o.pos[1] + q[1] * flip)).collect();
        // Monsters face the camp's turn, or the map centre.
        let (fx, fz) = if o.yaw != 0.0 {
            let a = o.yaw.to_radians();
            (o.pos[0] + a.sin() * 400.0, o.pos[1] + a.cos() * 400.0)
        } else {
            let (dx, dz) = (w / 2.0 - o.pos[0], d / 2.0 - o.pos[1]);
            let len = dx.hypot(dz).max(1.0);
            (o.pos[0] + dx / len * 400.0, o.pos[1] + dz / len * 400.0)
        };
        let face: Vec<String> = def.offsets.iter().map(|_| pt(fx, fz)).collect();
        let side = if o.team == 0 { "neutral" } else { team_word(o.team) };
        s += &format!("-- {} ({side})\r\nCampSpawnPoints[{n}] = {{ {} }}\r\nCampFacePoints[{n}] = {{ {} }}\r\nNeutralMinionCamps[{n}] = {{}}\r\n", o.kind.label(), pts.join(", "), face.join(", "));
    }
    s += "NUMBER_OF_CAMPS = table.getn(NeutralMinionCamps)\r\n\r\nfunction NeutralMinionInit()\r\n\tfor _, n in pairs(NeutralMinionNames) do\r\n\t\tPreloadCharacter(n)\r\n\tend\r\n\tfor i = 1, NUMBER_OF_CAMPS do\r\n\t\tlocal c = NeutralMinionCamps[i]\r\n\t\tc.Positions = CampSpawnPoints[i]\r\n\t\tc.FacePositions = CampFacePoints[i]\r\n\t\tc.UniqueNames = {}\r\n\t\tc.AliveTracker = {}\r\n\t\tc.Team = TEAM_NEUTRAL\r\n\t\tc.DamageBonus = 0\r\n\t\tc.HealthBonus = 0\r\n\t\tc.AIState = INACTIVE\r\n\t\tc.GroupsRespawnTime = 60\r\n\t\tc.CampLevel = 1\r\n\t\tc.TimerType = \"\"\r\n\t\tc.MinimapIcon = \"LesserCamp\"\r\n\tend\r\n";
    for (i, (o, def)) in camps.iter().enumerate() {
        let c = format!("\tNeutralMinionCamps[{}]", i + 1);
        s += &format!("{c}.Groups = {{ PredefinedCamps[{}] }}\r\n{c}.GroupsChance = {{ 100 }}\r\n{c}.GroupsRespawnTime = {}\r\n{c}.GroupDelaySpawnTime = {}\r\n", def.camp_type, def.respawn, def.delay);
        if o.team != 0 && o.kind != ObjKind::HealthRelic {
            s += &format!("{c}.GroupBuffSide = {}\r\n", if o.team == 1 { "TEAM_ORDER" } else { "TEAM_CHAOS" });
        }
        if let Some(t) = def.timer {
            s += &format!("{c}.TimerType = \"{}\"\r\n", t[usize::from(o.team == 2)]);
        }
        if def.icon != "LesserCamp" {
            s += &format!("{c}.MinimapIcon = \"{}\"\r\n", def.icon);
        }
        if def.level != 1 {
            s += &format!("{c}.CampLevel = {}\r\n", def.level);
        }
        if let Some((reveal, kill, spec)) = def.events {
            s += &format!("{c}.RevealEvent = \"{reveal}\"\r\n{c}.KillEvents = {{ \"{kill}\" }}\r\n{c}.SpectatorKillEvents = {{ \"{spec}\" }}\r\n");
        }
    }
    s += "end\r\n\r\n";
    s += &include_str!("neutral_functions.lua").replace("\r\n", "\n").replace('\n', "\r\n");
    s
}

/// Particles.dat ("name x y z quality rx ry rz", quality INT_MIN = always; the client reads
/// ry twice), Light_Env.dat (version 3: x y z r g b r1 g1 b1 inner outer shadows intensity) and
/// Light.dat (x y z r g b radius; only read when Light_Env.dat is missing).
pub fn world_files(l: &Layout, height: &dyn Fn(f32, f32) -> f32) -> (String, String, String) {
    let mut particles = String::new();
    for e in &l.effects {
        let y = height(e.pos[0], e.pos[1]) + e.lift;
        let stem = e.name.trim_end_matches(".troybin").trim_end_matches(".troy");
        particles += &format!("{stem}.troy {} {} {} -2147483648 0 {} {} \r\n", e.pos[0], y, e.pos[1], e.yaw, e.yaw);
    }
    let mut env = String::from("3\r\n");
    let mut old = String::new();
    for lt in &l.lights {
        let y = height(lt.pos[0], lt.pos[1]) + lt.lift;
        let [r, g, b] = lt.color;
        env += &format!("{} {} {} {r} {g} {b} 0 0 0 {} {} 1 {:.6}\r\n", lt.pos[0] as i32, y as i32, lt.pos[1] as i32, lt.inner as i32, lt.outer as i32, lt.intensity);
        old += &format!("{} {} {} {r} {g} {b} {}\r\n", lt.pos[0] as i32, y as i32, lt.pos[1] as i32, lt.outer as i32);
    }
    (particles, env, old)
}

const NEUTRAL_STUB: &str = "-- Generated by LoL Map Studio: this map has no jungle camps.\r\nNUMBER_OF_CAMPS = 0\r\nNeutralMinionCamps = {}\r\nfunction NeutralMinionInit() end\r\nfunction NeutralMinionDeath() end\r\nfunction CreateRespawnNeutralTimer() end\r\nfunction SpawnInitialNeutralMinions() end\r\nfunction InitializeNeutralMinionInfo() end\r\n";

const SANDBOX_LEVEL_SCRIPT: &str = "-- Generated by LoL Map Studio (Sandbox rules): structures and spawns, no minion waves.\r\nfunction OnLevelInit()\r\n\tLoadLevelScriptIntoScript(\"CreateLevelProps.lua\")\r\n\tCreateLevelProps()\r\nend\r\nfunction OnLevelInitServer() end\r\nfunction OnPostLevelLoad() end\r\nfunction PostGameSetup() end\r\nfunction PostGameUpdate() end\r\nfunction HandleDestroyedObject() end\r\n";

/// Files of a rules map that are generic gameplay data (not shaped by its terrain).
fn is_rules_file(rel: &str) -> bool {
    let l = rel.to_ascii_lowercase();
    !(l.starts_with("scene/")
        || l.starts_with("info/")
        || l.starts_with("aipath")
        || l.starts_with("navgrideditor/")
        || matches!(l.as_str(), "particles.dat" | "light.dat" | "light_env.dat" | "locations.dat" | "terrain.inibin" | "particles.inibin")
        || l == "scripts/createlevelprops.luaobj"
        || l == "scripts/neutralminionspawn.luaobj"
        || l.starts_with("encounters/"))
}

fn sun_inibin(l: &Layout, base: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut ini = base.and_then(|b| Inibin::parse(b).ok()).unwrap_or_default();
    let th = &l.theme;
    let v3 = |v: [f32; 3]| Value::Str(format!("{:.4} {:.4} {:.4}", v[0], v[1], v[2]));
    ini.entries.retain(|e| e.key != hash_key("Environment", "Renderer"));
    ini.set(hash_key("Sun", "SunDir"), v3(th.sun_dir));
    ini.set(hash_key("Sun", "SunLightColor"), v3(th.sun_color));
    ini.set(hash_key("Sun", "AmbientLightColor"), v3(th.ambient));
    ini.write()
}

// ---------------------------------------------------------------- the build

/// Map objects, ObjectCFG sections and shop meshes for the layout's structures.
pub(crate) fn write_structures(named: &[Named], height: &dyn Fn(f32, f32) -> f32, objects: &mut Vec<MapObject>, dsc: &mut Vec<DscEntry>, cfg: &mut String, p: &mut MapProject) -> Result<()> {
    let mut shops = Vec::new();
    for n in named {
        let o = &n.obj;
        if o.kind == ObjKind::Shop {
            continue;
        }
        for (name, kind) in n.names.iter() {
            let pos2 = o.pos;
            let y = height(pos2[0], pos2[1]);
            let mut m = MapObject::new(name, *kind, [pos2[0], y, pos2[1]]);
            m.pitch_yaw_roll = [0.0, o.yaw, 0.0];
            let r = match kind {
                ObjectType::Nexus => 330.0,
                ObjectType::Inhibitor => 210.0,
                ObjectType::Turret => 150.0,
                _ => 0.0,
            };
            if r > 0.0 {
                m.bbox_min = [pos2[0] - r, y - 300.0, pos2[1] - r];
                m.bbox_max = [pos2[0] + r, y + 700.0, pos2[1] + r];
            }
            if *kind == ObjectType::Barracks {
                m.scale = [1.0; 3];
            }
            objects.push(m);
            *cfg += &cfg_section(name, *kind, o.team, o.yaw);
        }
        if o.kind == ObjKind::Shop {
            shops.push((o.team, o.pos, n.names[0].0.clone()));
        }
    }
    // Shops: the game makes a shop from a room.dsc mesh named *Shop*.
    for (team, at, stem) in shops {
        let stem = stem.as_str();
        let y = height(at[0], at[1]);
        let r = 120.0;
        let v = vec![[at[0] - r, y, at[1] - r], [at[0] + r, y, at[1] - r], [at[0] + r, y, at[1] + r], [at[0] - r, y, at[1] + r], [at[0], y + 300.0, at[1]]];
        let faces = (0..4u32).map(|k| ([k, (k + 1) % 4, 4], "defaultShader".to_string(), [[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]])).collect();
        let s = Scb::new(stem, v, faces);
        p.add_or_replace(MapFile::generated(&format!("Scene/{stem}.SCB"), s.write()?));
        p.add_or_replace(MapFile::generated(&format!("Scene/{stem}.sco"), s.to_sco()));
        dsc.push(DscEntry { file: format!("{stem}.sco"), quality: -100 });
        let _ = team;
        let mut sec = stem.to_string();
        sec[..1].make_ascii_uppercase();
        *cfg += &format!("[{sec}]\r\nSelectionHeight=396.9950\r\nSelectionRadius=269.6738\r\nPathfindingCollisionRadius=256.1901\r\n");
    }
    Ok(())
}

/// The minions' nav points along the layout's lanes, blue to red.
pub(crate) fn lane_points(l: &Layout, height: &dyn Fn(f32, f32) -> f32) -> Vec<MapObject> {
    let mut out = Vec::new();
    for lp in &l.lanes {
        let total: f32 = lp.points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
        let count = ((total / 750.0).round() as usize).max(2);
        for k in 0..=count {
            let pt = crate::layout::point_along(&lp.points, k as f32 / count as f32);
            let name = format!("__NAV_{}0{}", lp.lane.letter(), k + 2);
            let mut m = MapObject::new(&name, ObjectType::NavPoint, [pt[0], height(pt[0], pt[1]), pt[1]]);
            m.scale = [0.6; 3];
            out.push(m);
        }
    }
    out
}

pub fn build(l: &Layout, opt: &BuildOptions) -> Result<MapProject> {
    if l.base.is_some() {
        return crate::layout_base::build_on_base(l, opt);
    }
    let folder = format!("Map{}", l.map_id);
    let mut p = MapProject::empty(&folder);
    let [mw, md] = l.size();
    let t = terrain(l);
    let th = &l.theme;

    // Rules files first, so generated files replace them.
    let rules = if opt.preview { None } else { opt.rules_source.as_deref().filter(|_| l.rules == Rules::Classic).map(MapProject::open).transpose()? };
    if let Some(r) = &rules {
        for f in &r.files {
            if is_rules_file(&f.path) {
                p.add_or_replace(MapFile::new(&f.path, f.original.to_vec()));
            }
        }
    }

    // Textures.
    let mut tex_names = Vec::new();
    let ground_refs: Vec<&String> = th.ground.iter().map(|g| &g.texture).collect();
    for r in ground_refs.iter().copied().chain([&th.cliff, &th.brush]) {
        let (name, data) = resolve_texture(r)?;
        if p.find(&format!("Scene/Textures/{name}")).is_none() {
            p.add_or_replace(MapFile::generated(&format!("Scene/Textures/{name}"), data));
        }
        tex_names.push(name);
    }
    let nl = th.ground.len().max(1);
    // League walls: the mound is leafy ground (classic Summoner's Rift's foliage), not a cliff face.
    let cliff_tex = if th.forest.league_walls {
        let r = mound_texture(th);
        match resolve_texture(&r) {
            Ok((name, data)) => {
                if p.find(&format!("Scene/Textures/{name}")).is_none() {
                    p.add_or_replace(MapFile::generated(&format!("Scene/Textures/{name}"), data));
                }
                name
            }
            Err(_) => tex_names[0].clone(),
        }
    } else {
        tex_names[nl].clone()
    };
    let brush_tex = tex_names[nl + 1].clone();
    let gm = ground_materials(l);
    let mut mats = vec![
        Material::new("lms_cliff", &cliff_tex, material_type::DEFAULT, 0x11),
        Material::new("lms_brush", &brush_tex, material_type::WALL_OF_GRASS, 0x10),
    ];
    let side = if opt.preview { 512 } else { 1024 };
    for (k, set) in gm.sets.iter().enumerate() {
        let mask_name = format!("lms_blendmask{k}.dds");
        let mask = blend_mask(l, set, side);
        p.add_or_replace(MapFile::generated(&format!("Scene/Textures/{mask_name}"), dds::encode_argb(&mask, false)));
        let t4: [String; 4] = std::array::from_fn(|i| tex_names[*set.get(i).unwrap_or(&set[0])].clone());
        mats.push(four_blend_material(&format!("lms_ground{k}"), &t4, &mask_name));
    }

    // Scene geometry: ground split by area material, cliffs, grass, props.
    let parts = terrain_geometry(l, &t);
    let mut geo: Vec<(u32, Geometry)> = vec![(0, parts.cliff), (1, parts.grass)];
    let mut by_set: Vec<Vec<u32>> = vec![Vec::new(); gm.sets.len()];
    for tri in parts.ground.indices.chunks_exact(3) {
        let c = [0, 2].map(|a| (parts.ground.positions[tri[0] as usize][a] + parts.ground.positions[tri[1] as usize][a] + parts.ground.positions[tri[2] as usize][a]) / 3.0);
        let (ci, cj) = (((c[0] / CELL) as usize / CHUNK).min(gm.cx - 1), (c[1] / CELL) as usize / CHUNK);
        let set = gm.chunk_set.get(cj * gm.cx + ci).copied().unwrap_or(0);
        by_set[set].extend_from_slice(tri);
    }
    for (k, idx) in by_set.into_iter().enumerate() {
        if !idx.is_empty() {
            geo.push(((k + 2) as u32, crate::formats::nvr::subset(&parts.ground, &idx)));
        }
    }
    // Lava: a glowing surface over deep water.
    if l.theme.lava {
        let lava = lava_geometry(l, t.water_y + 6.0);
        if !lava.indices.is_empty() {
            p.add_or_replace(MapFile::generated("Scene/Textures/lms_lava.dds", dds::encode_argb(&generated_texture("lava"), true)));
            mats.push(Material::new("lms_lava", "lms_lava.dds", material_type::DEFAULT, 0x10));
            geo.push(((mats.len() - 1) as u32, lava));
        }
    }
    let blockers = add_props(l, opt, None, &|x, z| t.height_at(x, z), &mut mats, &mut geo, &mut p)?;
    geo.retain(|(_, g)| !g.indices.is_empty());
    let nvr = Nvr::build(mats, &geo, 900.0)?;
    p.add_or_replace(MapFile::generated("Scene/room.nvr", nvr.write()?));

    // Water: one Lake01 object over rivers and deep water.
    let mut room_mat = String::from("[MaterialBegin]\r\nName= defaultShader\r\nFlags= texture_gouraud_\r\nEmissiveColor= 0 0 0\r\nUVScroll = 0 0\r\nDisableBackfaceCulling = 0\r\nShaderName = Prop\r\nSimpleShader = 1\r\nOpacity= 0\r\nColor24= 255 0 0\r\n[MaterialEnd]\r\n\r\n");
    let mut dsc = Vec::new();
    let mut lake_v: Vec<[f32; 3]> = Vec::new();
    let mut lake_f = Vec::new();
    let step = 4usize;
    for j in (0..l.ny).step_by(step) {
        for i in (0..l.nx).step_by(step) {
            // On lava maps deep water is lava (its own mesh), so only rivers get water.
            let wet = (j..(j + step).min(l.ny)).any(|jj| (i..(i + step).min(l.nx)).any(|ii| match l.surface_at(ii, jj) {
                Surface::River => true,
                Surface::DeepWater => !l.theme.lava_at(ii as f32 * CELL),
                _ => false,
            }));
            if !wet {
                continue;
            }
            let (x0, z0) = ((i as f32 - 2.0) * CELL, (j as f32 - 2.0) * CELL);
            let (x1, z1) = (((i + step) as f32 + 2.0) * CELL, ((j + step) as f32 + 2.0) * CELL);
            let b = lake_v.len() as u32;
            for (x, z) in [(x0, z0), (x1, z0), (x1, z1), (x0, z1)] {
                lake_v.push([x, t.water_y, z]);
            }
            let uv = |x: f32, z: f32| [x / mw, z / md];
            lake_f.push(([b, b + 2, b + 1], "LakeMask01_".to_string(), [uv(x0, z0), uv(x1, z1), uv(x1, z0)]));
            lake_f.push(([b, b + 3, b + 2], "LakeMask01_".to_string(), [uv(x0, z0), uv(x0, z1), uv(x1, z1)]));
        }
    }
    if !lake_f.is_empty() {
        let lake = Scb::new("Lake01", lake_v, lake_f);
        p.add_or_replace(MapFile::generated("Scene/Lake01.SCB", lake.write()?));
        p.add_or_replace(MapFile::generated("Scene/Lake01.sco", lake.to_sco()));
        dsc.push(DscEntry { file: "Lake01.sco".into(), quality: -100 });
        room_mat += "[MaterialBegin]\r\nName= LakeMask01_\r\nFlags= texture_gouraud_\r\nEmissiveColor= 0 0 0\r\nUVScroll = 0 0\r\nDisableBackfaceCulling = 0\r\nShaderName = Prop\r\nSimpleShader = 1\r\nOpacity= 255\r\nTexture= lakemask01.tga\r\nColor24= 204 204 204\r\n[MaterialEnd]\r\n\r\n";
        p.add_or_replace(MapFile::generated("Scene/Textures/LakeMask01.dds", dds::encode_argb(&Image { width: 64, height: 64, pixels: vec![255; 64 * 64 * 4] }, false)));
        p.add_or_replace(MapFile::generated("Scene/CFG/Lake01.cfg", b"[General]\r\n\r\nScrollX1 = 2\r\nScrollY1 = -1\r\nScrollSpeed1 = .1\r\n\r\nScrollX2 = 1\r\nScrollY2 = -2\r\nScrollSpeed2 = .3\r\nTileFrequency = 2\r\n".to_vec()));
    }

    // Map objects.
    let named = object_names(l);
    let mut mob = MapObjects { reserved: 0, objects: Vec::new() };
    let mut cfg = String::new();
    write_structures(&named, &|x, z| t.height_at(x, z), &mut mob.objects, &mut dsc, &mut cfg, &mut p)?;
    mob.objects.extend(lane_points(l, &|x, z| t.height_at(x, z)));
    let mut ls = MapObject::new("__LevelSize_", ObjectType::LevelSize, [mw / 2.0, -1997.0, md / 2.0]);
    ls.bbox_min = [0.0, -1997.0, 0.0];
    ls.bbox_max = [mw, -1997.0, md];
    mob.objects.push(ls);
    mob.objects.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    p.add_or_replace(MapFile::generated("Scene/MapObjects.mob", mob.write()?));
    p.add_or_replace(MapFile::generated("Scene/room.dsc", crate::formats::bin::latin1_encode(&write_dsc(&dsc))));
    p.add_or_replace(MapFile::generated("Scene/room.mat", crate::formats::bin::latin1_encode(&room_mat)));
    p.add_or_replace(MapFile::generated("Scene/CFG/ObjectCFG.cfg", crate::formats::bin::latin1_encode(&cfg)));

    // Scripts.
    p.add_or_replace(MapFile::generated("Scripts/CreateLevelProps.lua", crate::formats::bin::latin1_encode(&create_level_props(&named))));
    p.add_or_replace(MapFile::generated("Scripts/NeutralMinionSpawn.lua", crate::formats::bin::latin1_encode(&neutral_script(l, &|x, z| t.height_at(x, z)))));
    if rules.is_none() && !opt.preview {
        p.add_or_replace(MapFile::generated("Scripts/LevelScript.lua", SANDBOX_LEVEL_SCRIPT.as_bytes().to_vec()));
    }

    // Settings: sun (keeping the rules map's other terrain settings), lights, locations.
    let base_terrain = rules.as_ref().and_then(|r| r.file("terrain.inibin")).map(|f| f.original.to_vec());
    p.add_or_replace(MapFile::generated("terrain.inibin", sun_inibin(l, base_terrain.as_deref())?));
    // Placed effects and lights, in the files the client's LoadWorldParticles / LoadWorldLights read.
    let (particles, light_env, light) = world_files(l, &|x, z| t.height_at(x, z));
    p.add_or_replace(MapFile::generated("Particles.dat", particles.into_bytes()));
    p.add_or_replace(MapFile::generated("Light_Env.dat", light_env.into_bytes()));
    p.add_or_replace(MapFile::generated("Light.dat", light.into_bytes()));
    let mid: Vec<&MapObject> = mob.objects.iter().filter(|o| o.name.starts_with("__NAV_C")).collect();
    if !mid.is_empty() {
        let pick = |k: usize| mid[k.min(mid.len() - 1)].name.clone();
        let text = format!("; Name \t\t\t\t\tMesh\r\nSpawnPointAlpha\t\t\t{}\r\nSpawnPointBravo\t\t\t{}\r\nSpawnPointCharlie\t\t{}", pick(0), pick(mid.len() / 2), pick(mid.len() - 1));
        p.add_or_replace(MapFile::generated("Locations.dat", text.into_bytes()));
    }

    // Navigation grid at the game's 50-unit cells. Units stand on walk-through props' tops.
    let surf = walk_surfaces(l, opt.props.as_deref(), &|x, z| t.height_at(x, z));
    let stand = |x: f32, z: f32| {
        let h = t.height_at(x, z);
        match l.cell_of(x, z) {
            Some((i, j)) if !surf[j * l.nx + i].is_nan() => surf[j * l.nx + i].max(h),
            _ => h,
        }
    };
    let cs = 50.0;
    let (gx, gy) = ((mw / cs).ceil() as usize, (md / cs).ceil() as usize);
    let (lo, hi) = t.h.iter().chain(surf.iter().filter(|h| !h.is_nan())).fold((f32::MAX, f32::MIN), |(a, b), &h| (a.min(h), b.max(h)));
    let mut g = NavGrid::new_blank(3, [0.0, lo, 0.0], [gx as f32 * cs, hi, gy as f32 * cs], cs, gx, gy);
    for j in 0..gy {
        for i in 0..gx {
            let (mut walk, mut brush, mut n) = (0, 0, 0);
            for jj in j * 2..(j * 2 + 2).min(l.ny) {
                for ii in i * 2..(i * 2 + 2).min(l.nx) {
                    let s = l.surface_at(ii, jj);
                    n += 1;
                    let steep = t.wall[jj * (l.nx + 1) + ii] > 0.45;
                    if s.walkable() && !steep {
                        walk += 1;
                    }
                    if s == Surface::Brush {
                        brush += 1;
                    }
                }
            }
            let k = j * gx + i;
            let mut f = if walk * 2 >= n.max(1) { 0 } else { flags::WALL };
            if brush * 2 >= n.max(1) && f == 0 {
                f |= flags::BRUSH;
            }
            g.set_flags(k, f);
            let [cx, cz] = g.cell_center(i, j);
            g.set_height(k, stand(cx, cz));
        }
    }
    for (x, z, r) in &blockers {
        let n = (r / cs).ceil() as i64;
        let (ci, cj) = ((x / cs) as i64, (z / cs) as i64);
        for dj in -n..=n {
            for di in -n..=n {
                let (i, j) = (ci + di, cj + dj);
                if i < 0 || j < 0 || i as usize >= gx || j as usize >= gy {
                    continue;
                }
                let [px, pz] = g.cell_center(i as usize, j as usize);
                if (px - x).hypot(pz - z) <= *r {
                    let k = j as usize * gx + i as usize;
                    g.set_flags(k, (g.flags(k) | flags::WALL) & !flags::BRUSH);
                }
            }
        }
    }
    for j in 0..g.hy {
        for i in 0..g.hx {
            g.set_sampled_height(i, j, stand(i as f32 * g.hdx, j as f32 * g.hdy));
        }
    }
    if !opt.preview {
        g.rebuild_hints();
        let walk = g.walkable();
        let heights: Vec<f32> = (0..g.cell_count()).map(|k| g.height(k)).collect();
        if let Ok(b) = AiMesh::from_walkable(&walk, &heights, gx, gy, cs, [0.0, 0.0]).or_else(|_| {
            // Coarser blocks keep the AI mesh within 16-bit indices.
            let (bx, by) = (gx.div_ceil(4), gy.div_ceil(4));
            let bw: Vec<bool> = (0..bx * by).map(|k| walk[(k / bx * 4).min(gy - 1) * gx + (k % bx * 4).min(gx - 1)]).collect();
            let bh: Vec<f32> = (0..bx * by).map(|k| heights[(k / bx * 4).min(gy - 1) * gx + (k % bx * 4).min(gx - 1)]).collect();
            AiMesh::from_walkable(&bw, &bh, bx, by, cs * 4.0, [0.0, 0.0])
        }) {
            p.add_or_replace(MapFile::generated("AIPath.aimesh", b));
        }
    }
    p.add_or_replace(MapFile::generated("AIPath.aimesh_ngrid", g.write()));
    crate::map_settings::apply(&mut p, &l.settings);

    if !opt.preview {
        let mini = minimap(&p, l, &t, 512);
        p.add_or_replace(MapFile::generated("INFO/2DLevelMiniMap.dds", dds::encode_argb(&mini, false)));
    }
    p.name = folder;
    Ok(p)
}

/// Places scenery (props you placed and jungle-wall trees) into the scene. Returns the circles
/// of props that block walking.
/// A model placed by the wall dressing.
#[derive(Clone, Debug)]
pub struct Dressed {
    pub key: String,
    pub pos: [f32; 2],
    pub yaw: f32,
    pub scale: f32,
    pub lift: f32,
    pub stretch: [f32; 3],
}

/// The wall dressing: every painted wall is traced into a
/// smooth outline; rocks (and some trees) are placed by walking that outline at exact steps, turned
/// along it and set a little inside, and the inside is filled with trees kept off the edge.
/// `only_new`: on opened maps, the walls as imported (only walls painted since are dressed).
pub fn wall_dressing(l: &Layout, lib: &crate::props::PropLibrary, only_new: Option<&[u8]>) -> Vec<Dressed> {
    use crate::autodress::{EdgeRule, FillRule, Item};
    let f = &l.theme.forest;
    if !f.auto_dress {
        return Vec::new();
    }
    let forest: Vec<String> = if f.props.is_empty() { lib.default_forest_in(&l.asset_maps) } else { f.props.clone() };
    let rocks: Vec<String> = if f.edge_props.is_empty() { lib.default_wall_rocks_in(&l.asset_maps) } else { f.edge_props.clone() };
    let trees_on = (f.enabled || only_new.is_some()) && !forest.is_empty();
    let share = if trees_on { f.edge_tree_share.clamp(0.0, 1.0) } else { 0.0 };
    // The edge ring: rocks and/or trees (classic walls are ringed with trees: tree / wall / tree).
    let rocks_on = f.league_walls && (share < 1.0 && !rocks.is_empty() || share > 0.0);
    if !trees_on && !rocks_on {
        return Vec::new();
    }
    let (nx, ny) = (l.nx, l.ny);
    let wall = Surface::Wall as u8;
    let mask: Vec<bool> = (0..nx * ny).map(|k| l.surface[k] == wall && only_new.is_none_or(|w| w.get(k).is_some_and(|&v| v != wall))).collect();
    // Distance inside the walls (units), two-pass chamfer.
    let mut dist: Vec<f32> = mask.iter().map(|&m| if m { 1e9 } else { 0.0 }).collect();
    let d1 = CELL;
    let d2 = CELL * std::f32::consts::SQRT_2;
    for j in 0..ny {
        for i in 0..nx {
            let k = j * nx + i;
            if dist[k] == 0.0 {
                continue;
            }
            let mut v = dist[k];
            if i > 0 { v = v.min(dist[k - 1] + d1); }
            if j > 0 {
                v = v.min(dist[k - nx] + d1);
                if i > 0 { v = v.min(dist[k - nx - 1] + d2); }
                if i + 1 < nx { v = v.min(dist[k - nx + 1] + d2); }
            }
            dist[k] = v;
        }
    }
    for j in (0..ny).rev() {
        for i in (0..nx).rev() {
            let k = j * nx + i;
            if dist[k] == 0.0 {
                continue;
            }
            let mut v = dist[k];
            if i + 1 < nx { v = v.min(dist[k + 1] + d1); }
            if j + 1 < ny {
                v = v.min(dist[k + nx] + d1);
                if i + 1 < nx { v = v.min(dist[k + nx + 1] + d2); }
                if i > 0 { v = v.min(dist[k + nx - 1] + d2); }
            }
            dist[k] = v;
        }
    }
    let inside = |x: f32, z: f32| -> f32 { l.cell_of(x, z).map_or(0.0, |(i, j)| dist[j * nx + i]) - CELL * 0.5 };
    let radius = |k: &str| lib.get(k).map(|d| d.radius.max(1.0));
    let [mw, md] = l.size();
    let mut placed = Vec::new();
    let mut out = Vec::new();
    if rocks_on {
        let mut items: Vec<Item> = if share < 1.0 { rocks.iter().map(|k| Item { key: k.clone(), weight: (1.0 - share) / rocks.len() as f32, size: f.edge_size }).collect() } else { Vec::new() };
        if share > 0.0 {
            for k in &forest {
                if let Some(r) = radius(k) {
                    items.push(Item { key: k.clone(), weight: share / forest.len() as f32, size: [2.0 * r * f.scale[0], 2.0 * r * f.scale[1]] });
                }
            }
        }
        // Average size of the edge models, so the step along the outline is about the spacing.
        let wsum: f32 = items.iter().map(|i| i.weight).sum::<f32>().max(1e-6);
        let mean = items.iter().map(|i| i.weight * (i.size[0] + i.size[1]) * 0.5).sum::<f32>() / wsum;
        let rule = EdgeRule { items, spacing: f.edge_spacing.max(40.0) / mean.max(1.0), inset: f.edge_inset, align: f.edge_align };
        for line in crate::autodress::outlines(&mask, nx, ny, CELL) {
            if crate::autodress::area(&line).abs() < 4.0 * CELL * CELL {
                continue;
            }
            // Stable per wall while the rest of the map changes.
            let (mx, mz) = line.iter().fold((f32::MAX, f32::MAX), |a, p| (a.0.min(p[0]), a.1.min(p[1])));
            let seed = ((mx / 100.0) as u32).wrapping_mul(7919) ^ ((mz / 100.0) as u32).wrapping_mul(104_729) ^ line.len() as u32;
            crate::autodress::along(&line, true, &rule, seed, 1.0, &mut placed);
        }
        // Not along the map's own edge (the border wall's outside).
        let m = 2.0 * CELL;
        placed.retain(|p| p.on[0] > m && p.on[1] > m && p.on[0] < mw - m && p.on[1] < md - m);
    }
    if trees_on && f.enabled || trees_on && only_new.is_some() {
        let items: Vec<Item> = forest.iter().filter_map(|k| radius(k).map(|r| Item { key: k.clone(), weight: 1.0, size: [2.0 * r * f.scale[0], 2.0 * r * f.scale[1]] })).collect();
        // Ringed walls keep their mossy middle open: fewer trees inside.
        let rule = FillRule { items, step: f.spacing.max(60.0), margin: 0.25, chance: if rocks_on && share >= 0.5 { 0.6 } else { 0.9 } };
        let mut trees = Vec::new();
        crate::autodress::fill([0.0, 0.0, mw, md], &rule, 900, &|x, z| inside(x, z) - f.inset, &mut trees);
        // Not inside the edge rocks.
        trees.retain(|t| !placed.iter().any(|r| r.edge && (r.at[0] - t.at[0]).hypot(r.at[1] - t.at[1]) < r.size * 0.4));
        placed.extend(trees);
    }
    for p in placed {
        let Some(r) = radius(&p.key) else { continue };
        let is_rock = rocks.contains(&p.key) && p.edge;
        let scale = (p.size / (2.0 * r)).clamp(0.2, 4.0);
        let h = hash01(p.at[0].to_bits() >> 8, p.at[1].to_bits() >> 8, 86);
        out.push(Dressed {
            key: p.key,
            pos: p.at,
            yaw: p.yaw,
            scale,
            lift: if is_rock { -8.0 } else { 0.0 },
            // Rocks stretched along the outline so neighbours join into one stone face.
            stretch: if is_rock && f.edge_align { [1.25, 0.8 + 0.25 * h, 0.85] } else if is_rock { [1.0, 0.8 + 0.25 * h, 1.0] } else { [1.0; 3] },
        });
    }
    out
}

/// `only_new`: on opened maps, the walls as imported; only walls painted since are dressed
/// (the map's own walls already have their rocks and trees).
pub(crate) fn add_props(l: &Layout, opt: &BuildOptions, only_new: Option<&[u8]>, height: &dyn Fn(f32, f32) -> f32, mats: &mut Vec<Material>, geo: &mut Vec<(u32, Geometry)>, p: &mut MapProject) -> Result<Vec<(f32, f32, f32)>> {
    let Some(lib) = &opt.props else { return Ok(Vec::new()) };
    let mut lib = (**lib).clone();
    lib.set_custom(&l.custom_props);
    let mut placed: Vec<(String, [f32; 2], f32, f32, f32, bool, [f32; 2], [f32; 3])> = l.props.iter().map(|q| (q.prop.clone(), q.pos, q.yaw, q.scale, q.lift, !q.walk_through, q.tilt, q.stretch)).collect();
    // Jungle walls, dressed along their outlines (rocks on the edges, trees inside).
    for d in wall_dressing(l, &lib, only_new) {
        placed.push((d.key, d.pos, d.yaw, d.scale, d.lift, false, [0.0; 2], d.stretch));
    }
    let mut mat_index: HashMap<String, u32> = mats.iter().enumerate().map(|(i, m)| (m.name.to_ascii_lowercase(), i as u32)).collect();
    let mut part_geo: HashMap<u32, Geometry> = HashMap::new();
    let mut used: Vec<&crate::props::PropDef> = Vec::new();
    let mut blockers = Vec::new();
    for (key, pos, yaw, scale, lift, user, tilt, stretch) in &placed {
        let Some(def) = lib.get(key) else { continue };
        if !used.iter().any(|d| d.key == def.key) {
            used.push(def);
        }
        // Decals sit just above the ground so they do not flicker.
        let y = height(pos[0], pos[1]) + lift + if def.is_decal() { 2.0 } else { 0.0 };
        if *user && def.blocks_footprint() {
            // Every 50-unit cell under geometry taller than a champion's knee.
            let mut seen = std::collections::HashSet::new();
            for part in &def.parts {
                for v in &part.geo.positions {
                    // Placed exactly as drawn (size, stretch, tilt, turn).
                    let (o, _) = crate::props::place_vertex(*v, [0.0, 1.0, 0.0], *yaw, *scale, *tilt, *stretch);
                    if o[1] + lift < 60.0 {
                        continue;
                    }
                    let w = [pos[0] + o[0], pos[1] + o[2]];
                    if seen.insert(((w[0] / 50.0) as i32, (w[1] / 50.0) as i32)) {
                        blockers.push((w[0], w[1], 45.0));
                    }
                }
            }
        } else if *user && def.blocks() {
            blockers.push((pos[0], pos[1], def.radius * scale * stretch[0].max(stretch[2]) * 0.4));
        }
        for part in &def.parts {
            let mi = *mat_index.entry(part.material.name.to_ascii_lowercase()).or_insert_with(|| {
                mats.push(part.material.clone());
                (mats.len() - 1) as u32
            });
            let g = part_geo.entry(mi).or_default();
            let base = g.positions.len() as u32;
            for (k, v) in part.geo.positions.iter().enumerate() {
                let n = part.geo.normals.get(k).copied().unwrap_or([0.0, 1.0, 0.0]);
                let (o, n) = crate::props::place_vertex(*v, n, *yaw, *scale, *tilt, *stretch);
                g.positions.push([pos[0] + o[0], y + o[1], pos[1] + o[2]]);
                g.normals.push(n);
                let uv = part.geo.uvs.get(k).copied().unwrap_or([0.0; 2]);
                g.uvs.push(uv);
                g.uvs1.push(uv);
                g.colors.push(part.geo.colors.get(k).copied().unwrap_or(0xff000000));
            }
            g.indices.extend(part.geo.indices.iter().map(|i| i + base));
        }
    }
    for (mi, g) in part_geo {
        geo.push((mi, g));
    }
    // Their textures.
    for (name, bytes) in crate::props::prop_textures(&used) {
        if p.find(&format!("Scene/Textures/{name}")).is_none() {
            p.add_or_replace(MapFile::generated(&format!("Scene/Textures/{name}"), bytes));
        }
    }
    for c in &l.custom_props {
        if used.iter().any(|d| d.key == format!("custom:{}", c.name))
            && let Ok((name, bytes)) = resolve_texture(&c.texture)
        {
            p.add_or_replace(MapFile::generated(&format!("Scene/Textures/{name}"), bytes));
        }
    }
    Ok(blockers)
}

/// The in-game minimap: the built ground seen from above, structures marked.
fn minimap(p: &MapProject, l: &Layout, t: &Terrain, side: usize) -> Image {
    let avg = |name: &str| p.file(&format!("Scene/Textures/{name}")).and_then(|f| f.bytes().ok()).and_then(|b| dds::average_color(&b)).unwrap_or([100, 120, 80]);
    let names: Vec<String> = l.theme.ground.iter().map(|g| &g.texture).chain([&l.theme.cliff]).map(|r| resolve_name(r)).collect();
    let cols: Vec<[u8; 3]> = names.iter().map(|n| avg(n)).collect();
    let nl = l.theme.ground.len();
    let [mw, md] = l.size();
    let (lo, hi) = t.h.iter().fold((f32::MAX, f32::MIN), |(a, b), &h| (a.min(h), b.max(h)));
    let mut px = vec![0u8; side * side * 4];
    for y in 0..side {
        for x in 0..side {
            let (wx, wz) = ((x as f32 + 0.5) / side as f32 * mw, md - (y as f32 + 0.5) / side as f32 * md);
            let o = (y * side + x) * 4;
            let Some((i, j)) = l.cell_of(wx, wz) else { continue };
            let k = j * l.nx + i;
            let s = l.surface_at(i, j);
            let mut c = [0.0f32; 3];
            for n in 0..nl {
                let wv = l.paint[k][n] as f32 / 255.0;
                for a in 0..3 {
                    c[a] += cols[n][a] as f32 * wv;
                }
            }
            match s {
                Surface::Wall => c = cols[nl].map(|v| v as f32 * 0.85),
                Surface::Brush => c = [40.0, 110.0, 44.0],
                Surface::River | Surface::DeepWater => c = [44.0, 104.0, 128.0],
                Surface::Void => c = [0.0; 3],
                _ => {}
            }
            let shade = 0.75 + 0.4 * ((t.height_at(wx, wz) - lo) / (hi - lo).max(1.0));
            for a in 0..3 {
                px[o + a] = (c[a] * shade).clamp(0.0, 255.0) as u8;
            }
            px[o + 3] = 255;
        }
    }
    for o in &l.objectives {
        let (x, y) = ((o.pos[0] / mw * side as f32) as i64, ((md - o.pos[1]) / md * side as f32) as i64);
        let col = if o.team == 1 { [60, 140, 255] } else { [255, 70, 60] };
        let r = if o.kind == ObjKind::Nexus { 6 } else { 3 };
        for dy in -r..=r {
            for dx in -r..=r {
                let (xx, yy) = (x + dx, y + dy);
                if dx * dx + dy * dy <= r * r && xx >= 0 && yy >= 0 && (xx as usize) < side && (yy as usize) < side {
                    let k = (yy as usize * side + xx as usize) * 4;
                    px[k..k + 3].copy_from_slice(&col);
                }
            }
        }
    }
    Image { width: side, height: side, pixels: px }
}

pub fn resolve_name(r: &str) -> String {
    if let Some(k) = r.strip_prefix("gen:") {
        return format!("lms_{k}.dds");
    }
    let n = Path::new(r).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if n.to_ascii_lowercase().ends_with(".dds") { n } else { format!("{}.dds", n.rsplit_once('.').map_or(n.as_str(), |x| x.0)) }
}

