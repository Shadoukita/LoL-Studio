//! 3D models from the internet for AI maps: Poly Haven (free, CC0, no key needed). A model is
//! downloaded (1k glTF), then made to look like classic Summoner's Rift (Map1) art: fewer
//! polygons, League scale, a small hand-painted texture graded toward Map1's own textures. It is
//! saved to the asset library ("asset:<name>") so it can be placed like any other prop.

use crate::asset::{self, Asset, AssetPart};
use crate::formats::dds::{self, Image};
use crate::meshops::{self, Mesh};
use crate::props::PropLibrary;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const UA: &str = "LoLMapStudio/1.0 (custom map editor)";

/// One model in the online catalogue.
#[derive(Clone, Debug)]
pub struct OnlineModel {
    pub id: String,
    pub name: String,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub polycount: u64,
}

fn cache_dir() -> PathBuf {
    asset::library_dir().parent().map(|p| p.join("online")).unwrap_or_else(|| PathBuf::from("online"))
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(120))).build().into()
}

fn get_bytes(url: &str) -> Result<Vec<u8>> {
    let mut r = agent().get(url).header("User-Agent", UA).call().with_context(|| format!("downloading {url}"))?;
    Ok(r.body_mut().with_config().limit(200 * 1024 * 1024).read_to_vec()?)
}

/// The catalogue of models (cached for a week).
pub fn catalogue() -> Result<Vec<OnlineModel>> {
    catalogue_of("models")
}

/// The catalogue of ground / surface textures (cached for a week).
pub fn texture_catalogue() -> Result<Vec<OnlineModel>> {
    catalogue_of("textures")
}

fn catalogue_of(kind: &str) -> Result<Vec<OnlineModel>> {
    let dir = cache_dir();
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("polyhaven_{kind}.json"));
    let fresh = std::fs::metadata(&file).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e.as_secs() < 7 * 24 * 3600));
    let text = if fresh {
        std::fs::read_to_string(&file)?
    } else {
        let b = get_bytes(&format!("https://api.polyhaven.com/assets?t={kind}")).context("could not reach Poly Haven")?;
        std::fs::write(&file, &b)?;
        String::from_utf8_lossy(&b).to_string()
    };
    let v: serde_json::Value = serde_json::from_str(&text).context("reading the Poly Haven catalogue")?;
    let strs = |x: Option<&serde_json::Value>| x.and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|s| s.as_str().map(|s| s.to_ascii_lowercase())).collect()).unwrap_or_default();
    let mut out: Vec<OnlineModel> = v.as_object().into_iter().flatten().map(|(id, a)| OnlineModel {
        id: id.clone(),
        name: a.get("name").and_then(|n| n.as_str()).unwrap_or(id).to_string(),
        categories: strs(a.get("categories")),
        tags: strs(a.get("tags")),
        polycount: a.get("polycount").and_then(|p| p.as_u64()).unwrap_or(0),
    }).collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// The best matches for a search ("wooden barrel", "mossy boulder"), best first. Modern things
/// (electronics, appliances, vehicles…) are never offered: they do not fit a fantasy map.
pub fn search<'a>(cat: &'a [OnlineModel], query: &str) -> Vec<&'a OnlineModel> {
    const MODERN: [&str; 12] = ["electronics", "appliances", "office", "firearms", "plastic", "computer", "phone", "car", "concrete", "industrial", "radioactive", "modern"];
    let words: Vec<String> = query.to_ascii_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3).map(|w| w.trim_end_matches('s').to_string()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(f32, &OnlineModel)> = cat.iter().filter(|m| !m.categories.iter().chain(&m.tags).any(|t| MODERN.iter().any(|x| t.contains(x)))).filter_map(|m| {
        let id = m.id.to_ascii_lowercase();
        let mut s = 0.0;
        for w in &words {
            if id.contains(w.as_str()) {
                s += 3.0;
            }
            if m.tags.iter().any(|t| t.contains(w.as_str())) {
                s += 2.0;
            }
            if m.categories.iter().any(|t| t.contains(w.as_str())) {
                s += 1.0;
            }
        }
        // The object itself (the last word, "barrel" in "wooden barrel") must match.
        let noun = words.last().map(|w| w.as_str()).unwrap_or("");
        if !id.contains(noun) && !m.tags.iter().any(|t| t.contains(noun)) {
            return None;
        }
        // Lighter models are closer to the classic polygon budget.
        (s > 0.0).then_some((s - (m.polycount as f32 / 200_000.0).min(1.0), m))
    }).collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.id.cmp(&b.1.id)));
    scored.into_iter().map(|x| x.1).collect()
}

/// Downloads a model (1k glTF and its files); returns the .gltf path.
pub fn download(id: &str) -> Result<PathBuf> {
    let dir = cache_dir().join("polyhaven").join(asset::safe_name(id));
    let b = get_bytes(&format!("https://api.polyhaven.com/files/{id}"))?;
    let v: serde_json::Value = serde_json::from_slice(&b).context("reading the model's file list")?;
    let g = v.pointer("/gltf/1k/gltf").or_else(|| v.pointer("/gltf/2k/gltf")).context("the model has no glTF download")?;
    let url = g.get("url").and_then(|u| u.as_str()).context("no glTF url")?;
    let main = dir.join(url.rsplit('/').next().unwrap_or("model.gltf"));
    if !main.is_file() {
        std::fs::create_dir_all(&dir)?;
        if let Some(inc) = g.get("include").and_then(|i| i.as_object()) {
            for (rel, f) in inc {
                if rel.contains("..") {
                    bail!("unexpected file path in the download");
                }
                let u = f.get("url").and_then(|u| u.as_str()).context("no file url")?;
                let to = dir.join(rel);
                if let Some(p) = to.parent() {
                    std::fs::create_dir_all(p)?;
                }
                std::fs::write(&to, get_bytes(u)?)?;
            }
        }
        std::fs::write(&main, get_bytes(url)?)?;
    }
    Ok(main)
}

/// Picks the scenery category for an online model.
pub fn category_for(m: &OnlineModel, wanted: &str) -> String {
    if !wanted.trim().is_empty() {
        return wanted.to_string();
    }
    let has = |w: &str| m.categories.iter().chain(&m.tags).any(|t| t.contains(w));
    if has("tree") {
        "Trees".into()
    } else if has("rock") || has("boulder") || has("stone") {
        "Rocks".into()
    } else if has("plant") || has("flower") || has("grass") || has("bush") {
        "Plants & flowers".into()
    } else if has("statue") || has("ruin") {
        "Statues & ruins".into()
    } else if has("lantern") || has("lamp") || has("candle") || has("torch") {
        "Lights".into()
    } else {
        "Props".into()
    }
}

/// The Map1 texture whose colours an online model of this category is graded toward.
fn reference_texture(category: &str) -> &'static [&'static str] {
    match category {
        "Trees" => &["Tree01_md.dds", "Tree02_.dds"],
        "Rocks" => &["stonehenge_revised.dds", "Stonehenge_03.dds"],
        "Plants & flowers" => &["Plant01.dds", "foliage_sr_leaves.dds"],
        "Statues & ruins" => &["GoodStatue01.dds", "Stonehenge_03.dds"],
        "Lights" => &["OrderTorch01.dds", "TorchSet01.dds"],
        "Base & walls" => &["OrderGate.dds", "ChaosShrineWall02.dds"],
        _ => &["wagon_tx_dm.dds", "barrel_tx_dm.dds", "Fence01.dds"],
    }
}

fn channel_stats(img: &Image) -> ([f32; 3], [f32; 3]) {
    let mut sum = [0f64; 3];
    let mut sq = [0f64; 3];
    let mut n = 0f64;
    for px in img.pixels.chunks_exact(4) {
        if px[3] < 128 {
            continue;
        }
        for k in 0..3 {
            let v = px[k] as f64;
            sum[k] += v;
            sq[k] += v * v;
        }
        n += 1.0;
    }
    let n = n.max(1.0);
    let mean = [0, 1, 2].map(|k| (sum[k] / n) as f32);
    let sd = [0, 1, 2].map(|k| ((sq[k] / n - (sum[k] / n).powi(2)).max(1.0)).sqrt() as f32);
    (mean, sd)
}

fn resize(img: &Image, max: usize) -> Image {
    if img.width <= max && img.height <= max {
        return img.clone();
    }
    let s = max as f32 / img.width.max(img.height) as f32;
    let (w, h) = (((img.width as f32 * s) as usize).max(1), ((img.height as f32 * s) as usize).max(1));
    let src = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.pixels.clone()).expect("image size");
    let out = image::imageops::resize(&src, w as u32, h as u32, image::imageops::FilterType::Triangle);
    Image { width: w, height: h, pixels: out.into_raw() }
}

/// The classic look: a small texture, softened and posterized like hand-painted 2009-2014 art,
/// richer colour, then graded toward a Map1 texture of the same kind.
pub fn classic_texture(img: &Image, reference: Option<&Image>) -> Image {
    classic_texture_sized(img, reference, 256)
}

/// The same at a chosen size (ground textures keep 512 so they tile well).
pub fn classic_texture_sized(img: &Image, reference: Option<&Image>, max: usize) -> Image {
    let mut out = resize(img, max);
    let (w, h) = (out.width, out.height);
    // Soften fine photographic detail.
    let src = out.pixels.clone();
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u32; 3];
            let mut n = 0;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    // Wrapping, so tiling textures stay seamless.
                    let (xx, yy) = ((x as i64 + dx).rem_euclid(w as i64) as usize, (y as i64 + dy).rem_euclid(h as i64) as usize);
                    let k = (yy * w + xx) * 4;
                    for c in 0..3 {
                        acc[c] += src[k + c] as u32;
                    }
                    n += 1;
                }
            }
            let k = (y * w + x) * 4;
            for c in 0..3 {
                out.pixels[k + c] = ((acc[c] / n) as f32 * 0.6 + src[k + c] as f32 * 0.4) as u8;
            }
        }
    }
    let (m0, s0) = channel_stats(&out);
    let target = reference.map(channel_stats);
    for px in out.pixels.chunks_exact_mut(4) {
        let mut c = [px[0] as f32, px[1] as f32, px[2] as f32];
        // Grade toward the Map1 texture (mean and spread per channel), mostly.
        if let Some((m1, s1)) = target {
            for k in 0..3 {
                let t = (c[k] - m0[k]) / s0[k] * s1[k] + m1[k];
                c[k] = c[k] * 0.3 + t * 0.7;
            }
        }
        // A little more colour, and darker like classic textures with their shading painted in.
        let l = (c[0] + c[1] + c[2]) / 3.0;
        for v in &mut c {
            *v = (l + (*v - l) * 1.12) * 0.88;
        }
        // Posterize into soft bands.
        for (k, v) in c.iter().enumerate() {
            let q = 255.0 / 24.0;
            px[k] = (((v / q).round() * q) * 0.5 + v * 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Downloads a ground texture and restyles it like Map1's ground (graded toward the matching
/// Map1 texture; volcanic ones keep their own colours). Returns the saved PNG, usable as a ground
/// layer, cliff or brush texture.
pub fn add_texture(m: &OnlineModel, map1_textures: Option<&Path>) -> Result<PathBuf> {
    let dir = asset::library_dir().parent().map(|p| p.join("textures")).unwrap_or_else(|| PathBuf::from("textures"));
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{}_classic.png", asset::safe_name(&m.id)));
    if out.is_file() {
        return Ok(out);
    }
    let b = get_bytes(&format!("https://api.polyhaven.com/files/{}", m.id))?;
    let v: serde_json::Value = serde_json::from_slice(&b).context("reading the texture's file list")?;
    let url = v.pointer("/Diffuse/1k/jpg/url").or_else(|| v.pointer("/Diffuse/2k/jpg/url")).and_then(|u| u.as_str()).context("the texture has no colour map")?;
    let img = image::load_from_memory(&get_bytes(url)?).context("decoding the texture")?.to_rgba8();
    let (w, h) = img.dimensions();
    let img = Image { width: w as usize, height: h as usize, pixels: img.into_raw() };
    let has = |x: &str| m.tags.iter().chain(&m.categories).any(|t| t.contains(x)) || m.id.to_ascii_lowercase().contains(x);
    let reference = if ["lava", "burn", "ash", "volcan", "basalt", "obsidian", "magma", "snow", "ice", "frost", "frozen"].iter().any(|x| has(x)) {
        None
    } else if has("grass") || has("moss") {
        Some("grass_tx_cm.dds")
    } else if has("sand") {
        Some("td_sand1.dds")
    } else if has("rock") || has("cliff") || has("stone") {
        Some("rocks.dds")
    } else {
        Some("order_dirt.dds")
    };
    let reference = reference.and_then(|f| map1_textures.and_then(|d| std::fs::read_dir(d).ok()).and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(f))))).and_then(|p| std::fs::read(p).ok()).and_then(|b| dds::decode(&b, 256).ok());
    let t = classic_texture_sized(&img, reference.as_ref(), 512);
    image::RgbaImage::from_raw(t.width as u32, t.height as u32, t.pixels).context("texture size")?.save(&out)?;
    Ok(out)
}

/// Typical height of a category on Map1 (median), for League scale.
fn classic_height(lib: Option<&PropLibrary>, category: &str) -> f32 {
    let mut v: Vec<f32> = lib.map(|l| l.props.iter().filter(|p| p.key.starts_with("Map1/") && p.category == category && p.height > 10.0).map(|p| p.height).collect()).unwrap_or_default();
    if v.is_empty() {
        return match category {
            "Trees" => 330.0,
            "Rocks" => 160.0,
            "Statues & ruins" => 260.0,
            _ => 120.0,
        };
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Downloads a model and adds it to the asset library in the classic Map1 style; returns its key
/// ("asset:<name>"). `map1_textures` is Map1's Scene/Textures folder (for the colour grading).
pub fn add_classic(m: &OnlineModel, category: &str, lib: Option<&PropLibrary>, map1_textures: Option<&Path>) -> Result<String> {
    let name = format!("{} (classic)", m.name);
    let key = format!("asset:{name}");
    if asset::asset_dir(&name).join("asset.json").is_file() {
        return Ok(key);
    }
    let path = download(&m.id)?;
    let model = crate::import3d::import_file(&path).with_context(|| format!("reading {}", m.name))?;
    let mut a = Asset::new(&name);
    a.category = category.to_string();
    a.source = format!("Poly Haven: {} (CC0)", m.id);
    let reference = map1_textures.and_then(|d| reference_texture(category).iter().find_map(|f| std::fs::read(d.join(f)).ok()).and_then(|b| dds::decode(&b, 256).ok()));
    let mut textures: HashMap<String, Image> = HashMap::new();
    for p in model.parts {
        // Parts sharing a texture file share one restyled copy.
        let texture = p.texture.map(|t| {
            let n = asset::safe_name(&t.name);
            textures.entry(n.clone()).or_insert_with(|| classic_texture(&t.image, reference.as_ref()));
            n
        });
        a.parts.push(AssetPart { name: p.name, mesh: Mesh { positions: p.positions, normals: p.normals, uvs: p.uvs, colors: Vec::new(), indices: p.indices }, texture, color: p.color, visible: true });
    }
    if a.parts.is_empty() {
        bail!("{} has no meshes", m.name);
    }
    // Classic polygon budget.
    let tris: usize = a.parts.iter().map(|p| p.mesh.indices.len() / 3).sum();
    let budget = if category == "Trees" { 1800 } else { 1000 };
    if tris > budget {
        let r = budget as f32 / tris as f32;
        for p in &mut a.parts {
            meshops::decimate(&mut p.mesh, r);
        }
    }
    // Standing on the ground at its centre, at League scale (1 m ≈ 100 units, kept near the
    // size such things have on Map1).
    let (lo, hi) = a.bounds();
    let height_m = (hi[1] - lo[1]).max(0.01);
    let typical = classic_height(lib, category);
    let want = (height_m * 100.0).clamp(typical * 0.5, typical * 1.5);
    let s = want / height_m;
    let off = [-(lo[0] + hi[0]) / 2.0, -lo[1], -(lo[2] + hi[2]) / 2.0];
    let t = meshops::trs([0.0; 3], [0.0; 3], [s; 3]);
    let mv = meshops::trs(off, [0.0; 3], [1.0; 3]);
    for p in &mut a.parts {
        meshops::transform(&mut p.mesh, &mv);
        meshops::transform(&mut p.mesh, &t);
    }
    a.blocks = matches!(category, "Rocks" | "Statues & ruins" | "Base & walls") || (category == "Props" && want > 80.0);
    asset::save(&a, &textures)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_ranks_and_filters_modern() {
        let mk = |id: &str, cats: &[&str], tags: &[&str], poly: u64| OnlineModel { id: id.into(), name: id.into(), categories: cats.iter().map(|s| s.to_string()).collect(), tags: tags.iter().map(|s| s.to_string()).collect(), polycount: poly };
        let cat = vec![mk("Barrel_01", &["industrial", "containers"], &["barrel", "oil"], 2000), mk("wine_barrel_01", &["props", "containers"], &["barrel", "wooden"], 3000), mk("boulder_01", &["nature", "rocks"], &["boulder", "rock"], 5000)];
        let r = search(&cat, "wooden barrels");
        assert_eq!(r[0].id, "wine_barrel_01");
        assert!(r.iter().all(|m| m.id != "Barrel_01"), "industrial barrel filtered");
        assert_eq!(search(&cat, "rocks")[0].id, "boulder_01");
        assert_eq!(category_for(&cat[2], ""), "Rocks");
    }


    #[test]
    fn classic_texture_grades_toward_reference() {
        let img = Image { width: 8, height: 8, pixels: [200u8, 200, 200, 255].repeat(64) };
        let mut rp = Vec::new();
        for k in 0..64 {
            rp.extend_from_slice(&[40 + (k % 2) as u8 * 20, 90 + (k % 3) as u8 * 10, 30, 255]);
        }
        let r = Image { width: 8, height: 8, pixels: rp };
        let o = classic_texture(&img, Some(&r));
        // Greener and darker than the grey source.
        assert!(o.pixels[1] > o.pixels[0] && o.pixels[1] < 200, "{:?}", &o.pixels[..4]);
    }
}
