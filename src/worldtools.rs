//! World-building operations in the style of Roblox Studio, Unreal and the StarCraft II editor:
//! terrain generation, heightmap import/export, ramps, noise; align / distribute / randomize /
//! mirror / replace for selections; prefabs (saved groups of objects).

use crate::layout::{CELL, Clipboard, ItemRef, Layout, Surface, Symmetry};
use crate::layout_build::{hash01, noise2};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------ terrain

/// Options for generated terrain.
#[derive(Clone, Debug)]
pub struct GenOptions {
    pub seed: u32,
    /// Share of the map that becomes jungle walls (0..1).
    pub walls: f32,
    /// Size of wall blobs in units.
    pub feature: f32,
    /// Share of open ground covered by brush.
    pub brush: f32,
    /// Hills: height variation in units.
    pub hills: f32,
    /// Keep a clear path along existing lanes and around objectives.
    pub keep_lanes: bool,
    /// Add a river across the map (bottom-left to top-right diagonal mirror).
    pub river: bool,
}

impl Default for GenOptions {
    fn default() -> Self {
        Self { seed: 1, walls: 0.38, feature: 1400.0, brush: 0.12, hills: 60.0, keep_lanes: true, river: true }
    }
}

/// Procedural terrain (Roblox Studio's Generate): jungle walls, brush, hills and a river, kept
/// symmetric when the layout is, and clear around lanes and objectives.
pub fn generate(l: &mut Layout, o: &GenOptions) {
    let [w, d] = l.size();
    let (nx, ny) = (l.nx, l.ny);
    let sym = l.symmetry;
    let border = (350.0 / CELL) as usize;
    // Thresholds from the noise itself, so the sliders are real shares of the map.
    let quantile = |seed: u32, scale: f32, share: f32| -> f32 {
        let mut v: Vec<f32> = (0..4096).map(|k| noise2((k % 64) as f32 * w / 64.0 / scale, (k / 64) as f32 * d / 64.0 / scale, seed)).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((1.0 - share.clamp(0.0, 1.0)) * 4095.0) as usize]
    };
    let wall_t = if o.walls <= 0.0 { f32::MAX } else { quantile(o.seed, o.feature, o.walls) };
    let brush_t = if o.brush <= 0.0 { f32::MAX } else { quantile(o.seed + 7, o.feature * 0.25, o.brush) };
    for j in 0..ny {
        for i in 0..nx {
            let k = j * nx + i;
            let (x, z) = ((i as f32 + 0.5) * CELL, (j as f32 + 0.5) * CELL);
            // Symmetric maps sample the noise at a canonical point so both halves match.
            let c = if sym != Symmetry::None {
                let m = l.mirror_point([x, z]);
                if (m[0] + m[1]) < (x + z) || ((m[0] + m[1]) == (x + z) && m[0] < x) { m } else { [x, z] }
            } else {
                [x, z]
            };
            let n = noise2(c[0] / o.feature, c[1] / o.feature, o.seed);
            let fine = noise2(c[0] / (o.feature * 0.25), c[1] / (o.feature * 0.25), o.seed + 7);
            let edge = i < border || j < border || i + border >= nx || j + border >= ny;
            let mut s = if edge || n > wall_t { Surface::Wall } else if fine > brush_t { Surface::Brush } else { Surface::Ground };
            if o.river && !edge {
                // A meandering river across the other diagonal.
                let t = (x / w - z / d) * 0.5 + 0.5;
                let along = (x / w + z / d) * 0.5;
                let wiggle = (along * 9.0 + o.seed as f32).sin() * 0.03;
                let dist = (t - 0.5 - wiggle).abs() * w.min(d);
                if dist < 300.0 {
                    s = Surface::River;
                } else if dist < 420.0 && s == Surface::Wall {
                    s = Surface::Ground;
                }
            }
            l.surface[k] = s as u8;
            l.height[k] = (noise2(c[0] / 3000.0, c[1] / 3000.0, o.seed + 13) - 0.5) * 2.0 * o.hills;
        }
    }
    if o.keep_lanes {
        let lanes: Vec<Vec<[f32; 2]>> = l.lanes.iter().map(|lp| lp.points.clone()).collect();
        let s = l.symmetry;
        l.symmetry = Symmetry::None;
        for pts in lanes {
            for wv in pts.windows(2) {
                let len = (wv[1][0] - wv[0][0]).hypot(wv[1][1] - wv[0][1]);
                let steps = (len / 100.0).ceil().max(1.0) as usize;
                for t in 0..=steps {
                    let f = t as f32 / steps as f32;
                    let (x, z) = (wv[0][0] + (wv[1][0] - wv[0][0]) * f, wv[0][1] + (wv[1][1] - wv[0][1]) * f);
                    l.brush(x, z, 380.0, false, |l, k, _| {
                        if l.surface[k] != Surface::River as u8 {
                            l.surface[k] = Surface::Ground as u8;
                        }
                    });
                }
            }
        }
        let objs: Vec<([f32; 2], f32)> = l.objectives.iter().map(|o| (o.pos, o.kind.radius() + 250.0)).collect();
        for (p, r) in objs {
            l.paint_surface(p[0], p[1], r, false, Surface::Ground);
        }
        l.symmetry = s;
    }
}

/// Heights as a grey image (white = high), and the height range it covers.
pub fn export_heightmap(l: &Layout) -> (image::GrayImage, f32, f32) {
    let (lo, hi) = l.height.iter().fold((f32::MAX, f32::MIN), |(a, b), &h| (a.min(h), b.max(h)));
    let span = (hi - lo).max(1e-3);
    let img = image::GrayImage::from_fn(l.nx as u32, l.ny as u32, |i, j| {
        // Image rows go top to bottom; the map's +Z is up on the minimap.
        let k = (l.ny - 1 - j as usize) * l.nx + i as usize;
        image::Luma([((l.height[k] - lo) / span * 255.0).round() as u8])
    });
    (img, lo, hi)
}

/// Heights from an image (any size, resampled): black = `low`, white = `high`. Bright areas can
/// also become walls (the StarCraft-style cliff level) above `wall_above` (0..1, >1 = never).
pub fn import_heightmap(l: &mut Layout, path: &Path, low: f32, high: f32, wall_above: f32) -> Result<()> {
    let img = image::open(path).with_context(|| format!("cannot read {}", path.display()))?.to_luma16();
    let (w, h) = (img.width() as f32, img.height() as f32);
    for j in 0..l.ny {
        for i in 0..l.nx {
            let u = (i as f32 + 0.5) / l.nx as f32 * w;
            let v = (1.0 - (j as f32 + 0.5) / l.ny as f32) * h;
            let px = img.get_pixel((u as u32).min(img.width() - 1), (v as u32).min(img.height() - 1))[0] as f32 / 65535.0;
            let k = j * l.nx + i;
            l.height[k] = low + (high - low) * px;
            if px > wall_above {
                l.surface[k] = Surface::Wall as u8;
            }
        }
    }
    Ok(())
}

/// Adds noise to the heights around a point (Unreal's Noise tool).
pub fn sculpt_noise(l: &mut Layout, x: f32, z: f32, radius: f32, amount: f32, seed: u32) {
    l.brush(x, z, radius, false, |l, k, fall| {
        let (i, j) = ((k % l.nx) as f32, (k / l.nx) as f32);
        let n = noise2(i * CELL / 250.0, j * CELL / 250.0, seed) - 0.5;
        l.height[k] += n * 2.0 * amount * fall;
    });
}

/// A straight ramp from a (at its current height) to b (at its height), `width` wide, with soft
/// edges (Unreal's Ramp tool, the StarCraft ramp).
pub fn ramp(l: &mut Layout, a: [f32; 2], b: [f32; 2], width: f32) {
    let h_at = |l: &Layout, p: [f32; 2]| l.cell_of(p[0], p[1]).map_or(0.0, |(i, j)| l.height[j * l.nx + i]);
    let (ha, hb) = (h_at(l, a), h_at(l, b));
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let len2 = (dx * dx + dz * dz).max(1.0);
    let half = width / 2.0;
    for j in 0..l.ny {
        for i in 0..l.nx {
            let (x, z) = ((i as f32 + 0.5) * CELL, (j as f32 + 0.5) * CELL);
            let t = ((x - a[0]) * dx + (z - a[1]) * dz) / len2;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let (px, pz) = (a[0] + dx * t, a[1] + dz * t);
            let d = (x - px).hypot(z - pz);
            if d > half * 1.4 {
                continue;
            }
            let k = j * l.nx + i;
            let target = ha + (hb - ha) * t;
            let f = if d <= half { 1.0 } else { 1.0 - (d - half) / (half * 0.4) };
            l.height[k] += (target - l.height[k]) * f;
        }
    }
}

// ------------------------------------------------------------------ selections

/// Lines items up on one axis (0 = X, 1 = Z) at the selection's minimum, centre or maximum
/// (Roblox Studio's Align tool).
pub fn align(l: &mut Layout, items: &[ItemRef], axis: usize, mode: u8) {
    let pts: Vec<(ItemRef, [f32; 2])> = items.iter().filter_map(|r| l.item_pos(*r).map(|p| (*r, p))).collect();
    if pts.len() < 2 {
        return;
    }
    let vals: Vec<f32> = pts.iter().map(|x| x.1[axis]).collect();
    let target = match mode {
        0 => vals.iter().cloned().fold(f32::MAX, f32::min),
        2 => vals.iter().cloned().fold(f32::MIN, f32::max),
        _ => vals.iter().sum::<f32>() / vals.len() as f32,
    };
    for (r, p) in pts {
        let mut d = [0.0; 2];
        d[axis] = target - p[axis];
        l.shift_item(r, d);
    }
}

/// Spaces items evenly between the outermost two along an axis.
pub fn distribute(l: &mut Layout, items: &[ItemRef], axis: usize) {
    let mut pts: Vec<(ItemRef, [f32; 2])> = items.iter().filter_map(|r| l.item_pos(*r).map(|p| (*r, p))).collect();
    if pts.len() < 3 {
        return;
    }
    pts.sort_by(|a, b| a.1[axis].partial_cmp(&b.1[axis]).unwrap());
    let (a, b) = (pts[0].1[axis], pts[pts.len() - 1].1[axis]);
    let n = pts.len() - 1;
    for (k, (r, p)) in pts.into_iter().enumerate() {
        let mut d = [0.0; 2];
        d[axis] = a + (b - a) * k as f32 / n as f32 - p[axis];
        l.shift_item(r, d);
    }
}

/// Random turn and size for props (natural variety, like foliage painting).
pub fn randomize(l: &mut Layout, items: &[ItemRef], turn: bool, scale: [f32; 2], seed: u32) {
    for (n, r) in items.iter().enumerate() {
        if let ItemRef::Prop(id) = r
            && let Some(p) = l.props.iter_mut().find(|p| p.id == *id)
        {
            if turn {
                p.yaw = 360.0 * hash01(seed, n as u32, 1);
            }
            p.scale = scale[0] + (scale[1] - scale[0]) * hash01(seed, n as u32, 2);
        }
    }
}

/// Props back on the ground: no lift, no tilt.
pub fn drop_to_ground(l: &mut Layout, items: &[ItemRef]) {
    for r in items {
        if let ItemRef::Prop(id) = r
            && let Some(p) = l.props.iter_mut().find(|p| p.id == *id)
        {
            p.lift = 0.0;
            p.tilt = [0.0; 2];
        }
    }
}

/// Copies items to the other team's side (the map's symmetry, or diagonal).
pub fn mirror_items(l: &mut Layout, items: &[ItemRef]) -> Vec<ItemRef> {
    let saved = l.symmetry;
    if saved == Symmetry::None {
        l.symmetry = Symmetry::Diagonal;
    }
    let mut out = Vec::new();
    let cb = l.copy_items(items);
    for (p, src) in cb.props.iter().zip(items.iter().filter(|r| matches!(r, ItemRef::Prop(_)))) {
        let Some(at) = l.item_pos(*src) else { continue };
        let m = l.mirror_point(at);
        let id = l.add_prop(&p.prop, m, p.yaw + 180.0, p.scale);
        if let Some(q) = l.props.iter_mut().find(|q| q.id == id) {
            q.lift = p.lift;
            q.tilt = p.tilt;
            q.stretch = p.stretch;
        }
        out.push(ItemRef::Prop(id));
    }
    for r in items {
        let Some(at) = l.item_pos(*r) else { continue };
        let m = l.mirror_point(at);
        match *r {
            ItemRef::Objective(id) => {
                if let Some(o) = l.objective(id).cloned() {
                    let nid = l.next_id;
                    l.next_id += 1;
                    let team = if o.team == 0 { 0 } else { 3 - o.team };
                    l.objectives.push(crate::layout::Objective { id: nid, team, pos: m, yaw: o.yaw + 180.0, mirror: None, name: None, ..o });
                    out.push(ItemRef::Objective(nid));
                }
            }
            ItemRef::Effect(id) => {
                if let Some(e) = l.effects.iter().find(|e| e.id == id).cloned() {
                    let nid = l.add_effect(&e.name, m, e.lift, e.yaw + 180.0);
                    out.push(ItemRef::Effect(nid));
                }
            }
            ItemRef::Light(id) => {
                if let Some(x) = l.lights.iter().find(|x| x.id == id).cloned() {
                    let nid = l.add_light(m, x.lift, x.color, x.inner, x.outer, x.intensity);
                    out.push(ItemRef::Light(nid));
                }
            }
            ItemRef::Prop(_) => {}
        }
    }
    l.symmetry = saved;
    out
}

/// Swaps the model of the selected props, keeping their placement.
pub fn replace_props(l: &mut Layout, items: &[ItemRef], key: &str) -> usize {
    let mut n = 0;
    for r in items {
        if let ItemRef::Prop(id) = r
            && let Some(p) = l.props.iter_mut().find(|p| p.id == *id)
        {
            p.prop = key.to_string();
            n += 1;
        }
    }
    n
}

// ------------------------------------------------------------------ prefabs

pub fn prefab_dir() -> PathBuf {
    crate::asset::library_dir().parent().map(|p| p.join("prefabs")).unwrap_or_else(|| PathBuf::from("prefabs"))
}

/// Saves a group of objects as a reusable prefab.
pub fn save_prefab(name: &str, cb: &Clipboard) -> Result<PathBuf> {
    let dir = prefab_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", crate::asset::safe_name(name)));
    let json = serde_json::json!({ "name": name, "props": cb.props, "objectives": cb.objectives, "effects": cb.effects, "lights": cb.lights });
    std::fs::write(&path, serde_json::to_string(&json)?)?;
    Ok(path)
}

/// The saved prefabs (name, contents), sorted by name.
pub fn list_prefabs() -> Vec<(String, Clipboard)> {
    let mut out: Vec<(String, Clipboard)> = std::fs::read_dir(prefab_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            let get = |k: &str| v.get(k).cloned().unwrap_or(serde_json::Value::Array(Vec::new()));
            let cb = Clipboard {
                props: serde_json::from_value(get("props")).ok()?,
                objectives: serde_json::from_value(get("objectives")).ok()?,
                effects: serde_json::from_value(get("effects")).ok()?,
                lights: serde_json::from_value(get("lights")).ok()?,
            };
            Some((v.get("name")?.as_str()?.to_string(), cb))
        })
        .collect();
    out.sort_by_key(|x| x.0.to_ascii_lowercase());
    out
}

pub fn delete_prefab(name: &str) {
    let _ = std::fs::remove_file(prefab_dir().join(format!("{}.json", crate::asset::safe_name(name))));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_ramp_align() {
        let mut l = Layout::classic_start("T", 30, 6000.0);
        generate(&mut l, &GenOptions::default());
        let walls = l.surface.iter().filter(|&&s| s == Surface::Wall as u8).count();
        assert!(walls > l.surface.len() / 10 && walls < l.surface.len() * 9 / 10, "walls {walls}");
        // Objectives stay walkable.
        for o in &l.objectives {
            let (i, j) = l.cell_of(o.pos[0], o.pos[1]).unwrap();
            assert_ne!(l.surface[j * l.nx + i], Surface::Wall as u8);
        }
        let (lo_k, hi_k) = (l.cell_of(1000.0, 3000.0).unwrap(), l.cell_of(3000.0, 3000.0).unwrap());
        l.height[lo_k.1 * l.nx + lo_k.0] = 0.0;
        l.height[hi_k.1 * l.nx + hi_k.0] = 200.0;
        ramp(&mut l, [1000.0, 3000.0], [3000.0, 3000.0], 300.0);
        let mid = l.cell_of(2000.0, 3000.0).unwrap();
        let h = l.height[mid.1 * l.nx + mid.0];
        assert!((h - 100.0).abs() < 15.0, "ramp middle {h}");
        let a = l.add_prop("x", [100.0, 100.0], 0.0, 1.0);
        let b = l.add_prop("x", [500.0, 900.0], 0.0, 1.0);
        let c = l.add_prop("x", [300.0, 400.0], 0.0, 1.0);
        let items = [ItemRef::Prop(a), ItemRef::Prop(b), ItemRef::Prop(c)];
        align(&mut l, &items, 0, 0);
        assert!(items.iter().all(|r| l.item_pos(*r).unwrap()[0] == 100.0));
        distribute(&mut l, &items, 1);
        assert_eq!(l.item_pos(ItemRef::Prop(c)).unwrap()[1], 500.0);
    }
}
