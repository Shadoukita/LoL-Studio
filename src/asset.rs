//! User assets: models built or imported in the Model Studio. Each lives in the asset library
//! (`%APPDATA%/LoL Map Studio/assets/<name>/`: `asset.json` plus game-ready DDS textures) and
//! can be placed on maps as scenery (library key `asset:<name>`).

use crate::formats::{
    dds::{self, Image},
    nvr::{Geometry, Material, material_type},
};
use crate::meshops::Mesh;
use crate::props::{PropDef, PropPart};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn white() -> [f32; 4] {
    [1.0; 4]
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetPart {
    pub name: String,
    pub mesh: Mesh,
    /// Texture name (a key of the asset's textures).
    #[serde(default)]
    pub texture: Option<String>,
    /// Tint multiplied into the texture (or the colour of an untextured part).
    #[serde(default = "white")]
    pub color: [f32; 4],
    #[serde(default = "yes")]
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    /// Scenery category it is listed under ("My assets" when empty).
    #[serde(default)]
    pub category: String,
    pub parts: Vec<AssetPart>,
    /// Units walk around it.
    #[serde(default)]
    pub blocks: bool,
    /// Where it came from (a file, a map prop, a character).
    #[serde(default)]
    pub source: String,
}

impl Asset {
    pub fn new(name: &str) -> Self {
        Self { name: name.into(), category: String::new(), parts: Vec::new(), blocks: false, source: String::new() }
    }

    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in self.parts.iter().filter(|p| p.visible) {
            for v in &p.mesh.positions {
                for a in 0..3 {
                    lo[a] = lo[a].min(v[a]);
                    hi[a] = hi[a].max(v[a]);
                }
            }
        }
        if lo[0] > hi[0] { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
    }

    pub fn stats(&self) -> (usize, usize) {
        self.parts.iter().fold((0, 0), |(v, t), p| (v + p.mesh.positions.len(), t + p.mesh.indices.len() / 3))
    }
}

/// The user's asset library folder.
pub fn library_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("LoL Map Studio").join("assets")
}

/// A file-system-safe version of an asset name.
pub fn safe_name(n: &str) -> String {
    let s: String = n.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if s.is_empty() { "asset".into() } else { s }
}

/// The asset's folder. Names that only differ in punctuation or case get their own folder
/// (a short hash of the exact name), so saving one never overwrites another.
pub fn asset_dir(name: &str) -> PathBuf {
    let plain = library_dir().join(safe_name(name));
    let owner = std::fs::read_to_string(plain.join("asset.json")).ok().and_then(|t| serde_json::from_str::<Asset>(&t).ok()).map(|a| a.name);
    match owner {
        Some(n) if n != name => {
            let h = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
            library_dir().join(format!("{}_{:08x}", safe_name(name), h))
        }
        _ => plain,
    }
}

/// The texture file the game uses for a part (tint baked in; untextured parts get a solid colour).
pub fn part_texture_file(a: &Asset, i: usize) -> String {
    let p = &a.parts[i];
    let base = safe_name(&a.name);
    match &p.texture {
        Some(t) if p.color == [1.0; 4] => format!("{}_{}.dds", base, safe_name(t)),
        Some(t) => format!("{}_{}_tint{}.dds", base, safe_name(t), i),
        None => format!("{}_solid{}.dds", base, i),
    }
}

fn tinted(img: &Image, c: [f32; 4]) -> Image {
    let mut out = img.clone();
    for px in out.pixels.chunks_exact_mut(4) {
        for k in 0..4 {
            px[k] = (px[k] as f32 * c[k]).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn solid(c: [f32; 4]) -> Image {
    let px = [(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8, (c[3] * 255.0) as u8];
    Image { width: 4, height: 4, pixels: px.repeat(16) }
}

/// Saves an asset and its textures; returns its folder. Source textures are kept as PNG for
/// editing; the game textures (DDS, tints baked) are written next to them.
pub fn save(a: &Asset, textures: &HashMap<String, Image>) -> Result<PathBuf> {
    if let Some(p) = a.parts.iter().find(|p| p.texture.as_ref().is_some_and(|t| !textures.contains_key(t))) {
        anyhow::bail!("the texture of part \"{}\" is missing; load it again (Load image…) or remove it before saving", p.name);
    }
    let dir = asset_dir(&a.name);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // Everything is written first; files no longer used are removed afterwards.
    let mut keep = std::collections::HashSet::new();
    for (name, img) in textures {
        if a.parts.iter().any(|p| p.texture.as_deref() == Some(name.as_str())) {
            let file = format!("src_{}.png", safe_name(name));
            image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.pixels.clone()).context("bad texture")?.save(dir.join(&file))?;
            keep.insert(file.to_ascii_lowercase());
        }
    }
    for (i, p) in a.parts.iter().enumerate() {
        let img = match p.texture.as_ref().and_then(|t| textures.get(t)) {
            Some(t) if p.color == [1.0; 4] => t.clone(),
            Some(t) => tinted(t, p.color),
            None => solid(p.color),
        };
        let file = part_texture_file(a, i);
        std::fs::write(dir.join(&file), dds::encode_argb(&img, true))?;
        keep.insert(file.to_ascii_lowercase());
    }
    std::fs::write(dir.join("asset.json"), serde_json::to_string(a)?)?;
    for e in std::fs::read_dir(&dir)?.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dds") || x.eq_ignore_ascii_case("png")) && !keep.contains(&name) {
            let _ = std::fs::remove_file(p);
        }
    }
    Ok(dir)
}

/// Loads an asset folder: the asset and its source textures.
pub fn load(dir: &Path) -> Result<(Asset, HashMap<String, Image>)> {
    let text = std::fs::read_to_string(dir.join("asset.json")).with_context(|| format!("no asset in {}", dir.display()))?;
    let a: Asset = serde_json::from_str(&text)?;
    let mut textures = HashMap::new();
    for p in &a.parts {
        let Some(t) = &p.texture else { continue };
        if textures.contains_key(t) {
            continue;
        }
        let path = dir.join(format!("src_{}.png", safe_name(t)));
        if let Ok(img) = image::open(&path) {
            let rgba = img.to_rgba8();
            textures.insert(t.clone(), Image { width: rgba.width() as usize, height: rgba.height() as usize, pixels: rgba.into_raw() });
        }
    }
    Ok((a, textures))
}

/// The assets in the library (name, folder), sorted by name.
pub fn list() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(library_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("asset.json").is_file())
        .filter_map(|p| {
            let text = std::fs::read_to_string(p.join("asset.json")).ok()?;
            let a: Asset = serde_json::from_str(&text).ok()?;
            Some((a.name, p))
        })
        .collect();
    out.sort_by_key(|x| x.0.to_ascii_lowercase());
    out
}

pub fn mesh_to_geometry(m: &Mesh) -> Geometry {
    let n = m.positions.len();
    Geometry {
        positions: m.positions.clone(),
        normals: if m.normals.len() == n { m.normals.clone() } else { vec![[0.0, 1.0, 0.0]; n] },
        uvs: if m.uvs.len() == n { m.uvs.clone() } else { vec![[0.0, 0.0]; n] },
        uvs1: if m.uvs.len() == n { m.uvs.clone() } else { vec![[0.0, 0.0]; n] },
        // A little ambient light, like the map's own props.
        colors: vec![0xff101010; n],
        indices: m.indices.clone(),
    }
}

/// The asset as a scenery prop (its folder holds the game textures).
pub fn to_propdef(a: &Asset, dir: &Path) -> PropDef {
    let mut parts = Vec::new();
    for (i, p) in a.parts.iter().enumerate() {
        if !p.visible || p.mesh.indices.is_empty() {
            continue;
        }
        let mat = Material::new(&format!("asset_{}_{}", safe_name(&a.name), i), &part_texture_file(a, i), material_type::DEFAULT, 0x10);
        parts.push(PropPart { material: mat, geo: mesh_to_geometry(&p.mesh) });
    }
    let (lo, hi) = a.bounds();
    let mut d = PropDef {
        key: format!("asset:{}", a.name),
        name: a.name.clone(),
        category: if a.category.is_empty() { "My assets".into() } else { a.category.clone() },
        parts,
        texture_dir: dir.to_path_buf(),
        radius: ((hi[0] - lo[0]).max(hi[2] - lo[2]) / 2.0).max(1.0),
        height: hi[1] - lo[1],
        copies: 1,
        thumb: Vec::new(),
        solid: Some(a.blocks),
    };
    let mut colors = HashMap::new();
    for (i, p) in a.parts.iter().enumerate() {
        let c = p.color;
        colors.insert(part_texture_file(a, i).to_ascii_lowercase(), [(c[0] * 200.0) as u8, (c[1] * 200.0) as u8, (c[2] * 200.0) as u8]);
        if let Ok(b) = std::fs::read(dir.join(part_texture_file(a, i)))
            && let Some(avg) = dds::average_color(&b)
        {
            colors.insert(part_texture_file(a, i).to_ascii_lowercase(), avg);
        }
    }
    d.thumb = crate::props::thumbnail(&d, &colors);
    d
}

/// Every library asset as a scenery prop.
pub fn library_props() -> Vec<PropDef> {
    list().into_iter().filter_map(|(_, dir)| load(&dir).ok().map(|(a, _)| to_propdef(&a, &dir))).collect()
}
