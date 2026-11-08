//! Scenery props for custom maps: trees, rocks, statues and the like, taken from the example
//! maps' room.nvr (each object is a connected piece of geometry), plus OBJ models you import.

use crate::formats::{dds, nvr::{Geometry, Material, Nvr, material_flags, material_type}};
use crate::layout::CustomProp;
use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct PropPart {
    pub material: Material,
    /// Local coordinates: centred on X/Z, standing on Y = 0.
    pub geo: Geometry,
}

#[derive(Clone)]
pub struct PropDef {
    pub key: String,
    pub name: String,
    pub category: String,
    pub parts: Vec<PropPart>,
    /// Folder the textures come from, and their file names.
    pub texture_dir: PathBuf,
    pub radius: f32,
    pub height: f32,
    /// How many copies the source map has.
    pub copies: usize,
    /// 48x48 RGBA preview.
    pub thumb: Vec<u8>,
    /// Set by assets: units walk around it (Some(true)) or through it (Some(false)).
    pub solid: Option<bool>,
}

impl PropDef {
    /// Whether units walk around it (trees, rocks, statues) or through it (plants, decorations).
    pub fn blocks(&self) -> bool {
        if let Some(s) = self.solid {
            return s;
        }
        self.radius > 45.0 && matches!(self.category.as_str(), "Trees" | "Rocks" | "Statues & ruins")
    }

    /// Walls and buildings block walking under their tall geometry, not as a circle.
    pub fn blocks_footprint(&self) -> bool {
        self.category == "Base & walls" && self.height > 60.0
    }

    pub fn is_decal(&self) -> bool {
        self.category == "Decals"
    }
}

#[derive(Default, Clone)]
pub struct PropLibrary {
    pub props: Vec<PropDef>,
}

impl PropLibrary {
    pub fn get(&self, key: &str) -> Option<&PropDef> {
        self.props.iter().find(|p| p.key == key)
    }

    pub fn categories(&self) -> Vec<String> {
        let mut c: Vec<String> = self.props.iter().map(|p| p.category.clone()).collect();
        c.sort();
        c.dedup();
        c
    }

    /// Whether a prop may be used on a map drawing from `maps` (empty = every map). The
    /// user's own models ("custom:", "asset:") are always allowed.
    pub fn allowed(key: &str, maps: &[String]) -> bool {
        if maps.is_empty() || key.starts_with("custom:") || key.starts_with("asset:") {
            return true;
        }
        let src = key.split('/').next().unwrap_or("");
        maps.iter().any(|m| m.eq_ignore_ascii_case(src))
    }

    /// The maps the library's scenery comes from (folder names).
    pub fn sources(&self) -> Vec<String> {
        let mut v: Vec<String> = self.props.iter().filter(|p| !p.key.starts_with("custom:") && !p.key.starts_with("asset:")).filter_map(|p| p.key.split_once('/').map(|x| x.0.to_string())).collect();
        v.sort_by_key(|a| a.to_ascii_lowercase());
        v.dedup();
        v
    }

    /// Big rocks and cliff pieces, the stones that line jungle walls (classic Summoner's Rift).
    pub fn default_wall_rocks(&self) -> Vec<String> {
        self.default_wall_rocks_in(&crate::layout::classic_assets())
    }

    /// The same, from the given maps' scenery (falls back to classic Summoner's Rift).
    pub fn default_wall_rocks_in(&self, maps: &[String]) -> Vec<String> {
        let pick = |maps: &[String]| {
            let mut v: Vec<&PropDef> = self.props.iter().filter(|p| p.category == "Rocks" && p.height > 80.0 && (70.0..450.0).contains(&p.radius) && !p.key.starts_with("custom:") && !p.key.starts_with("asset:") && Self::allowed(&p.key, maps)).collect();
            v.sort_by(|a, b| (b.copies as f32 * b.radius).partial_cmp(&(a.copies as f32 * a.radius)).unwrap());
            v.into_iter().take(8).map(|p| p.key.clone()).collect::<Vec<String>>()
        };
        let v = pick(maps);
        if v.is_empty() { pick(&crate::layout::classic_assets()) } else { v }
    }

    /// The tree-like props, most used first (classic Summoner's Rift).
    pub fn default_forest(&self) -> Vec<String> {
        self.default_forest_in(&crate::layout::classic_assets())
    }

    /// The same, from the given maps' scenery (falls back to classic Summoner's Rift).
    pub fn default_forest_in(&self, maps: &[String]) -> Vec<String> {
        let pick = |maps: &[String]| {
            let mut v: Vec<&PropDef> = self.props.iter().filter(|p| p.category == "Trees" && p.height > 250.0 && p.radius < 420.0 && !p.key.starts_with("custom:") && !p.key.starts_with("asset:") && Self::allowed(&p.key, maps)).collect();
            v.sort_by(|a, b| b.copies.cmp(&a.copies));
            v.into_iter().take(4).map(|p| p.key.clone()).collect::<Vec<String>>()
        };
        let v = pick(maps);
        if v.is_empty() { pick(&crate::layout::classic_assets()) } else { v }
    }

    /// Adds the user's imported props (replacing older versions of them).
    /// Replaces the asset-library props (key "asset:<name>").
    pub fn set_assets(&mut self, defs: Vec<PropDef>) {
        self.props.retain(|p| !p.key.starts_with("asset:"));
        self.props.extend(defs);
    }

    pub fn set_custom(&mut self, custom: &[CustomProp]) -> Vec<String> {
        self.props.retain(|p| !p.key.starts_with("custom:"));
        let mut errors = Vec::new();
        for c in custom {
            match load_custom(c) {
                Ok(p) => self.props.push(p),
                Err(e) => errors.push(format!("{}: {e:#}", c.name)),
            }
        }
        errors
    }
}

/// Widest scenery piece taken from a map (Summoner's Rift's base walls are about 5000 units).
const MAX_EXTENT: f32 = 9000.0;

fn category_of(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    let has = |k: &[&str]| k.iter().any(|x| n.contains(x));
    if has(&["creep_camp", "camp_base"]) {
        "Jungle"
    } else if has(&["pantheon", "gate", "shrine", "house", "wall", "steps", "shop_base", "shop_trol", "base_", "platform", "bridge", "river_corner"]) {
        "Base & walls"
    } else if has(&["tree", "leaves", "leaf", "foliage", "palm", "pine", "trunk"]) {
        "Trees"
    } else if has(&["rock", "stone", "henge", "boulder", "cliff", "pebble"]) {
        "Rocks"
    } else if has(&["mushroom", "flower", "plant", "bush", "grass", "vine", "fern", "weed"]) {
        "Plants & flowers"
    } else if has(&["torch", "fire", "brazier", "lamp", "cauldron", "candle"]) {
        "Lights"
    } else if has(&["statue", "shrine", "pillar", "column", "ruin", "wall", "gate", "pantheon", "arch", "tomb"]) {
        "Statues & ruins"
    } else if has(&["fence", "log", "stump", "barrel", "wagon", "sign", "crate", "pole", "helmet", "shield", "book", "ladder", "house", "shop", "bridge"]) {
        "Props"
    } else {
        "Other"
    }
}

fn clean_name(n: &str) -> String {
    let mut s = n.trim_end_matches(|c: char| c == '_' || c.is_ascii_digit()).replace('_', " ");
    if s.trim().is_empty() {
        s = n.to_string();
    }
    s.trim().to_string()
}

struct Uf(Vec<u32>);
impl Uf {
    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            self.0[x as usize] = self.0[self.0[x as usize] as usize];
            x = self.0[x as usize];
        }
        x
    }
    fn union(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a as usize] = b;
        }
    }
}

struct Piece {
    material: usize,
    geo: Geometry,
    bbox: [f32; 6],
}

/// Props of one map's room.nvr. `label` prefixes their keys ("Map1").
pub fn extract(map_dir: &Path, label: &str) -> Result<Vec<PropDef>> {
    let data = std::fs::read(map_dir.join("Scene").join("room.nvr")).context("no Scene/room.nvr")?;
    let nvr = Nvr::parse(&data)?;
    let tex_dir = ["Scene/Textures", "Scene/textures"].iter().map(|d| map_dir.join(d)).find(|d| d.is_dir()).unwrap_or_else(|| map_dir.join("Scene"));
    let tex_files: HashMap<String, PathBuf> = std::fs::read_dir(&tex_dir).into_iter().flatten().flatten().map(|e| (e.file_name().to_string_lossy().to_ascii_lowercase(), e.path())).collect();
    let scenery: Vec<usize> = (0..nvr.materials.len())
        .filter(|&i| {
            let m = &nvr.materials[i];
            (m.kind == material_type::DEFAULT || m.kind == material_type::DECAL) && m.flags & (material_flags::GROUND | material_flags::BACKGROUND | material_flags::BK_WITH_FOG) == 0
        })
        .collect();
    // Each material's category (from its name and texture); decals are their own.
    let mat_cat: Vec<&'static str> = nvr
        .materials
        .iter()
        .map(|m| if m.kind == material_type::DECAL { "Decals" } else { category_of(&format!("{} {}", m.name, m.channels[0].texture)) })
        .collect();
    // Connected pieces per material (vertices joined by position across mesh chunks).
    let pieces: Vec<Piece> = scenery
        .par_iter()
        .flat_map(|&mi| {
            let mut all = Geometry::default();
            for (k, mesh) in nvr.meshes.iter().enumerate() {
                if mesh.material as usize != mi || mesh.quality >= 100 {
                    continue;
                }
                let g = nvr.mesh_geometry(k);
                let base = all.positions.len() as u32;
                all.positions.extend(&g.positions);
                all.normals.extend(&g.normals);
                all.uvs.extend(&g.uvs);
                all.colors.extend(&g.colors);
                all.indices.extend(g.indices.iter().map(|i| i + base));
            }
            let n = all.positions.len();
            let mut uf = Uf((0..n as u32).collect());
            let mut at: HashMap<[i32; 3], u32> = HashMap::new();
            for (i, p) in all.positions.iter().enumerate() {
                let key = p.map(|v| (v * 2.0).round() as i32);
                match at.get(&key) {
                    Some(&j) => uf.union(i as u32, j),
                    None => {
                        at.insert(key, i as u32);
                    }
                }
            }
            for t in all.indices.chunks_exact(3) {
                uf.union(t[0], t[1]);
                uf.union(t[1], t[2]);
            }
            let mut groups: HashMap<u32, Vec<u32>> = HashMap::new();
            for t in all.indices.chunks_exact(3) {
                groups.entry(uf.find(t[0])).or_default().extend_from_slice(t);
            }
            groups
                .into_values()
                .filter(|idx| idx.len() >= 12)
                .map(|idx| {
                    let geo = crate::formats::nvr::subset(&all, &idx);
                    let (bbox, _) = crate::formats::nvr::bounds(&geo.positions);
                    Piece { material: mi, geo, bbox }
                })
                .filter(|p| {
                    let ext = (p.bbox[3] - p.bbox[0]).max(p.bbox[5] - p.bbox[2]);
                    // Decals lie flat; whole base walls can be thousands of units long.
                    let flat_ok = matches!(mat_cat[p.material], "Decals" | "Jungle" | "Base & walls");
                    (20.0..MAX_EXTENT).contains(&ext) && (p.bbox[4] - p.bbox[1] > 5.0 || flat_ok)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    // Pieces of different materials that overlap form one object (a trunk and its leaves).
    let n = pieces.len();
    let mut uf = Uf((0..n as u32).collect());
    let cell = 300.0;
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, p) in pieces.iter().enumerate() {
        let c = ((p.bbox[0] + p.bbox[3]) / 2.0, (p.bbox[2] + p.bbox[5]) / 2.0);
        grid.entry(((c.0 / cell) as i32, (c.1 / cell) as i32)).or_default().push(i);
    }
    let big = |p: &Piece| (p.bbox[3] - p.bbox[0]).max(p.bbox[5] - p.bbox[2]) > 2600.0;
    let decal = |p: &Piece| nvr.materials[p.material].kind == material_type::DECAL;
    for (i, a) in pieces.iter().enumerate() {
        // Large structures and decals stay on their own (their boxes cover many small props).
        if big(a) || decal(a) {
            continue;
        }
        let c = ((a.bbox[0] + a.bbox[3]) / 2.0, (a.bbox[2] + a.bbox[5]) / 2.0);
        let (gx, gz) = ((c.0 / cell) as i32, (c.1 / cell) as i32);
        for dz in -1..=1 {
            for dx in -1..=1 {
                for &j in grid.get(&(gx + dx, gz + dz)).into_iter().flatten() {
                    if j <= i {
                        continue;
                    }
                    let b = &pieces[j];
                    // Only pieces of the same kind join (a trunk and its leaves, a gate and its wall),
                    // so trees and torches standing by a wall stay separate props.
                    if big(b) || decal(b) || mat_cat[a.material] != mat_cat[b.material] {
                        continue;
                    }
                    let ix = (a.bbox[3].min(b.bbox[3]) - a.bbox[0].max(b.bbox[0])).max(0.0);
                    let iz = (a.bbox[5].min(b.bbox[5]) - a.bbox[2].max(b.bbox[2])).max(0.0);
                    let iy = a.bbox[4].min(b.bbox[4]) - a.bbox[1].max(b.bbox[1]);
                    let area = |p: &Piece| ((p.bbox[3] - p.bbox[0]) * (p.bbox[5] - p.bbox[2])).max(1.0);
                    if iy > -10.0 && ix * iz > 0.35 * area(a).min(area(b)) {
                        uf.union(i as u32, j as u32);
                    }
                }
            }
        }
    }
    // Ordered maps, so prop names ("lambert #12") come out the same on every load.
    let mut objects: std::collections::BTreeMap<u32, Vec<usize>> = Default::default();
    for i in 0..n {
        objects.entry(uf.find(i as u32)).or_default().push(i);
    }
    // One prop per distinct shape, counting identical copies.
    let mut by_sig: std::collections::BTreeMap<String, (Vec<usize>, usize)> = Default::default();
    let mut groups: Vec<Vec<usize>> = objects.into_values().collect();
    groups.sort_by_key(|m| m[0]);
    for members in groups {
        let mut b = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
        let mut tris = 0;
        let mut mats: Vec<usize> = Vec::new();
        for &m in &members {
            let p = &pieces[m];
            for a in 0..3 {
                b[a] = b[a].min(p.bbox[a]);
                b[a + 3] = b[a + 3].max(p.bbox[a + 3]);
            }
            tris += p.geo.indices.len() / 3;
            mats.push(p.material);
        }
        mats.sort();
        mats.dedup();
        let ext = (b[3] - b[0]).max(b[5] - b[2]);
        let structural = members.iter().all(|&m| mat_cat[pieces[m].material] == "Base & walls");
        // Small broken-off bits of walls are not useful on their own.
        if ext > MAX_EXTENT || tris < 8 || (structural && ext < 80.0 && b[4] - b[1] < 120.0) {
            continue;
        }
        // Near-identical copies (same materials, similar size and detail) count as one prop.
        let sig = format!("{:?}|{}|{}|{}", mats, (tris as f32).log2().round(), ((b[3] - b[0]) / 90.0).round(), ((b[4] - b[1]) / 90.0).round());
        let e = by_sig.entry(sig).or_insert((members, 0));
        e.1 += 1;
    }
    let mut defs: Vec<PropDef> = by_sig
        .into_values()
        .map(|(members, copies)| {
            let mut b = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
            for &m in &members {
                for a in 0..3 {
                    b[a] = b[a].min(pieces[m].bbox[a]);
                    b[a + 3] = b[a + 3].max(pieces[m].bbox[a + 3]);
                }
            }
            let (cx, cz, y0) = ((b[0] + b[3]) / 2.0, (b[2] + b[5]) / 2.0, b[1]);
            let mut parts: Vec<PropPart> = Vec::new();
            for &m in &members {
                let p = &pieces[m];
                let mut g = p.geo.clone();
                for v in &mut g.positions {
                    *v = [v[0] - cx, v[1] - y0, v[2] - cz];
                }
                let mat = nvr.materials[p.material].clone();
                match parts.iter_mut().find(|q| q.material.name == mat.name) {
                    Some(q) => {
                        let base = q.geo.positions.len() as u32;
                        q.geo.positions.extend(&g.positions);
                        q.geo.normals.extend(&g.normals);
                        q.geo.uvs.extend(&g.uvs);
                        q.geo.colors.extend(&g.colors);
                        q.geo.indices.extend(g.indices.iter().map(|i| i + base));
                    }
                    None => parts.push(PropPart { material: mat, geo: g }),
                }
            }
            // The biggest part names the prop.
            parts.sort_by_key(|p| std::cmp::Reverse(p.geo.indices.len()));
            let main = &parts[0].material;
            let label_src = format!("{} {}", main.name, main.channels[0].texture);
            let is_decal = main.kind == material_type::DECAL;
            let radius = ((b[3] - b[0]).max(b[5] - b[2])) / 2.0;
            PropDef {
                key: String::new(),
                name: clean_name(&main.name),
                category: if is_decal { "Decals".to_string() } else { category_of(&label_src).to_string() },
                parts,
                texture_dir: tex_dir.clone(),
                radius,
                height: b[4] - b[1],
                copies,
                thumb: Vec::new(),
                solid: None,
            }
        })
        .collect();
    defs.sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)).then(b.copies.cmp(&a.copies)));
    let mut seen: HashMap<String, usize> = HashMap::new();
    for d in &mut defs {
        let n = seen.entry(d.name.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            d.name = format!("{} #{n}", d.name);
        }
        d.key = format!("{label}/{}", d.name);
    }
    let colors: HashMap<String, [u8; 3]> = defs
        .iter()
        .flat_map(|d| d.parts.iter().map(|p| p.material.channels[0].texture.to_ascii_lowercase()))
        .collect::<std::collections::HashSet<_>>()
        .into_par_iter()
        .map(|t| {
            let c = tex_files.get(&t).and_then(|p| std::fs::read(p).ok()).and_then(|b| dds::average_color(&b)).unwrap_or([128, 128, 128]);
            (t, c)
        })
        .collect();
    defs.par_iter_mut().for_each(|d| d.thumb = thumbnail(d, &colors));
    Ok(defs)
}

/// A small three-quarter view of the prop, flat-shaded with its textures' colours.
pub fn thumbnail(d: &PropDef, colors: &HashMap<String, [u8; 3]>) -> Vec<u8> {
    const S: usize = 48;
    let mut px = vec![0u8; S * S * 4];
    let mut z = vec![f32::MIN; S * S];
    let extent = d.radius.max(d.height * 0.6).max(1.0) * 1.15;
    let (sa, ca) = (0.6f32.sin(), 0.6f32.cos());
    let proj = |p: [f32; 3]| -> [f32; 3] {
        let x = p[0] * ca - p[2] * sa;
        let depth = p[0] * sa + p[2] * ca;
        let y = p[1] * 0.9 + depth * 0.35;
        [(x / extent * 0.5 + 0.5) * S as f32, (1.0 - (y - d.height * 0.45) / extent * 0.5 - 0.5) * S as f32, -depth + p[1] * 0.3]
    };
    for part in &d.parts {
        let col = colors.get(&part.material.channels[0].texture.to_ascii_lowercase()).copied().unwrap_or([150, 150, 150]);
        for t in part.geo.indices.chunks_exact(3) {
            let w: Vec<[f32; 3]> = t.iter().map(|&i| part.geo.positions[i as usize]).collect();
            let e1 = [w[1][0] - w[0][0], w[1][1] - w[0][1], w[1][2] - w[0][2]];
            let e2 = [w[2][0] - w[0][0], w[2][1] - w[0][1], w[2][2] - w[0][2]];
            let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
            let shade = (0.55 + 0.45 * (n[1] / l).abs()).min(1.0);
            let p: Vec<[f32; 3]> = w.iter().map(|&v| proj(v)).collect();
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
            if area.abs() < 1e-6 {
                continue;
            }
            let (x0, x1) = (p.iter().map(|v| v[0]).fold(f32::MAX, f32::min).max(0.0) as usize, (p.iter().map(|v| v[0]).fold(f32::MIN, f32::max).ceil() as usize).min(S - 1));
            let (y0, y1) = (p.iter().map(|v| v[1]).fold(f32::MAX, f32::min).max(0.0) as usize, (p.iter().map(|v| v[1]).fold(f32::MIN, f32::max).ceil() as usize).min(S - 1));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((p[1][0] - fx) * (p[2][1] - fy) - (p[2][0] - fx) * (p[1][1] - fy)) / area;
                    let w1 = ((p[2][0] - fx) * (p[0][1] - fy) - (p[0][0] - fx) * (p[2][1] - fy)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let dz = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
                    let k = y * S + x;
                    if dz > z[k] {
                        z[k] = dz;
                        for a in 0..3 {
                            px[k * 4 + a] = (col[a] as f32 * shade * 1.3).min(255.0) as u8;
                        }
                        px[k * 4 + 3] = 255;
                    }
                }
            }
        }
    }
    px
}

/// A prop from an OBJ file and a texture.
pub fn load_custom(c: &CustomProp) -> Result<PropDef> {
    let text = std::fs::read_to_string(&c.obj).with_context(|| format!("reading {}", c.obj))?;
    let mut g = crate::scene::read_obj(&text)?;
    crate::scene::place_geometry(&mut g, c.scale, 0.0, [0.0, 0.0, 0.0]);
    if g.positions.is_empty() {
        bail!("the model has no vertices");
    }
    let (b, _) = crate::formats::nvr::bounds(&g.positions);
    let tex_name = crate::layout_build::resolve_name(&c.texture);
    let tex_dir = Path::new(&c.texture).parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let mat = Material::new(&format!("lms_prop_{}", c.name.replace(' ', "_")), &tex_name, material_type::DEFAULT, 0x10);
    let mut d = PropDef {
        key: format!("custom:{}", c.name),
        name: c.name.clone(),
        category: "My props".into(),
        parts: vec![PropPart { material: mat, geo: g }],
        texture_dir: tex_dir,
        radius: ((b[3] - b[0]).max(b[5] - b[2])) / 2.0,
        height: b[4] - b[1],
        copies: 1,
        thumb: Vec::new(),
        solid: None,
    };
    let col = crate::layout_build::resolve_texture(&c.texture).ok().and_then(|(_, bytes)| dds::average_color(&bytes)).unwrap_or([150, 150, 150]);
    let mut colors = HashMap::new();
    colors.insert(tex_name.to_ascii_lowercase(), col);
    d.thumb = thumbnail(&d, &colors);
    Ok(d)
}

/// The library from every example map with a room.nvr.
pub fn library_from(dirs: &[PathBuf]) -> PropLibrary {
    let mut lib = PropLibrary::default();
    for (d, label) in map_folders(dirs) {
        if let Ok(mut v) = extract(&d, &label) {
            // Older maps name textures .tga while the files are .dds: use the file that exists.
            let files: std::collections::HashMap<String, String> = std::fs::read_dir(d.join("Scene").join("Textures")).into_iter().flatten().flatten().map(|e| { let n = e.file_name().to_string_lossy().to_string(); (n.to_ascii_lowercase(), n) }).collect();
            for def in &mut v {
                for part in &mut def.parts {
                    for ch in &mut part.material.channels {
                        let low = ch.texture.to_ascii_lowercase();
                        if ch.texture.is_empty() || files.contains_key(&low) {
                            continue;
                        }
                        let stem = low.rsplit_once('.').map_or(low.as_str(), |x| x.0).to_string();
                        if let Some(real) = files.get(&format!("{stem}.dds")) {
                            ch.texture = real.clone();
                        }
                    }
                }
            }
            lib.props.append(&mut v);
        }
    }
    lib
}

/// Map folders with their labels: each folder that holds a map, and the maps inside collection
/// folders (Example Maps/Season1/Map8 is "Season1-Map8").
pub fn map_folders(dirs: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let is_map = |d: &std::path::Path| d.join("Scene").join("room.nvr").is_file();
    let name = |d: &std::path::Path| d.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut out = Vec::new();
    for d in dirs {
        if is_map(d) {
            out.push((d.clone(), name(d)));
        } else if d.is_dir() {
            let mut subs: Vec<PathBuf> = std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| is_map(p)).collect();
            subs.sort();
            for sub in subs {
                let label = format!("{}-{}", name(d), name(&sub));
                out.push((sub, label));
            }
        }
    }
    out
}

/// The texture files a set of props needs: (file name, bytes).
pub fn prop_textures(defs: &[&PropDef]) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    for d in defs {
        for p in &d.parts {
            for ch in &p.material.channels {
                if ch.texture.is_empty() || out.iter().any(|(n, _)| n.eq_ignore_ascii_case(&ch.texture)) {
                    continue;
                }
                let path = d.texture_dir.join(&ch.texture);
                let bytes = std::fs::read(&path).ok().or_else(|| {
                    let lower = ch.texture.to_ascii_lowercase();
                    std::fs::read_dir(&d.texture_dir).ok()?.flatten().find(|e| e.file_name().to_string_lossy().to_ascii_lowercase() == lower).and_then(|e| std::fs::read(e.path()).ok())
                });
                if let Some(b) = bytes {
                    out.push((ch.texture.clone(), b));
                }
            }
        }
    }
    out
}


/// Where a prop's model vertex (and normal) ends up relative to its placement point: stretch and
/// size, tilt around X then Z, then the turn around Y (the editor's convention).
pub fn place_vertex(v: [f32; 3], n: [f32; 3], yaw: f32, scale: f32, tilt: [f32; 2], stretch: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let s = [stretch[0] * scale, stretch[1] * scale, stretch[2] * scale];
    let mut p = [v[0] * s[0], v[1] * s[1], v[2] * s[2]];
    // Normals use the inverse stretch so they stay perpendicular.
    let mut q = [n[0] / s[0].abs().max(1e-6), n[1] / s[1].abs().max(1e-6), n[2] / s[2].abs().max(1e-6)];
    let rot_x = |a: [f32; 3], d: f32| {
        let (sn, cs) = d.to_radians().sin_cos();
        [a[0], a[1] * cs - a[2] * sn, a[1] * sn + a[2] * cs]
    };
    let rot_z = |a: [f32; 3], d: f32| {
        let (sn, cs) = d.to_radians().sin_cos();
        [a[0] * cs - a[1] * sn, a[0] * sn + a[1] * cs, a[2]]
    };
    if tilt != [0.0, 0.0] {
        p = rot_z(rot_x(p, tilt[0]), tilt[1]);
        q = rot_z(rot_x(q, tilt[0]), tilt[1]);
    }
    let (sn, cs) = yaw.to_radians().sin_cos();
    let p = [cs * p[0] - sn * p[2], p[1], sn * p[0] + cs * p[2]];
    let q = [cs * q[0] - sn * q[2], q[1], sn * q[0] + cs * q[2]];
    let l = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt().max(1e-6);
    (p, [q[0] / l, q[1] / l, q[2] / l])
}
