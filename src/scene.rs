//! Scene geometry for display: triangles of room.nvr / room.wgeo tinted by their textures' average
//! colours, a top-down rasterised image of them, and OBJ import for new geometry.

use crate::formats::{dds, nvr::Geometry};
use crate::project::{Doc, MapProject};
use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use std::collections::HashMap;

pub struct SceneMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub color: [u8; 3],
    /// Index in room.nvr meshes / room.wgeo meshes.
    pub source: usize,
    pub hidden: bool,
}

pub struct SceneData {
    pub meshes: Vec<SceneMesh>,
    pub bounds: [f32; 6],
    pub triangles: usize,
}

/// Finds a texture by file name in the map (case-insensitive, any scene texture folder).
pub fn find_texture<'a>(p: &'a MapProject, name: &str) -> Option<&'a crate::project::MapFile> {
    let n = name.replace('\\', "/");
    let base = n.rsplit('/').next().unwrap_or(&n);
    let stem = base.rsplit_once('.').map_or(base, |x| x.0);
    for cand in [base.to_string(), format!("{stem}.dds")] {
        for dir in ["Scene/Textures/", "Scene/textures/", "Scene/", ""] {
            if let Some(f) = p.file(&format!("{dir}{cand}")) {
                return Some(f);
            }
        }
    }
    None
}

pub fn texture_colors(p: &MapProject, names: &[String]) -> HashMap<String, [u8; 3]> {
    let mut uniq: Vec<String> = names.iter().map(|s| s.to_ascii_lowercase()).collect();
    uniq.sort();
    uniq.dedup();
    uniq.par_iter()
        .map(|n| {
            let c = find_texture(p, n).and_then(|f| f.bytes().ok()).and_then(|b| dds::average_color(&b)).unwrap_or([128, 128, 128]);
            (n.clone(), c)
        })
        .collect()
}

pub fn extract(p: &MapProject) -> Option<SceneData> {
    let wgeo_first = p.scene_format() == Some(crate::project::SceneFormat::Baked);
    let order: [&str; 2] = if wgeo_first { ["Scene/room.wgeo", "Scene/room.nvr"] } else { ["Scene/room.nvr", "Scene/room.wgeo"] };
    for path in order {
        match p.file(path).map(|f| &f.doc) {
            Some(Doc::Nvr(n)) => {
                let names: Vec<String> = n.materials.iter().map(|m| m.channels[0].texture.clone()).collect();
                let colors = texture_colors(p, &names);
                let meshes: Vec<SceneMesh> = (0..n.meshes.len())
                    .into_par_iter()
                    .map(|i| {
                        let g = n.mesh_geometry(i);
                        let tex = n.materials[n.meshes[i].material as usize].channels[0].texture.to_ascii_lowercase();
                        SceneMesh { positions: g.positions, indices: g.indices, color: colors.get(&tex).copied().unwrap_or([128; 3]), source: i, hidden: n.meshes[i].quality >= 100 }
                    })
                    .collect();
                return Some(finish(meshes));
            }
            Some(Doc::Wgeo(w)) => {
                let names: Vec<String> = w.meshes.iter().map(|m| m.texture.clone()).collect();
                let colors = texture_colors(p, &names);
                let meshes = w
                    .meshes
                    .iter()
                    .enumerate()
                    .map(|(i, m)| SceneMesh { positions: m.positions.clone(), indices: m.indices.clone(), color: colors.get(&m.texture.to_ascii_lowercase()).copied().unwrap_or([128; 3]), source: i, hidden: false })
                    .collect();
                return Some(finish(meshes));
            }
            _ => {}
        }
    }
    None
}

fn finish(meshes: Vec<SceneMesh>) -> SceneData {
    let mut b = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
    let mut tris = 0;
    for m in &meshes {
        tris += m.indices.len() / 3;
        for v in &m.positions {
            for a in 0..3 {
                b[a] = b[a].min(v[a]);
                b[a + 3] = b[a + 3].max(v[a]);
            }
        }
    }
    SceneData { meshes, bounds: b, triangles: tris }
}

/// Rasterises the scene from above into an RGBA image covering `rect` = [min_x, min_z, max_x, max_z].
/// The highest surface wins; shading follows the slope.
pub fn topdown(scene: &SceneData, rect: [f32; 4], w: usize, h: usize) -> Vec<u8> {
    let sx = w as f32 / (rect[2] - rect[0]);
    let sz = h as f32 / (rect[3] - rect[1]);
    // Bin triangles by row band so threads can work independently.
    let bands = 64usize.min(h);
    let band_h = h.div_ceil(bands);
    let mut binned: Vec<Vec<([[f32; 3]; 3], [u8; 3])>> = vec![Vec::new(); bands];
    for m in scene.meshes.iter().filter(|m| !m.hidden) {
        for t in m.indices.chunks_exact(3) {
            let tri = [m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]];
            let ys = tri.map(|p| (rect[3] - p[2]) * sz);
            let (lo, hi) = (ys.iter().cloned().fold(f32::MAX, f32::min), ys.iter().cloned().fold(f32::MIN, f32::max));
            if hi < 0.0 || lo >= h as f32 {
                continue;
            }
            let b0 = (lo.max(0.0) as usize / band_h).min(bands - 1);
            let b1 = (hi.min(h as f32 - 1.0) as usize / band_h).min(bands - 1);
            for b in b0..=b1 {
                binned[b].push((tri, m.color));
            }
        }
    }
    let results: Vec<(usize, Vec<Vec<(f32, [u8; 3])>>)> = binned
        .into_par_iter()
        .enumerate()
        .map(|(b, tris)| {
            let y0 = b * band_h;
            let y1 = ((b + 1) * band_h).min(h);
            let mut buf = vec![vec![(f32::MIN, [0u8; 3]); w]; y1.saturating_sub(y0)];
            for (tri, col) in tris {
                let p: Vec<[f32; 3]> = tri.iter().map(|v| [(v[0] - rect[0]) * sx, (rect[3] - v[2]) * sz, v[1]]).collect();
                let e1 = [tri[1][0] - tri[0][0], tri[1][1] - tri[0][1], tri[1][2] - tri[0][2]];
                let e2 = [tri[2][0] - tri[0][0], tri[2][1] - tri[0][1], tri[2][2] - tri[0][2]];
                let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                let light = ((n[1] / len).abs() * 0.7 + ((n[0] - n[2]) / len * 0.25)).clamp(0.25, 1.15);
                let c = col.map(|v| (v as f32 * light * 1.5 + 8.0).clamp(0.0, 255.0) as u8);
                let minx = p.iter().map(|v| v[0]).fold(f32::MAX, f32::min).floor().max(0.0) as i64;
                let maxx = p.iter().map(|v| v[0]).fold(f32::MIN, f32::max).ceil().min(w as f32 - 1.0) as i64;
                let miny = p.iter().map(|v| v[1]).fold(f32::MAX, f32::min).floor().max(y0 as f32) as i64;
                let maxy = p.iter().map(|v| v[1]).fold(f32::MIN, f32::max).ceil().min(y1 as f32 - 1.0) as i64;
                let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
                if area.abs() < 1e-6 {
                    continue;
                }
                for y in miny..=maxy {
                    for x in minx..=maxx {
                        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((p[1][0] - fx) * (p[2][1] - fy) - (p[2][0] - fx) * (p[1][1] - fy)) / area;
                        let w1 = ((p[2][0] - fx) * (p[0][1] - fy) - (p[0][0] - fx) * (p[2][1] - fy)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let z = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
                        let cell = &mut buf[y as usize - y0][x as usize];
                        if z > cell.0 {
                            *cell = (z, c);
                        }
                    }
                }
            }
            (y0, buf)
        })
        .collect();
    let mut out = vec![0u8; w * h * 4];
    for (y0, buf) in results {
        for (dy, row) in buf.into_iter().enumerate() {
            for (x, (z, c)) in row.into_iter().enumerate() {
                let o = ((y0 + dy) * w + x) * 4;
                if z > f32::MIN {
                    out[o..o + 3].copy_from_slice(&c);
                    out[o + 3] = 255;
                } else {
                    out[o..o + 4].copy_from_slice(&[18, 20, 24, 255]);
                }
            }
        }
    }
    out
}

/// Reads a Wavefront OBJ (triangulating polygons). Faces may use any combination of v/vt/vn.
pub fn read_obj(text: &str) -> Result<Geometry> {
    let mut v: Vec<[f32; 3]> = Vec::new();
    let mut vt: Vec<[f32; 2]> = Vec::new();
    let mut vn: Vec<[f32; 3]> = Vec::new();
    let mut g = Geometry::default();
    let mut map: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let idx = |s: &str, n: usize| -> Result<i64> {
        if s.is_empty() {
            return Ok(-1);
        }
        let i: i64 = s.parse().context("bad OBJ index")?;
        Ok(if i < 0 { n as i64 + i } else { i - 1 })
    };
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f32> = it.take(3).map(|s| s.parse().unwrap_or(0.0)).collect();
                if c.len() == 3 {
                    v.push([c[0], c[1], c[2]]);
                }
            }
            Some("vt") => {
                let c: Vec<f32> = it.take(2).map(|s| s.parse().unwrap_or(0.0)).collect();
                vt.push([c.first().copied().unwrap_or(0.0), 1.0 - c.get(1).copied().unwrap_or(0.0)]);
            }
            Some("vn") => {
                let c: Vec<f32> = it.take(3).map(|s| s.parse().unwrap_or(0.0)).collect();
                if c.len() == 3 {
                    vn.push([c[0], c[1], c[2]]);
                }
            }
            Some("f") => {
                let mut poly = Vec::new();
                for corner in it {
                    let parts: Vec<&str> = corner.split('/').collect();
                    let key = (idx(parts[0], v.len())?, idx(parts.get(1).copied().unwrap_or(""), vt.len())?, idx(parts.get(2).copied().unwrap_or(""), vn.len())?);
                    if key.0 < 0 || key.0 as usize >= v.len() {
                        bail!("OBJ face uses a missing vertex");
                    }
                    let id = *map.entry(key).or_insert_with(|| {
                        g.positions.push(v[key.0 as usize]);
                        g.uvs.push(if key.1 >= 0 { vt.get(key.1 as usize).copied().unwrap_or([0.0; 2]) } else { [0.0; 2] });
                        g.normals.push(if key.2 >= 0 { vn.get(key.2 as usize).copied().unwrap_or([0.0, 1.0, 0.0]) } else { [0.0, 1.0, 0.0] });
                        g.colors.push(0xff000000);
                        (g.positions.len() - 1) as u32
                    });
                    poly.push(id);
                }
                for k in 1..poly.len().saturating_sub(1) {
                    g.indices.extend_from_slice(&[poly[0], poly[k], poly[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if g.indices.is_empty() {
        bail!("the OBJ file has no faces");
    }
    Ok(g)
}

/// Scales, rotates (about Y, degrees) and moves geometry; the model's base is placed at `at`.
pub fn place_geometry(g: &mut Geometry, scale: f32, yaw_deg: f32, at: [f32; 3]) {
    let (s, c) = yaw_deg.to_radians().sin_cos();
    let min_y = g.positions.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
    let (cx, cz) = {
        let n = g.positions.len().max(1) as f32;
        (g.positions.iter().map(|p| p[0]).sum::<f32>() / n, g.positions.iter().map(|p| p[2]).sum::<f32>() / n)
    };
    for p in &mut g.positions {
        let (x, y, z) = ((p[0] - cx) * scale, (p[1] - min_y) * scale, (p[2] - cz) * scale);
        *p = [at[0] + c * x - s * z, at[1] + y, at[2] + s * x + c * z];
    }
    for n in &mut g.normals {
        *n = [c * n[0] - s * n[2], n[1], s * n[0] + c * n[2]];
    }
}
