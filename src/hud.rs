//! The game's in-match HUD, read from `DATA/menu/hud` the way the client lays it out:
//! element files listed in
//! `ElementsListNormal.ini`, each with icons, texts and effects placed in a 1024x768 authoring
//! space around an anchor.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Icon,
    Text,
    Effect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug)]
pub struct Element {
    pub kind: Kind,
    pub name: String,
    pub group: String,
    pub enabled: bool,
    pub layer: i32,
    pub left_anchor: [f32; 2],
    pub right_anchor: [f32; 2],
    /// Normalised (0..1) rectangle: left, top, right, bottom.
    pub rect: [f32; 4],
    pub color: [u8; 4],
    /// Alpha blending on (`Alpha: 1`); otherwise the icon draws opaque.
    pub blend: bool,
    pub texture: String,
    /// Normalised texture rectangle.
    pub uv: [f32; 4],
    pub font_size: f32,
    pub align: Align,
    pub drop_shadow: bool,
    pub keep_max_scale: bool,
    pub per_pixel_uvs_x: bool,
    pub effect: String,
    /// Element file it came from (stem).
    pub file: String,
}

impl Element {
    fn new(kind: Kind, name: &str, file: &str) -> Self {
        Self {
            kind,
            name: name.to_string(),
            group: String::new(),
            enabled: true,
            layer: 0,
            left_anchor: [0.0; 2],
            right_anchor: [0.0; 2],
            rect: [0.0; 4],
            color: [255; 4],
            blend: false,
            texture: String::new(),
            uv: [0.0, 0.0, 1.0, 1.0],
            font_size: 12.0,
            align: Align::Left,
            drop_shadow: false,
            keep_max_scale: false,
            per_pixel_uvs_x: false,
            effect: String::new(),
            file: file.to_string(),
        }
    }

    /// Screen rectangle in pixels (CalcElementDisplayCoords with global scale 1, user scale 1).
    pub fn screen_rect(&self, w: f32, h: f32) -> [f32; 4] {
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let r = self.rect;
        // Global HUD scale 1.0: Lerp(0.5, 1, 1) = 1, so the default rect is unchanged.
        let ratio = h / 768.0;
        let sx = ratio * 1024.0 / w;
        let sy = ratio * 768.0 / h;
        let x0 = lerp(self.left_anchor[0], r[0], sx);
        let y0 = lerp(self.left_anchor[1], r[1], sy);
        let x1 = lerp(self.right_anchor[0], r[2], sx);
        let y1 = lerp(self.right_anchor[1], r[3], sy);
        [(x0 * w).floor(), (y0 * h).floor(), (x1 * w).ceil(), (y1 * h).ceil()]
    }

    /// Font size in pixels at this screen height.
    pub fn font_px(&self, h: f32) -> f32 {
        self.font_size * h / 768.0
    }
}

/// A HUD scene (Scenes.ini): the group elements belong to, shown or hidden as a whole and
/// drawn in its own layer.
#[derive(Clone, Debug)]
pub struct Scene {
    pub enabled: bool,
    pub layer: i32,
}

pub struct Hud {
    pub root: PathBuf,
    pub elements: Vec<Element>,
    pub scenes: std::collections::HashMap<String, Scene>,
}

pub fn parse_scenes(text: &str) -> std::collections::HashMap<String, Scene> {
    let mut out = std::collections::HashMap::new();
    let mut name: Option<String> = None;
    let mut sc = Scene { enabled: true, layer: 0 };
    for line in text.lines().map(str::trim) {
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Name" => {
                if let Some(n) = name.take() {
                    out.insert(n, sc.clone());
                }
                name = Some(v.to_string());
                sc = Scene { enabled: true, layer: 0 };
            }
            "Enabled" => sc.enabled = v.starts_with('1'),
            "Layer" => sc.layer = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    if let Some(n) = name {
        out.insert(n, sc);
    }
    out
}

fn nums(s: &str) -> Vec<f32> {
    s.split(|c: char| c == ',' || c.is_whitespace() || c == '/' || c == 'x')
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect()
}

/// "x0,y0 - x1,y1 / WxH" (the size part is optional).
fn rect4(s: &str) -> Option<[f32; 4]> {
    let (body, size) = match s.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    // Split on the " - " between corners; corner values may themselves be negative.
    let (a, b) = body.split_once(" - ").or_else(|| body.split_once("- "))?;
    let pa = nums(a);
    let pb = nums(b);
    if pa.len() < 2 || pb.len() < 2 {
        return None;
    }
    let mut r = [pa[0], pa[1], pb[0], pb[1]];
    if let Some(sz) = size {
        let d = nums(sz);
        if d.len() >= 2 && d[0] > 0.0 && d[1] > 0.0 {
            r = [r[0] / d[0], r[1] / d[1], r[2] / d[0], r[3] / d[1]];
        }
    }
    Some(r)
}

fn pair(s: &str) -> Option<[f32; 2]> {
    let v = nums(s);
    (v.len() >= 2).then(|| [v[0], v[1]])
}

pub fn parse_file(text: &str, file: &str) -> Vec<Element> {
    let mut out: Vec<Element> = Vec::new();
    let mut base = [0.0f32; 2];
    let mut cur: Option<Element> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('/') || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "BaseOffset" if cur.is_none() => base = pair(v).unwrap_or([0.0; 2]),
            "Type" => {
                if let Some(e) = cur.take() {
                    out.push(e);
                }
                let kind = match v {
                    "Icon" => Some(Kind::Icon),
                    "Text" => Some(Kind::Text),
                    "Effect" => Some(Kind::Effect),
                    _ => None,
                };
                cur = kind.map(|k| Element::new(k, "", file));
            }
            _ => {
                let Some(e) = cur.as_mut() else { continue };
                match k {
                    "Name" => e.name = v.to_string(),
                    "Group" => e.group = v.to_string(),
                    "Enabled" => e.enabled = v.starts_with('1'),
                    "Layer" => e.layer = v.parse().unwrap_or(0),
                    "Anchor" => {
                        let a = pair(v).unwrap_or([0.0; 2]);
                        e.left_anchor = a;
                        e.right_anchor = a;
                    }
                    "LeftAnchor" => e.left_anchor = pair(v).unwrap_or([0.0; 2]),
                    "RightAnchor" => e.right_anchor = pair(v).unwrap_or([0.0; 2]),
                    "Rect" => {
                        if let Some(r) = rect4(v) {
                            e.rect = [r[0] + base[0], r[1] + base[1], r[2] + base[0], r[3] + base[1]];
                        }
                    }
                    "Color" => {
                        let c = nums(v);
                        if c.len() == 4 {
                            e.color = [c[0] as u8, c[1] as u8, c[2] as u8, c[3] as u8];
                        }
                    }
                    "Alpha" => e.blend = v.starts_with('1'),
                    "Texture" => e.texture = v.split_whitespace().next().unwrap_or("").to_string(),
                    "UV" | "MinUV" => {
                        if let Some(r) = rect4(v) {
                            e.uv = r;
                        }
                    }
                    "Font" => {}
                    "Size" => e.font_size = v.parse().unwrap_or(12.0),
                    "Align" => {
                        e.align = match v {
                            "Center" => Align::Center,
                            "Right" => Align::Right,
                            _ => Align::Left,
                        }
                    }
                    "DropShadow" => e.drop_shadow = v.starts_with('1'),
                    "KeepMaxScale" => e.keep_max_scale = v.starts_with('1'),
                    "PerPixelUVsX" => e.per_pixel_uvs_x = v.starts_with('1'),
                    "Effect" => e.effect = v.to_string(),
                    _ => {}
                }
            }
        }
    }
    if let Some(e) = cur.take() {
        out.push(e);
    }
    out.retain(|e| !e.name.is_empty());
    out
}

fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let lower = name.to_ascii_lowercase();
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|f| f.to_string_lossy().to_ascii_lowercase() == lower))
}

impl Hud {
    /// Loads the normal (not flipped) HUD from `<data>/menu/hud`.
    pub fn load(data_root: &Path) -> Result<Self> {
        let menu = find_ci(data_root, "menu").context("DATA/menu is missing")?;
        let root = find_ci(&menu, "hud").context("DATA/menu/hud is missing")?;
        let elems = find_ci(&root, "Elements").context("menu/hud/Elements is missing")?;
        let list = std::fs::read_to_string(root.join("ElementsListNormal.ini")).context("ElementsListNormal.ini is missing")?;
        let mut elements = Vec::new();
        for f in list.lines().map(str::trim).filter(|l| l.to_ascii_lowercase().ends_with(".ini")) {
            let Some(p) = find_ci(&elems, f) else { continue };
            let Ok(b) = std::fs::read(&p) else { continue };
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            elements.extend(parse_file(&String::from_utf8_lossy(&b), &stem));
        }
        let scenes = std::fs::read(root.join("Scenes.ini")).map(|b| parse_scenes(&String::from_utf8_lossy(&b))).unwrap_or_default();
        Ok(Self { root, elements, scenes })
    }

    /// A HUD texture by its element name (HUDAtlas -> menu/Textures/HUDAtlas.dds).
    pub fn texture_path(&self, name: &str) -> Option<PathBuf> {
        let tex = find_ci(self.root.parent()?, "Textures")?;
        find_ci(&tex, &format!("{name}.dds")).or_else(|| find_ci(&tex, &format!("{name}.tga")))
    }

    /// The scene layer an element draws in (elements without a known scene draw at 0).
    pub fn scene_layer(&self, e: &Element) -> i32 {
        self.scenes.get(&e.group).map_or(0, |s| s.layer)
    }

    /// Whether the element's scene starts shown (overrides name scenes the game switches on).
    pub fn scene_on(&self, e: &Element, overrides: &[(&str, bool)]) -> bool {
        if let Some((_, on)) = overrides.iter().find(|(n, _)| *n == e.group) {
            return *on;
        }
        self.scenes.get(&e.group).is_none_or(|s| s.enabled)
    }

    pub fn get(&self, name: &str) -> Option<&Element> {
        self.elements.iter().find(|e| e.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_and_scaling() {
        let r = rect4("300.8,657.92 - 752.0,769.28 / 1024x768").unwrap();
        assert!((r[0] - 0.29375).abs() < 1e-5 && (r[3] - 1.001_667).abs() < 1e-4);
        let r = rect4("-0.64,661.76 - 251.52,768.64 / 1024x768").unwrap();
        assert!(r[0] < 0.0);
        let mut e = Element::new(Kind::Icon, "x", "f");
        e.rect = [0.25, 0.5, 0.75, 1.0];
        e.left_anchor = [0.5, 1.0];
        e.right_anchor = [0.5, 1.0];
        // 4:3 screen: unchanged; 16:9 at 1080: x shrinks toward the anchor.
        assert_eq!(e.screen_rect(1024.0, 768.0), [256.0, 384.0, 768.0, 768.0]);
        let s = e.screen_rect(1920.0, 1080.0);
        assert_eq!(s[1], 540.0);
        assert!((s[2] - s[0] - 0.5 * 1080.0 * 1024.0 / 768.0).abs() < 2.0);
    }
}
