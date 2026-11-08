//! Importing 3D models from common interchange formats (OBJ + MTL, glTF 2.0 / GLB, STL, PLY,
//! binary FBX 7.x) and from League of Legends' own meshes (`.skn`, `.scb`, `.sco`) into a plain,
//! renderer-friendly form: one [`ImportedPart`] per material, each with its own vertices,
//! triangles, base colour and (optional) RGBA texture.
//!
//! # Conventions
//!
//! * **Up axis:** Y stays up; nothing is rotated.
//! * **Handedness:** glTF, OBJ, STL, PLY and FBX are right-handed, League is left-handed. Models
//!   from those formats are **mirrored on X**: the x of every position and normal is negated and
//!   every triangle's winding is flipped, so the model looks as it did in the authoring tool once
//!   placed in League. League's own `.skn`, `.scb` and `.sco` are used as they are (not mirrored).
//! * **UVs** use the DirectX / League convention (v = 0 at the top row of the image). OBJ, PLY
//!   and FBX store v from the bottom, so their v is flipped; glTF and League UVs are kept.
//! * **Units:** never rescaled (the caller chooses a scale). If the model's bounding box is under
//!   5 or over 20000 units, a warning suggesting a scale is added to [`ImportedModel::warnings`].
//! * **Normals** are always present: taken from the file when it has them, otherwise computed
//!   (smooth, area weighted, shared between corners at the same position; STL gets flat normals).
//! * **Textures** are decoded to RGBA and downscaled to at most 2048 px on their longest side.

use crate::formats::dds::{self, Image};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// One mesh part of an imported model (one material).
#[derive(Clone, Debug)]
pub struct ImportedPart {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>, // same length as positions; computed (smooth) if the file has none
    pub uvs: Vec<[f32; 2]>,     // same length; zeros if none
    pub indices: Vec<u32>,      // triangles
    pub texture: Option<ImportedTexture>,
    /// Material base colour (multiplies the texture), RGBA 0..1.
    pub color: [f32; 4],
}

#[derive(Clone)]
pub struct ImportedTexture {
    /// File stem, e.g. "rock_diffuse".
    pub name: String,
    /// RGBA8.
    pub image: Image,
}

impl fmt::Debug for ImportedTexture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportedTexture").field("name", &self.name).field("width", &self.image.width).field("height", &self.image.height).finish()
    }
}

#[derive(Clone, Debug)]
pub struct ImportedModel {
    pub name: String,
    pub parts: Vec<ImportedPart>,
    pub warnings: Vec<String>,
}

const MAX_TEXTURE: usize = 2048;

/// File extensions [`import_file`] accepts (lowercase, without the dot).
pub fn supported_extensions() -> &'static [&'static str] {
    &["obj", "gltf", "glb", "stl", "ply", "fbx", "skn", "scb", "sco"]
}

/// Imports a model file, choosing the reader by its extension (case-insensitive).
/// See the module documentation for the coordinate conventions applied.
pub fn import_file(path: &Path) -> Result<ImportedModel> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("model").to_string();
    let mut warnings = Vec::new();
    let (parts, mirror) = match ext.as_str() {
        "obj" => (import_obj(path, &mut warnings)?, true),
        "gltf" | "glb" => (import_gltf(path, &mut warnings)?, true),
        "stl" => (import_stl(path, &name)?, true),
        "ply" => (import_ply(path, &name, &mut warnings)?, true),
        "fbx" => (import_fbx(path, &mut warnings)?, true),
        "skn" => (import_skn(path, &name, &mut warnings)?, false),
        "scb" => (import_scb(path, &mut warnings)?, false),
        "sco" => (import_sco(path, &mut warnings)?, false),
        "dds" | "tga" | "png" | "jpg" | "jpeg" | "bmp" => bail!("{} is an image, not a 3D model", path.display()),
        "" => bail!("{} has no file extension; supported: {}", path.display(), supported_extensions().join(", ")),
        _ => bail!(".{ext} models are not supported; supported: {}", supported_extensions().join(", ")),
    };
    finish_model(name, parts, warnings, mirror)
}

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

fn new_part(name: &str) -> ImportedPart {
    ImportedPart { name: name.to_string(), positions: Vec::new(), normals: Vec::new(), uvs: Vec::new(), indices: Vec::new(), texture: None, color: [1.0; 4] }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len3(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = len3(a);
    (l > 1e-12 && l.is_finite()).then(|| [a[0] / l, a[1] / l, a[2] / l])
}
fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    cross(sub(b, a), sub(c, a))
}

/// Validates a part, applies the X mirror if asked, fills in missing normals/UVs.
fn finish_part(p: &mut ImportedPart, mirror: bool) {
    let n = p.positions.len();
    p.indices.truncate(p.indices.len() / 3 * 3);
    // Drop triangles that point past the vertex list.
    if p.indices.iter().any(|&i| i as usize >= n) {
        let tris: Vec<u32> = p.indices.chunks(3).filter(|t| t.iter().all(|&i| (i as usize) < n)).flatten().copied().collect();
        p.indices = tris;
    }
    p.uvs.resize(n, [0.0; 2]);
    p.normals.resize(n, [0.0; 3]);
    if mirror {
        for v in &mut p.positions {
            v[0] = -v[0];
        }
        for v in &mut p.normals {
            v[0] = -v[0];
        }
        for t in p.indices.chunks_mut(3) {
            t.swap(1, 2);
        }
    }
    let mut missing = false;
    for v in &mut p.normals {
        match normalize(*v) {
            Some(u) => *v = u,
            None => {
                *v = [0.0; 3];
                missing = true;
            }
        }
    }
    if missing {
        // Smooth normals, shared by all corners at the same position (so UV seams don't show).
        let key = |v: [f32; 3]| (v[0].to_bits(), v[1].to_bits(), v[2].to_bits());
        let mut acc: HashMap<(u32, u32, u32), [f32; 3]> = HashMap::new();
        for t in p.indices.chunks(3) {
            let (a, b, c) = (p.positions[t[0] as usize], p.positions[t[1] as usize], p.positions[t[2] as usize]);
            let f = face_normal(a, b, c);
            for v in [a, b, c] {
                let e = acc.entry(key(v)).or_insert([0.0; 3]);
                e[0] += f[0];
                e[1] += f[1];
                e[2] += f[2];
            }
        }
        for (i, nrm) in p.normals.iter_mut().enumerate() {
            if *nrm == [0.0; 3] {
                *nrm = acc.get(&key(p.positions[i])).and_then(|&s| normalize(s)).unwrap_or([0.0, 1.0, 0.0]);
            }
        }
    }
}

fn finish_model(name: String, parts: Vec<ImportedPart>, mut warnings: Vec<String>, mirror: bool) -> Result<ImportedModel> {
    let mut out = Vec::new();
    for mut p in parts {
        finish_part(&mut p, mirror);
        if !p.indices.is_empty() {
            out.push(p);
        }
    }
    if out.is_empty() {
        bail!("the model has no triangles");
    }
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &out {
        for &i in &p.indices {
            let v = p.positions[i as usize];
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
    }
    let size = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max);
    if size < 5.0 {
        warnings.push(format!(
            "the model is only {size:.3} units across (League units are about 1 cm; a champion is ~200 tall): try a scale of about {:.0}",
            (200.0 / size.max(1e-6)).max(1.0)
        ));
    } else if size > 20000.0 {
        warnings.push(format!("the model is {size:.0} units across, larger than a whole map: try a scale of about {:.4}", 1000.0 / size));
    }
    Ok(ImportedModel { name, parts: out, warnings })
}

/// Fits an image into `MAX_TEXTURE` px (Triangle filter).
fn limit_size(img: Image) -> Image {
    let m = img.width.max(img.height);
    if m <= MAX_TEXTURE || img.width == 0 || img.height == 0 {
        return img;
    }
    let s = MAX_TEXTURE as f64 / m as f64;
    let (nw, nh) = (((img.width as f64 * s).round() as u32).max(1), ((img.height as f64 * s).round() as u32).max(1));
    let Some(src) = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.pixels.clone()) else {
        return img;
    };
    let r = image::imageops::resize(&src, nw, nh, image::imageops::FilterType::Triangle);
    Image { width: nw as usize, height: nh as usize, pixels: r.into_raw() }
}

fn rgba_image(i: image::RgbaImage) -> Image {
    Image { width: i.width() as usize, height: i.height() as usize, pixels: i.into_raw() }
}

/// Decodes an image file's bytes (DDS, PNG, JPEG, TGA, BMP), `ext` hinting the format.
fn decode_image(bytes: &[u8], ext: &str) -> Result<Image> {
    let ext = ext.to_ascii_lowercase();
    let img = if ext == "dds" || bytes.starts_with(b"DDS ") {
        dds::decode(bytes, MAX_TEXTURE)?
    } else {
        let decoded = match image::ImageFormat::from_extension(&ext) {
            Some(f) => image::load_from_memory_with_format(bytes, f).or_else(|_| image::load_from_memory(bytes)),
            None => image::load_from_memory(bytes),
        };
        rgba_image(decoded.context("could not decode the image")?.to_rgba8())
    };
    Ok(limit_size(img))
}

fn stem_of(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("texture").to_string()
}

fn load_texture(path: &Path) -> Result<ImportedTexture> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let image = decode_image(&bytes, ext).with_context(|| format!("decoding {}", path.display()))?;
    Ok(ImportedTexture { name: stem_of(path), image })
}

/// Texture loader that decodes each file once and records failures as warnings.
#[derive(Default)]
struct TextureCache {
    map: HashMap<PathBuf, Option<ImportedTexture>>,
}

impl TextureCache {
    fn get(&mut self, path: &Path, warnings: &mut Vec<String>) -> Option<ImportedTexture> {
        self.map
            .entry(path.to_path_buf())
            .or_insert_with(|| match load_texture(path) {
                Ok(t) => Some(t),
                Err(e) => {
                    warnings.push(format!("texture {}: {e:#}", path.display()));
                    None
                }
            })
            .clone()
    }
}

/// Case-insensitive lookup of `name` among the files of `dir`.
fn find_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let lower = name.to_ascii_lowercase();
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_ascii_lowercase() == lower))
}

/// Resolves a texture path written in a model file (relative to `base`, maybe absolute, maybe
/// with backslashes, maybe pointing to a folder that no longer exists): tries the path as is,
/// then the bare file name next to the model and in a `textures` subfolder.
fn resolve_file(base: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.trim().trim_matches('"').replace('\\', "/");
    if rel.is_empty() {
        return None;
    }
    let p = Path::new(&rel);
    if p.is_absolute() && p.is_file() {
        return Some(p.to_path_buf());
    }
    let joined = base.join(p);
    if joined.is_file() {
        return Some(joined);
    }
    let file = p.file_name()?.to_str()?.to_string();
    for dir in [base.to_path_buf(), base.join("textures"), base.join("Textures"), base.join("tex")] {
        if let Some(f) = find_in_dir(&dir, &file) {
            return Some(f);
        }
    }
    None
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

// ---------------------------------------------------------------------------------------------
// Small 4x4 matrix helpers (row-major, column vectors: p' = M * p)
// ---------------------------------------------------------------------------------------------

type M4 = [[f64; 4]; 4];

fn m_ident() -> M4 {
    let mut m = [[0.0; 4]; 4];
    for (i, r) in m.iter_mut().enumerate() {
        r[i] = 1.0;
    }
    m
}
fn m_mul(a: &M4, b: &M4) -> M4 {
    let mut m = [[0.0; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            m[r][c] = (0..4).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    m
}
fn m_point(m: &M4, p: [f32; 3]) -> [f32; 3] {
    let v = [p[0] as f64, p[1] as f64, p[2] as f64];
    let mut o = [0.0f32; 3];
    for (r, out) in o.iter_mut().enumerate() {
        *out = (m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2] + m[r][3]) as f32;
    }
    o
}
fn m_det3(m: &M4) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}
/// Inverse transpose of the upper 3x3 (for normals), as a 4x4 without translation.
fn m_normal(m: &M4) -> M4 {
    let d = m_det3(m);
    let d = if d.abs() < 1e-30 { 1.0 } else { d };
    let a = |r: usize, c: usize| m[r][c];
    // cofactor matrix / det = inverse transpose
    let mut n = m_ident();
    n[0][0] = (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) / d;
    n[0][1] = -(a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0)) / d;
    n[0][2] = (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0)) / d;
    n[1][0] = -(a(0, 1) * a(2, 2) - a(0, 2) * a(2, 1)) / d;
    n[1][1] = (a(0, 0) * a(2, 2) - a(0, 2) * a(2, 0)) / d;
    n[1][2] = -(a(0, 0) * a(2, 1) - a(0, 1) * a(2, 0)) / d;
    n[2][0] = (a(0, 1) * a(1, 2) - a(0, 2) * a(1, 1)) / d;
    n[2][1] = -(a(0, 0) * a(1, 2) - a(0, 2) * a(1, 0)) / d;
    n[2][2] = (a(0, 0) * a(1, 1) - a(0, 1) * a(1, 0)) / d;
    n
}
fn m_dir(m: &M4, v: [f32; 3]) -> [f32; 3] {
    let mut o = [0.0f32; 3];
    for (r, out) in o.iter_mut().enumerate() {
        *out = (m[r][0] * v[0] as f64 + m[r][1] * v[1] as f64 + m[r][2] * v[2] as f64) as f32;
    }
    o
}
fn m_translate(t: [f64; 3]) -> M4 {
    let mut m = m_ident();
    m[0][3] = t[0];
    m[1][3] = t[1];
    m[2][3] = t[2];
    m
}
fn m_scale(s: [f64; 3]) -> M4 {
    let mut m = m_ident();
    m[0][0] = s[0];
    m[1][1] = s[1];
    m[2][2] = s[2];
    m
}
/// FBX Euler rotation (degrees, XYZ order: X applied first).
fn m_euler_xyz(r: [f64; 3]) -> M4 {
    let (sx, cx) = r[0].to_radians().sin_cos();
    let (sy, cy) = r[1].to_radians().sin_cos();
    let (sz, cz) = r[2].to_radians().sin_cos();
    let mut rx = m_ident();
    rx[1][1] = cx;
    rx[1][2] = -sx;
    rx[2][1] = sx;
    rx[2][2] = cx;
    let mut ry = m_ident();
    ry[0][0] = cy;
    ry[0][2] = sy;
    ry[2][0] = -sy;
    ry[2][2] = cy;
    let mut rz = m_ident();
    rz[0][0] = cz;
    rz[0][1] = -sz;
    rz[1][0] = sz;
    rz[1][1] = cz;
    m_mul(&rz, &m_mul(&ry, &rx))
}

// ---------------------------------------------------------------------------------------------
// OBJ + MTL
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Mtl {
    kd: [f32; 3],
    alpha: f32,
    map_kd: Option<String>,
}

impl Default for Mtl {
    fn default() -> Self {
        Self { kd: [1.0; 3], alpha: 1.0, map_kd: None }
    }
}

/// The file name of a `map_Kd` line, skipping its options (`-s 1 1 1`, `-bm 0.5`, `-clamp on`...).
fn mtl_map_path(rest: &str) -> String {
    let toks: Vec<&str> = rest.split_whitespace().collect();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if t.starts_with('-') && t.parse::<f64>().is_err() && t.len() > 1 {
            i += 1;
            while i + 1 < toks.len() && (toks[i].parse::<f64>().is_ok() || matches!(toks[i], "on" | "off" | "r" | "g" | "b" | "m" | "l" | "z")) {
                i += 1;
            }
            continue;
        }
        break;
    }
    toks[i.min(toks.len())..].join(" ")
}

fn parse_mtl(text: &str, out: &mut HashMap<String, Mtl>) {
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let nums = || rest.split_whitespace().filter_map(|s| s.parse::<f32>().ok()).collect::<Vec<_>>();
        match key.to_ascii_lowercase().as_str() {
            "newmtl" => {
                cur = Some(rest.to_string());
                out.insert(rest.to_string(), Mtl::default());
            }
            "kd" => {
                let n = nums();
                if let (Some(c), 3..) = (&cur, n.len()) {
                    out.get_mut(c).unwrap().kd = [n[0], n[1], n[2]];
                }
            }
            "d" => {
                if let (Some(c), Some(&a)) = (&cur, nums().first()) {
                    out.get_mut(c).unwrap().alpha = a.clamp(0.0, 1.0);
                }
            }
            "tr" => {
                if let (Some(c), Some(&a)) = (&cur, nums().first()) {
                    out.get_mut(c).unwrap().alpha = (1.0 - a).clamp(0.0, 1.0);
                }
            }
            "map_kd" => {
                if let Some(c) = &cur {
                    let p = mtl_map_path(rest);
                    if !p.is_empty() {
                        out.get_mut(c).unwrap().map_kd = Some(p);
                    }
                }
            }
            _ => {}
        }
    }
}

fn import_obj(path: &Path, warnings: &mut Vec<String>) -> Result<Vec<ImportedPart>> {
    let text = read_text(path)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut mtls: HashMap<String, Mtl> = HashMap::new();
    let (mut v, mut vt, mut vn): (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>) = (Vec::new(), Vec::new(), Vec::new());
    let mut parts: Vec<(ImportedPart, HashMap<(i64, i64, i64), u32>)> = Vec::new();
    let mut by_mat: HashMap<String, usize> = HashMap::new();
    let mut cur_mat = String::new();
    let mut cur_obj = String::new();
    let mut bad_faces = 0usize;
    // Joins lines continued with a trailing backslash.
    let mut lines: Vec<String> = Vec::new();
    let mut pending = String::new();
    for l in text.lines() {
        if let Some(s) = l.strip_suffix('\\') {
            pending.push_str(s);
            pending.push(' ');
        } else {
            pending.push_str(l);
            lines.push(std::mem::take(&mut pending));
        }
    }
    if !pending.is_empty() {
        lines.push(pending);
    }
    for line in &lines {
        let line = line.trim();
        let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let floats = || rest.split_whitespace().map(|s| s.parse::<f32>().unwrap_or(0.0)).collect::<Vec<_>>();
        match key {
            "v" => {
                let c = floats();
                if c.len() >= 3 {
                    v.push([c[0], c[1], c[2]]);
                }
            }
            "vt" => {
                let c = floats();
                vt.push([c.first().copied().unwrap_or(0.0), 1.0 - c.get(1).copied().unwrap_or(0.0)]);
            }
            "vn" => {
                let c = floats();
                vn.push(if c.len() >= 3 { [c[0], c[1], c[2]] } else { [0.0; 3] });
            }
            "mtllib" => {
                let mut found = false;
                let names: Vec<String> = std::iter::once(rest.to_string()).chain(rest.split_whitespace().map(str::to_string)).collect();
                for n in names {
                    if let Some(p) = resolve_file(dir, &n) {
                        if let Ok(t) = read_text(&p) {
                            parse_mtl(&t, &mut mtls);
                            found = true;
                            if n == rest {
                                break;
                            }
                        }
                    }
                }
                if !found {
                    warnings.push(format!("material library {rest} not found next to the OBJ"));
                }
            }
            "usemtl" => cur_mat = rest.to_string(),
            "o" | "g" => cur_obj = rest.to_string(),
            "f" => {
                let pi = *by_mat.entry(cur_mat.clone()).or_insert_with(|| {
                    let name = if !cur_mat.is_empty() { cur_mat.clone() } else if !cur_obj.is_empty() { cur_obj.clone() } else { "default".to_string() };
                    parts.push((new_part(&name), HashMap::new()));
                    parts.len() - 1
                });
                let (part, map) = &mut parts[pi];
                let resolve = |s: &str, n: usize| -> Option<i64> {
                    if s.is_empty() {
                        return None;
                    }
                    let i: i64 = s.parse().ok()?;
                    let r = if i < 0 { n as i64 + i } else { i - 1 };
                    (r >= 0 && (r as usize) < n).then_some(r)
                };
                let mut poly = Vec::new();
                let mut ok = true;
                for corner in rest.split_whitespace() {
                    let mut it = corner.split('/');
                    let Some(pv) = resolve(it.next().unwrap_or(""), v.len()) else {
                        ok = false;
                        break;
                    };
                    let pt = resolve(it.next().unwrap_or(""), vt.len()).unwrap_or(-1);
                    let pn = resolve(it.next().unwrap_or(""), vn.len()).unwrap_or(-1);
                    let id = *map.entry((pv, pt, pn)).or_insert_with(|| {
                        part.positions.push(v[pv as usize]);
                        part.uvs.push(if pt >= 0 { vt[pt as usize] } else { [0.0; 2] });
                        part.normals.push(if pn >= 0 { vn[pn as usize] } else { [0.0; 3] });
                        (part.positions.len() - 1) as u32
                    });
                    poly.push(id);
                }
                if !ok || poly.len() < 3 {
                    bad_faces += 1;
                    continue;
                }
                for k in 1..poly.len() - 1 {
                    part.indices.extend_from_slice(&[poly[0], poly[k], poly[k + 1]]);
                }
            }
            _ => {}
        }
    }
    if bad_faces > 0 {
        warnings.push(format!("{bad_faces} OBJ faces were skipped (missing vertices or fewer than 3 corners)"));
    }
    let mut cache = TextureCache::default();
    let mut out = Vec::new();
    for (mut part, _) in parts {
        let mat_name = part.name.clone();
        if let Some(m) = mtls.get(&mat_name) {
            part.color = [m.kd[0], m.kd[1], m.kd[2], m.alpha];
            if let Some(t) = &m.map_kd {
                match resolve_file(dir, t) {
                    Some(p) => part.texture = cache.get(&p, warnings),
                    None => warnings.push(format!("texture {t} of material {mat_name} not found")),
                }
                // A texture usually comes with Kd = (0.8, 0.8, 0.8) or black in exporters; the
                // texture is the colour then.
                if part.texture.is_some() && m.kd.iter().all(|&c| c <= 0.0) {
                    part.color = [1.0, 1.0, 1.0, m.alpha];
                }
            }
        }
        out.push(part);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// glTF 2.0
// ---------------------------------------------------------------------------------------------

fn gltf_image_to_rgba(d: &gltf::image::Data) -> Option<Image> {
    use gltf::image::Format as F;
    let (w, h) = (d.width as usize, d.height as usize);
    let n = w * h;
    let p = &d.pixels;
    let u16_at = |i: usize| u16::from_le_bytes([p[i * 2], p[i * 2 + 1]]);
    let f32_at = |i: usize| f32::from_le_bytes([p[i * 4], p[i * 4 + 1], p[i * 4 + 2], p[i * 4 + 3]]);
    let (ch, kind) = match d.format {
        F::R8 => (1, 0),
        F::R8G8 => (2, 0),
        F::R8G8B8 => (3, 0),
        F::R8G8B8A8 => (4, 0),
        F::R16 => (1, 1),
        F::R16G16 => (2, 1),
        F::R16G16B16 => (3, 1),
        F::R16G16B16A16 => (4, 1),
        F::R32G32B32FLOAT => (3, 2),
        F::R32G32B32A32FLOAT => (4, 2),
    };
    let bytes = [1, 2, 4][kind];
    if p.len() < n * ch * bytes {
        return None;
    }
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        let get = |c: usize| -> u8 {
            let k = i * ch + c;
            match kind {
                0 => p[k],
                1 => (u16_at(k) >> 8) as u8,
                _ => (f32_at(k).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            }
        };
        let px = match ch {
            1 => [get(0), get(0), get(0), 255],
            2 => [get(0), get(0), get(0), get(1)],
            3 => [get(0), get(1), get(2), 255],
            _ => [get(0), get(1), get(2), get(3)],
        };
        out.extend_from_slice(&px);
    }
    Some(Image { width: w, height: h, pixels: out })
}

fn import_gltf(path: &Path, warnings: &mut Vec<String>) -> Result<Vec<ImportedPart>> {
    let (doc, buffers, images) = gltf::import(path).with_context(|| format!("reading glTF {}", path.display()))?;
    let mut textures: HashMap<usize, Option<ImportedTexture>> = HashMap::new();
    let mut parts = Vec::new();
    let model_stem = stem_of(path);

    let mut stack: Vec<(gltf::Node, M4)> = Vec::new();
    match doc.default_scene().or_else(|| doc.scenes().next()) {
        Some(scene) => stack.extend(scene.nodes().map(|n| (n, m_ident()))),
        None => {
            // No scene: take every root node.
            let mut has_parent = vec![false; doc.nodes().len()];
            for n in doc.nodes() {
                for c in n.children() {
                    has_parent[c.index()] = true;
                }
            }
            stack.extend(doc.nodes().filter(|n| !has_parent[n.index()]).map(|n| (n, m_ident())));
        }
    }
    let mut visited = 0usize;
    while let Some((node, parent)) = stack.pop() {
        visited += 1;
        if visited > 100_000 {
            bail!("the glTF node hierarchy is too deep or cyclic");
        }
        let cm = node.transform().matrix(); // column-major
        let mut local = [[0.0f64; 4]; 4];
        for (c, col) in cm.iter().enumerate() {
            for (r, &v) in col.iter().enumerate() {
                local[r][c] = v as f64;
            }
        }
        let world = m_mul(&parent, &local);
        for child in node.children() {
            stack.push((child, world));
        }
        let Some(mesh) = node.mesh() else { continue };
        let nm = m_normal(&world);
        let flip = m_det3(&world) < 0.0;
        let mesh_name = mesh.name().map(str::to_string).or_else(|| node.name().map(str::to_string)).unwrap_or_else(|| format!("mesh{}", mesh.index()));
        for (pi, prim) in mesh.primitives().enumerate() {
            use gltf::mesh::Mode;
            let mode = prim.mode();
            if !matches!(mode, Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan) {
                warnings.push(format!("{mesh_name}: skipped a {mode:?} primitive (only triangles are imported)"));
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(pos) = reader.read_positions() else { continue };
            let positions: Vec<[f32; 3]> = pos.map(|p| m_point(&world, p)).collect();
            let normals: Vec<[f32; 3]> = reader.read_normals().map(|it| it.map(|n| m_dir(&nm, n)).collect()).unwrap_or_default();
            let material = prim.material();
            let pbr = material.pbr_metallic_roughness();
            let tex_info = pbr.base_color_texture();
            let uv_set = tex_info.as_ref().map_or(0, |t| t.tex_coord());
            let uvs: Vec<[f32; 2]> = reader.read_tex_coords(uv_set).map(|t| t.into_f32().collect()).unwrap_or_default();
            let raw: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };
            let mut indices = Vec::new();
            match mode {
                Mode::Triangles => indices = raw,
                Mode::TriangleStrip => {
                    for k in 2..raw.len() {
                        if k % 2 == 0 {
                            indices.extend_from_slice(&[raw[k - 2], raw[k - 1], raw[k]]);
                        } else {
                            indices.extend_from_slice(&[raw[k - 1], raw[k - 2], raw[k]]);
                        }
                    }
                }
                _ => {
                    for k in 2..raw.len() {
                        indices.extend_from_slice(&[raw[0], raw[k - 1], raw[k]]);
                    }
                }
            }
            if flip {
                for t in indices.chunks_mut(3) {
                    if t.len() == 3 {
                        t.swap(1, 2);
                    }
                }
            }
            let name = match material.name() {
                Some(m) if mesh.primitives().len() > 1 => format!("{mesh_name}_{m}"),
                _ if mesh.primitives().len() > 1 => format!("{mesh_name}_{pi}"),
                _ => mesh_name.clone(),
            };
            let mut part = new_part(&name);
            part.positions = positions;
            part.normals = normals;
            part.uvs = uvs;
            part.indices = indices;
            part.color = pbr.base_color_factor();
            if let Some(info) = tex_info {
                let src = info.texture().source();
                let idx = src.index();
                let tex = textures.entry(idx).or_insert_with(|| {
                    let data = images.get(idx)?;
                    let Some(img) = gltf_image_to_rgba(data) else {
                        warnings.push(format!("glTF image {idx} has an unexpected pixel layout"));
                        return None;
                    };
                    let name = match src.source() {
                        gltf::image::Source::Uri { uri, .. } if !uri.starts_with("data:") => stem_of(Path::new(uri.rsplit('/').next().unwrap_or(uri))),
                        _ => src.name().map(str::to_string).unwrap_or_else(|| format!("{model_stem}_tex{idx}")),
                    };
                    Some(ImportedTexture { name, image: limit_size(img) })
                });
                part.texture = tex.clone();
            }
            parts.push(part);
        }
    }
    Ok(parts)
}

// ---------------------------------------------------------------------------------------------
// STL
// ---------------------------------------------------------------------------------------------

fn import_stl(path: &Path, name: &str) -> Result<Vec<ImportedPart>> {
    let d = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let binary_size_ok = d.len() >= 84 && 84 + u32::from_le_bytes(d[80..84].try_into().unwrap()) as usize * 50 == d.len();
    let looks_ascii = String::from_utf8_lossy(&d[..d.len().min(512)]).trim_start().to_ascii_lowercase().starts_with("solid");
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    if binary_size_ok || !looks_ascii {
        if d.len() < 84 {
            bail!("the STL file is too short");
        }
        let n = u32::from_le_bytes(d[80..84].try_into().unwrap()) as usize;
        let f = |o: usize| f32::from_le_bytes(d[o..o + 4].try_into().unwrap());
        for i in 0..n {
            let o = 84 + i * 50;
            if o + 50 > d.len() {
                break;
            }
            let v = |k: usize| [f(o + 12 + k * 12), f(o + 16 + k * 12), f(o + 20 + k * 12)];
            tris.push([v(0), v(1), v(2)]);
        }
    } else {
        let text = String::from_utf8_lossy(&d);
        let mut cur = Vec::new();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            if it.next().is_some_and(|k| k.eq_ignore_ascii_case("vertex")) {
                let c: Vec<f32> = it.map(|s| s.parse().unwrap_or(0.0)).collect();
                if c.len() >= 3 {
                    cur.push([c[0], c[1], c[2]]);
                }
                if cur.len() == 3 {
                    tris.push([cur[0], cur[1], cur[2]]);
                    cur.clear();
                }
            } else if line.trim_start().to_ascii_lowercase().starts_with("endfacet") {
                cur.clear();
            }
        }
    }
    let mut p = new_part(name);
    for t in tris {
        // Flat normals from the winding (the stored facet normals are often zero or wrong).
        let n = normalize(face_normal(t[0], t[1], t[2])).unwrap_or([0.0; 3]);
        for v in t {
            p.indices.push(p.positions.len() as u32);
            p.positions.push(v);
            p.normals.push(n);
        }
    }
    Ok(vec![p])
}

// ---------------------------------------------------------------------------------------------
// PLY
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum PlyType {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl PlyType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => return None,
        })
    }
    fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
    fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }
}

struct PlyProp {
    name: String,
    ty: PlyType,
    list: Option<PlyType>,
}

struct PlyElem {
    name: String,
    count: usize,
    props: Vec<PlyProp>,
}

enum PlySrc<'a> {
    Ascii(std::str::SplitAsciiWhitespace<'a>),
    Bin { d: &'a [u8], pos: usize, big: bool },
}

impl PlySrc<'_> {
    fn read(&mut self, ty: PlyType) -> Result<f64> {
        match self {
            PlySrc::Ascii(it) => it.next().ok_or_else(|| anyhow::anyhow!("PLY data ends early"))?.parse::<f64>().context("bad PLY number"),
            PlySrc::Bin { d, pos, big } => {
                let n = ty.size();
                if *pos + n > d.len() {
                    bail!("PLY data ends early");
                }
                let mut b = [0u8; 8];
                b[..n].copy_from_slice(&d[*pos..*pos + n]);
                if *big {
                    b[..n].reverse();
                }
                *pos += n;
                Ok(match ty {
                    PlyType::I8 => b[0] as i8 as f64,
                    PlyType::U8 => b[0] as f64,
                    PlyType::I16 => i16::from_le_bytes([b[0], b[1]]) as f64,
                    PlyType::U16 => u16::from_le_bytes([b[0], b[1]]) as f64,
                    PlyType::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
                    PlyType::U32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
                    PlyType::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
                    PlyType::F64 => f64::from_le_bytes(b),
                })
            }
        }
    }
}

fn import_ply(path: &Path, name: &str, warnings: &mut Vec<String>) -> Result<Vec<ImportedPart>> {
    let d = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if !d.starts_with(b"ply") {
        bail!("not a PLY file");
    }
    let end = d.windows(10).position(|w| w == b"end_header").ok_or_else(|| anyhow::anyhow!("PLY has no end_header"))?;
    let header = String::from_utf8_lossy(&d[..end]).to_string();
    let mut body = end + 10;
    while body < d.len() && (d[body] == b'\r' || d[body] == b' ') {
        body += 1;
    }
    if body < d.len() && d[body] == b'\n' {
        body += 1;
    }
    let mut format = "";
    let mut elems: Vec<PlyElem> = Vec::new();
    for line in header.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        match t.first().copied() {
            Some("format") => format = if t.get(1) == Some(&"ascii") { "ascii" } else if t.get(1) == Some(&"binary_big_endian") { "be" } else { "le" },
            Some("element") if t.len() >= 3 => elems.push(PlyElem { name: t[1].to_string(), count: t[2].parse().context("bad PLY element count")?, props: Vec::new() }),
            Some("property") => {
                let e = elems.last_mut().ok_or_else(|| anyhow::anyhow!("PLY property outside an element"))?;
                if t.get(1) == Some(&"list") && t.len() >= 5 {
                    let (Some(c), Some(i)) = (PlyType::parse(t[2]), PlyType::parse(t[3])) else { bail!("bad PLY list type in '{line}'") };
                    e.props.push(PlyProp { name: t[4].to_string(), ty: i, list: Some(c) });
                } else if t.len() >= 3 {
                    let Some(ty) = PlyType::parse(t[1]) else { bail!("bad PLY property type in '{line}'") };
                    e.props.push(PlyProp { name: t[2].to_string(), ty, list: None });
                }
            }
            _ => {}
        }
    }
    let text;
    let mut src = if format == "ascii" {
        text = String::from_utf8_lossy(&d[body..]).to_string();
        PlySrc::Ascii(text.split_ascii_whitespace())
    } else {
        PlySrc::Bin { d: &d[body..], pos: 0, big: format == "be" }
    };
    let mut part = new_part(name);
    let mut colors: Vec<[f64; 3]> = Vec::new();
    let mut faces_read = false;
    for e in &elems {
        let is_vertex = e.name == "vertex";
        let is_face = e.name == "face";
        let find = |names: &[&str]| e.props.iter().position(|p| p.list.is_none() && names.contains(&p.name.as_str()));
        let (ix, iy, iz) = (find(&["x"]), find(&["y"]), find(&["z"]));
        let (inx, iny, inz) = (find(&["nx"]), find(&["ny"]), find(&["nz"]));
        let (iu, iv) = (find(&["u", "s", "texture_u", "texture_s"]), find(&["v", "t", "texture_v", "texture_t"]));
        let (ir, ig, ib) = (find(&["red", "r", "diffuse_red"]), find(&["green", "g", "diffuse_green"]), find(&["blue", "b", "diffuse_blue"]));
        let iface = e.props.iter().position(|p| p.list.is_some() && (p.name == "vertex_indices" || p.name == "vertex_index"));
        let mut vals = vec![0.0f64; e.props.len()];
        for _ in 0..e.count {
            let mut poly: Vec<u32> = Vec::new();
            for (k, p) in e.props.iter().enumerate() {
                match p.list {
                    Some(ct) => {
                        let n = src.read(ct)? as usize;
                        if n > 1_000_000 {
                            bail!("PLY list of {n} items");
                        }
                        for _ in 0..n {
                            let x = src.read(p.ty)?;
                            if is_face && Some(k) == iface {
                                poly.push(x as u32);
                            }
                        }
                    }
                    None => vals[k] = src.read(p.ty)?,
                }
            }
            if is_vertex {
                let g = |i: Option<usize>| i.map_or(0.0, |i| vals[i]) as f32;
                part.positions.push([g(ix), g(iy), g(iz)]);
                if inx.is_some() {
                    part.normals.push([g(inx), g(iny), g(inz)]);
                }
                if iu.is_some() {
                    part.uvs.push([g(iu), 1.0 - g(iv)]);
                }
                if let (Some(r), Some(gg), Some(b)) = (ir, ig, ib) {
                    let sc = |i: usize| if e.props[i].ty.is_float() { vals[i] } else { vals[i] / 255.0 };
                    colors.push([sc(r), sc(gg), sc(b)]);
                }
            } else if is_face && poly.len() >= 3 {
                faces_read = true;
                for k in 1..poly.len() - 1 {
                    part.indices.extend_from_slice(&[poly[0], poly[k], poly[k + 1]]);
                }
            }
        }
    }
    if !faces_read {
        bail!("the PLY file has no faces (point clouds can't be imported)");
    }
    if !colors.is_empty() {
        let n = colors.len() as f64;
        let s = colors.iter().fold([0.0; 3], |a, c| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]);
        part.color = [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32, 1.0];
        warnings.push("the PLY has per-vertex colours; their average is used as the part colour".to_string());
    }
    Ok(vec![part])
}

// ---------------------------------------------------------------------------------------------
// FBX (binary 7.x)
// ---------------------------------------------------------------------------------------------

fn import_fbx(path: &Path, warnings: &mut Vec<String>) -> Result<Vec<ImportedPart>> {
    use fbxcel_dom::any::AnyDocument;
    use fbxcel_dom::v7400::data::mesh::layer::TypedLayerElementHandle;
    use fbxcel_dom::v7400::data::mesh::{PolygonVertexIndex, PolygonVertices};
    use fbxcel_dom::v7400::object::model::{ModelHandle, TypedModelHandle};
    use fbxcel_dom::v7400::object::property::loaders::F64Arr3Loader;
    use fbxcel_dom::v7400::object::TypedObjectHandle;

    const HINT: &str = "export the model as glTF (.glb) or OBJ from Blender";
    let head = {
        use std::io::Read;
        let mut b = [0u8; 20];
        let mut f = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
        let n = f.read(&mut b)?;
        b[..n].to_vec()
    };
    if !head.starts_with(b"Kaydara FBX Binary") {
        bail!("ASCII FBX is not supported: {HINT}");
    }
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    let doc = match AnyDocument::from_seekable_reader(file) {
        Ok(AnyDocument::V7400(_, doc)) => doc,
        Ok(_) => bail!("this FBX version is not supported: {HINT}"),
        Err(e) => bail!("could not read the FBX ({e}): {HINT}"),
    };

    fn fan(_: &PolygonVertices<'_>, poly: &[PolygonVertexIndex], out: &mut Vec<[PolygonVertexIndex; 3]>) -> Result<(), anyhow::Error> {
        for k in 2..poly.len() {
            out.push([poly[0], poly[k - 1], poly[k]]);
        }
        Ok(())
    }
    let vec3 = |m: &ModelHandle<'_>, name: &str, def: [f64; 3]| -> [f64; 3] {
        m.properties_by_native_typename("FbxNode").get_property(name).and_then(|p| p.load_value(F64Arr3Loader).ok()).unwrap_or(def)
    };
    let local = |m: &ModelHandle<'_>| -> M4 {
        let t = m_translate(vec3(m, "Lcl Translation", [0.0; 3]));
        let pre = m_euler_xyz(vec3(m, "PreRotation", [0.0; 3]));
        let r = m_euler_xyz(vec3(m, "Lcl Rotation", [0.0; 3]));
        let s = m_scale(vec3(m, "Lcl Scaling", [1.0; 3]));
        m_mul(&t, &m_mul(&pre, &m_mul(&r, &s)))
    };

    let mut parts: Vec<ImportedPart> = Vec::new();
    let mut cache = TextureCache::default();
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut mat_textures: HashMap<i64, (Option<ImportedTexture>, [f32; 4], String)> = HashMap::new();
    for obj in doc.objects() {
        let TypedObjectHandle::Model(TypedModelHandle::Mesh(mesh)) = obj.get_typed() else { continue };
        let mesh_name = mesh.name().unwrap_or("mesh").to_string();
        let geom = match mesh.geometry() {
            Ok(g) => g,
            Err(e) => {
                warnings.push(format!("{mesh_name}: {e}"));
                continue;
            }
        };
        // World transform: walk up the parents.
        let model: &ModelHandle<'_> = &mesh;
        let mut world = local(model);
        let mut cur = model.parent_model();
        let mut depth = 0;
        while let Some(p) = cur {
            let pm: &ModelHandle<'_> = &p;
            world = m_mul(&local(pm), &world);
            cur = pm.parent_model();
            depth += 1;
            if depth > 1000 {
                break;
            }
        }
        let geo = m_mul(
            &m_translate(vec3(model, "GeometricTranslation", [0.0; 3])),
            &m_mul(&m_euler_xyz(vec3(model, "GeometricRotation", [0.0; 3])), &m_scale(vec3(model, "GeometricScaling", [1.0; 3]))),
        );
        let world = m_mul(&world, &geo);
        let nm = m_normal(&world);
        let flip = m_det3(&world) < 0.0;

        let pv = geom.polygon_vertices().with_context(|| format!("{mesh_name}: polygons"))?;
        let tris = pv.triangulate_each(fan).with_context(|| format!("{mesh_name}: triangulating"))?;
        let (mut normals, mut uv, mut mats) = (None, None, None);
        for layer in geom.layers() {
            for entry in layer.layer_element_entries() {
                match entry.typed_layer_element() {
                    Ok(TypedLayerElementHandle::Normal(h)) if normals.is_none() => normals = h.normals().ok(),
                    Ok(TypedLayerElementHandle::Uv(h)) if uv.is_none() => uv = h.uv().ok(),
                    Ok(TypedLayerElementHandle::Material(h)) if mats.is_none() => mats = h.materials().ok(),
                    _ => {}
                }
            }
        }
        let materials: Vec<_> = mesh.materials().collect();
        let mut part_of: HashMap<u32, usize> = HashMap::new();
        let tvis: Vec<_> = tris.triangle_vertex_indices().collect();
        for tri in tvis.chunks(3) {
            if tri.len() < 3 {
                break;
            }
            let mi = mats.as_ref().and_then(|m| m.material_index(&tris, tri[0]).ok()).map_or(0, |m| m.to_u32());
            let pi = *part_of.entry(mi).or_insert_with(|| {
                let (tex, color, mname) = match materials.get(mi as usize) {
                    Some(mat) => mat_textures
                        .entry(mat.object_id().raw())
                        .or_insert_with(|| {
                            let mname = mat.name().unwrap_or("material").to_string();
                            let props = mat.properties();
                            let c = props.diffuse_color_or_default().map(|c| [c.r as f32, c.g as f32, c.b as f32]).unwrap_or([0.8; 3]);
                            let mut tex = None;
                            if let Some(t) = mat.diffuse_texture() {
                                let clip = t.video_clip();
                                let rel = clip.as_ref().and_then(|c| c.relative_filename().ok()).map(str::to_string).or_else(|| {
                                    t.node().children_by_name("RelativeFilename").chain(t.node().children_by_name("FileName")).find_map(|n| n.attributes().first().and_then(|a| a.get_string()).map(str::to_string))
                                });
                                let file = rel.as_deref().and_then(|r| resolve_file(dir, r)).or_else(|| {
                                    t.node().children_by_name("FileName").find_map(|n| n.attributes().first().and_then(|a| a.get_string()).and_then(|f| resolve_file(dir, f)))
                                });
                                tex = match file {
                                    Some(f) => cache.get(&f, warnings),
                                    None => match clip.as_ref().and_then(|c| c.content()).filter(|b| !b.is_empty()) {
                                        Some(bytes) => {
                                            let stem = rel.as_deref().map(|r| stem_of(Path::new(&r.replace('\\', "/")))).unwrap_or_else(|| format!("{mname}_diffuse"));
                                            let ext = rel.as_deref().and_then(|r| Path::new(r).extension().and_then(|e| e.to_str()).map(str::to_string)).unwrap_or_default();
                                            match decode_image(bytes, &ext) {
                                                Ok(image) => Some(ImportedTexture { name: stem, image }),
                                                Err(e) => {
                                                    warnings.push(format!("embedded texture of {mname}: {e:#}"));
                                                    None
                                                }
                                            }
                                        }
                                        None => {
                                            warnings.push(format!("texture {} of material {mname} not found", rel.unwrap_or_default()));
                                            None
                                        }
                                    },
                                };
                            }
                            let color = if tex.is_some() { [1.0; 4] } else { [c[0], c[1], c[2], 1.0] };
                            (tex, color, mname)
                        })
                        .clone(),
                    None => (None, [0.8, 0.8, 0.8, 1.0], mesh_name.clone()),
                };
                let mut p = new_part(&if materials.len() > 1 { format!("{mesh_name}_{mname}") } else { mesh_name.clone() });
                p.texture = tex;
                p.color = color;
                parts.push(p);
                parts.len() - 1
            });
            let part = &mut parts[pi];
            let base = part.positions.len() as u32;
            for &tv in tri {
                let Some(cp) = tris.control_point(tv) else { bail!("{mesh_name}: bad control point") };
                part.positions.push(m_point(&world, [cp.x as f32, cp.y as f32, cp.z as f32]));
                let n = normals.as_ref().and_then(|n| n.normal(&tris, tv).ok()).map_or([0.0; 3], |n| m_dir(&nm, [n.x as f32, n.y as f32, n.z as f32]));
                part.normals.push(n);
                let t = uv.as_ref().and_then(|u| u.uv(&tris, tv).ok()).map_or([0.0; 2], |t| [t.x as f32, 1.0 - t.y as f32]);
                part.uvs.push(t);
            }
            if flip {
                part.indices.extend_from_slice(&[base, base + 2, base + 1]);
            } else {
                part.indices.extend_from_slice(&[base, base + 1, base + 2]);
            }
        }
    }
    if parts.is_empty() {
        bail!("the FBX has no meshes");
    }
    Ok(parts)
}

// ---------------------------------------------------------------------------------------------
// League of Legends meshes
// ---------------------------------------------------------------------------------------------

/// Submesh ranges of an SKN: (name, first index, index count).
fn skn_ranges(d: &[u8]) -> Vec<(String, usize, usize)> {
    let u16_at = |o: usize| d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let Some(major) = u16_at(4) else { return Vec::new() };
    if major == 0 {
        return Vec::new();
    }
    let n = u32_at(8).unwrap_or(0) as usize;
    let mut out = Vec::new();
    for k in 0..n.min(1000) {
        let o = 12 + k * 80;
        let Some(name) = d.get(o..o + 64) else { break };
        let end = name.iter().position(|&b| b == 0).unwrap_or(64);
        let name: String = name[..end].iter().map(|&b| b as char).collect();
        let (Some(si), Some(ic)) = (u32_at(o + 72), u32_at(o + 76)) else { break };
        out.push((name, si as usize, ic as usize));
    }
    out
}

/// The diffuse texture of a League mesh: `<stem>.dds`/`.tga`, else `<submesh>.dds`, else the
/// folder's only texture (ignoring loading-screen art).
fn find_league_texture(path: &Path, stems: &[&str]) -> Option<PathBuf> {
    let dir = path.parent()?;
    for s in stems {
        for ext in ["dds", "tga", "png"] {
            if let Some(p) = find_in_dir(dir, &format!("{s}.{ext}")) {
                return Some(p);
            }
        }
    }
    let imgs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("dds") || e.eq_ignore_ascii_case("tga")))
        .collect();
    if imgs.len() == 1 {
        return imgs.into_iter().next();
    }
    // Several textures: rank them (colour maps up; loading screens, low-detail, masks down).
    let score = |p: &PathBuf| -> i32 {
        let n = stem_of(p).to_ascii_lowercase();
        let mut s = 0;
        if n.contains("tx_cm") || n.ends_with("_cm") {
            s += 4;
        }
        if n.contains("diffuse") || n.ends_with("_d") || n.ends_with("_color") {
            s += 3;
        }
        for (bad, pen) in [("loadscreen", 20), ("splash", 20), ("orthograph", 10), ("test", 5), ("low", 3), ("_circle", 20), ("_square", 20), ("normal", 10), ("mask", 10)] {
            if n.contains(bad) {
                s -= pen;
            }
        }
        for suf in ["_gm", "_rm", "_nm", "_n", "_tx_gm", "_tx_rm", "_tx_nm", "_mask", "_em"] {
            if n.ends_with(suf) {
                s -= 10;
            }
        }
        // Names that start like the mesh's stems win ties.
        if stems.iter().any(|st| !st.is_empty() && n.starts_with(&st.to_ascii_lowercase().trim_end_matches("_mat").to_string())) {
            s += 1;
        }
        s
    };
    let mut ranked: Vec<(i32, PathBuf)> = imgs.into_iter().map(|p| (score(&p), p)).filter(|(s, _)| *s >= 0).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    match ranked.as_slice() {
        [] => None,
        [(_, p)] => Some(p.clone()),
        [(a, p), (b, _), ..] if a > b => Some(p.clone()),
        _ => None,
    }
}

fn import_skn(path: &Path, name: &str, warnings: &mut Vec<String>) -> Result<Vec<ImportedPart>> {
    let d = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let skn = crate::formats::skn::Skn::parse(&d)?;
    let mut ranges = skn_ranges(&d);
    let total = skn.indices.len();
    if ranges.is_empty() || ranges.iter().any(|r| r.1 + r.2 > total) {
        ranges = vec![(skn.submeshes.first().cloned().unwrap_or_else(|| name.to_string()), 0, total)];
    }
    let mut cache = TextureCache::default();
    let mut stems: Vec<&str> = vec![name];
    stems.extend(ranges.iter().map(|r| r.0.as_str()));
    let shared = find_league_texture(path, &stems);
    let mut parts = Vec::new();
    for (sub, start, count) in ranges {
        let mut p = new_part(&sub);
        let mut remap: HashMap<u32, u32> = HashMap::new();
        for &i in &skn.indices[start..(start + count / 3 * 3)] {
            let id = *remap.entry(i).or_insert_with(|| {
                p.positions.push(skn.positions[i as usize]);
                p.normals.push(skn.normals[i as usize]);
                p.uvs.push(skn.uvs[i as usize]);
                (p.positions.len() - 1) as u32
            });
            p.indices.push(id);
        }
        let tex = find_league_texture(path, &[&sub]).filter(|t| shared.is_none() || stem_of(t).eq_ignore_ascii_case(&sub)).or_else(|| shared.clone());
        p.texture = tex.and_then(|t| cache.get(&t, warnings));
        parts.push(p);
    }
    if parts.iter().all(|p| p.texture.is_none()) {
        warnings.push(format!("no texture found next to {} (looked for {name}.dds)", path.display()));
    }
    Ok(parts)
}

/// Builds parts from League static-object faces: (corner indices, material, corner UVs).
fn league_static_parts(path: &Path, vertices: &[[f32; 3]], faces: &[([u32; 3], String, [[f32; 2]; 3])], warnings: &mut Vec<String>) -> Vec<ImportedPart> {
    let mut parts: Vec<ImportedPart> = Vec::new();
    let mut by_mat: HashMap<&str, usize> = HashMap::new();
    let mut bad = 0;
    for (idx, mat, uv) in faces {
        if idx.iter().any(|&i| i as usize >= vertices.len()) {
            bad += 1;
            continue;
        }
        let pi = *by_mat.entry(mat.as_str()).or_insert_with(|| {
            parts.push(new_part(mat));
            parts.len() - 1
        });
        let p = &mut parts[pi];
        for k in 0..3 {
            p.indices.push(p.positions.len() as u32);
            p.positions.push(vertices[idx[k] as usize]);
            p.uvs.push(uv[k]);
        }
    }
    if bad > 0 {
        warnings.push(format!("{bad} faces use missing vertices and were skipped"));
    }
    let mut cache = TextureCache::default();
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut missing = Vec::new();
    for p in &mut parts {
        let found = [dir.to_path_buf(), dir.join("textures"), dir.parent().map(Path::to_path_buf).unwrap_or_default()]
            .iter()
            .find_map(|d| ["dds", "tga"].iter().find_map(|e| find_in_dir(d, &format!("{}.{e}", p.name))));
        match found {
            Some(f) => p.texture = cache.get(&f, warnings),
            None => missing.push(p.name.clone()),
        }
    }
    if !missing.is_empty() {
        warnings.push(format!("no texture file found for material(s): {}", missing.join(", ")));
    }
    parts
}

fn import_scb(path: &Path, warnings: &mut Vec<String>) -> Result<Vec<ImportedPart>> {
    let d = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let scb = crate::formats::scb::Scb::parse(&d)?;
    let faces: Vec<_> = scb.faces.iter().map(|f| (f.indices, f.material.clone(), f.uv)).collect();
    Ok(league_static_parts(path, &scb.vertices, &faces, warnings))
}

fn import_sco(path: &Path, warnings: &mut Vec<String>) -> Result<Vec<ImportedPart>> {
    let d = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let sco = crate::formats::scb::Sco::parse(&d)?;
    let text: String = d.iter().map(|&b| b as char).collect();
    let mut lines = text.lines().skip_while(|l| !l.trim_start().to_ascii_lowercase().starts_with("faces="));
    lines.next();
    let mut faces = Vec::new();
    for l in lines {
        let t: Vec<&str> = l.split_whitespace().collect();
        if t.first().is_some_and(|s| s.starts_with('[')) {
            break;
        }
        if t.len() < 11 {
            continue;
        }
        let (Ok(a), Ok(b), Ok(c)) = (t[1].parse::<u32>(), t[2].parse::<u32>(), t[3].parse::<u32>()) else { continue };
        let f: Vec<f32> = t[t.len() - 6..].iter().map(|s| s.parse().unwrap_or(0.0)).collect();
        let mat = t[4..t.len() - 6].join(" ");
        faces.push(([a, b, c], mat, [[f[0], f[1]], [f[2], f[3]], [f[4], f[5]]]));
    }
    if faces.len() != sco.face_count {
        warnings.push(format!("the SCO says {} faces but {} were read", sco.face_count, faces.len()));
    }
    Ok(league_static_parts(path, &sco.vertices, &faces, warnings))
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lms_import3d_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    fn check(m: &ImportedModel) {
        for p in &m.parts {
            assert_eq!(p.normals.len(), p.positions.len());
            assert_eq!(p.uvs.len(), p.positions.len());
            assert_eq!(p.indices.len() % 3, 0);
            assert!(p.indices.iter().all(|&i| (i as usize) < p.positions.len()));
            assert!(p.normals.iter().all(|n| (len3(*n) - 1.0).abs() < 1e-3));
        }
    }

    /// Mean of dot(normal, centroid -> vertex): positive when normals point outwards.
    fn outwardness(m: &ImportedModel) -> f32 {
        let (mut c, mut n) = ([0.0f32; 3], 0.0f32);
        for p in &m.parts {
            for v in &p.positions {
                for k in 0..3 {
                    c[k] += v[k];
                }
                n += 1.0;
            }
        }
        let c = [c[0] / n, c[1] / n, c[2] / n];
        let mut s = 0.0;
        for p in &m.parts {
            for (v, nr) in p.positions.iter().zip(&p.normals) {
                let d = normalize(sub(*v, c)).unwrap_or([0.0; 3]);
                s += d[0] * nr[0] + d[1] * nr[1] + d[2] * nr[2];
            }
        }
        s / n
    }

    /// Mean agreement between the stored normals and normals recomputed from the winding.
    fn winding_agreement(p: &ImportedPart) -> f32 {
        let mut s = 0.0;
        for t in p.indices.chunks(3) {
            let f = normalize(face_normal(p.positions[t[0] as usize], p.positions[t[1] as usize], p.positions[t[2] as usize])).unwrap_or([0.0; 3]);
            let n = p.normals[t[0] as usize];
            s += f[0] * n[0] + f[1] * n[1] + f[2] * n[2];
        }
        s / (p.indices.len() / 3).max(1) as f32
    }

    const CUBE_OBJ: &str = "mtllib cube.mtl\n\
v -50 0 -50\nv 50 0 -50\nv 50 100 -50\nv -50 100 -50\nv -50 0 50\nv 50 0 50\nv 50 100 50\nv -50 100 50\n\
vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
o cube\nusemtl red\n\
f 1/1 4/4 3/3 2/2\nf 5/1 6/2 7/3 8/4\nf 1 5 8 4\n\
usemtl tex\n\
f 2 3 7 6\nf -8 -7 -3 -4\nf 4 8 7 3\n";

    #[test]
    fn obj_with_mtl() {
        let p = tmp("cube.obj");
        std::fs::write(&p, CUBE_OBJ).unwrap();
        std::fs::write(tmp("cube.mtl"), "newmtl red\nKd 1 0 0\nnewmtl tex\nKd 0.8 0.8 0.8\nmap_Kd -s 1 1 1 checker.png\n").unwrap();
        let img = image::RgbaImage::from_fn(4, 4, |x, y| if (x + y) % 2 == 0 { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([0, 0, 0, 255]) });
        img.save(tmp("checker.png")).unwrap();
        let m = import_file(&p).unwrap();
        check(&m);
        assert_eq!(m.parts.len(), 2);
        let red = m.parts.iter().find(|p| p.name == "red").unwrap();
        assert_eq!(red.color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(red.indices.len(), 18);
        let tex = m.parts.iter().find(|p| p.name == "tex").unwrap();
        let t = tex.texture.as_ref().expect("texture");
        assert_eq!((t.name.as_str(), t.image.width, t.image.height), ("checker", 4, 4));
        // X mirrored: the cube spans -50..50 so check a vertex's v flip instead and outward normals.
        assert!(outwardness(&m) > 0.3, "normals should point out: {}", outwardness(&m));
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
        println!("OBJ: {} parts, {:?}", m.parts.len(), m.warnings);
    }

    #[test]
    fn stl_ascii() {
        let p = tmp("tet.stl");
        std::fs::write(
            &p,
            "solid t\n\
facet normal 0 0 0\nouter loop\nvertex 0 0 0\nvertex 0 1 0\nvertex 1 0 0\nendloop\nendfacet\n\
facet normal 0 0 0\nouter loop\nvertex 0 0 0\nvertex 0 0 1\nvertex 0 1 0\nendloop\nendfacet\n\
facet normal 0 0 0\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 0 1\nendloop\nendfacet\n\
facet normal 0 0 0\nouter loop\nvertex 1 0 0\nvertex 0 1 0\nvertex 0 0 1\nendloop\nendfacet\n\
endsolid t\n",
        )
        .unwrap();
        let m = import_file(&p).unwrap();
        check(&m);
        assert_eq!(m.parts[0].indices.len(), 12);
        assert!(m.parts[0].positions.iter().all(|v| v[0] <= 0.0), "x is mirrored");
        assert!(outwardness(&m) > 0.2);
        assert!(winding_agreement(&m.parts[0]) > 0.99);
        assert!(m.warnings.iter().any(|w| w.contains("scale")), "tiny model warns: {:?}", m.warnings);
    }

    #[test]
    fn ply_ascii() {
        let p = tmp("quad.ply");
        std::fs::write(
            &p,
            "ply\nformat ascii 1.0\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nproperty float s\nproperty float t\n\
property uchar red\nproperty uchar green\nproperty uchar blue\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n\
0 0 0 0 0 255 0 0\n100 0 0 1 0 255 0 0\n100 100 0 1 1 255 0 0\n0 100 0 0 1 255 0 0\n4 0 1 2 3\n",
        )
        .unwrap();
        let m = import_file(&p).unwrap();
        check(&m);
        let part = &m.parts[0];
        assert_eq!(part.indices.len(), 6);
        assert_eq!(part.color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(part.uvs[0], [0.0, 1.0]);
        // A CCW quad facing +z, mirrored, still faces +z.
        assert!(part.normals[0][2] > 0.99, "{:?}", part.normals[0]);
        assert_eq!(part.positions[1], [-100.0, 0.0, 0.0]);
    }

    #[test]
    fn gltf_data_uri() {
        use base64::Engine;
        let mut bin = Vec::new();
        for v in [[0.0f32, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 100.0, 0.0]] {
            for c in v {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        for i in [0u16, 1, 2, 0] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bin);
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],
"nodes":[{{"mesh":0,"translation":[10,0,0],"name":"tri"}}],
"meshes":[{{"name":"tri","primitives":[{{"attributes":{{"POSITION":0}},"indices":1,"material":0}}]}}],
"materials":[{{"pbrMetallicRoughness":{{"baseColorFactor":[0.5,1,1,1]}}}}],
"buffers":[{{"byteLength":{len},"uri":"data:application/octet-stream;base64,{b64}"}}],
"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":6}}],
"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[100,100,0]}},
{{"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}}]}}"#,
            len = bin.len()
        );
        let p = tmp("tri.gltf");
        std::fs::write(&p, json).unwrap();
        let m = import_file(&p).unwrap();
        check(&m);
        let part = &m.parts[0];
        assert_eq!(part.color, [0.5, 1.0, 1.0, 1.0]);
        assert_eq!(part.positions[1], [-110.0, 0.0, 0.0]);
        assert_eq!(part.indices, vec![0, 2, 1]);
        assert!(part.normals[0][2] > 0.99, "{:?}", part.normals[0]);
    }

    #[test]
    fn league_garen_skn() {
        // Game files are not part of the repository (see game/README.md); skipped without them.
        let Some(data) = crate::paths::data_dir() else { return };
        let root_buf = data.join("Characters").join("Garen");
        let root = root_buf.as_path();
        if !root.exists() {
            return;
        }
        let mut stack = vec![root.to_path_buf()];
        let mut skn = None;
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("skn")) && (skn.is_none() || stem_of(&p).eq_ignore_ascii_case("garen")) {
                    skn = Some(p);
                }
            }
        }
        let Some(skn) = skn else { return };
        let m = import_file(&skn).unwrap();
        check(&m);
        let tris: usize = m.parts.iter().map(|p| p.indices.len() / 3).sum();
        println!(
            "SKN {}: parts {:?}, {tris} triangles, texture {:?}, warnings {:?}, outwardness {:.2}",
            skn.display(),
            m.parts.iter().map(|p| &p.name).collect::<Vec<_>>(),
            m.parts[0].texture,
            m.warnings,
            outwardness(&m)
        );
        assert!(tris > 100);
        for p in &m.parts {
            let w = winding_agreement(p);
            println!("  {} winding agreement {w:.2}", p.name);
            // League's winding matches the convention the computed normals assume.
            assert!(w > 0.5);
        }
    }

    #[test]
    fn league_turret_sco() {
        let dir_buf = crate::paths::map_dir("Map1").join("Scene");
        let dir = dir_buf.as_path();
        if !dir.exists() {
            return;
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("sco")) && stem_of(p).starts_with("Turret_"))
            .collect();
        files.sort();
        let Some(f) = files.first() else { return };
        let m = import_file(f).unwrap();
        check(&m);
        println!("SCO {}: parts {:?}, warnings {:?}, outwardness {:.2}", f.display(), m.parts.iter().map(|p| (&p.name, p.indices.len() / 3, p.texture.is_some())).collect::<Vec<_>>(), m.warnings, outwardness(&m));
        assert!(!m.parts.is_empty());
        // Same model as SCB.
        let scb = f.with_extension("SCB");
        if scb.exists() {
            let m2 = import_file(&scb).unwrap();
            check(&m2);
            println!("SCB: parts {:?}", m2.parts.iter().map(|p| (&p.name, p.indices.len() / 3)).collect::<Vec<_>>());
        }
        // A bigger one, to check the winding convention against the shape.
        if let Some(big) = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("sco"))).max_by_key(|p| p.metadata().map(|m| m.len()).unwrap_or(0)) {
            let m3 = import_file(&big).unwrap();
            check(&m3);
            println!("largest SCO {}: {} parts, outwardness {:.2}", big.display(), m3.parts.len(), outwardness(&m3));
        }
    }

    #[test]
    fn real_glb_files() {
        // A folder of .glb files to test with (optional): LMS_TEST_GLB_DIR.
        let Ok(dir) = std::env::var("LMS_TEST_GLB_DIR") else { return };
        let Ok(rd) = std::fs::read_dir(&dir) else { return };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("glb"))).collect();
        files.sort();
        for f in files.iter().take(4) {
            let m = import_file(f).unwrap();
            check(&m);
            println!(
                "GLB {}: {:?}, warnings {:?}",
                stem_of(f),
                m.parts.iter().map(|p| (&p.name, p.indices.len() / 3, p.texture.as_ref().map(|t| (t.name.clone(), t.image.width)))).collect::<Vec<_>>(),
                m.warnings
            );
        }
    }

    #[test]
    fn rejects_images_and_unknown() {
        assert!(import_file(Path::new("x.dds")).is_err());
        assert!(import_file(Path::new("x.xyz")).is_err());
    }
}
