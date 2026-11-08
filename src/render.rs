//! What the game draws, prepared for the editor's renderer: per-material batches of room.nvr /
//! room.wgeo / object meshes with their textures, shading model and blend mode, and the sun.
//!
//! Shading follows the client's HLSL (Shaders/HLSL/Environment/LIT_*.ps_2_0, BAKED_ENV_PS):
//! lit materials use `lighting = 2·sun·saturate(-L·N) + 4·vertexColor + ambient`, four-blend ground
//! lerps texture channels 0/2/4/6 by the r/g/b of the channel-1 mask (second UV set), decals and
//! wall-of-grass blend, baked WGEO meshes show their texture as it is.

use crate::formats::{bin::get_f32, bin::get_u32, dds, inibin::{Value, hash_key}, nvr::Nvr, scb::Scb, wgeo::Wgeo};
use crate::project::{Doc, MapProject};
use rayon::prelude::*;
use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    /// Vertex colour (lighting term for lit materials), RGBA 0..1.
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shading {
    Lit = 0,
    FourBlend = 1,
    Baked = 2,
    Unlit = 3,
    Water = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Blend {
    Opaque,
    AlphaTest,
    Blend,
    Premultiplied,
}

#[derive(Clone, Debug)]
pub struct Batch {
    pub vertices: Vec<RVertex>,
    pub indices: Vec<u32>,
    pub shading: Shading,
    pub blend: Blend,
    /// Texture slots: 0 diffuse (four-blend: layers 0..3 in 0..3), 4 mask / emissive.
    pub textures: [Option<usize>; 5],
    pub clamp: bool,
    pub label: String,
    pub bounds: [f32; 6],
}

pub struct Texture {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct Sun {
    pub dir: [f32; 3],
    pub color: [f32; 3],
    pub ambient: [f32; 3],
}

impl Default for Sun {
    /// The client's built-in defaults (r3dSun::LoadSun).
    fn default() -> Self {
        Self { dir: [0.3822, -0.9227, -0.0499], color: [137.0 / 255.0, 149.0 / 255.0, 180.0 / 255.0], ambient: [0x13 as f32 / 255.0, 0x5c as f32 / 255.0, 0xc2 as f32 / 255.0] }
    }
}

pub struct RenderScene {
    pub batches: Vec<Batch>,
    pub textures: Vec<Texture>,
    pub sun: Sun,
    pub bounds: [f32; 6],
    pub triangles: usize,
}

fn vec3_value(v: &Value) -> Option<[f32; 3]> {
    match v {
        Value::F32x3(a) => Some(*a),
        Value::Str(s) => {
            let n: Vec<f32> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            (n.len() >= 3).then(|| [n[0], n[1], n[2]])
        }
        Value::U8x3(a) => Some(a.map(|x| x as f32)),
        _ => None,
    }
}

pub fn sun_of(p: &MapProject) -> Sun {
    let mut s = Sun::default();
    if let Some(Doc::Inibin(ini)) = p.file("terrain.inibin").map(|f| &f.doc) {
        if let Some(d) = ini.get(hash_key("Sun", "SunDir")).and_then(vec3_value) {
            s.dir = d;
        }
        if let Some(c) = ini.get(hash_key("Sun", "SunLightColor")).and_then(vec3_value) {
            s.color = c.map(|x| x / 255.0);
        }
        if let Some(c) = ini.get(hash_key("Sun", "AmbientLightColor")).and_then(vec3_value) {
            s.ambient = c.map(|x| x / 255.0);
        }
    }
    s
}

/// Loads textures by name on demand (decoded once, shared by batches).
pub struct TextureSet<'a> {
    p: &'a MapProject,
    data: Option<&'a crate::gamedata::GameData>,
    index: HashMap<String, Option<usize>>,
    pub list: Vec<Texture>,
    pending: Vec<(String, usize)>,
}

impl<'a> TextureSet<'a> {
    pub fn new(p: &'a MapProject) -> Self {
        Self { p, data: None, index: HashMap::new(), list: Vec::new(), pending: Vec::new() }
    }

    pub fn get(&mut self, name: &str) -> Option<usize> {
        if name.is_empty() {
            return None;
        }
        let key = name.to_ascii_lowercase();
        if let Some(v) = self.index.get(&key) {
            return *v;
        }
        let found = crate::scene::find_texture(self.p, name).is_some() || self.data_texture(name).is_some();
        let v = found.then(|| {
            self.list.push(Texture { name: name.to_string(), width: 0, height: 0, rgba: Vec::new() });
            let i = self.list.len() - 1;
            self.pending.push((name.to_string(), i));
            i
        });
        self.index.insert(key, v);
        v
    }

    /// Textures the map's room.mat names but the game keeps in DATA (structure meshes).
    fn data_texture(&self, name: &str) -> Option<std::path::PathBuf> {
        let d = self.data?;
        let base = name.replace('\\', "/");
        let base = base.rsplit('/').next().unwrap_or(&base).to_ascii_lowercase();
        DATA_TEXTURES.get_or_init(|| index_data_textures(&d.root)).get(&base).cloned()
    }

    /// Decodes all requested textures (in parallel).
    pub fn finish(mut self, max_side: usize) -> Vec<Texture> {
        let p = self.p;
        let decoded: Vec<(usize, Option<dds::Image>)> = self
            .pending
            .par_iter()
            .map(|(name, i)| {
                let bytes = crate::scene::find_texture(p, name).and_then(|f| f.bytes().ok()).or_else(|| self.data_texture(name).and_then(|x| std::fs::read(x).ok()));
                (*i, bytes.and_then(|b| dds::decode(&b, max_side).ok()))
            })
            .collect();
        for (i, img) in decoded {
            if let Some(img) = img {
                self.list[i].width = img.width;
                self.list[i].height = img.height;
                self.list[i].rgba = img.pixels;
            }
        }
        self.list
    }

    /// Adds an already decoded texture.
    pub fn push(&mut self, name: &str, img: dds::Image) -> usize {
        self.list.push(Texture { name: name.into(), width: img.width, height: img.height, rgba: img.pixels });
        self.list.len() - 1
    }
}

static DATA_TEXTURES: std::sync::OnceLock<HashMap<String, std::path::PathBuf>> = std::sync::OnceLock::new();

/// File name to path of every .dds under DATA (built once, on first use).
fn index_data_textures(root: &std::path::Path) -> HashMap<String, std::path::PathBuf> {
    let mut out = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dds")) {
                out.entry(p.file_name().unwrap().to_string_lossy().to_ascii_lowercase()).or_insert(p);
            }
        }
    }
    out
}

fn color_of(v: u32) -> [f32; 4] {
    // D3DCOLOR: 0xAARRGGBB
    [((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0, ((v >> 24) & 255) as f32 / 255.0]
}

fn bounds_of(v: &[RVertex]) -> [f32; 6] {
    let mut b = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
    for x in v {
        for a in 0..3 {
            b[a] = b[a].min(x.pos[a]);
            b[a + 3] = b[a + 3].max(x.pos[a]);
        }
    }
    b
}

fn nvr_batches(n: &Nvr, tex: &mut TextureSet) -> Vec<Batch> {
    use crate::formats::nvr::material_type as mt;
    let mut by_mat: HashMap<u32, Batch> = HashMap::new();
    for m in n.meshes.iter() {
        if m.quality >= 100 {
            continue;
        }
        let mat = &n.materials[m.material as usize];
        let stride = mat.vertex_size();
        let p = m.prims[0];
        let vb = &n.vertex_buffers[p.vertex_buffer as usize];
        let b = by_mat.entry(m.material).or_insert_with(|| {
            let ch = |i: usize| mat.channels.get(i).map(|c| c.texture.as_str()).unwrap_or("");
            let (shading, textures, blend, clamp) = match mat.kind {
                mt::FOUR_BLEND => (Shading::FourBlend, [tex.get(ch(0)), tex.get(ch(2)), tex.get(ch(4)), tex.get(ch(6)), tex.get(ch(1))], Blend::Opaque, false),
                mt::DECAL => (Shading::Lit, [tex.get(ch(0)), None, None, None, tex.get(ch(1))], Blend::Blend, true),
                mt::WALL_OF_GRASS => (Shading::Lit, [tex.get(ch(0)), None, None, None, None], Blend::Blend, false),
                _ => {
                    let bg = mat.flags & (crate::formats::nvr::material_flags::BACKGROUND | crate::formats::nvr::material_flags::BK_WITH_FOG) != 0;
                    (if bg { Shading::Unlit } else { Shading::Lit }, [tex.get(ch(0)), None, None, None, tex.get(ch(1))], Blend::AlphaTest, false)
                }
            };
            Batch { vertices: Vec::new(), indices: Vec::new(), shading, blend, textures, clamp, label: mat.name.clone(), bounds: [0.0; 6] }
        });
        let base = b.vertices.len() as u32;
        for v in 0..p.vertex_count {
            let o = (p.first_vertex + v) as usize * stride;
            if o + stride > vb.len() {
                break;
            }
            let f = |k: usize| get_f32(vb, o + k);
            let (uv1, col) = match stride {
                0x2c => ([f(32), f(36)], get_u32(vb, o + 40)),
                _ => ([f(24), f(28)], get_u32(vb, o + 32)),
            };
            b.vertices.push(RVertex { pos: [f(0), f(4), f(8)], normal: [f(12), f(16), f(20)], uv0: [f(24), f(28)], uv1, color: color_of(col) });
        }
        let got = b.vertices.len() as u32 - base;
        let ib = &n.index_buffers[p.index_buffer as usize];
        let idx = |i: u32| -> u32 {
            if ib.format == crate::formats::nvr::INDEX32 {
                get_u32(&ib.data, i as usize * 4)
            } else {
                crate::formats::bin::get_u16(&ib.data, i as usize * 2) as u32
            }
        };
        let raw: Vec<u32> = (0..p.index_count).map(|i| idx(p.first_index + i)).collect();
        let rebase = raw.iter().min().is_some_and(|&m| m >= p.first_vertex);
        for t in raw.chunks_exact(3) {
            let l: Vec<u32> = t.iter().map(|&i| if rebase { i - p.first_vertex } else { i }).collect();
            if l.iter().all(|&i| i < got) {
                b.indices.extend(l.iter().map(|i| i + base));
            }
        }
    }
    let mut v: Vec<Batch> = by_mat.into_values().collect();
    for b in &mut v {
        b.bounds = bounds_of(&b.vertices);
    }
    v
}

fn wgeo_batches(w: &Wgeo, tex: &mut TextureSet) -> Vec<Batch> {
    w.meshes
        .iter()
        .map(|m| {
            let vertices: Vec<RVertex> = m.positions.iter().zip(&m.uvs).map(|(p, uv)| RVertex { pos: *p, normal: [0.0, 1.0, 0.0], uv0: *uv, uv1: *uv, color: [0.0; 4] }).collect();
            let alpha = m.material.to_ascii_lowercase().contains("alpha") || m.texture.to_ascii_lowercase().contains("alpha");
            Batch {
                bounds: bounds_of(&vertices),
                vertices,
                indices: m.indices.clone(),
                shading: Shading::Baked,
                blend: if alpha { Blend::Premultiplied } else { Blend::AlphaTest },
                textures: [tex.get(&m.texture), None, None, None, None],
                clamp: false,
                label: m.material.clone(),
            }
        })
        .collect()
}

/// room.mat sections: material name → (texture, opacity).
pub fn room_materials(text: &str) -> HashMap<String, (String, u8)> {
    let mut out = HashMap::new();
    let (mut name, mut texture, mut opacity) = (String::new(), String::new(), 255u8);
    for line in text.lines() {
        let l = line.trim();
        if l.eq_ignore_ascii_case("[MaterialBegin]") {
            name.clear();
            texture.clear();
            opacity = 255;
        } else if l.eq_ignore_ascii_case("[MaterialEnd]") {
            out.insert(name.to_ascii_lowercase(), (texture.clone(), opacity));
        } else if let Some((k, v)) = l.split_once('=') {
            match k.trim().to_ascii_lowercase().as_str() {
                "name" => name = v.trim().to_string(),
                "texture" => {
                    let t = v.trim();
                    texture = format!("{}.dds", t.rsplit_once('.').map_or(t, |x| x.0));
                }
                "opacity" => opacity = v.trim().parse().unwrap_or(255),
                _ => {}
            }
        }
    }
    out
}

fn scb_batches(s: &Scb, mats: &HashMap<String, (String, u8)>, tex: &mut TextureSet, label: &str) -> Vec<Batch> {
    let mut by: HashMap<String, Batch> = HashMap::new();
    let lake = label.to_ascii_lowercase().starts_with("lake");
    for f in &s.faces {
        let key = f.material.to_ascii_lowercase();
        let b = by.entry(key.clone()).or_insert_with(|| {
            let (t, op) = mats.get(&key).cloned().unwrap_or_default();
            Batch {
                vertices: Vec::new(),
                indices: Vec::new(),
                shading: if lake { Shading::Water } else { Shading::Lit },
                blend: if lake || op < 255 { Blend::Blend } else { Blend::AlphaTest },
                textures: [tex.get(&t), None, None, None, None],
                clamp: false,
                label: format!("{label}:{}", f.material),
                bounds: [0.0; 6],
            }
        });
        let p: Vec<[f32; 3]> = f.indices.iter().map(|&i| s.vertices.get(i as usize).copied().unwrap_or_default()).collect();
        let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
        let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
        let mut n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
        n = n.map(|x| x / l);
        let base = b.vertices.len() as u32;
        for k in 0..3 {
            b.vertices.push(RVertex { pos: p[k], normal: n, uv0: f.uv[k], uv1: f.uv[k], color: [0.0, 0.0, 0.0, 1.0] });
        }
        b.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    let mut v: Vec<Batch> = by.into_values().collect();
    for b in &mut v {
        b.bounds = bounds_of(&b.vertices);
    }
    v
}

/// Skins the turret script gives each turret (CreateChildTurret(name, skin, ...)).
pub fn script_skins(p: &MapProject) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for f in &p.files {
        if !f.path.to_ascii_lowercase().contains("createlevelprops") {
            continue;
        }
        let strings: Vec<String> = match &f.doc {
            Doc::Text(t) => t.split('"').skip(1).step_by(2).map(|s| s.to_string()).collect(),
            _ => crate::formats::luaobj::inspect(&f.original).map(|i| i.strings).unwrap_or_default(),
        };
        for w in strings.windows(2) {
            if w[0].starts_with("Turret_") && !w[1].starts_with("Turret_") && !w[1].starts_with("TEAM") {
                out.insert(w[0].to_ascii_lowercase(), w[1].clone());
            }
        }
    }
    out
}

/// Turrets, inhibitors and nexuses drawn with their character models from DATA.
fn structure_batches(p: &MapProject, data: &crate::gamedata::GameData, tex: &mut TextureSet) -> Vec<Batch> {
    let skins = script_skins(p);
    let cfg = p.text("Scene/CFG/ObjectCFG.cfg").unwrap_or("").to_string();
    let cfg_skin = |name: &str| -> Option<String> {
        let (a, b) = crate::edit::section_range(&cfg, name)?;
        cfg[a..b].lines().find_map(|l| l.trim().strip_prefix("SkinName1=").map(|s| s.trim().to_string()))
    };
    let mut items: Vec<(String, [f32; 3], f32, [f32; 3])> = Vec::new();
    if let Some(m) = p.mob() {
        for o in &m.objects {
            use crate::formats::mob::ObjectType as T;
            if matches!(o.kind, T::Turret | T::Nexus | T::Inhibitor) {
                items.push((o.name.clone(), o.position, o.pitch_yaw_roll[1], o.scale));
            }
        }
    }
    let ground = |x: f32, z: f32, y: f32| p.navgrid().map(|g| crate::create::grid_height(g, x, z)).unwrap_or(y);
    for e in p.dsc_entries() {
        let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0).to_string();
        let l = stem.to_ascii_lowercase();
        if !(l.starts_with("turret") || l.starts_with("hq_") || l.starts_with("barracks_t")) || items.iter().any(|i| i.0.eq_ignore_ascii_case(&stem)) {
            continue;
        }
        let c = p.files.iter().filter(|f| f.stem().eq_ignore_ascii_case(&stem)).find_map(|f| match &f.doc {
            Doc::Scb(s) => Some(s.central),
            Doc::Sco(s) => Some(s.central),
            _ => None,
        });
        if let Some(c) = c {
            items.push((stem, [c[0], ground(c[0], c[2], c[1]), c[2]], 0.0, [1.0; 3]));
        }
    }
    let mut out = Vec::new();
    let mut tex_of: HashMap<String, Option<usize>> = HashMap::new();
    // Jungle monsters are drawn animated by the views (gui::models), not baked in here.
    let units = items.into_iter().map(|(n, p, y, s)| (n, p, y, s, false));
    for (name, pos, yaw, scale, monster) in units {
        let (skin, model) = if monster {
            let Some(m) = data.posed_model(&name) else { continue };
            (name.clone(), m)
        } else {
            let Some(skin) = crate::gamedata::structure_skin(&name, cfg_skin(&name).as_deref(), skins.get(&name.to_ascii_lowercase()).map(|s| s.as_str())) else { continue };
            let Some(model) = data.model(&skin) else { continue };
            (skin, model)
        };
        let t = *tex_of.entry(skin.to_ascii_lowercase()).or_insert_with(|| model.texture.as_ref().map(|img| tex.push(&skin, dds::Image { width: img.width, height: img.height, pixels: img.pixels.clone() })));
        let (s, c) = yaw.to_radians().sin_cos();
        let k = model.scale;
        let vertices: Vec<RVertex> = model
            .mesh
            .positions
            .iter()
            .zip(&model.mesh.normals)
            .zip(&model.mesh.uvs)
            .map(|((v, n), uv)| {
                let (x, y, z) = (v[0] * k * scale[0], v[1] * k * scale[1], v[2] * k * scale[2]);
                RVertex { pos: [pos[0] + c * x + s * z, pos[1] + y, pos[2] - s * x + c * z], normal: [c * n[0] + s * n[2], n[1], -s * n[0] + c * n[2]], uv0: *uv, uv1: *uv, color: [0.0, 0.0, 0.0, 1.0] }
            })
            .collect();
        out.push(Batch { bounds: bounds_of(&vertices), vertices, indices: model.mesh.indices.clone(), shading: Shading::Lit, blend: Blend::AlphaTest, textures: [t, None, None, None, None], clamp: false, label: format!("{name} ({skin})") });
    }
    out
}

/// Everything drawable in a map: environment geometry plus room.dsc object meshes, and with
/// the game's DATA folder, the structures' character models.
pub fn build(p: &MapProject, max_texture: usize, data: Option<&crate::gamedata::GameData>) -> RenderScene {
    let mut tex = TextureSet::new(p);
    tex.data = data;
    let mut batches = Vec::new();
    let baked = p.scene_format() == Some(crate::project::SceneFormat::Baked);
    match (baked, p.file("Scene/room.wgeo").map(|f| &f.doc), p.file("Scene/room.nvr").map(|f| &f.doc)) {
        (true, Some(Doc::Wgeo(w)), _) => batches.extend(wgeo_batches(w, &mut tex)),
        (_, _, Some(Doc::Nvr(n))) => batches.extend(nvr_batches(n, &mut tex)),
        (_, Some(Doc::Wgeo(w)), _) => batches.extend(wgeo_batches(w, &mut tex)),
        _ => {}
    }
    let mats = p.text("Scene/room.mat").map(room_materials).unwrap_or_default();
    for e in p.dsc_entries() {
        let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0).to_string();
        if stem.eq_ignore_ascii_case("AIPath") {
            continue;
        }
        let scb = p.files.iter().find(|f| f.path.to_ascii_lowercase().starts_with("scene/") && f.stem().eq_ignore_ascii_case(&stem) && matches!(f.doc, Doc::Scb(_)));
        // Shop and structure .SCB files are invisible proxies in game; only lakes are drawn.
        if !stem.to_ascii_lowercase().starts_with("lake") {
            continue;
        }
        if let Some(MapFile { doc: Doc::Scb(s), .. }) = scb {
            batches.extend(scb_batches(s, &mats, &mut tex, &stem));
        }
    }
    if let Some(d) = data {
        batches.extend(structure_batches(p, d, &mut tex));
    }
    batches.retain(|b| !b.indices.is_empty());
    let textures = tex.finish(max_texture);
    // Baked meshes draw blended when their texture has transparent texels (as the game sorts them).
    for b in &mut batches {
        if b.shading == Shading::Baked {
            let transparent = b.textures[0].and_then(|i| textures.get(i)).is_some_and(|t| t.rgba.chunks_exact(4).any(|px| px[3] < 250));
            b.blend = if transparent { Blend::Premultiplied } else { Blend::Opaque };
        }
    }
    let mut bounds = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
    let mut triangles = 0;
    for b in &batches {
        triangles += b.indices.len() / 3;
        for a in 0..3 {
            bounds[a] = bounds[a].min(b.bounds[a]);
            bounds[a + 3] = bounds[a + 3].max(b.bounds[a + 3]);
        }
    }
    RenderScene { batches, textures, sun: sun_of(p), bounds, triangles }
}

use crate::project::MapFile;
