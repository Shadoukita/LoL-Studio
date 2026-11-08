//! Particle effects running in the 3D views: one simulation (`particles::EffectInstance`) per
//! placed effect, drawn as camera-facing (or flat) quads through the renderer's dynamic meshes.

use super::gl_scene::{self, DynMesh, GlScene, Mat4};
use lol_map_studio::formats::dds;
use lol_map_studio::particles::{EffectDef, EffectInstance, load_effect};
use lol_map_studio::render::RVertex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Visual effect of objects the game draws only as particles (relics, shrines).
pub fn unit_effect(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "ha_ap_healthrelic" | "ha_fb_healthrelic" | "odinshieldrelic" | "tt_relic" => Some("HA_AP_healingBuff.troy"),
        "odinspeedshrine" | "aramspeedshrine" | "tt_speedshrine" => Some("odin_shrine_time.troy"),
        "odincenterrelic" => Some("odin_center_relic.troy"),
        _ => None,
    }
}

struct Running {
    inst: EffectInstance,
    pos: [f32; 3],
    seen: bool,
}

#[derive(Default)]
pub struct Fx {
    defs: HashMap<String, Option<Arc<EffectDef>>>,
    running: HashMap<String, Running>,
    last: Option<std::time::Instant>,
    seed: u32,
}

/// World-space right and up of the camera (for billboards).
fn camera_axes(inv: &Mat4) -> ([f32; 3], [f32; 3]) {
    let un = |x: f32, y: f32| {
        let v = gl_scene::transform(inv, [x, y, 0.5]);
        [v[0] / v[3], v[1] / v[3], v[2] / v[3]]
    };
    let o = un(0.0, 0.0);
    let norm = |a: [f32; 3]| {
        let d = [a[0] - o[0], a[1] - o[1], a[2] - o[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
        [d[0] / l, d[1] / l, d[2] / l]
    };
    (norm(un(0.01, 0.0)), norm(un(0.0, 0.01)))
}

fn soft_sprite() -> Vec<u8> {
    let n = 64;
    let mut px = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let d = (((x as f32 + 0.5) / n as f32 - 0.5).hypot((y as f32 + 0.5) / n as f32 - 0.5) * 2.0).min(1.0);
            let a = ((1.0 - d) * (1.0 - d) * 255.0) as u8;
            let k = (y * n + x) * 4;
            px[k..k + 4].copy_from_slice(&[255, 255, 255, a]);
        }
    }
    px
}

impl Fx {
    fn def(&mut self, data_root: &Path, name: &str) -> Option<Arc<EffectDef>> {
        self.defs.entry(name.to_ascii_lowercase()).or_insert_with(|| load_effect(data_root, name).ok().map(Arc::new)).clone()
    }

    /// Advances and draws `items` (key, effect name, position). Effects not listed stop.
    pub fn frame(&mut self, gl: &mut GlScene, data_root: &Path, items: &[(String, String, [f32; 3])], inv: &Mat4) -> Vec<DynMesh> {
        let now = std::time::Instant::now();
        let dt = self.last.map_or(0.016, |l| (now - l).as_secs_f32()).min(0.1);
        self.last = Some(now);
        for r in self.running.values_mut() {
            r.seen = false;
        }
        for (key, name, pos) in items {
            let Some(def) = self.def(data_root, name) else { continue };
            let moved = self.running.get(key).is_none_or(|r| (r.pos[0] - pos[0]).abs() + (r.pos[1] - pos[1]).abs() + (r.pos[2] - pos[2]).abs() > 1.0 || r.inst.def().name != def.name);
            if moved {
                self.seed = self.seed.wrapping_add(7919);
                let mut inst = EffectInstance::new(def, *pos, self.seed);
                // Start warmed up so effects do not pop in.
                for _ in 0..30 {
                    inst.update(0.05);
                }
                self.running.insert(key.clone(), Running { inst, pos: *pos, seen: true });
            } else if let Some(r) = self.running.get_mut(key) {
                r.seen = true;
            }
        }
        self.running.retain(|_, r| r.seen);
        if !gl.has_texture("fx:soft") {
            gl.add_texture("fx:soft", 64, 64, soft_sprite());
        }
        let (right, up) = camera_axes(inv);
        // Quads grouped by texture and blend mode.
        let mut groups: HashMap<(String, bool), (Vec<RVertex>, Vec<u32>)> = HashMap::new();
        for r in self.running.values_mut() {
            r.inst.update(dt);
            let def = r.inst.def();
            let keys: Vec<String> = def
                .emitters
                .iter()
                .map(|e| match &e.texture {
                    Some(p) => format!("fx:{}", p.to_string_lossy().to_ascii_lowercase()),
                    None => "fx:soft".to_string(),
                })
                .collect();
            for (k, e) in keys.iter().zip(&def.emitters) {
                if k != "fx:soft" && !gl.has_texture(k) {
                    match e.texture.as_ref().and_then(|p| std::fs::read(p).ok()).and_then(|b| dds::decode(&b, 128).ok()) {
                        Some(i) => gl.add_texture(k, i.width, i.height, i.pixels),
                        None => gl.add_texture(k, 64, 64, soft_sprite()),
                    }
                }
            }
            for b in r.inst.billboards() {
                let (vs, is) = groups.entry((keys[b.emitter].clone(), b.additive)).or_default();
                let h = b.size.max(1.0);
                let (s, c) = b.rotation.sin_cos();
                let (ax, ay) = if b.flat { ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]) } else { (right, up) };
                // Rotated corner axes.
                let rx: [f32; 3] = std::array::from_fn(|i| (ax[i] * c + ay[i] * s) * h);
                let ry: [f32; 3] = std::array::from_fn(|i| (ay[i] * c - ax[i] * s) * h);
                let base = vs.len() as u32;
                let [u0, v0, u1, v1] = b.uv;
                for (sx, sy, u, v) in [(-1.0, -1.0, u0, v1), (1.0, -1.0, u1, v1), (1.0, 1.0, u1, v0), (-1.0, 1.0, u0, v0)] {
                    let p: [f32; 3] = std::array::from_fn(|i| b.center[i] + rx[i] * sx + ry[i] * sy);
                    vs.push(RVertex { pos: p, normal: [0.0, 1.0, 0.0], uv0: [u, v], uv1: [u, v], color: b.color });
                }
                is.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }
        }
        groups
            .into_iter()
            .map(|((tex, additive), (vertices, indices))| DynMesh { texture: Some(tex), vertices, indices, tint: [1.0; 4], cutout: false, particle: true, additive })
            .collect()
    }
}

/// Effects listed in a map's Particles.dat ("name x y z quality rx ry rz [group]").
pub fn particles_dat(text: &str) -> Vec<(String, [f32; 3])> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 {
                return None;
            }
            let n = |i: usize| f[i].parse::<f32>().ok();
            let name = f[0];
            // Sound emitters share the file; they have nothing to draw.
            if name.to_ascii_lowercase().starts_with("audio") {
                return None;
            }
            Some((name.to_string(), [n(1)?, n(2)?, n(3)?]))
        })
        .collect()
}

/// The effects nearest to a point (at most `max`, within `radius`), keyed by index.
pub fn nearby(all: &[(String, [f32; 3])], at: [f32; 2], radius: f32, max: usize) -> Vec<(String, String, [f32; 3])> {
    let mut v: Vec<(f32, usize)> = all.iter().enumerate().map(|(i, e)| ((e.1[0] - at[0]).hypot(e.1[2] - at[1]), i)).filter(|x| x.0 < radius).collect();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    v.into_iter().take(max).map(|(_, i)| (format!("p{i}"), all[i].0.clone(), all[i].1)).collect()
}
