//! The AI map builder: Claude works the map editor itself, one tool call at a time (new map,
//! terrain, lanes, every turret, camps, brush, scenery, textures, effects, look), looks at the
//! map (a character overview) and runs a playability check, and fixes what the check finds.
//! The same tools run scripted recipes offline (see `run_script`).

use crate::layout::{CELL, Lane, Layout, ObjKind, SculptMode, Surface, Symmetry, TurretTier};
use crate::props::PropLibrary;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The map being built and what the tools need.
pub struct Workshop {
    pub l: Layout,
    pub lib: Option<Arc<PropLibrary>>,
    /// Map1's Scene/Textures (classic textures, colour references).
    pub map1_tex: Option<PathBuf>,
    /// Particle effects available (DATA/Particles).
    pub effects: Vec<String>,
    /// Set by the finish tool.
    pub done: Option<String>,
    pub calls: usize,
    /// Every example map's texture folder (Map1 first), for set_texture / list_textures.
    pub tex_dirs: Vec<PathBuf>,
}

impl Workshop {
    pub fn new(lib: Option<Arc<PropLibrary>>, map1_tex: Option<PathBuf>, effects: Vec<String>) -> Self {
        let mut l = Layout::new("AI Map", 30, [12000.0, 12000.0]);
        l.theme.forest.auto_dress = true;
        // Example Maps/<map>/Scene/Textures and Example Maps/<collection>/<map>/Scene/Textures.
        let mut tex_dirs: Vec<PathBuf> = map1_tex.iter().cloned().collect();
        if let Some(root) = map1_tex.as_ref().and_then(|t| t.parent()?.parent()?.parent().map(|p| p.to_path_buf())) {
            let top: Vec<PathBuf> = std::fs::read_dir(&root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            for (d, _) in crate::props::map_folders(&top) {
                let t = d.join("Scene").join("Textures");
                if t.is_dir() && !tex_dirs.contains(&t) {
                    tex_dirs.push(t);
                }
            }
        }
        if let Some(p) = crate::asset::library_dir().parent() {
            tex_dirs.push(p.join("textures"));
        }
        Self { l, lib, map1_tex, effects, done: None, calls: 0, tex_dirs }
    }
}

// ------------------------------------------------------------------ argument helpers

fn num(v: &Value, k: &str) -> Option<f32> {
    v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32)
}

fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

fn flag(v: &Value, k: &str, default: bool) -> bool {
    v.get(k).and_then(|x| x.as_bool()).unwrap_or(default)
}

fn point(v: &Value) -> Option<[f32; 2]> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
}

fn points(v: &Value, k: &str) -> Vec<[f32; 2]> {
    v.get(k).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(point).collect()).unwrap_or_default()
}

fn at(v: &Value) -> Result<[f32; 2]> {
    match (num(v, "x"), num(v, "z")) {
        (Some(x), Some(z)) => Ok([x, z]),
        _ => bail!("x and z are required"),
    }
}

fn surface_of(s: &str) -> Result<Surface> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "ground" | "floor" | "walkable" => Surface::Ground,
        "wall" | "cliff" | "jungle_wall" => Surface::Wall,
        "brush" | "bush" | "grass" => Surface::Brush,
        "river" | "shallow_water" => Surface::River,
        "deep_water" | "lava" | "chasm" => Surface::DeepWater,
        "void" | "empty" | "off_map" => Surface::Void,
        o => bail!("unknown surface \"{o}\" (ground, wall, brush, river, deep_water/lava, void)"),
    })
}

fn kind_of(s: &str) -> Result<ObjKind> {
    Ok(match s.to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
        "nexus" => ObjKind::Nexus,
        "inhibitor" => ObjKind::Inhibitor,
        "turret" | "tower" => ObjKind::Turret,
        "spawn" | "fountain" | "spawn_point" => ObjKind::SpawnPoint,
        "fountain_turret" => ObjKind::FountainTurret,
        "shop" => ObjKind::Shop,
        "minion_spawn" | "barracks" => ObjKind::MinionSpawn,
        "blue_buff" | "ancient_golem" => ObjKind::BlueBuff,
        "red_buff" | "lizard_elder" => ObjKind::RedBuff,
        "wolves" => ObjKind::Wolves,
        "wraiths" => ObjKind::Wraiths,
        "golems" => ObjKind::Golems,
        "wight" | "great_wraith" => ObjKind::Wight,
        "dragon" => ObjKind::Dragon,
        "baron" | "baron_nashor" => ObjKind::Baron,
        "health_relic" => ObjKind::HealthRelic,
        "capture_point" => ObjKind::CapturePoint,
        "speed_shrine" => ObjKind::SpeedShrine,
        o => bail!("unknown object kind \"{o}\""),
    })
}

fn team_of(v: &Value) -> u8 {
    match text(v, "team").unwrap_or("blue").to_ascii_lowercase().as_str() {
        "red" | "chaos" | "purple" | "2" => 2,
        "neutral" | "0" => 0,
        _ => 1,
    }
}

fn lane_of(v: &Value) -> Result<Lane> {
    Ok(match text(v, "lane").unwrap_or("mid").to_ascii_lowercase().as_str() {
        "top" => Lane::Top,
        "mid" | "middle" => Lane::Mid,
        "bot" | "bottom" => Lane::Bot,
        o => bail!("unknown lane \"{o}\" (top, mid, bot)"),
    })
}

fn tier_of(v: &Value) -> TurretTier {
    match text(v, "tier").unwrap_or("outer").to_ascii_lowercase().as_str() {
        "inner" => TurretTier::Inner,
        "inhibitor" | "inhib" | "base" => TurretTier::Inhibitor,
        "nexus" | "nexus_a" => TurretTier::NexusA,
        "nexus_b" => TurretTier::NexusB,
        _ => TurretTier::Outer,
    }
}

fn symmetry_of(s: &str) -> Symmetry {
    match s.to_ascii_lowercase().as_str() {
        "diagonal" => Symmetry::Diagonal,
        "point" | "rotational" | "rotate" => Symmetry::Point,
        "mirror_x" | "left_right" | "horizontal" => Symmetry::MirrorX,
        "mirror_z" | "top_bottom" | "vertical" => Symmetry::MirrorZ,
        _ => Symmetry::None,
    }
}

/// Runs `f` with automatic mirroring on or off.
fn mirrored_op(l: &mut Layout, mirror: bool, f: impl FnOnce(&mut Layout)) {
    if mirror {
        f(l);
    } else {
        let s = l.symmetry;
        l.symmetry = Symmetry::None;
        f(l);
        l.symmetry = s;
    }
}

/// The turn of a model's mirror copy (a reflection, not just a half turn).
pub fn mirror_yaw(sym: Symmetry, yaw: f32) -> f32 {
    match sym {
        Symmetry::MirrorX => 180.0 - yaw,
        Symmetry::MirrorZ => -yaw,
        Symmetry::Diagonal => -90.0 - yaw,
        Symmetry::Point => yaw + 180.0,
        Symmetry::None => yaw,
    }
}

/// Whether a point is on the half that gets mirrored (or on the mirror line itself).
fn first_half(l: &Layout, p: [f32; 2]) -> bool {
    if l.symmetry == Symmetry::None {
        return true;
    }
    let m = l.mirror_point(p);
    (m[0] - p[0]).hypot(m[1] - p[1]) < 60.0 || (p[0], p[1]) < (m[0], m[1])
}

/// Lanes are kept clear: a model of footprint radius `r` at `p` stays out of every lane
/// corridor (half width 360 plus a margin).
pub fn clear_of_lanes(l: &Layout, p: [f32; 2], r: f32) -> bool {
    l.lanes.iter().all(|lp| along_lane(&lp.points, p).0 > 380.0 + r * 0.85)
}

/// Adds a prop and, with symmetry, its reflected copy.
fn add_prop_sym(l: &mut Layout, key: &str, p: [f32; 2], yaw: f32, scale: f32, lift: f32, mirror: bool) -> u32 {
    let id = l.add_prop(key, p, yaw, scale);
    if lift != 0.0 {
        l.props.iter_mut().filter(|q| q.id == id).for_each(|q| q.lift = lift);
    }
    if mirror && l.symmetry != Symmetry::None {
        let m = l.mirror_point(p);
        if (m[0] - p[0]).hypot(m[1] - p[1]) > 60.0 {
            let id2 = l.add_prop(key, m, mirror_yaw(l.symmetry, yaw), scale);
            if lift != 0.0 {
                l.props.iter_mut().filter(|q| q.id == id2).for_each(|q| q.lift = lift);
            }
        }
    }
    id
}

/// Chamfer distance (units) inside a mask.
fn inside_dist(mask: &[bool], nx: usize, ny: usize) -> Vec<f32> {
    let mut d: Vec<f32> = mask.iter().map(|&m| if m { 1e9 } else { 0.0 }).collect();
    let (d1, d2) = (CELL, CELL * std::f32::consts::SQRT_2);
    for j in 0..ny {
        for i in 0..nx {
            let k = j * nx + i;
            if d[k] == 0.0 {
                continue;
            }
            let mut v = d[k];
            if i > 0 { v = v.min(d[k - 1] + d1); }
            if j > 0 {
                v = v.min(d[k - nx] + d1);
                if i > 0 { v = v.min(d[k - nx - 1] + d2); }
                if i + 1 < nx { v = v.min(d[k - nx + 1] + d2); }
            }
            d[k] = v;
        }
    }
    for j in (0..ny).rev() {
        for i in (0..nx).rev() {
            let k = j * nx + i;
            if d[k] == 0.0 {
                continue;
            }
            let mut v = d[k];
            if i + 1 < nx { v = v.min(d[k + 1] + d1); }
            if j + 1 < ny {
                v = v.min(d[k + nx] + d1);
                if i + 1 < nx { v = v.min(d[k + nx + 1] + d2); }
                if i > 0 { v = v.min(d[k + nx - 1] + d2); }
            }
            d[k] = v;
        }
    }
    d
}

fn keys_of(v: &Value, k: &str) -> Vec<String> {
    v.get(k).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// The diffuse texture of a prop's first part.
fn prop_texture(d: &crate::props::PropDef) -> String {
    d.parts.first().and_then(|p| p.material.channels.first()).map(|c| c.texture.clone()).unwrap_or_default()
}

/// Expands model selectors: a plain key stays as it is; "@Map:texture[:minH-maxH]" picks that
/// map's models whose texture contains the text (and whose height is in range), most used first.
/// Model names like "lambert #12" depend on what is loaded; textures do not.
pub fn expand_keys(lib: &PropLibrary, specs: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for spec in specs {
        let Some(sel) = spec.strip_prefix('@') else {
            out.push(spec.clone());
            continue;
        };
        let mut it = sel.splitn(3, ':');
        let (map, tex, range) = (it.next().unwrap_or(""), it.next().unwrap_or("").to_ascii_lowercase(), it.next());
        let (lo, hi) = range.and_then(|r| r.split_once('-')).map(|(a, b)| (a.parse().unwrap_or(0.0f32), b.parse().unwrap_or(f32::MAX))).unwrap_or((0.0, f32::MAX));
        let mut hits: Vec<&crate::props::PropDef> = lib.props.iter().filter(|d| d.key.starts_with(&format!("{map}/")) && prop_texture(d).to_ascii_lowercase().contains(&tex) && (lo..=hi).contains(&d.height)).collect();
        hits.sort_by(|a, b| b.copies.cmp(&a.copies).then(b.height.partial_cmp(&a.height).unwrap()));
        out.extend(hits.into_iter().take(6).map(|d| d.key.clone()));
    }
    out.dedup();
    out
}

fn range_of(v: &Value, k: &str, d: [f32; 2]) -> [f32; 2] {
    v.get(k).and_then(|a| a.as_array()).map(|a| [a.first().and_then(|x| x.as_f64()).unwrap_or(d[0] as f64) as f32, a.get(1).and_then(|x| x.as_f64()).unwrap_or(d[1] as f64) as f32]).unwrap_or(d)
}

/// Copies scene meshes from an example map into one asset (exact geometry and textures): the
/// meshes within `radius` of `center` whose texture is in `textures`, centred, standing on the
/// ground (`floor` = the source's ground height), turned so the source's `facing` points +x.
pub fn copy_scenery(map_dir: &std::path::Path, center: [f32; 2], radius: f32, textures: &[String], facing: [f32; 2], floor: f32, name: &str) -> Result<String> {
    use crate::asset::{Asset, AssetPart};
    use crate::meshops::Mesh;
    let key = format!("asset:{name}");
    let p = crate::project::MapProject::open(map_dir)?;
    let Some(crate::project::Doc::Nvr(n)) = p.files.iter().find(|f| f.path.to_ascii_lowercase().ends_with("room.nvr")).map(|f| &f.doc) else { bail!("{} has no room.nvr", map_dir.display()) };
    let want: Vec<String> = textures.iter().map(|t| t.to_ascii_lowercase()).collect();
    let phi = -facing[1].atan2(facing[0]);
    let (sn, cs) = phi.sin_cos();
    let tex_dir = map_dir.join("Scene").join("Textures");
    let mut a = Asset::new(name);
    a.category = "Base & walls".into();
    a.source = format!("{} scene meshes", map_dir.display());
    let mut images = std::collections::HashMap::new();
    for (i, m) in n.meshes.iter().enumerate() {
        let b = m.bbox;
        let (cx, cz) = ((b[0] + b[3]) / 2.0, (b[2] + b[5]) / 2.0);
        let tex = n.materials[m.material as usize].channels.first().map(|c| c.texture.clone()).unwrap_or_default();
        if (cx - center[0]).hypot(cz - center[1]) > radius || !want.contains(&tex.to_ascii_lowercase()) {
            continue;
        }
        let g = n.mesh_geometry(i);
        let positions: Vec<[f32; 3]> = g.positions.iter().map(|v| {
            let (x, z) = (v[0] - center[0], v[2] - center[1]);
            [cs * x - sn * z, v[1] - floor, sn * x + cs * z]
        }).collect();
        let normals: Vec<[f32; 3]> = g.normals.iter().map(|v| [cs * v[0] - sn * v[2], v[1], sn * v[0] + cs * v[2]]).collect();
        if !images.contains_key(&tex) {
            let file = std::fs::read_dir(&tex_dir).ok().and_then(|rd| rd.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(&tex))).map(|e| e.path());
            let img = file.and_then(|f| std::fs::read(f).ok()).and_then(|b| crate::formats::dds::decode(&b, 2048).ok()).with_context(|| format!("texture {tex}"))?;
            images.insert(tex.clone(), img);
        }
        a.parts.push(AssetPart { name: format!("{} {i}", n.materials[m.material as usize].name), mesh: Mesh { positions, normals, uvs: g.uvs.clone(), colors: Vec::new(), indices: g.indices.clone() }, texture: Some(tex), color: [1.0; 4], visible: true });
    }
    if a.parts.is_empty() {
        bail!("no meshes with those textures there");
    }
    crate::asset::save(&a, &images)?;
    Ok(key)
}

// ------------------------------------------------------------------ tools

impl Workshop {
    fn lib(&self) -> Result<&PropLibrary> {
        self.lib.as_deref().ok_or_else(|| anyhow!("the scenery library is not loaded"))
    }

    fn find_texture(&self, name: &str) -> Option<String> {
        if name.starts_with("gen:") || std::path::Path::new(name).is_file() {
            return Some(name.to_string());
        }
        let want = name.to_ascii_lowercase();
        let want_dds = if want.contains('.') { want.clone() } else { format!("{want}.dds") };
        for d in &self.tex_dirs {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let f = e.file_name().to_string_lossy().to_ascii_lowercase();
                if f == want || f == want_dds || f.trim_end_matches(".png") == want {
                    return Some(e.path().to_string_lossy().to_string());
                }
            }
        }
        None
    }

    fn set_texture(&mut self, slot: &str, path: String, label: Option<&str>) -> Result<String> {
        let th = &mut self.l.theme;
        match slot {
            "cliff" => th.cliff = path.clone(),
            "brush" => th.brush = path.clone(),
            "mound" => th.forest.mound_texture = path.clone(),
            s => {
                let i: usize = s.trim_start_matches("ground").trim_start_matches('_').parse().map_err(|_| anyhow!("slot is ground0..ground15, cliff, brush or mound"))?;
                // New layers are added as needed (up to the map's layer limit).
                if i >= crate::layout::MAX_LAYERS {
                    bail!("at most {} ground layers", crate::layout::MAX_LAYERS);
                }
                while th.ground.len() <= i {
                    let k = th.ground.len();
                    th.ground.push(crate::layout::GroundLayer { name: format!("Layer {k}"), texture: "gen:dirt".into() });
                }
                let g = &mut th.ground[i];
                g.texture = path.clone();
                if let Some(n) = label {
                    g.name = n.to_string();
                }
            }
        }
        Ok(format!("{slot} = {}", path.rsplit(['/', '\\']).next().unwrap_or(&path)))
    }

    /// Runs one tool; the result text goes back to the model.
    pub fn call(&mut self, name: &str, v: &Value) -> Result<String> {
        self.calls += 1;
        let l = &mut self.l;
        match name {
            "new_map" => {
                let w = num(v, "width").unwrap_or(12000.0).clamp(5000.0, 24000.0);
                let h = num(v, "height").unwrap_or(w).clamp(5000.0, 24000.0);
                let name = text(v, "name").unwrap_or("AI Map");
                let keep_theme = l.theme.clone();
                let keep_maps = l.asset_maps.clone();
                *l = Layout::new(name, num(v, "map_id").map_or(30, |x| x as u32), [w, h]);
                l.theme = keep_theme;
                l.asset_maps = keep_maps;
                l.symmetry = symmetry_of(text(v, "symmetry").unwrap_or("none"));
                if let Some(s) = text(v, "fill") {
                    let s = surface_of(s)?;
                    let border = (300.0 / CELL) as usize;
                    let (nx, ny) = (l.nx, l.ny);
                    for j in border..ny.saturating_sub(border) {
                        for i in border..nx.saturating_sub(border) {
                            l.surface[j * nx + i] = s as u8;
                        }
                    }
                }
                Ok(format!("New map \"{name}\" {w:.0} x {h:.0} units, symmetry {:?}, a 300-unit wall all round. Blue (Order) is usually at low x / low z; north (+z) is up in the overview.", l.symmetry))
            }
            "paint" => {
                let s = surface_of(text(v, "surface").unwrap_or("ground"))?;
                let shape = text(v, "shape").unwrap_or("circle").to_ascii_lowercase();
                let pts = points(v, "points");
                let pts = if pts.is_empty() { vec![at(v)?] } else { pts };
                let r = num(v, "radius").unwrap_or(300.0).max(10.0);
                let width = num(v, "width").unwrap_or(r * 2.0).max(20.0);
                let mirror = flag(v, "mirror", true);
                let before = l.surface.iter().filter(|&&x| x == s as u8).count();
                match shape.as_str() {
                    "circle" | "square" => mirrored_op(l, mirror, |l| {
                        for p in &pts {
                            l.paint_surface(p[0], p[1], r, shape == "square", s);
                        }
                    }),
                    "polygon" => {
                        if pts.len() < 3 {
                            bail!("a polygon needs at least 3 points");
                        }
                        mirrored_op(l, mirror, |l| l.fill_polygon(&pts, s));
                    }
                    "path" | "line" => {
                        l.carve_path(&pts, width, s, None);
                        if mirror && l.symmetry != Symmetry::None {
                            let m: Vec<[f32; 2]> = pts.iter().map(|p| l.mirror_point(*p)).collect();
                            l.carve_path(&m, width, s, None);
                        }
                    }
                    "fill" => mirrored_op(l, mirror, |l| l.flood_fill(pts[0][0], pts[0][1], s)),
                    o => bail!("unknown shape \"{o}\" (circle, square, polygon, path, fill)"),
                }
                let after = l.surface.iter().filter(|&&x| x == s as u8).count();
                Ok(format!("{:?}: {} cells changed ({:.0}% of the map is now {:?})", s, after.abs_diff(before), after as f32 * 100.0 / l.surface.len() as f32, s))
            }
            "sculpt" => {
                let mode = match text(v, "mode").unwrap_or("raise") {
                    "lower" => SculptMode::Lower,
                    "flatten" => SculptMode::Flatten,
                    "smooth" => SculptMode::Smooth,
                    "noise" => SculptMode::Noise,
                    _ => SculptMode::Raise,
                };
                let pts = points(v, "points");
                let pts = if pts.is_empty() { vec![at(v)?] } else { pts };
                let (r, amount, target) = (num(v, "radius").unwrap_or(400.0), num(v, "amount").unwrap_or(40.0), num(v, "target").unwrap_or(0.0));
                for p in &pts {
                    l.sculpt(p[0], p[1], r, amount, mode, target);
                }
                Ok(format!("sculpted {} spots", pts.len()))
            }
            "paint_texture" => {
                let layer = num(v, "layer").unwrap_or(1.0) as usize;
                let pts = points(v, "points");
                let pts = if pts.is_empty() { vec![at(v)?] } else { pts };
                let (r, s) = (num(v, "radius").unwrap_or(400.0), num(v, "strength").unwrap_or(1.0));
                let mirror = flag(v, "mirror", true);
                if text(v, "shape") == Some("polygon") && pts.len() >= 3 {
                    // Every cell inside the polygon (soft edges from the brush radius).
                    let step = (r * 0.7).max(CELL);
                    let (x0, x1) = pts.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p[0]), a.1.max(p[0])));
                    let (z0, z1) = pts.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p[1]), a.1.max(p[1])));
                    let mut n = 0;
                    mirrored_op(l, mirror, |l| {
                        let mut z = z0;
                        while z <= z1 {
                            let mut x = x0;
                            while x <= x1 {
                                if crate::layout::point_in_polygon(&pts, x, z) {
                                    l.paint_texture(x, z, r, layer, s);
                                    n += 1;
                                }
                                x += step;
                            }
                            z += step;
                        }
                    });
                    return Ok(format!("painted ground layer {layer} over a polygon ({n} dabs)"));
                }
                mirrored_op(l, mirror, |l| {
                    for p in &pts {
                        l.paint_texture(p[0], p[1], r, layer, s);
                    }
                });
                Ok(format!("painted ground layer {layer} at {} spots", pts.len()))
            }
            "set_lane" => {
                let lane = lane_of(v)?;
                let pts = points(v, "points");
                if pts.len() < 2 {
                    bail!("a lane needs at least 2 points, from the blue base to the red base");
                }
                let width = num(v, "width").unwrap_or(650.0);
                if flag(v, "carve", true) {
                    let layer = v.get("texture_layer").and_then(|x| x.as_u64()).map(|x| x as usize).or(Some(2));
                    l.carve_path(&pts, width, Surface::Ground, layer);
                }
                l.set_lane(lane, pts.clone());
                let len: f32 = pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
                Ok(format!("{lane:?} lane set: {} points, {len:.0} units long (Summoner's Rift lanes are about 13000-15000)", pts.len()))
            }
            "place" => {
                let kind = kind_of(text(v, "kind").unwrap_or(""))?;
                let p = at(v)?;
                let team = team_of(v);
                let mirror = flag(v, "mirror", true);
                let mut id = 0;
                mirrored_op(l, mirror, |l| {
                    id = if kind == ObjKind::SpawnPoint { l.add_fountain(team, p) } else { l.add_objective(kind, team, lane_of(v).unwrap_or(Lane::Mid), tier_of(v), p) };
                });
                if let Some(f) = num(v, "facing") {
                    let partner = l.objective(id).and_then(|o| o.mirror);
                    for o in l.objectives.iter_mut().filter(|o| o.id == id || Some(o.id) == partner) {
                        o.yaw = f;
                    }
                }
                let s = l.cell_of(p[0], p[1]).map(|(i, j)| l.surface_at(i, j));
                let warn = if s.is_some_and(|s| !s.walkable()) && !kind.is_camp() { format!(" WARNING: it stands on {:?}", s.unwrap()) } else { String::new() };
                let mirror_note = l.objective(id).and_then(|o| o.mirror).map(|m| format!(" (mirror copy id {m} for the other team)")).unwrap_or_default();
                Ok(format!("{kind:?} id {id} at ({:.0}, {:.0}){mirror_note}{warn}", p[0], p[1]))
            }
            "move" => {
                let id = num(v, "id").ok_or_else(|| anyhow!("id is required"))? as u32;
                if l.objective(id).is_none() {
                    bail!("no object with id {id}");
                }
                l.move_objective(id, at(v)?);
                Ok(format!("moved {id}"))
            }
            "remove" => {
                let id = num(v, "id").ok_or_else(|| anyhow!("id is required"))? as u32;
                if l.objective(id).is_some() {
                    l.remove_objective(id);
                } else {
                    l.props.retain(|p| p.id != id);
                    l.effects.retain(|p| p.id != id);
                    l.lights.retain(|p| p.id != id);
                }
                Ok(format!("removed {id}"))
            }
            "clear_area" => {
                let p = at(v)?;
                let r = num(v, "radius").unwrap_or(400.0);
                let what = text(v, "what").unwrap_or("props");
                let n0 = l.props.len() + l.objectives.len();
                if what == "props" || what == "all" {
                    l.erase_props(p[0], p[1], r);
                }
                if what == "objects" || what == "all" {
                    let ids: Vec<u32> = l.objectives.iter().filter(|o| (o.pos[0] - p[0]).hypot(o.pos[1] - p[1]) < r).map(|o| o.id).collect();
                    for id in ids {
                        l.remove_objective(id);
                    }
                }
                Ok(format!("removed {}", n0 - l.props.len() - l.objectives.len()))
            }
            "auto_structures" => {
                l.auto_structures();
                Ok(format!("standard turrets, inhibitors and minion spawns placed along the lanes: {} objects", l.objectives.len()))
            }
            "search_props" => {
                let lib = self.lib()?;
                let q = text(v, "query").unwrap_or("").to_ascii_lowercase();
                let cat = text(v, "category").unwrap_or("").to_ascii_lowercase();
                let lim = num(v, "limit").unwrap_or(15.0) as usize;
                let hits: Vec<String> = lib.props.iter().filter(|p| PropLibrary::allowed(&p.key, &self.l.asset_maps)).filter(|p| (cat.is_empty() || p.category.to_ascii_lowercase().contains(&cat)) && (q.is_empty() || p.name.to_ascii_lowercase().contains(&q) || p.key.to_ascii_lowercase().contains(&q) || p.category.to_ascii_lowercase().contains(&q) || prop_texture(p).to_ascii_lowercase().contains(&q))).take(lim.max(1)).map(|p| format!("{} [{}] texture {}, {:.0} wide, {:.0} tall, used {}x on its map", p.key, p.category, prop_texture(p), p.radius * 2.0, p.height, p.copies)).collect();
                let cats = lib.categories().join(", ");
                Ok(if hits.is_empty() { format!("nothing found; categories: {cats}") } else { hits.join("\n") })
            }
            "place_prop" => {
                let key = text(v, "key").ok_or_else(|| anyhow!("key is required (from search_props or fetch_model)"))?.to_string();
                let key = self.lib.as_deref().and_then(|lb| expand_keys(lb, &[key.clone()]).into_iter().next()).unwrap_or(key);
                if self.lib.as_ref().is_some_and(|lb| lb.get(&key).is_none()) {
                    bail!("no prop \"{key}\" in the library");
                }
                let p = at(v)?;
                let (yaw, scale) = (num(v, "yaw").unwrap_or(0.0), num(v, "scale").unwrap_or(1.0).clamp(0.1, 6.0));
                let tall = self.lib.as_deref().and_then(|lb| lb.get(&key)).filter(|d| !d.is_decal() && d.height * scale > 40.0).map(|d| d.radius * scale);
                let l = &mut self.l;
                let warn = match tall {
                    Some(r) if !clear_of_lanes(l, p, r) => " WARNING: it stands in a lane",
                    _ => "",
                };
                let n0 = l.props.len();
                let id = add_prop_sym(l, &key, p, yaw, scale, num(v, "lift").unwrap_or(0.0), flag(v, "mirror", true));
                if flag(v, "walk_through", false) {
                    l.props[n0..].iter_mut().for_each(|q| q.walk_through = true);
                }
                Ok(format!("prop id {id}{warn}"))
            }
            "prop_row" => {
                let key = text(v, "key").ok_or_else(|| anyhow!("key is required"))?.to_string();
                let lib = self.lib()?;
                let key = expand_keys(lib, &[key.clone()]).into_iter().next().unwrap_or(key);
                let def = lib.get(&key).ok_or_else(|| anyhow!("no prop \"{key}\""))?;
                let scale = num(v, "scale").unwrap_or(1.0);
                let step = num(v, "spacing").unwrap_or(def.radius * 2.0 * scale).max(30.0);
                let pts = points(v, "points");
                let spots = crate::layout::line_spots(&pts, step, num(v, "turn").unwrap_or(0.0));
                let l = &mut self.l;
                let mirror = flag(v, "mirror", true) && l.symmetry != Symmetry::None;
                for (q, y) in &spots {
                    add_prop_sym(l, &key, *q, *y, scale, 0.0, mirror);
                }
                Ok(format!("{} props along the line", spots.len()))
            }
            "scatter_props" => {
                let lib = self.lib()?;
                let keys: Vec<String> = match v.get("keys").and_then(|k| k.as_array()) {
                    Some(a) => expand_keys(lib, &a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()),
                    None => {
                        let cat = text(v, "category").unwrap_or("Rocks").to_ascii_lowercase();
                        lib.props.iter().filter(|p| PropLibrary::allowed(&p.key, &self.l.asset_maps) && p.category.to_ascii_lowercase() == cat).take(12).map(|p| p.key.clone()).collect()
                    }
                };
                if keys.is_empty() {
                    bail!("no props to scatter");
                }
                let p = at(v)?;
                let l = &mut self.l;
                let n0 = l.props.len();
                let sc = [num(v, "min_scale").unwrap_or(0.85), num(v, "max_scale").unwrap_or(1.2)];
                l.scatter_props(&keys, p[0], p[1], num(v, "radius").unwrap_or(500.0), num(v, "spacing").unwrap_or(200.0), sc, self.calls as u32 * 7919);
                Ok(format!("{} props scattered", l.props.len() - n0))
            }
            "scenery_wall" => {
                // A wall made of scenery: models along a line, and the ground under them unwalkable.
                let lib = self.lib()?;
                let keys = expand_keys(lib, &keys_of(v, "keys"));
                if keys.is_empty() {
                    bail!("keys (scenery models) are required (none matched)");
                }
                for k in &keys {
                    if lib.get(k).is_none() {
                        bail!("no prop \"{k}\"");
                    }
                }
                let radius: Vec<f32> = keys.iter().map(|k| lib.get(k).map_or(100.0, |d| d.radius.max(10.0))).collect();
                let pts = points(v, "points");
                if pts.len() < 2 {
                    bail!("points: at least 2");
                }
                let size = range_of(v, "size", [220.0, 340.0]);
                let width = num(v, "width").unwrap_or(size[0] * 0.8);
                let spacing = num(v, "spacing").unwrap_or((size[0] + size[1]) * 0.25);
                let mirror = flag(v, "mirror", true);
                let l = &mut self.l;
                if flag(v, "block", true) {
                    // Blocked along the line, except where it crosses a lane (the wall opens there).
                    let sym = l.symmetry;
                    if !mirror {
                        l.symmetry = Symmetry::None;
                    }
                    for (q, _) in crate::layout::line_spots(&pts, (width * 0.25).max(CELL), 0.0) {
                        if clear_of_lanes(l, q, width * 0.5) {
                            l.paint_surface(q[0], q[1], width * 0.5, false, Surface::Wall);
                        }
                    }
                    l.symmetry = sym;
                }
                let spots = crate::layout::line_spots(&pts, spacing, 0.0);
                let seed = (pts[0][0] as u32).wrapping_mul(31) ^ pts[0][1] as u32;
                let mut skipped = 0;
                for (k, (q, y)) in spots.iter().enumerate() {
                    let h = |c: u32| crate::layout_build::hash01(seed, k as u32, c);
                    let pick = (h(1) * keys.len() as f32) as usize % keys.len();
                    let sz = size[0] + (size[1] - size[0]) * h(2);
                    if !clear_of_lanes(l, *q, sz * 0.5) {
                        skipped += 1;
                        continue;
                    }
                    let sc = (sz / (2.0 * radius[pick])).clamp(0.2, 5.0);
                    add_prop_sym(l, &keys[pick], *q, y + (h(3) - 0.5) * 50.0 + if h(4) < 0.5 { 0.0 } else { 180.0 }, sc, -10.0, mirror);
                }
                Ok(format!("scenery wall: {} models along {} points (x2 mirrored), {skipped} left out to keep lanes clear", spots.len() - skipped, pts.len()))
            }
            "scenery_fill" => {
                // Every unwalkable wall area (optionally inside a polygon) dressed with scenery:
                // models along its outline, others filling the inside. Placed as real props.
                use crate::autodress::{EdgeRule, FillRule, Item};
                let lib = self.lib()?;
                let item = |k: &String, size: [f32; 2]| Item { key: k.clone(), weight: 1.0, size };
                let edge: Vec<Item> = expand_keys(lib, &keys_of(v, "edge_keys")).iter().filter(|k| lib.get(k).is_some()).map(|k| item(k, range_of(v, "edge_size", [220.0, 360.0]))).collect();
                let fill: Vec<Item> = expand_keys(lib, &keys_of(v, "fill_keys")).iter().filter(|k| lib.get(k).is_some()).map(|k| item(k, range_of(v, "fill_size", [260.0, 420.0]))).collect();
                if edge.is_empty() && fill.is_empty() {
                    bail!("edge_keys and/or fill_keys (scenery models) are required");
                }
                let radius = |k: &str| lib.get(k).map_or(100.0, |d| d.radius.max(10.0));
                let region = points(v, "region");
                let l = &self.l;
                let (nx, ny) = (l.nx, l.ny);
                let mask: Vec<bool> = (0..nx * ny).map(|k| {
                    if l.surface[k] != Surface::Wall as u8 {
                        return false;
                    }
                    let p = [((k % nx) as f32 + 0.5) * CELL, ((k / nx) as f32 + 0.5) * CELL];
                    region.len() < 3 || crate::layout::point_in_polygon(&region, p[0], p[1]) || (l.symmetry != Symmetry::None && crate::layout::point_in_polygon(&region.iter().map(|q| l.mirror_point(*q)).collect::<Vec<_>>(), p[0], p[1]))
                }).collect();
                let dist = inside_dist(&mask, nx, ny);
                let mut placed = Vec::new();
                if !edge.is_empty() {
                    let mean = edge.iter().map(|i| (i.size[0] + i.size[1]) * 0.5).sum::<f32>() / edge.len() as f32;
                    let rule = EdgeRule { items: edge, spacing: num(v, "edge_spacing").unwrap_or(mean * 0.5) / mean, inset: num(v, "inset").unwrap_or(0.3), align: true };
                    for line in crate::autodress::outlines(&mask, nx, ny, CELL) {
                        if crate::autodress::area(&line).abs() > 4.0 * CELL * CELL {
                            crate::autodress::along(&line, true, &rule, 7, 1.0, &mut placed);
                        }
                    }
                }
                let [mw, md] = l.size();
                if !fill.is_empty() {
                    let rule = FillRule { items: fill, step: num(v, "fill_spacing").unwrap_or(300.0), margin: 0.3, chance: num(v, "fill_chance").unwrap_or(0.85) };
                    let mut f2 = Vec::new();
                    crate::autodress::fill([0.0, 0.0, mw, md], &rule, 11, &|x, z| l.cell_of(x, z).map_or(0.0, |(i, j)| dist[j * nx + i]) - num(v, "fill_margin").unwrap_or(120.0), &mut f2);
                    f2.retain(|t| !placed.iter().any(|r: &crate::autodress::Placed| (r.at[0] - t.at[0]).hypot(r.at[1] - t.at[1]) < r.size * 0.45));
                    placed.extend(f2);
                }
                let mirror = flag(v, "mirror", true);
                // Symmetric: the first half's models, reflected onto the other half.
                let keep: Vec<_> = placed.into_iter().filter(|p| !mirror || first_half(l, p.on)).filter(|p| p.on[0] > 2.0 * CELL && p.on[1] > 2.0 * CELL && p.on[0] < mw - 2.0 * CELL && p.on[1] < md - 2.0 * CELL).filter(|p| clear_of_lanes(l, p.at, p.size * 0.5)).collect();
                let n = keep.len();
                // The mirror copy may use another model (e.g. a frost half and a volcanic half).
                // Selectors pair up by position: the i-th model of one with the i-th of the other.
                let mut swap: std::collections::HashMap<String, String> = Default::default();
                if let Some(o) = v.get("mirror_keys").and_then(|m| m.as_object()) {
                    let lib = self.lib()?;
                    for (k, x) in o {
                        let (a, b) = (expand_keys(lib, &[k.clone()]), expand_keys(lib, &[x.as_str().unwrap_or("").to_string()]));
                        if !b.is_empty() {
                            for (i, ka) in a.iter().enumerate() {
                                swap.insert(ka.clone(), b[i % b.len()].clone());
                            }
                        }
                    }
                }
                let radii: std::collections::HashMap<String, f32> = keep.iter().map(|p| p.key.clone()).chain(swap.values().cloned()).map(|k| { let r = radius(&k); (k, r) }).collect();
                let l = &mut self.l;
                for p in &keep {
                    let lift = if p.edge { -10.0 } else { 0.0 };
                    let sc = (p.size / (2.0 * radii[&p.key])).clamp(0.2, 5.0);
                    add_prop_sym(l, &p.key, p.at, p.yaw, sc, lift, false);
                    if mirror && l.symmetry != Symmetry::None {
                        let m = l.mirror_point(p.on);
                        let m = [m[0] + (l.mirror_point(p.at)[0] - m[0]), m[1] + (l.mirror_point(p.at)[1] - m[1])];
                        if (m[0] - p.at[0]).hypot(m[1] - p.at[1]) > 60.0 {
                            let k2 = swap.get(&p.key).unwrap_or(&p.key);
                            let sc2 = (p.size / (2.0 * radii.get(k2).copied().unwrap_or(100.0))).clamp(0.2, 5.0);
                            add_prop_sym(l, k2, m, mirror_yaw(l.symmetry, p.yaw), sc2, lift, false);
                        }
                    }
                }
                // The walls are now scenery: no automatic dressing on top.
                l.theme.forest.auto_dress = false;
                Ok(format!("{n} scenery models (and their mirror copies) on the walls; automatic wall dressing is off"))
            }
            "copy_scenery" => {
                // Exact pieces of an example map (e.g. Summoner's Rift's fountain shrine) as one asset.
                let name = text(v, "name").ok_or_else(|| anyhow!("name is required"))?;
                let map = text(v, "map").unwrap_or("Map1");
                let root = self.map1_tex.as_ref().and_then(|t| t.parent()?.parent()?.parent().map(|p| p.to_path_buf())).ok_or_else(|| anyhow!("example maps not found"))?;
                let top: Vec<PathBuf> = std::fs::read_dir(&root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
                let dir = crate::props::map_folders(&top).into_iter().find(|(_, l)| l.eq_ignore_ascii_case(map)).map(|(d, _)| d).ok_or_else(|| anyhow!("no example map \"{map}\""))?;
                let c = v.get("center").and_then(point).ok_or_else(|| anyhow!("center [x, z] is required"))?;
                let f = v.get("facing").and_then(point).unwrap_or([1.0, 0.0]);
                let key = copy_scenery(&dir, c, num(v, "radius").unwrap_or(800.0), &keys_of(v, "textures"), f, num(v, "floor").unwrap_or(0.0), name)?;
                if let Some(lb) = &self.lib {
                    let mut c2 = (**lb).clone();
                    c2.set_assets(crate::asset::library_props());
                    self.lib = Some(Arc::new(c2));
                }
                let d = self.lib.as_ref().and_then(|lb| lb.get(&key)).map(|d| format!(", {:.0} wide, {:.0} tall", d.radius * 2.0, d.height)).unwrap_or_default();
                Ok(format!("{key}{d}: copied from {map}; place it with place_prop (it faces +x at yaw 0)"))
            }
            "fetch_model" => {
                use crate::online_assets as oa;
                let q = text(v, "query").ok_or_else(|| anyhow!("query is required"))?;
                let cat = oa::catalogue()?;
                let hits = oa::search(&cat, q);
                let m = hits.first().ok_or_else(|| anyhow!("nothing found online for \"{q}\" (try other words; modern objects are skipped)"))?;
                let category = oa::category_for(m, text(v, "category").unwrap_or(""));
                let key = oa::add_classic(m, &category, self.lib.as_deref(), self.map1_tex.as_deref())?;
                // The new asset joins the library.
                if let Some(lb) = &self.lib {
                    let mut c = (**lb).clone();
                    c.set_assets(crate::asset::library_props());
                    self.lib = Some(Arc::new(c));
                }
                let d = self.lib.as_ref().and_then(|lb| lb.get(&key)).map(|d| format!(", {:.0} wide, {:.0} tall", d.radius * 2.0, d.height)).unwrap_or_default();
                Ok(format!("{key} [{category}]{d} - downloaded from Poly Haven (CC0) and restyled like classic Map1 art; place it with place_prop / prop_row"))
            }
            "list_textures" => {
                let q = text(v, "query").unwrap_or("").to_ascii_lowercase();
                let mut out: Vec<String> = ["gen:grass", "gen:dirt", "gen:path", "gen:sand", "gen:rock", "gen:brush", "gen:snow", "gen:ash", "gen:lava"].iter().filter(|g| q.is_empty() || g.contains(&q)).map(|s| s.to_string()).collect();
                let mut seen = std::collections::HashSet::new();
                for d in &self.tex_dirs {
                    let map = d.parent().and_then(|p| p.parent()).and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                        let f = e.file_name().to_string_lossy().to_string();
                        let fl = f.to_ascii_lowercase();
                        if (fl.ends_with(".dds") || fl.ends_with(".png")) && !fl.starts_with("2x_") && !fl.starts_with("4x_") && (q.is_empty() || fl.contains(&q)) && seen.insert(fl.clone()) {
                            out.push(format!("{f} ({map})"));
                        }
                    }
                }
                out.truncate(120);
                Ok(out.join(", "))
            }
            "set_texture" => {
                let slot = text(v, "slot").unwrap_or("ground0").to_ascii_lowercase();
                let t = text(v, "texture").ok_or_else(|| anyhow!("texture is required"))?;
                let path = self.find_texture(t).ok_or_else(|| anyhow!("texture \"{t}\" not found; use list_textures or fetch_texture"))?;
                self.set_texture(&slot, path, text(v, "label"))
            }
            "fetch_texture" => {
                use crate::online_assets as oa;
                let q = text(v, "query").ok_or_else(|| anyhow!("query is required"))?;
                let cat = oa::texture_catalogue()?;
                let hits = oa::search(&cat, q);
                let m = hits.first().ok_or_else(|| anyhow!("no texture found online for \"{q}\""))?;
                let path = oa::add_texture(m, self.map1_tex.as_deref())?;
                let path = path.to_string_lossy().to_string();
                let applied = match text(v, "slot") {
                    Some(s) => format!("; {}", self.set_texture(&s.to_ascii_lowercase(), path.clone(), text(v, "label"))?),
                    None => String::new(),
                };
                Ok(format!("{} ({}, Poly Haven CC0) restyled like Map1: {}{applied}", m.name, m.id, path.rsplit(['/', '\\']).next().unwrap_or(&path)))
            }
            "make_texture" => {
                // A new texture edited from an existing one, saved with the downloaded textures.
                let name = text(v, "name").ok_or_else(|| anyhow!("name is required"))?;
                let base = text(v, "base").ok_or_else(|| anyhow!("base texture is required"))?;
                let bp = self.find_texture(base).ok_or_else(|| anyhow!("texture \"{base}\" not found; use list_textures"))?;
                let img = if let Some(g) = bp.strip_prefix("gen:") { let i = crate::layout_build::generated_texture(g); crate::formats::dds::Image { width: i.width, height: i.height, pixels: i.pixels } } else { crate::texture_edit::load(std::path::Path::new(&bp))? };
                let img = if img.width != crate::texture_edit::SIZE { crate::texture_edit::load_image(img) } else { img };
                let steps: Vec<Value> = v.get("steps").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                let out = crate::texture_edit::compose(img, &steps, &|t| self.find_texture(t).filter(|p| !p.starts_with("gen:")).map(PathBuf::from))?;
                let dir = crate::asset::library_dir().parent().map(|p| p.join("textures")).unwrap_or_else(|| PathBuf::from("textures"));
                let path = dir.join(format!("{}.png", crate::asset::safe_name(name)));
                crate::texture_edit::save_png(&out, &path)?;
                if !self.tex_dirs.contains(&dir) {
                    self.tex_dirs.push(dir);
                }
                let path = path.to_string_lossy().to_string();
                let applied = match text(v, "slot") {
                    Some(sl) => format!("; {}", self.set_texture(&sl.to_ascii_lowercase(), path.clone(), text(v, "label"))?),
                    None => String::new(),
                };
                Ok(format!("made {}.png from {} with {} edits{applied}", crate::asset::safe_name(name), base, steps.len()))
            }
            "effect" => {
                let name = text(v, "name").ok_or_else(|| anyhow!("name is required"))?;
                if !self.effects.is_empty() && !self.effects.iter().any(|e| e.eq_ignore_ascii_case(name)) {
                    bail!("unknown effect \"{name}\"; use list_effects");
                }
                let l = &mut self.l;
                let p = at(v)?;
                let id = l.add_effect(name, p, num(v, "lift").unwrap_or(0.0), num(v, "yaw").unwrap_or(0.0));
                if flag(v, "mirror", true) && l.symmetry != Symmetry::None {
                    let m = l.mirror_point(p);
                    let y = mirror_yaw(l.symmetry, num(v, "yaw").unwrap_or(0.0));
                    l.add_effect(name, m, num(v, "lift").unwrap_or(0.0), y);
                }
                Ok(format!("effect id {id}"))
            }
            "list_effects" => {
                let q = text(v, "query").unwrap_or("").to_ascii_lowercase();
                let hits: Vec<&String> = self.effects.iter().filter(|e| q.is_empty() || e.to_ascii_lowercase().contains(&q)).take(40).collect();
                Ok(if hits.is_empty() { "none found".into() } else { hits.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ") })
            }
            "light" => {
                let p = at(v)?;
                let c = v.get("color").and_then(|c| c.as_array()).map(|a| [0, 1, 2].map(|k| a.get(k).and_then(|x| x.as_f64()).unwrap_or(255.0).clamp(0.0, 255.0) as u8)).unwrap_or([255, 140, 60]);
                let r = num(v, "radius").unwrap_or(600.0);
                let id = l.add_light(p, num(v, "lift").unwrap_or(150.0), c, r * 0.4, r, num(v, "intensity").unwrap_or(1.0));
                if flag(v, "mirror", true) && l.symmetry != Symmetry::None {
                    let m = l.mirror_point(p);
                    l.add_light(m, num(v, "lift").unwrap_or(150.0), c, r * 0.4, r, num(v, "intensity").unwrap_or(1.0));
                }
                Ok(format!("light id {id}"))
            }
            "look" => {
                let th = &mut l.theme;
                let col = |k: &str| v.get(k).and_then(|c| c.as_array()).map(|a| [0, 1, 2].map(|i| a.get(i).and_then(|x| x.as_f64()).unwrap_or(128.0) as f32));
                if let Some(c) = col("sun_color") {
                    th.sun_color = c;
                }
                if let Some(c) = col("ambient_color") {
                    th.ambient = c;
                }
                if let Some(b) = v.get("lava").and_then(|x| x.as_bool()) {
                    th.lava = b;
                }
                if let Some(x) = v.get("lava_from_x") {
                    th.lava_from_x = x.as_f64().map(|x| x as f32);
                }
                if let Some(s) = text(v, "wall_style") {
                    th.forest.league_walls = s != "raised";
                }
                for (k, f) in [("mound", &mut th.forest.mound), ("wall_height", &mut th.wall_height), ("river_depth", &mut th.river_depth), ("base_height", &mut th.base_height), ("tree_spacing", &mut th.forest.spacing), ("edge_spacing", &mut th.forest.edge_spacing), ("edge_trees", &mut th.forest.edge_tree_share)] {
                    if let Some(x) = num(v, k) {
                        *f = x;
                    }
                }
                if let Some(b) = v.get("trees_on_walls").and_then(|x| x.as_bool()) {
                    th.forest.enabled = b;
                }
                if let Some(b) = v.get("dress_walls").and_then(|x| x.as_bool()) {
                    th.forest.auto_dress = b;
                }
                if let Some(a) = v.get("scenery_from").and_then(|x| x.as_array()) {
                    l.asset_maps = a.iter().filter_map(|x| x.as_str().map(String::from)).collect();
                }
                if let Some(o) = v.get("settings").and_then(|x| x.as_object()) {
                    for (k, x) in o {
                        l.settings.insert(k.clone(), x.as_str().map(String::from).unwrap_or_else(|| x.to_string()));
                    }
                }
                Ok("look updated".into())
            }
            "overview" => Ok(overview(l)),
            "check" => Ok(check(l)),
            "finish" => {
                self.done = Some(text(v, "summary").unwrap_or("Done.").to_string());
                Ok("finished".into())
            }
            o => bail!("unknown tool \"{o}\""),
        }
    }
}

// ------------------------------------------------------------------ seeing the map

/// A character picture of the map (north up) with every object, lane and count.
pub fn overview(l: &Layout) -> String {
    let [w, d] = l.size();
    let cols = 72usize;
    let cw = w / cols as f32;
    let rows = ((d / cw).round() as usize).clamp(8, 90);
    let ch = d / rows as f32;
    let mut g = vec![vec![' '; cols]; rows];
    for (r, row) in g.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            let mut count = [0usize; 6];
            for (sx, sz) in [(0.5, 0.5), (0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let (x, z) = ((c as f32 + sx) * cw, d - (r as f32 + sz) * ch);
                if let Some((i, j)) = l.cell_of(x, z) {
                    count[l.surface[j * l.nx + i] as usize % 6] += 1;
                }
            }
            let s = (0..6).max_by_key(|&k| count[k]).unwrap_or(0);
            *cell = match Surface::from_u8(s as u8) {
                Surface::Ground => '.',
                Surface::Wall => '#',
                Surface::Brush => '"',
                Surface::River => '~',
                Surface::DeepWater => if l.theme.lava_at((c as f32 + 0.5) * cw) { '^' } else { '=' },
                Surface::Void => ' ',
            };
        }
    }
    let put = |g: &mut Vec<Vec<char>>, p: [f32; 2], ch_: char| {
        let (c, r) = ((p[0] / cw) as i64, ((d - p[1]) / ch) as i64);
        if c >= 0 && r >= 0 && (c as usize) < cols && (r as usize) < rows {
            g[r as usize][c as usize] = ch_;
        }
    };
    for lp in &l.lanes {
        for w2 in lp.points.windows(2) {
            let len = (w2[1][0] - w2[0][0]).hypot(w2[1][1] - w2[0][1]);
            let n = (len / (cw * 0.5)).ceil().max(1.0) as usize;
            for k in 0..=n {
                let t = k as f32 / n as f32;
                put(&mut g, [w2[0][0] + (w2[1][0] - w2[0][0]) * t, w2[0][1] + (w2[1][1] - w2[0][1]) * t], '+');
            }
        }
    }
    for o in &l.objectives {
        let c = match o.kind {
            ObjKind::Nexus => 'N',
            ObjKind::Inhibitor => 'I',
            ObjKind::Turret => 'T',
            ObjKind::SpawnPoint => 'S',
            ObjKind::FountainTurret => 'F',
            ObjKind::Shop => '$',
            ObjKind::MinionSpawn => 'M',
            ObjKind::BlueBuff => 'B',
            ObjKind::RedBuff => 'R',
            ObjKind::Dragon => 'D',
            ObjKind::Baron => 'X',
            ObjKind::HealthRelic => 'H',
            ObjKind::CapturePoint => 'P',
            ObjKind::SpeedShrine => 'Z',
            k if k.is_camp() => 'C',
            _ => '?',
        };
        put(&mut g, o.pos, if o.team == 2 { c.to_ascii_lowercase() } else { c });
    }
    let mut s = format!(
        "Map {w:.0} x {d:.0} units, symmetry {:?}, lava {}. One character = {cw:.0} x {ch:.0} units; top row = north (z = {d:.0}), left = x 0.\nLegend: . ground  # wall  \" brush  ~ river  {} deep water  (space) void  + lane path; objects UPPERCASE = blue team, lowercase = red: N nexus, I inhibitor, T turret, S spawn, F fountain turret, $ shop, M minion spawn, B blue buff, R red buff, C camp, D dragon, X baron, H relic, P capture point, Z speed shrine.\n",
        l.symmetry,
        l.theme.lava,
        if l.theme.lava { "^ lava" } else { "= deep water" }
    );
    for row in g {
        s.push_str(&row.into_iter().collect::<String>());
        s.push('\n');
    }
    s.push_str("Objects (id kind team lane tier x z):\n");
    for o in &l.objectives {
        s.push_str(&format!("  {} {:?} {} {:?} {:?} {:.0} {:.0}\n", o.id, o.kind, match o.team { 1 => "blue", 2 => "red", _ => "neutral" }, o.lane, o.tier, o.pos[0], o.pos[1]));
    }
    for lp in &l.lanes {
        s.push_str(&format!("Lane {:?}: {}\n", lp.lane, lp.points.iter().map(|p| format!("({:.0},{:.0})", p[0], p[1])).collect::<Vec<_>>().join(" ")));
    }
    let mut keys: std::collections::BTreeMap<&str, usize> = Default::default();
    for p in &l.props {
        *keys.entry(p.prop.as_str()).or_default() += 1;
    }
    s.push_str(&format!("{} props ({} kinds), {} effects, {} lights; ground layers: {}; cliff {}; brush {}\n", l.props.len(), keys.len(), l.effects.len(), l.lights.len(), l.theme.ground.iter().map(|g| format!("{}={}", g.name, g.texture.rsplit(['/', '\\']).next().unwrap_or(""))).collect::<Vec<_>>().join(", "), l.theme.cliff.rsplit(['/', '\\']).next().unwrap_or(""), l.theme.brush.rsplit(['/', '\\']).next().unwrap_or("")));
    s
}

/// Walking distances (units) over walkable cells from a point (8-neighbour Dijkstra-like BFS).
fn walk_field(l: &Layout, from: [f32; 2]) -> Vec<f32> {
    walk_field_min(l, from, None)
}

/// The same, only through cells whose open half-width is at least `min` (passages at least twice that wide).
fn walk_field_min(l: &Layout, from: [f32; 2], min: Option<(&[f32], f32)>) -> Vec<f32> {
    let (nx, ny) = (l.nx, l.ny);
    let mut dist = vec![f32::INFINITY; nx * ny];
    let Some((i0, j0)) = l.cell_of(from[0], from[1]) else { return dist };
    let walk = |k: usize| Surface::from_u8(l.surface[k]).walkable();
    let mut heap = std::collections::BinaryHeap::new();
    let k0 = j0 * nx + i0;
    dist[k0] = 0.0;
    heap.push((std::cmp::Reverse(0u32), k0));
    while let Some((std::cmp::Reverse(dq), k)) = heap.pop() {
        let dk = dq as f32 / 10.0;
        if dk > dist[k] {
            continue;
        }
        let (i, j) = ((k % nx) as i64, (k / nx) as i64);
        for (di, dj, c) in [(-1i64, 0i64, 1.0f32), (1, 0, 1.0), (0, -1, 1.0), (0, 1, 1.0), (-1, -1, 1.414), (1, -1, 1.414), (-1, 1, 1.414), (1, 1, 1.414)] {
            let (ii, jj) = (i + di, j + dj);
            if ii < 0 || jj < 0 || ii as usize >= nx || jj as usize >= ny {
                continue;
            }
            let kk = jj as usize * nx + ii as usize;
            if (!walk(kk) || min.is_some_and(|(w, m)| w[kk] < m)) && kk != k0 {
                continue;
            }
            let nd = dk + c * CELL;
            if nd < dist[kk] {
                dist[kk] = nd;
                heap.push((std::cmp::Reverse((nd * 10.0) as u32), kk));
            }
        }
    }
    dist
}

/// Half-width of the open space at each walkable cell (distance to the nearest blocked cell).
fn open_width(l: &Layout) -> Vec<f32> {
    let (nx, ny) = (l.nx, l.ny);
    let mut d: Vec<f32> = (0..nx * ny).map(|k| if Surface::from_u8(l.surface[k]).walkable() { 1e9 } else { 0.0 }).collect();
    let (d1, d2) = (CELL, CELL * std::f32::consts::SQRT_2);
    for j in 0..ny {
        for i in 0..nx {
            let k = j * nx + i;
            if d[k] == 0.0 {
                continue;
            }
            let mut v = d[k];
            if i > 0 { v = v.min(d[k - 1] + d1); }
            if j > 0 {
                v = v.min(d[k - nx] + d1);
                if i > 0 { v = v.min(d[k - nx - 1] + d2); }
                if i + 1 < nx { v = v.min(d[k - nx + 1] + d2); }
            }
            d[k] = v;
        }
    }
    for j in (0..ny).rev() {
        for i in (0..nx).rev() {
            let k = j * nx + i;
            if d[k] == 0.0 {
                continue;
            }
            let mut v = d[k];
            if i + 1 < nx { v = v.min(d[k + 1] + d1); }
            if j + 1 < ny {
                v = v.min(d[k + nx] + d1);
                if i + 1 < nx { v = v.min(d[k + nx + 1] + d2); }
                if i > 0 { v = v.min(d[k + nx - 1] + d2); }
            }
            d[k] = v;
        }
    }
    d
}

fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> (f32, f32) {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-6);
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0);
    ((a[0] + ab[0] * t - p[0]).hypot(a[1] + ab[1] * t - p[1]), t)
}

/// Distance of a point from a lane and how far along the lane it is (units from the blue end).
fn along_lane(pts: &[[f32; 2]], p: [f32; 2]) -> (f32, f32) {
    let mut best = (f32::MAX, 0.0);
    let mut run = 0.0;
    for w in pts.windows(2) {
        let len = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
        let (dd, t) = seg_dist(p, w[0], w[1]);
        if dd < best.0 {
            best = (dd, run + t * len);
        }
        run += len;
    }
    best
}

/// Playability check: reachability, lanes, structures, spacing, jungle paths, balance.
pub fn check(l: &Layout) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut problems = 0;
    let mut warn = |out: &mut Vec<String>, bad: bool, s: String| {
        if bad {
            problems += 1;
        }
        out.push(format!("{} {s}", if bad { "PROBLEM" } else { "WARN" }));
    };
    let find = |k: ObjKind, team: u8| l.objectives.iter().filter(move |o| o.kind == k && o.team == team);
    let nexus = |t: u8| find(ObjKind::Nexus, t).next().map(|o| o.pos);
    let spawn = |t: u8| find(ObjKind::SpawnPoint, t).next().map(|o| o.pos);
    for t in [1u8, 2] {
        let name = if t == 1 { "Blue" } else { "Red" };
        let c = |k| find(k, t).count();
        let (nx_, inh, tur, sp, ft, shop, ms) = (c(ObjKind::Nexus), c(ObjKind::Inhibitor), c(ObjKind::Turret), c(ObjKind::SpawnPoint), c(ObjKind::FountainTurret), c(ObjKind::Shop), c(ObjKind::MinionSpawn));
        out.push(format!("{name}: {nx_} nexus, {inh} inhibitors, {tur} turrets, {sp} spawn, {ft} fountain turret, {shop} shop, {ms} minion spawns"));
        if nx_ != 1 {
            warn(&mut out, true, format!("{name} needs exactly one nexus"));
        }
        if sp == 0 || shop == 0 || ft == 0 {
            warn(&mut out, true, format!("{name} needs a spawn (place kind spawn adds the fountain turret and shop)"));
        }
        if ms < l.lanes.len() {
            warn(&mut out, true, format!("{name} has {ms} minion spawns for {} lanes (one per lane, near the nexus)", l.lanes.len()));
        }
    }
    if l.lanes.is_empty() {
        warn(&mut out, true, "no lanes: minions need lane paths (set_lane)".into());
    }
    let walk_at = |p: [f32; 2]| l.cell_of(p[0], p[1]).is_some_and(|(i, j)| l.surface_at(i, j).walkable());
    for o in &l.objectives {
        if !o.kind.is_camp() && !walk_at(o.pos) {
            warn(&mut out, true, format!("{:?} id {} at ({:.0},{:.0}) stands on {:?}", o.kind, o.id, o.pos[0], o.pos[1], l.cell_of(o.pos[0], o.pos[1]).map(|(i, j)| l.surface_at(i, j))));
        }
    }
    let width = open_width(l);
    // Reachability and walking distances.
    if let (Some(bs), Some(rn), Some(rs), Some(bn)) = (spawn(1), nexus(2), spawn(2), nexus(1)) {
        let fb = walk_field(l, bs);
        let fr = walk_field(l, rs);
        let at = |f: &[f32], p: [f32; 2]| l.cell_of(p[0], p[1]).map(|(i, j)| f[j * l.nx + i]).unwrap_or(f32::INFINITY);
        let (b2r, r2b) = (at(&fb, rn), at(&fr, bn));
        if !b2r.is_finite() || !r2b.is_finite() {
            warn(&mut out, true, "the two bases are not connected by walkable ground".into());
        } else {
            out.push(format!("OK walking spawn to enemy nexus: blue {b2r:.0}, red {r2b:.0} units (~{:.0} s at 345 move speed; Summoner's Rift ~ 13000-14500)", b2r / 345.0));
            if (b2r - r2b).abs() > 0.05 * b2r.max(r2b) {
                warn(&mut out, false, format!("walking distances differ by {:.0}%: one team is closer", (b2r - r2b).abs() * 100.0 / b2r.max(r2b)));
            }
        }
        let walkable = (0..l.surface.len()).filter(|&k| Surface::from_u8(l.surface[k]).walkable()).count();
        let unreached = (0..l.surface.len()).filter(|&k| Surface::from_u8(l.surface[k]).walkable() && !fb[k].is_finite()).count();
        if unreached * 100 > walkable * 2 {
            let k = (0..l.surface.len()).find(|&k| Surface::from_u8(l.surface[k]).walkable() && !fb[k].is_finite()).unwrap_or(0);
            warn(&mut out, false, format!("{:.0}% of the walkable ground cannot be reached (e.g. near {:.0},{:.0}): open it or fill it with wall", unreached as f32 * 100.0 / walkable as f32, (k % l.nx) as f32 * CELL, (k / l.nx) as f32 * CELL));
        }
    }
    // Lanes and their turrets.
    for lp in &l.lanes {
        let len: f32 = lp.points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
        let mut blocked = 0;
        let mut narrow = f32::MAX;
        let mut samples = 0;
        for w in lp.points.windows(2) {
            let sl = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
            let n = (sl / 50.0).ceil().max(1.0) as usize;
            for k in 0..=n {
                let t = k as f32 / n as f32;
                let p = [w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t];
                samples += 1;
                match l.cell_of(p[0], p[1]) {
                    Some((i, j)) if l.surface_at(i, j).walkable() => narrow = narrow.min(width[j * l.nx + i] * 2.0),
                    _ => blocked += 1,
                }
            }
        }
        out.push(format!("Lane {:?}: {len:.0} units, narrowest {:.0} units wide{}", lp.lane, if narrow == f32::MAX { 0.0 } else { narrow }, if blocked > 0 { format!(", {:.0}% blocked", blocked as f32 * 100.0 / samples as f32) } else { String::new() }));
        if blocked > 0 {
            warn(&mut out, true, format!("{:?} lane path crosses unwalkable ground", lp.lane));
        } else if narrow < 300.0 {
            warn(&mut out, false, format!("{:?} lane narrows to {narrow:.0} units (lanes are usually 500+ wide)", lp.lane));
        }
        for t in [1u8, 2] {
            let mut along: Vec<(f32, f32, u32)> = l.objectives.iter().filter(|o| o.team == t && o.kind == ObjKind::Turret && o.lane == lp.lane && !matches!(o.tier, TurretTier::NexusA | TurretTier::NexusB)).map(|o| {
                let (dd, s) = along_lane(&lp.points, o.pos);
                (if t == 1 { s } else { len - s }, dd, o.id)
            }).collect();
            along.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            let name = if t == 1 { "blue" } else { "red" };
            if along.len() != 3 {
                warn(&mut out, along.len() < 2, format!("{name} {:?} lane has {} lane turrets (classic: 3, outer / inner / inhibitor)", lp.lane, along.len()));
            }
            for (s, dd, id) in &along {
                if *dd > 700.0 {
                    warn(&mut out, false, format!("{name} turret {id} is {dd:.0} units off the {:?} lane (turrets guard the lane: within ~500)", lp.lane));
                }
                let _ = s;
            }
            let gaps: Vec<String> = along.windows(2).map(|w| format!("{:.0}", w[1].0 - w[0].0)).collect();
            for w in along.windows(2) {
                let g = w[1].0 - w[0].0;
                if !(1200.0..=4500.0).contains(&g) {
                    warn(&mut out, false, format!("{name} {:?} turrets {} and {} are {g:.0} apart along the lane (classic: 1800-3500)", lp.lane, w[0].2, w[1].2));
                }
            }
            if let Some(last) = along.last()
                && last.0 > len * 0.48
            {
                warn(&mut out, false, format!("{name} {:?} outer turret {} is past the lane's middle", lp.lane, last.2));
            }
            if !gaps.is_empty() {
                out.push(format!("  {name} {:?} turret distances from base along the lane: {} (gaps {})", lp.lane, along.iter().map(|a| format!("{:.0}", a.0)).collect::<Vec<_>>().join(", "), gaps.join(", ")));
            }
        }
    }
    // Jungle paths.
    let walk_cells: Vec<usize> = (0..l.surface.len()).filter(|&k| Surface::from_u8(l.surface[k]).walkable()).collect();
    if !walk_cells.is_empty() {
        // Chokepoints: centre-line cells (local maxima of the open width) of passages under 160.
        let nx = l.nx;
        let ridge = |k: usize| {
            let (i, j) = (k % nx, k / nx);
            [(i.wrapping_sub(1), j), (i + 1, j), (i, j.wrapping_sub(1)), (i, j + 1)].iter().all(|&(ii, jj)| ii >= nx || jj >= l.ny || width[jj * nx + ii] <= width[k])
        };
        let tight_cells: Vec<usize> = walk_cells.iter().copied().filter(|&k| width[k] * 2.0 < 160.0 && ridge(k)).collect();
        let tight = tight_cells.len();
        let brush = l.surface.iter().filter(|&&s| s == Surface::Brush as u8).count();
        let wall = l.surface.iter().filter(|&&s| s == Surface::Wall as u8).count();
        out.push(format!("Terrain: {:.0}% walkable, {:.0}% wall, {:.0}% brush; {tight} chokepoint cells under 160 units wide (Summoner's Rift jungle paths are 250-500)", walk_cells.len() as f32 * 100.0 / l.surface.len() as f32, wall as f32 * 100.0 / l.surface.len() as f32, brush as f32 * 100.0 / l.surface.len() as f32));
        if tight > 40 {
            let k = tight_cells[tight / 2];
            warn(&mut out, false, format!("narrow chokepoints (e.g. near {:.0},{:.0}): widen jungle paths to 250+", (k % nx) as f32 * CELL, (k / nx) as f32 * CELL));
        }
        if brush == 0 {
            warn(&mut out, false, "no brush: add some near lanes and jungle entrances".into());
        }
    }
    let camps = l.objectives.iter().filter(|o| o.kind.is_camp()).count();
    out.push(format!("Jungle: {camps} camps ({} epic)", l.objectives.iter().filter(|o| o.kind.is_epic()).count()));
    // Camps: off the lanes, on open ground, reachable from their team's base through real paths.
    let mut fields: std::collections::HashMap<u8, (Vec<f32>, Vec<f32>)> = Default::default();
    for o in l.objectives.iter().filter(|o| o.kind.is_camp()) {
        let side = if o.team == 2 { 2 } else { 1 };
        let Some(home) = spawn(side) else { continue };
        let (any, wide) = fields.entry(side).or_insert_with(|| (walk_field(l, home), walk_field_min(l, home, Some((&width, 120.0)))));
        let near = |f: &[f32]| {
            let mut best = f32::INFINITY;
            let r = (260.0 / CELL) as i64;
            if let Some((ci, cj)) = l.cell_of(o.pos[0], o.pos[1]) {
                for dj in -r..=r {
                    for di in -r..=r {
                        let (i, j) = (ci as i64 + di, cj as i64 + dj);
                        if i >= 0 && j >= 0 && (i as usize) < l.nx && (j as usize) < l.ny && di * di + dj * dj <= r * r {
                            best = best.min(f[j as usize * l.nx + i as usize]);
                        }
                    }
                }
            }
            best
        };
        let lane_d = l.lanes.iter().map(|lp| along_lane(&lp.points, o.pos).0).fold(f32::MAX, f32::min);
        let name = format!("{:?} {} id {} at ({:.0},{:.0})", o.kind, match o.team { 1 => "blue", 2 => "red", _ => "neutral" }, o.id, o.pos[0], o.pos[1]);
        let clear = l.cell_of(o.pos[0], o.pos[1]).map_or(0.0, |(i, j)| width[j * l.nx + i]);
        let (da, dw) = (near(any), near(wide));
        if !walk_at(o.pos) {
            warn(&mut out, true, format!("{name} stands on unwalkable ground"));
        }
        if lane_d < 500.0 {
            warn(&mut out, true, format!("{name} is on a lane ({lane_d:.0} units from it): move it into the jungle"));
        } else if lane_d < 900.0 {
            warn(&mut out, false, format!("{name} is only {lane_d:.0} units from a lane (camps sit 900+ off lanes)"));
        }
        if !da.is_finite() {
            warn(&mut out, true, format!("{name} cannot be reached from its base"));
        } else if !dw.is_finite() {
            warn(&mut out, false, format!("{name} is only reachable through passages under 240 units wide"));
        }
        if clear < 220.0 {
            warn(&mut out, false, format!("{name} has a small clearing ({:.0} across; camps need ~450+)", clear * 2.0));
        }
        if da.is_finite() {
            out.push(format!("  {name}: {lane_d:.0} from the nearest lane, {da:.0} walk from its base, clearing {:.0} across", clear * 2.0));
        }
    }
    out.insert(0, if problems == 0 { "Playability: no blocking problems.".into() } else { format!("Playability: {problems} problem(s) to fix.") });
    out.join("\n")
}

// ------------------------------------------------------------------ tool definitions

fn tool(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({ "name": name, "description": desc, "input_schema": { "type": "object", "properties": props, "required": required } })
}

pub fn tool_defs() -> Value {
    let p2 = json!({ "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "description": "[[x, z], ...] in map units" });
    let xz = |extra: Value| {
        let mut m = json!({ "x": { "type": "number" }, "z": { "type": "number" } });
        if let (Some(a), Some(b)) = (m.as_object_mut(), extra.as_object()) {
            for (k, v) in b {
                a.insert(k.clone(), v.clone());
            }
        }
        m
    };
    let mirror = json!({ "type": "boolean", "description": "Also apply to the other team's half (default true when the map has symmetry)" });
    json!([
        tool("new_map", "Start a new empty map (all ground, a 300-unit wall all round). Replaces everything.", json!({ "name": { "type": "string" }, "width": { "type": "number", "description": "5000-24000 units (Summoner's Rift is ~14800)" }, "height": { "type": "number" }, "symmetry": { "type": "string", "enum": ["none", "diagonal", "point", "mirror_x", "mirror_z"], "description": "Competitive fairness: the red half is generated from the blue half. mirror_x = left/right, mirror_z = top/bottom, point = rotated half a turn" }, "fill": { "type": "string", "description": "Optional surface to fill the inside with, e.g. wall to carve everything out" } }), &["width", "height", "symmetry"]),
        tool("paint", "Paint walkability/terrain. ground = walkable floor; wall = jungle wall (unwalkable, dressed with trees and rocks); brush = tall grass (walkable, hides champions); river = shallow walkable water; deep_water = unwalkable water or LAVA when look.lava is on; void = off the map.", json!({ "surface": { "type": "string" }, "shape": { "type": "string", "enum": ["circle", "square", "polygon", "path", "fill"] }, "points": p2, "radius": { "type": "number", "description": "circle/square radius" }, "width": { "type": "number", "description": "path width" }, "mirror": mirror }), &["surface", "shape", "points"]),
        tool("sculpt", "Change ground height (looks and vision; walkability comes from paint). Mirrored automatically with symmetry. Give maps real relief: raised base plateaus, lanes sloping down toward the middle, sunken river / pits, rolling jungle, then smooth.", json!({ "mode": { "type": "string", "enum": ["raise", "lower", "flatten", "smooth", "noise"] }, "points": p2, "radius": { "type": "number" }, "amount": { "type": "number" }, "target": { "type": "number", "description": "flatten height" } }), &["mode", "points"]),
        tool("paint_texture", "Paint a ground texture layer (0 grass, 1 dirt, 2 stone path, 3 sand by default; see set_texture) in circles at points, or over a polygon (shape polygon). mirror false to paint one side only (e.g. a themed half).", json!({ "layer": { "type": "integer" }, "points": p2, "shape": { "type": "string", "enum": ["circles", "polygon"] }, "radius": { "type": "number" }, "strength": { "type": "number" }, "mirror": mirror }), &["layer", "points"]),
        tool("set_lane", "Define a lane's minion path from the BLUE base to the RED base (whole path) and carve it walkable with the stone-path texture.", json!({ "lane": { "type": "string", "enum": ["top", "mid", "bot"] }, "points": p2, "width": { "type": "number", "description": "carved width, default 650" }, "carve": { "type": "boolean" } }), &["lane", "points"]),
        tool("place", "Place one structure, spawn, camp or game object. With symmetry on, place the BLUE one and the red copy is made for you. kind spawn adds the fountain turret and shop. Turrets need lane and tier.", xz(json!({ "kind": { "type": "string", "enum": ["nexus", "inhibitor", "turret", "spawn", "minion_spawn", "blue_buff", "red_buff", "wolves", "wraiths", "golems", "wight", "dragon", "baron", "health_relic", "capture_point", "speed_shrine"] }, "team": { "type": "string", "enum": ["blue", "red", "neutral"] }, "lane": { "type": "string", "enum": ["top", "mid", "bot"] }, "tier": { "type": "string", "enum": ["outer", "inner", "inhibitor", "nexus_a", "nexus_b"] }, "facing": { "type": "number" }, "mirror": mirror })), &["kind", "x", "z"]),
        tool("move", "Move an object by id (its mirror copy follows).", xz(json!({ "id": { "type": "integer" } })), &["id", "x", "z"]),
        tool("remove", "Remove an object, prop, effect or light by id.", json!({ "id": { "type": "integer" } }), &["id"]),
        tool("clear_area", "Remove props and/or objects in a circle.", xz(json!({ "radius": { "type": "number" }, "what": { "type": "string", "enum": ["props", "objects", "all"] } })), &["x", "z"]),
        tool("auto_structures", "Shortcut: place the standard turrets, inhibitors and minion spawns along every lane (you can then move or replace them). Prefer placing them yourself when the layout is unusual.", json!({}), &[]),
        tool("search_props", "Find scenery models in the library (classic Map1 and Season 1 maps): trees, rocks, plants, statues & ruins, lights, props, base & walls, decals. Matches names, categories and texture names (e.g. 'frozen', 'charred', 'crystal', 'rock formations'). Anywhere a key is expected you may also write a selector \"@Map:texture[:minHeight-maxHeight]\" (e.g. \"@Season1-Map8:frozen_props_tree:300-1200\"), which picks that map's matching models.", json!({ "query": { "type": "string" }, "category": { "type": "string" }, "limit": { "type": "integer" } }), &[]),
        tool("place_prop", "Place a scenery model. walk_through: decoration only, no collision (floors, podiums, plinths units stand on).", xz(json!({ "walk_through": { "type": "boolean" }, "key": { "type": "string" }, "yaw": { "type": "number" }, "scale": { "type": "number" }, "lift": { "type": "number" }, "mirror": mirror })), &["key", "x", "z"]),
        tool("prop_row", "A row of a model along a line at an exact spacing, turned along it (fences, torches, pillars, tree lines).", json!({ "key": { "type": "string" }, "points": p2, "spacing": { "type": "number" }, "scale": { "type": "number" }, "turn": { "type": "number" }, "mirror": mirror }), &["key", "points"]),
        tool("scatter_props", "Scatter models (given keys, or a category) in a circle.", xz(json!({ "keys": { "type": "array", "items": { "type": "string" } }, "category": { "type": "string" }, "radius": { "type": "number" }, "spacing": { "type": "number" }, "min_scale": { "type": "number" }, "max_scale": { "type": "number" } })), &["x", "z"]),
        tool("scenery_wall", "Build a wall out of scenery: models (rocks, spires, trees) placed along a line at a spacing, turned along it, and the ground under the line made unwalkable. The real League way to build jungle walls, cliffs and rims.", json!({ "points": p2, "keys": { "type": "array", "items": { "type": "string" } }, "size": { "type": "array", "items": { "type": "number" }, "description": "model size range across, units" }, "spacing": { "type": "number" }, "width": { "type": "number", "description": "blocked width under the models" }, "block": { "type": "boolean" }, "mirror": mirror }), &["points", "keys"]),
        tool("scenery_fill", "Dress every painted wall area (or those inside a region polygon) with scenery models as real, editable props: edge_keys along the outlines (turned along them), fill_keys inside. Turns automatic wall dressing off. Use after painting the wall layout.", json!({ "edge_keys": { "type": "array", "items": { "type": "string" } }, "fill_keys": { "type": "array", "items": { "type": "string" } }, "edge_size": { "type": "array", "items": { "type": "number" } }, "fill_size": { "type": "array", "items": { "type": "number" } }, "edge_spacing": { "type": "number" }, "fill_spacing": { "type": "number" }, "fill_chance": { "type": "number" }, "inset": { "type": "number" }, "region": p2, "mirror": mirror, "mirror_keys": { "type": "object", "description": "model to use for the mirrored copy, e.g. {\"frost rock\": \"lava rock\"}: same positions, different art per side" } }), &[]),
        tool("copy_scenery", "Copy exact scene pieces of an example map (meshes near a point with the given textures) into one asset, e.g. Summoner's Rift's fountain shrine: map \"Map1\", center [105, 285], textures [\"shrine.dds\", \"stone_steps.dds\", \"base_stone_steps.dds\", \"shop_base.dds\"], facing [1, 1], floor 93.", json!({ "name": { "type": "string" }, "map": { "type": "string" }, "center": { "type": "array", "items": { "type": "number" } }, "radius": { "type": "number" }, "textures": { "type": "array", "items": { "type": "string" } }, "facing": { "type": "array", "items": { "type": "number" } }, "floor": { "type": "number" } }), &["name", "center", "textures"]),
        tool("fetch_model", "Download a free CC0 3D model from Poly Haven for something the library lacks (2-3 English words, e.g. 'basalt boulder', 'stone pillar', 'dead tree'); it is cut to a classic polygon budget, scaled to League size and repainted like classic Map1 art. Returns a key for place_prop.", json!({ "query": { "type": "string" }, "category": { "type": "string" } }), &["query"]),
        tool("list_textures", "List ground/cliff textures: classic Map1 textures, generated ones (gen:grass, gen:dirt, gen:path, gen:sand, gen:rock, gen:brush, gen:snow, gen:ash, gen:lava) and downloaded ones.", json!({ "query": { "type": "string" } }), &[]),
        tool("set_texture", "Use a texture for a slot: ground0..ground15 (the paintable ground layers; new ones are added), cliff (raised walls), brush, mound (top of jungle walls).", json!({ "slot": { "type": "string" }, "texture": { "type": "string" }, "label": { "type": "string" } }), &["slot", "texture"]),
        tool("fetch_texture", "Download a free CC0 ground texture from Poly Haven (e.g. 'dark volcanic rock', 'burned ground', 'cracked red ground', 'basalt'), restyled like classic Map1 art; optionally put it in a slot.", json!({ "query": { "type": "string" }, "slot": { "type": "string" }, "label": { "type": "string" } }), &["query"]),
        tool("make_texture", "Make your own ground texture by editing an existing one (seamless, 512 px): steps applied in order - {op: tint, color, amount}, {op: desaturate, amount}, {op: brightness, factor}, {op: overlay, texture (or color), mask: bright|dark|noise|all, threshold, softness, scale, amount} (e.g. snow on the high parts of dirt), {op: cracks, color, scale, width, amount} (glowing lava veins or dark cracks), {op: speckle, color, density, amount} (ash, snow flecks). Optionally put it in a slot.", json!({ "name": { "type": "string" }, "base": { "type": "string" }, "steps": { "type": "array", "items": { "type": "object" } }, "slot": { "type": "string" }, "label": { "type": "string" } }), &["name", "base", "steps"]),
        tool("effect", "Place a particle effect (fire, embers, smoke, water...). Use list_effects for names.", xz(json!({ "name": { "type": "string" }, "lift": { "type": "number" }, "mirror": mirror })), &["name", "x", "z"]),
        tool("list_effects", "Find particle effects by name.", json!({ "query": { "type": "string" } }), &[]),
        tool("light", "A coloured point light (e.g. orange glow over lava, torches).", xz(json!({ "color": { "type": "array", "items": { "type": "number" } }, "radius": { "type": "number" }, "intensity": { "type": "number" }, "lift": { "type": "number" }, "mirror": mirror })), &["x", "z"]),
        tool("look", "Map look and behaviour: sun/ambient colour (0-255), lava (deep water becomes glowing lava), wall style (league = trees and rocks on a mossy mound, raised = cliffs), heights, trees on walls, and game settings such as fog (\"terrain.inibin|Fog*enable\": \"1\", Fog*red/green/blue 0-255, Fog*opacity 0-1) or \"Gamma.inibin|Gamma*Brightness\".", json!({ "sun_color": { "type": "array", "items": { "type": "number" } }, "ambient_color": { "type": "array", "items": { "type": "number" } }, "lava": { "type": "boolean" }, "lava_from_x": { "type": "number", "description": "lava only east of this x (west stays water): one volcanic side" }, "wall_style": { "type": "string", "enum": ["league", "raised"] }, "mound": { "type": "number" }, "wall_height": { "type": "number" }, "river_depth": { "type": "number" }, "trees_on_walls": { "type": "boolean" }, "dress_walls": { "type": "boolean" }, "edge_trees": { "type": "number", "description": "0 = rocks on wall edges, 1 = trees" }, "settings": { "type": "object" }, "scenery_from": { "type": "array", "items": { "type": "string" }, "description": "map folders whose scenery search_props offers, e.g. [\"Map1\", \"Season1-Map8\"]" } }), &[]),
        tool("overview", "See the map: a character picture (north up), every object with id and position, lanes, counts and textures.", json!({}), &[]),
        tool("check", "Playability check: bases connected, walking distances and balance, lanes blocked or narrow, structures per team, turret spacing along lanes, narrow jungle passages, brush, camps. Fix every PROBLEM.", json!({}), &[]),
        tool("finish", "The map is done (call after check reports no problems). Give a short summary for the user.", json!({ "summary": { "type": "string" } }), &["summary"]),
    ])
}

pub const SYSTEM: &str = r#"You are a senior League of Legends level designer building a map in LoL Map Studio (game client 4.17, classic 2014 Summoner's Rift art) by using its editor tools yourself, step by step, like a designer at the editor.

Coordinates: x from 0 (left/west) to the map width, z from 0 (bottom/south) to the map height; north (+z) is up in the overview. The blue team (Order) usually starts at low x / low z.

Reference numbers (Summoner's Rift, Map1): map ~14800 x 14800; champions move ~345 units/s; a lane is ~13000-15000 units long and 500-900 units wide; turret range ~775; on each lane each team has 3 turrets (outer ~5500-7000 from its nexus along the lane, inner ~2500-3000 behind it, inhibitor turret ~2000-2500 behind that), an inhibitor ~600-800 behind the inhibitor turret, 2 nexus turrets ~450-600 in front of the nexus, the nexus ~1200-1600 from the spawn/fountain; one minion spawn per lane next to the nexus; jungle walls 300+ thick; jungle paths 250-500 wide; brush patches 300-600 across near lanes and jungle entrances; 4-6 camps per side (blue buff, red buff, wolves, wraiths, golems, wight) plus dragon and baron in pits off the river.

Work like this:
1. Think about the layout and gameplay first (lanes, rotations, gank paths, objectives, fairness), then new_map with symmetry for a competitive map.
2. Shape the terrain: paint walls, open the jungle paths, river or lava chasms, bases.
3. set_lane for every lane (whole path, blue to red), then place every structure yourself on the blue side (the red side is mirrored): nexus, spawn, nexus turrets, inhibitors, lane turrets with the right lane and tier, minion spawns.
4. Jungle camps, epic monsters, brush, special objects.
5. Look: textures (classic Map1 textures first; fetch_texture for what is missing), scenery (search_props; fetch_model only for things the library lacks; both are restyled to the classic look), effects and lights.
6. Call overview and check often; fix every PROBLEM and reasonable WARN, check again.
7. finish with a short summary.
Prefer many precise tool calls over shortcuts. Keep everything readable at the game camera: clear lanes, distinct jungle quadrants, landmarks."#;

// ------------------------------------------------------------------ the loop

pub struct AgentOptions {
    pub api_key: String,
    pub model: String,
    pub effort: String,
    pub max_turns: usize,
}

/// Runs Claude on the workshop until it finishes, the turn limit, or `stop`. `progress` is called
/// after every tool with a one-line note.
pub fn run(prompt: &str, image: Option<(&str, &[u8])>, opt: &AgentOptions, ws: &mut Workshop, stop: &AtomicBool, mut progress: impl FnMut(&Workshop, &str)) -> Result<String> {
    use base64::Engine;
    let mut first = Vec::new();
    if let Some((media, bytes)) = image {
        first.push(json!({ "type": "image", "source": { "type": "base64", "media_type": media, "data": base64::engine::general_purpose::STANDARD.encode(bytes) } }));
    }
    let intro = format!("{prompt}\n\n{}Build the whole map with the tools, then check it and finish.", if image.is_some() { "The attached image is the reference: follow its layout (lanes, bases, chasms, jungle, landmarks), scaled to League units. " } else { "" });
    first.push(json!({ "type": "text", "text": intro }));
    let mut messages = vec![json!({ "role": "user", "content": first })];
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(std::time::Duration::from_secs(600))).build().into();
    let tools = tool_defs();
    let mut use_fallback = true;
    for _turn in 0..opt.max_turns {
        if stop.load(Ordering::Relaxed) {
            bail!("stopped");
        }
        let mut body = json!({
            "model": opt.model,
            "max_tokens": 16000,
            "system": [{ "type": "text", "text": SYSTEM }],
            "tools": tools,
            "messages": messages,
            "output_config": { "effort": opt.effort },
            "cache_control": { "type": "ephemeral" },
        });
        let mut req = agent.post("https://api.anthropic.com/v1/messages").header("x-api-key", opt.api_key.trim()).header("anthropic-version", "2023-06-01").header("content-type", "application/json");
        if use_fallback {
            // On a refusal the API retries on a fallback model inside the same call.
            body["fallbacks"] = json!("default");
            req = req.header("anthropic-beta", "server-side-fallback-2026-07-01");
        }
        let mut resp = req.send(body.to_string()).context("could not reach the Anthropic API")?;
        let status = resp.status();
        let text_ = resp.body_mut().with_config().limit(64 * 1024 * 1024).read_to_string().context("reading the API answer")?;
        let v: Value = serde_json::from_str(&text_).with_context(|| format!("unexpected API answer: {}", text_.chars().take(300).collect::<String>()))?;
        if !status.is_success() {
            let msg = v.pointer("/error/message").and_then(|m| m.as_str()).unwrap_or(&text_).to_string();
            if use_fallback && status.as_u16() == 400 && msg.to_ascii_lowercase().contains("fallback") {
                use_fallback = false;
                continue;
            }
            bail!("the API answered {status}: {msg}");
        }
        let stop_reason = v.get("stop_reason").and_then(|s| s.as_str()).unwrap_or("");
        if stop_reason == "refusal" {
            bail!("Claude declined to continue this map");
        }
        let content = v.get("content").cloned().unwrap_or_else(|| json!([]));
        messages.push(json!({ "role": "assistant", "content": content }));
        let mut results = Vec::new();
        for b in content.as_array().into_iter().flatten() {
            match b.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = b.get("text").and_then(|t| t.as_str()).filter(|t| !t.trim().is_empty()) {
                        progress(ws, t.trim());
                    }
                }
                Some("tool_use") => {
                    let id = b.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("");
                    let input = b.get("input").cloned().unwrap_or(json!({}));
                    let r = ws.call(name, &input);
                    let (out, err) = match r {
                        Ok(s) => (s, false),
                        Err(e) => (format!("Error: {e:#}"), true),
                    };
                    progress(ws, &format!("{name}: {}", out.lines().next().unwrap_or("")));
                    results.push(json!({ "type": "tool_result", "tool_use_id": id, "content": out, "is_error": err }));
                }
                _ => {}
            }
        }
        if let Some(s) = &ws.done {
            return Ok(s.clone());
        }
        if results.is_empty() {
            if stop_reason == "max_tokens" {
                messages.push(json!({ "role": "user", "content": "Continue." }));
                continue;
            }
            // Talking without tools: remind it to keep building.
            messages.push(json!({ "role": "user", "content": "Keep building with the tools; call check, then finish when the map is complete." }));
            continue;
        }
        messages.push(json!({ "role": "user", "content": results }));
    }
    Ok(ws.done.clone().unwrap_or_else(|| "Stopped at the step limit; the map so far is open in the editor.".into()))
}

/// Runs a scripted list of tool calls ([{"tool": name, ...input}]) - offline recipes and tests.
pub fn run_script(script: &Value, ws: &mut Workshop, mut progress: impl FnMut(&Workshop, &str)) -> Result<Vec<String>> {
    let mut log = Vec::new();
    for step in script.as_array().ok_or_else(|| anyhow!("a script is a list of steps"))? {
        let name = step.get("tool").and_then(|t| t.as_str()).ok_or_else(|| anyhow!("step without \"tool\""))?;
        let line = match ws.call(name, step) {
            Ok(s) => format!("{name}: {}", s.lines().next().unwrap_or("")),
            Err(e) => format!("{name}: Error: {e:#}"),
        };
        progress(ws, &line);
        log.push(line);
    }
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_build_a_playable_two_lane_map() {
        let mut ws = Workshop::new(None, None, Vec::new());
        let script = json!([
            {"tool": "new_map", "width": 10000, "height": 6000, "symmetry": "mirror_x", "fill": "wall"},
            {"tool": "set_lane", "lane": "top", "points": [[1200, 3600], [3000, 4600], [7000, 4600], [8800, 3600]], "width": 700},
            {"tool": "set_lane", "lane": "bot", "points": [[1200, 2400], [3000, 1400], [7000, 1400], [8800, 2400]], "width": 700},
            {"tool": "paint", "surface": "ground", "shape": "circle", "points": [[1000, 3000]], "radius": 900},
            {"tool": "place", "kind": "nexus", "x": 1300, "z": 3000},
            {"tool": "place", "kind": "spawn", "x": 600, "z": 3000},
            {"tool": "place", "kind": "turret", "lane": "top", "tier": "outer", "x": 4300, "z": 4600},
            {"tool": "place", "kind": "turret", "lane": "top", "tier": "inner", "x": 2700, "z": 4450},
            {"tool": "place", "kind": "turret", "lane": "top", "tier": "inhibitor", "x": 1700, "z": 3900},
            {"tool": "place", "kind": "minion_spawn", "lane": "top", "x": 1500, "z": 3300},
            {"tool": "place", "kind": "minion_spawn", "lane": "bot", "x": 1500, "z": 2700}
        ]);
        let log = run_script(&script, &mut ws, |_, _| {}).unwrap();
        assert!(log.iter().all(|s| !s.contains("Error")), "{log:?}");
        // Mirrored for red.
        assert_eq!(ws.l.objectives.iter().filter(|o| o.kind == ObjKind::Nexus).count(), 2);
        let red = ws.l.objectives.iter().find(|o| o.kind == ObjKind::Nexus && o.team == 2).unwrap();
        assert!((red.pos[0] - 8700.0).abs() < 60.0, "{:?}", red.pos);
        let c = check(&ws.l);
        assert!(c.contains("OK walking"), "{c}");
        assert!(c.contains("top lane has 3") == false);
        let o = overview(&ws.l);
        assert!(o.contains('N') && o.contains('n') && o.contains('T'), "{o}");
    }
}
