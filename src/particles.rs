//! Particle effect previews. Map effects (`Foo.troy`) are stored as `DATA/Particles/Foo.troybin`
//! (inibin version 2). Each effect lists emitters (`System*GroupPartN`) whose settings live under
//! the emitter's own section; this module reads the common ones and simulates them on the CPU as
//! billboards, which is enough to preview torches, cauldrons and similar ambient effects.

use crate::formats::inibin::{Inibin, Value, hash_key};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Most live particles one emitter keeps.
const MAX_PARTICLES: usize = 200;
/// Highest `System*GroupPartN` index probed.
const MAX_GROUP_PARTS: u32 = 64;

/// One emitter of an effect, with the settings the preview uses.
#[derive(Clone, Debug)]
pub struct EmitterDef {
    pub name: String,
    /// The flipbook texture (`p-texture`, resolved to the .dds next to the effect).
    pub texture: Option<PathBuf>,
    /// Flipbook columns and rows (`p-texdiv`).
    pub texdiv: [u32; 2],
    /// Flipbook frames played over a particle's life (`p-numframes`).
    pub frames: u32,
    /// Particles per second (`e-rate`).
    pub rate: f32,
    /// How long the emitter spawns, in seconds; negative loops forever (`e-life`).
    pub emitter_life: f32,
    /// Particle life in seconds (`p-life`).
    pub life: f32,
    pub vel: [f32; 3],
    pub accel: [f32; 3],
    /// Half extents of the random spawn box (`p-offset`).
    pub offset: [f32; 3],
    /// Birth size (`p-scale`).
    pub scale: f32,
    /// Size multiplier over normalised age, as sorted (t, v) points (`p-xscaleN`).
    pub scale_curve: Vec<(f32, f32)>,
    /// Colour/alpha ramp over life (`p-rgba`).
    pub ramp: Option<PathBuf>,
    /// The ramp's middle row, decoded at load time; empty means white.
    pub ramp_colors: Vec<[f32; 4]>,
    /// 0 = camera-facing billboard, 2 = flat on the ground (`p-simpleorient`).
    pub orient: u8,
    /// Additive blending (`rendermode` other than 1).
    pub additive: bool,
    /// Spin in radians per second (`p-rotvel`, its largest component). `p-quadrot` holds an
    /// orientation axis rather than an angle, so particles are born unrotated.
    pub rot_vel: f32,
}

/// A loaded effect: its emitters and a rough radius that bounds its particles.
#[derive(Clone, Debug)]
pub struct EffectDef {
    pub name: String,
    pub emitters: Vec<EmitterDef>,
    pub vis_radius: f32,
}

/// `DATA/Particles`, matched case-insensitively.
pub fn particles_dir(data_root: &Path) -> Option<PathBuf> {
    find_child(data_root, "particles").filter(|p| p.is_dir())
}

/// Every effect in `DATA/Particles`, as sorted map names like "Fire_Cauldron.troy".
pub fn list_effects(data_root: &Path) -> Vec<String> {
    let Some(dir) = particles_dir(data_root) else { return Vec::new() };
    let mut names: Vec<String> = dir_index(&dir)
        .values()
        .filter_map(|p| {
            let file = p.file_name()?.to_str()?;
            let stem = file.strip_suffix(".troybin").or_else(|| {
                file.to_ascii_lowercase().ends_with(".troybin").then(|| &file[..file.len() - 8])
            })?;
            Some(format!("{stem}.troy"))
        })
        .collect();
    names.sort_by_key(|n| n.to_ascii_lowercase());
    names
}

/// Loads `name` ("X.troy", "X.troybin" or "X") from `DATA/Particles`, with its ramps decoded.
pub fn load_effect(data_root: &Path, name: &str) -> Result<EffectDef> {
    let dir = particles_dir(data_root).context("DATA/Particles was not found")?;
    let index = dir_index(&dir);
    let stem = file_stem(name);
    let path = index
        .get(&format!("{}.troybin", stem.to_ascii_lowercase()))
        .with_context(|| format!("{stem}.troybin was not found in {}", dir.display()))?;
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let ini = Inibin::parse(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    let texture_path = |v: Option<&Value>| -> Option<PathBuf> {
        let Some(Value::Str(s)) = v else { return None };
        let s = file_stem(s.trim());
        (!s.is_empty()).then(|| index.get(&format!("{}.dds", s.to_ascii_lowercase())).cloned()).flatten()
    };

    let mut emitters = Vec::new();
    for n in 1..=MAX_GROUP_PARTS {
        let Some(Value::Str(part)) = ini.get(hash_key("System", &format!("GroupPart{n}"))) else { continue };
        let sec = part.trim();
        if sec.is_empty() {
            continue;
        }
        let get = |key: &str| ini.get(hash_key(sec, key));
        let f = |key: &str, default: f32| get(key).and_then(num).unwrap_or(default);
        if get("e-disabled").and_then(num).unwrap_or(0.0) != 0.0 {
            continue;
        }
        let texdiv = get("p-texdiv").map(vec_n).unwrap_or_default();
        let texdiv = [
            texdiv.first().copied().unwrap_or(1.0).max(1.0) as u32,
            texdiv.get(1).copied().unwrap_or(1.0).max(1.0) as u32,
        ];
        let mut scale_curve: Vec<(f32, f32)> = (1..=16)
            .filter_map(|i| {
                let v = vec_n(get(&format!("p-xscale{i}"))?);
                (v.len() >= 2).then(|| (v[0], v[1]))
            })
            .collect();
        scale_curve.sort_by(|a, b| a.0.total_cmp(&b.0));
        let ramp = texture_path(get("p-rgba"));
        let ramp_colors = ramp.as_deref().and_then(load_ramp).unwrap_or_default();
        let orient = f("p-simpleorient", 0.0) as u8;
        emitters.push(EmitterDef {
            name: sec.to_string(),
            texture: texture_path(get("p-texture")),
            texdiv,
            frames: (f("p-numframes", 1.0).max(1.0) as u32).min(texdiv[0] * texdiv[1]),
            rate: f("e-rate", 0.0).max(0.0),
            emitter_life: f("e-life", -1.0),
            life: {
                let l = f("p-life", 3.0);
                if l > 0.0 { l } else { 3.0 }
            },
            vel: vec3(get("p-vel")),
            accel: vec3(get("p-accel")),
            offset: vec3(get("p-offset")),
            scale: f("p-scale", 10.0),
            scale_curve,
            ramp,
            ramp_colors,
            orient,
            additive: f("rendermode", 0.0) as i32 != 1,
            rot_vel: spin(get("p-rotvel")).to_radians(),
        });
    }

    // Bound each emitter by spawn box + drift over its life + its largest size.
    let vis_radius = emitters
        .iter()
        .map(|e| {
            let t = e.life;
            let drift: f32 = (0..3)
                .map(|i| {
                    let d = e.offset[i].abs() + e.vel[i].abs() * t + 0.5 * e.accel[i].abs() * t * t;
                    d * d
                })
                .sum::<f32>()
                .sqrt();
            let peak = e.scale_curve.iter().map(|c| c.1).fold(1.0f32, f32::max);
            drift + e.scale.abs() * peak
        })
        .fold(0.0f32, f32::max)
        .max(1.0);
    Ok(EffectDef { name: format!("{}.troy", file_stem(name)), emitters, vis_radius })
}

/// Decodes a ramp texture's middle row into linear 0..1 RGBA.
pub fn load_ramp(path: &Path) -> Option<Vec<[f32; 4]>> {
    let img = crate::formats::dds::decode(&std::fs::read(path).ok()?, 256).ok()?;
    if img.width == 0 || img.height == 0 {
        return None;
    }
    let row = &img.pixels[(img.height / 2) * img.width * 4..][..img.width * 4];
    Some(row.chunks_exact(4).map(|p| [0, 1, 2, 3].map(|i| p[i] as f32 / 255.0)).collect())
}

/// One particle quad to draw.
#[derive(Clone, Copy, Debug)]
pub struct Billboard {
    pub center: [f32; 3],
    pub size: f32,
    pub rotation: f32,
    /// u0 v0 u1 v1 of the flipbook frame.
    pub uv: [f32; 4],
    pub color: [f32; 4],
    /// Lies flat in the XZ plane instead of facing the camera.
    pub flat: bool,
    pub additive: bool,
    /// Index into `EffectDef::emitters`.
    pub emitter: usize,
}

#[derive(Clone, Debug)]
struct Particle {
    pos: [f32; 3],
    vel: [f32; 3],
    age: f32,
    rotation: f32,
}

/// A running copy of an effect at a position.
#[derive(Clone, Debug)]
pub struct EffectInstance {
    def: Arc<EffectDef>,
    pos: [f32; 3],
    /// Live particles per emitter.
    particles: Vec<Vec<Particle>>,
    /// Fractional particles owed per emitter.
    spawn_acc: Vec<f32>,
    rng: u32,
    time: f32,
}

impl EffectInstance {
    pub fn new(def: Arc<EffectDef>, pos: [f32; 3], seed: u32) -> Self {
        let n = def.emitters.len();
        Self { def, pos, particles: vec![Vec::new(); n], spawn_acc: vec![0.0; n], rng: seed | 1, time: 0.0 }
    }

    pub fn def(&self) -> &EffectDef {
        &self.def
    }

    /// Random value in -1..1 (xorshift32).
    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }

    /// Advances the simulation: ages and moves particles, drops expired ones and spawns new ones.
    pub fn update(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.25);
        let def = self.def.clone();
        for (i, e) in def.emitters.iter().enumerate() {
            let list = &mut self.particles[i];
            for p in list.iter_mut() {
                p.age += dt;
                for k in 0..3 {
                    p.vel[k] += e.accel[k] * dt;
                    p.pos[k] += p.vel[k] * dt;
                }
                p.rotation += e.rot_vel * dt;
            }
            list.retain(|p| p.age < e.life);

            let spawning = e.emitter_life < 0.0 || self.time < e.emitter_life;
            if !spawning {
                self.spawn_acc[i] = 0.0;
                continue;
            }
            self.spawn_acc[i] += e.rate * dt;
            while self.spawn_acc[i] >= 1.0 {
                self.spawn_acc[i] -= 1.0;
                if self.particles[i].len() >= MAX_PARTICLES {
                    continue;
                }
                let jitter = [self.rand(), self.rand(), self.rand()];
                let pos = std::array::from_fn(|k| self.pos[k] + e.offset[k] * jitter[k]);
                self.particles[i].push(Particle { pos, vel: e.vel, age: 0.0, rotation: 0.0 });
            }
        }
        self.time += dt;
    }

    /// The live particles as quads, in emitter order.
    pub fn billboards(&self) -> Vec<Billboard> {
        let mut out = Vec::new();
        for (i, e) in self.def.emitters.iter().enumerate() {
            let [cols, rows] = e.texdiv;
            for p in &self.particles[i] {
                let t = (p.age / e.life).clamp(0.0, 1.0);
                let frame = ((t * e.frames as f32) as u32).min(e.frames.saturating_sub(1));
                let (c, r) = ((frame % cols) as f32, (frame / cols) as f32);
                let (du, dv) = (1.0 / cols as f32, 1.0 / rows as f32);
                out.push(Billboard {
                    center: p.pos,
                    size: e.scale * curve(&e.scale_curve, t),
                    rotation: p.rotation,
                    uv: [c * du, r * dv, (c + 1.0) * du, (r + 1.0) * dv],
                    color: ramp_sample(&e.ramp_colors, t),
                    flat: e.orient == 2,
                    additive: e.additive,
                    emitter: i,
                });
            }
        }
        out
    }
}

/// Piecewise-linear lookup over sorted (t, v) points; 1.0 when empty.
fn curve(points: &[(f32, f32)], t: f32) -> f32 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return 1.0 };
    if t <= first.0 {
        return first.1;
    }
    if t >= last.0 {
        return last.1;
    }
    for w in points.windows(2) {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        if t <= t1 {
            let span = t1 - t0;
            return if span > 0.0 { v0 + (v1 - v0) * (t - t0) / span } else { v1 };
        }
    }
    last.1
}

/// Samples a ramp row at u = t (nearest texel); white when there is no ramp.
fn ramp_sample(ramp: &[[f32; 4]], t: f32) -> [f32; 4] {
    if ramp.is_empty() {
        return [1.0; 4];
    }
    ramp[((t * ramp.len() as f32) as usize).min(ramp.len() - 1)]
}

/// A number from any value type (strings are parsed, vectors give their first element).
fn num(v: &Value) -> Option<f32> {
    Some(match v {
        Value::I32(x) => *x as f32,
        Value::F32(x) => *x,
        Value::U8Scaled(x) => *x as f32 / 10.0,
        Value::I16(x) => *x as f32,
        Value::U8(x) => *x as f32,
        Value::Bool(x) => *x as u8 as f32,
        Value::I64(x) => *x as f32,
        Value::Str(s) => s.split_whitespace().next()?.parse().ok()?,
        other => *vec_n(other).first()?,
    })
}

/// All numbers in a value (strings are split on whitespace).
fn vec_n(v: &Value) -> Vec<f32> {
    match v {
        Value::U8x2(a) => a.iter().map(|&x| x as f32 / 10.0).collect(),
        Value::U8x3(a) => a.iter().map(|&x| x as f32 / 10.0).collect(),
        Value::U8x4(a) => a.iter().map(|&x| x as f32 / 10.0).collect(),
        Value::F32x2(a) => a.to_vec(),
        Value::F32x3(a) => a.to_vec(),
        Value::F32x4(a) => a.to_vec(),
        Value::Str(s) => s.split_whitespace().filter_map(|t| t.parse().ok()).collect(),
        other => num(other).into_iter().collect(),
    }
}

/// A vec3 value; a lone number fills only x, missing parts are 0.
fn vec3(v: Option<&Value>) -> [f32; 3] {
    let v = v.map(vec_n).unwrap_or_default();
    std::array::from_fn(|i| v.get(i).copied().unwrap_or(0.0))
}

/// The largest-magnitude component (degrees) of a rotation value.
fn spin(v: Option<&Value>) -> f32 {
    v.map(vec_n).unwrap_or_default().into_iter().fold(0.0, |a, x| if x.abs() > a.abs() { x } else { a })
}

/// The file name without directories or extension.
fn file_stem(name: &str) -> &str {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    file.rsplit_once('.').map_or(file, |(s, _)| s)
}

/// The entry of `dir` whose name matches `name` ignoring case.
fn find_child(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
        .map(|e| e.path())
}

/// A lowercase file name -> path index of a directory, cached because Particles holds ~15k files.
fn dir_index(dir: &Path) -> Arc<HashMap<String, PathBuf>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<HashMap<String, PathBuf>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(dir) {
        return hit.clone();
    }
    let index: HashMap<String, PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.file_name().to_str()?.to_ascii_lowercase(), e.path())))
        .collect();
    let index = Arc::new(index);
    cache.lock().unwrap().insert(dir.to_path_buf(), index.clone());
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_root() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Example Maps/DATA");
        p.is_dir().then_some(p)
    }

    #[test]
    fn curve_and_helpers() {
        assert_eq!(curve(&[], 0.5), 1.0);
        assert!((curve(&[(0.0, 1.0), (1.0, 3.0)], 0.5) - 2.0).abs() < 1e-6);
        assert_eq!(vec_n(&Value::Str("0.0 -1".into())), vec![0.0, -1.0]);
        assert_eq!(file_stem("a\\b/Fire_Torch_02.tga"), "Fire_Torch_02");
    }

    #[test]
    fn simulate_real_effects() {
        let Some(root) = data_root() else {
            eprintln!("no DATA folder; skipping");
            return;
        };
        let all = list_effects(&root);
        println!("{} effects listed", all.len());
        let wanted = ["Fire_Cauldron.troy", "FireTorch_Med.troy"];
        let names: Vec<String> = if wanted.iter().all(|w| all.iter().any(|a| a.eq_ignore_ascii_case(w))) {
            wanted.iter().map(|s| s.to_string()).collect()
        } else {
            all.iter().take(3).cloned().collect()
        };
        for name in names {
            let def = Arc::new(load_effect(&root, &name).unwrap());
            println!("{} ({} emitters, radius {:.0})", def.name, def.emitters.len(), def.vis_radius);
            for e in &def.emitters {
                println!(
                    "  {}: tex {:?} div {:?} frames {} rate {} elife {} life {} vel {:?} accel {:?} off {:?} \
                     scale {} curve {:?} ramp {:?} ({} texels) orient {} additive {} spin {:.2}",
                    e.name,
                    e.texture.as_ref().and_then(|p| p.file_name()),
                    e.texdiv,
                    e.frames,
                    e.rate,
                    e.emitter_life,
                    e.life,
                    e.vel,
                    e.accel,
                    e.offset,
                    e.scale,
                    e.scale_curve,
                    e.ramp.as_ref().and_then(|p| p.file_name()),
                    e.ramp_colors.len(),
                    e.orient,
                    e.additive,
                    e.rot_vel
                );
            }
            let mut inst = EffectInstance::new(def.clone(), [0.0; 3], 12345);
            for _ in 0..120 {
                inst.update(1.0 / 60.0);
            }
            let bbs = inst.billboards();
            println!("  -> {} billboards after 2 s", bbs.len());
            assert!(!bbs.is_empty(), "{name} produced no billboards");
        }
    }

    /// Counts which known keys appear across a sample of real effects (informational).
    #[test]
    fn key_survey() {
        let Some(root) = data_root() else { return };
        let keys = [
            "p-texture", "p-texdiv", "p-numframes", "e-rate", "e-life", "p-life", "p-vel", "p-accel",
            "p-offset", "p-scale", "p-xscale1", "p-rgba", "p-simpleorient", "rendermode", "p-quadrot",
            "p-rotvel", "e-disabled",
        ];
        let dir = particles_dir(&root).unwrap();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let mut kinds: HashMap<&str, std::collections::BTreeSet<String>> = HashMap::new();
        let mut modes: std::collections::BTreeMap<String, usize> = Default::default();
        let (mut files, mut emitters) = (0, 0);
        for name in list_effects(&root).iter().step_by(25) {
            let Ok(bytes) = std::fs::read(dir.join(format!("{}bin", name))) else { continue };
            let Ok(ini) = Inibin::parse(&bytes) else { continue };
            files += 1;
            for n in 1..=MAX_GROUP_PARTS {
                let Some(Value::Str(sec)) = ini.get(hash_key("System", &format!("GroupPart{n}"))) else { continue };
                emitters += 1;
                for k in keys {
                    if let Some(v) = ini.get(hash_key(sec, k)) {
                        *counts.entry(k).or_default() += 1;
                        let seen = kinds.entry(k).or_default();
                        if seen.len() < 6 {
                            seen.insert(v.to_text());
                        }
                        if k == "rendermode" {
                            *modes.entry(v.to_text()).or_default() += 1;
                        }
                    }
                }
            }
        }
        println!("survey: {files} files, {emitters} emitters");
        for k in keys {
            println!("  {k}: {} {:?}", counts.get(k).unwrap_or(&0), kinds.get(k));
        }
        println!("  rendermode values: {modes:?}");
    }
}
