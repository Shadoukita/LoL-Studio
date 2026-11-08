//! Making new ground textures from existing ones: tint, blend another texture in through a mask
//! (bright parts, dark parts or seamless noise), glowing cracks, speckles, desaturation and
//! brightness. Results stay seamless so they tile on the ground.

use crate::formats::dds::{self, Image};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;

pub const SIZE: usize = 512;

/// Loads a .dds / .png / .jpg texture as RGBA, resized to SIZE x SIZE.
pub fn load(path: &Path) -> Result<Image> {
    let low = path.to_string_lossy().to_ascii_lowercase();
    let img = if low.ends_with(".dds") {
        dds::decode(&std::fs::read(path).with_context(|| format!("reading {}", path.display()))?, 2048)?
    } else {
        let i = image::open(path).with_context(|| format!("reading {}", path.display()))?.to_rgba8();
        let (w, h) = i.dimensions();
        Image { width: w as usize, height: h as usize, pixels: i.into_raw() }
    };
    Ok(resize(&img, SIZE))
}

/// An image resized to SIZE x SIZE.
pub fn load_image(img: Image) -> Image {
    resize(&img, SIZE)
}

fn resize(img: &Image, n: usize) -> Image {
    if img.width == n && img.height == n {
        return img.clone();
    }
    let src = image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.pixels.clone()).expect("image size");
    let out = image::imageops::resize(&src, n as u32, n as u32, image::imageops::FilterType::Triangle);
    Image { width: n, height: n, pixels: out.into_raw() }
}

/// Seamless value noise 0..1 at (u, v) in 0..1: hashed lattice values wrapped on the tile,
/// smoothly interpolated, four octaves.
pub fn noise(u: f32, v: f32, scale: f32, seed: u32) -> f32 {
    let mut n = 0.0;
    let mut amp = 0.5;
    let mut total = 0.0;
    let mut f = scale.max(1.0).round() as i64;
    for o in 0..4u32 {
        let (x, y) = (u * f as f32, v * f as f32);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let h = |i: i64, j: i64| crate::layout_build::hash01(i.rem_euclid(f) as u32, j.rem_euclid(f) as u32, seed * 13 + o);
        let (i, j) = (x0 as i64, y0 as i64);
        let top = h(i, j) * (1.0 - sx) + h(i + 1, j) * sx;
        let bot = h(i, j + 1) * (1.0 - sx) + h(i + 1, j + 1) * sx;
        n += amp * (top * (1.0 - sy) + bot * sy);
        total += amp;
        amp *= 0.5;
        f *= 2;
    }
    // Stretch the narrow middle range of summed octaves back toward 0..1.
    (((n / total) - 0.5) * 1.8 + 0.5).clamp(0.0, 1.0)
}

fn lum(p: &[u8]) -> f32 {
    (0.3 * p[0] as f32 + 0.59 * p[1] as f32 + 0.11 * p[2] as f32) / 255.0
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    // Reversed edges fade the other way.
    if e1 < e0 {
        return 1.0 - smooth(e1, e0, x);
    }
    let t = ((x - e0) / (e1 - e0).max(1e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn color(v: &Value, k: &str, d: [f32; 3]) -> [f32; 3] {
    v.get(k).and_then(|c| c.as_array()).map(|a| [0, 1, 2].map(|i| a.get(i).and_then(|x| x.as_f64()).unwrap_or(d[i] as f64) as f32)).unwrap_or(d)
}

fn num(v: &Value, k: &str, d: f32) -> f32 {
    v.get(k).and_then(|x| x.as_f64()).map_or(d, |x| x as f32)
}

/// Applies the steps to a base texture. `resolve` turns a texture name into a file.
pub fn compose(base: Image, steps: &[Value], resolve: &dyn Fn(&str) -> Option<std::path::PathBuf>) -> Result<Image> {
    let mut img = base;
    let n = img.width;
    for (si, st) in steps.iter().enumerate() {
        let op = st.get("op").and_then(|o| o.as_str()).unwrap_or("");
        let amount = num(st, "amount", 1.0).clamp(0.0, 1.0);
        let seed = 17 + si as u32 * 31;
        match op {
            "tint" => {
                let c = color(st, "color", [255.0; 3]);
                for p in img.pixels.chunks_exact_mut(4) {
                    let l = lum(p);
                    for k in 0..3 {
                        let t = c[k] * l * 1.6;
                        p[k] = (p[k] as f32 * (1.0 - amount) + t.min(255.0) * amount).clamp(0.0, 255.0) as u8;
                    }
                }
            }
            "desaturate" => {
                for p in img.pixels.chunks_exact_mut(4) {
                    let l = lum(p) * 255.0;
                    for k in 0..3 {
                        p[k] = (p[k] as f32 * (1.0 - amount) + l * amount) as u8;
                    }
                }
            }
            "brightness" => {
                let m = num(st, "factor", 1.0);
                for p in img.pixels.chunks_exact_mut(4) {
                    for k in 0..3 {
                        p[k] = (p[k] as f32 * m).clamp(0.0, 255.0) as u8;
                    }
                }
            }
            "overlay" => {
                // Another texture (or a flat colour) blended in where the mask says.
                let other = match st.get("texture").and_then(|t| t.as_str()) {
                    Some(t) => Some(load(&resolve(t).with_context(|| format!("texture \"{t}\" not found"))?)?),
                    None => None,
                };
                let c = color(st, "color", [235.0, 240.0, 250.0]);
                let mask = st.get("mask").and_then(|m| m.as_str()).unwrap_or("noise");
                let (th, soft, scale) = (num(st, "threshold", 0.5), num(st, "softness", 0.15), num(st, "scale", 4.0));
                let src = img.pixels.clone();
                for y in 0..n {
                    for x in 0..n {
                        let k = (y * n + x) * 4;
                        let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
                        let l = lum(&src[k..k + 4]);
                        let nz = noise(u, v, scale, seed);
                        let m = match mask {
                            "bright" => smooth(th - soft, th + soft, l * 0.7 + nz * 0.3),
                            "dark" => smooth(th + soft, th - soft, l * 0.7 + (1.0 - nz) * 0.3),
                            "all" => 1.0,
                            _ => smooth(th - soft, th + soft, nz),
                        } * amount;
                        for ch in 0..3 {
                            let o = other.as_ref().map_or(c[ch], |t| t.pixels[k + ch] as f32);
                            img.pixels[k + ch] = (src[k + ch] as f32 * (1.0 - m) + o * m) as u8;
                        }
                    }
                }
            }
            "cracks" => {
                // Thin glowing (or dark) veins following the noise's zero lines, plus the texture's
                // own darkest crevices.
                let c = color(st, "color", [255.0, 120.0, 30.0]);
                let (scale, width) = (num(st, "scale", 3.0), num(st, "width", 0.035));
                let src = img.pixels.clone();
                for y in 0..n {
                    for x in 0..n {
                        let k = (y * n + x) * 4;
                        let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
                        let a = (noise(u, v, scale, seed) - 0.5).abs();
                        let b = (noise(u, v, scale * 2.0, seed + 5) - 0.5).abs();
                        let vein = smooth(width, 0.0, a).max(smooth(width * 0.6, 0.0, b) * 0.7);
                        let crevice = smooth(0.22, 0.08, lum(&src[k..k + 4])) * 0.6;
                        let m = (vein.max(crevice) * amount).min(1.0);
                        for ch in 0..3 {
                            img.pixels[k + ch] = (src[k + ch] as f32 * (1.0 - m) + c[ch] * m).clamp(0.0, 255.0) as u8;
                        }
                    }
                }
            }
            "speckle" => {
                let c = color(st, "color", [250.0, 250.0, 255.0]);
                let density = num(st, "density", 0.05);
                for y in 0..n {
                    for x in 0..n {
                        if crate::layout_build::hash01(x as u32, y as u32, seed) < density {
                            let k = (y * n + x) * 4;
                            for ch in 0..3 {
                                img.pixels[k + ch] = (img.pixels[k + ch] as f32 * (1.0 - amount) + c[ch] * amount) as u8;
                            }
                        }
                    }
                }
            }
            o => bail!("unknown step \"{o}\" (tint, desaturate, brightness, overlay, cracks, speckle)"),
        }
    }
    // Ground textures are opaque.
    for p in img.pixels.chunks_exact_mut(4) {
        p[3] = 255;
    }
    Ok(img)
}

pub fn save_png(img: &Image, path: &Path) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.pixels.clone()).context("image size")?.save(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_tiles_and_steps_apply() {
        // Seamless: the left and right edges agree.
        for k in 0..20 {
            let v = k as f32 / 20.0;
            assert!((noise(0.0, v, 4.0, 3) - noise(1.0, v, 4.0, 3)).abs() < 1e-3);
        }
        let base = Image { width: SIZE, height: SIZE, pixels: [90u8, 70, 50, 255].repeat(SIZE * SIZE) };
        let steps: Vec<Value> = serde_json::from_str(r#"[{"op":"overlay","mask":"noise","threshold":0.5,"color":[240,245,255]},{"op":"cracks","color":[255,120,30],"amount":1}]"#).unwrap();
        let out = compose(base, &steps, &|_| None).unwrap();
        let white = out.pixels.chunks_exact(4).filter(|p| p[2] > 200).count();
        assert!(white > SIZE * SIZE / 5 && white < SIZE * SIZE * 4 / 5, "{white}");
        assert!(out.pixels.chunks_exact(4).any(|p| p[0] > 200 && p[2] < 100), "glowing veins");
    }
}
