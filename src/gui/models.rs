//! Meshes for things drawn on top of the built scene: placement ghosts of props and
//! objectives, and animated jungle monsters. Everything becomes `DynMesh`es for the renderer.

use super::gl_scene::{DynMesh, GlScene};
use lol_map_studio::champion::Champion;
use lol_map_studio::formats::dds;
use lol_map_studio::gamedata::GameData;
use lol_map_studio::layout::{ObjKind, Objective};
use lol_map_studio::props::PropDef;
use lol_map_studio::render::RVertex;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Placement of a model: ground position, height, turn (degrees), uniform scale.
#[derive(Clone, Copy, Debug)]
pub struct Place {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub scale: f32,
}

fn find_ci(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let lower = name.to_ascii_lowercase();
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|f| f.to_string_lossy().to_ascii_lowercase() == lower))
}

/// Same rotation as structure models in the renderer and the Play-mode champion.
fn place_unit(p: &Place, v: [f32; 3], n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let (s, c) = p.yaw.to_radians().sin_cos();
    let (x, y, z) = (v[0] * p.scale, v[1] * p.scale, v[2] * p.scale);
    ([p.x + c * x + s * z, p.y + y, p.z - s * x + c * z], [c * n[0] + s * n[2], n[1], -s * n[0] + c * n[2]])
}

/// Character skins standing for an objective (with offsets for camps), as (skin, x, z, yaw, monster).
pub fn objective_models(o: &Objective) -> Vec<(String, [f32; 2], f32, bool)> {
    let side = if o.team == 2 { "Chaos" } else { "Order" };
    let one = |s: String| vec![(s, o.pos, o.yaw, false)];
    match o.kind {
        ObjKind::Nexus => one(format!("{side}Nexus")),
        ObjKind::Inhibitor => one(format!("{side}Inhibitor")),
        // Imported maps keep their own turret skins (map11's SRUAP turrets).
        ObjKind::Turret => one(o.skin.clone().unwrap_or_else(|| o.tier.skin(o.team).to_string())),
        ObjKind::FountainTurret => one(o.skin.clone().unwrap_or_else(|| format!("{side}TurretShrine"))),
        ObjKind::Shop => one("ShopKeeper".to_string()),
        ObjKind::CapturePoint => one("OdinNeutralGuardian".to_string()),
        ObjKind::Crystal => one(format!("Odin_SOG_{side}_Crystal")),
        k if k.is_camp() => {
            let (dx, dz) = (7000.0 - o.pos[0], 7000.0 - o.pos[1]);
            let yaw = if o.yaw != 0.0 { o.yaw } else { dx.atan2(dz).to_degrees() };
            lol_map_studio::layout_build::camp_monsters(k).iter().zip(lol_map_studio::layout_build::camp_positions(o)).map(|(m, p)| (m.to_string(), p, yaw, true)).collect()
        }
        _ => Vec::new(),
    }
}

/// Caches decoded textures and loaded characters for ghosts and monsters.
#[derive(Default)]
pub struct Models {
    champs: HashMap<String, Option<Arc<Champion>>>,
}

impl Models {
    /// A prop as see-through (or solid) meshes at a place.
    pub fn prop(&self, gl: &mut GlScene, def: &PropDef, at: Place, tint: [f32; 4]) -> Vec<DynMesh> {
        self.prop_ex(gl, def, at, [0.0; 2], [1.0; 3], tint)
    }

    /// A prop with tilt and per-axis stretch (as placed on the map).
    pub fn prop_ex(&self, gl: &mut GlScene, def: &PropDef, at: Place, tilt: [f32; 2], stretch: [f32; 3], tint: [f32; 4]) -> Vec<DynMesh> {
        let mut out = Vec::new();
        for part in &def.parts {
            let tex = part.material.channels[0].texture.clone();
            let key = format!("prop:{}", tex.to_ascii_lowercase());
            if !tex.is_empty() && !gl.has_texture(&key) {
                let img = find_ci(&def.texture_dir, &tex).and_then(|p| std::fs::read(p).ok()).and_then(|b| dds::decode(&b, 256).ok());
                match img {
                    Some(i) => gl.add_texture(&key, i.width, i.height, i.pixels),
                    None => gl.add_texture(&key, 1, 1, vec![180, 180, 180, 255]),
                }
            }
            let g = &part.geo;
            let vertices = g
                .positions
                .iter()
                .enumerate()
                .map(|(k, v)| {
                    let (o, normal) = lol_map_studio::props::place_vertex(*v, g.normals.get(k).copied().unwrap_or([0.0, 1.0, 0.0]), at.yaw, at.scale, tilt, stretch);
                    let pos = [at.x + o[0], at.y + o[1], at.z + o[2]];
                    let uv = g.uvs.get(k).copied().unwrap_or([0.0; 2]);
                    RVertex { pos, normal, uv0: uv, uv1: uv, color: [0.03, 0.03, 0.03, 1.0] }
                })
                .collect();
            out.push(DynMesh { texture: (!tex.is_empty()).then_some(key), vertices, indices: g.indices.clone(), tint, cutout: true, particle: false, additive: false });
        }
        out
    }

    /// A character model (structure, monster) in its bind or idle pose.
    pub fn unit(&self, gl: &mut GlScene, data: &GameData, skin: &str, monster: bool, at: Place, tint: [f32; 4]) -> Option<DynMesh> {
        let model = if monster { data.posed_model(skin) } else { data.model(skin) }?;
        let key = format!("unit:{}", skin.to_ascii_lowercase());
        if !gl.has_texture(&key) {
            match &model.texture {
                Some(t) => gl.add_texture(&key, t.width, t.height, t.pixels.clone()),
                None => gl.add_texture(&key, 1, 1, vec![180, 180, 180, 255]),
            }
        }
        let at = Place { scale: at.scale * model.scale, ..at };
        let m = &model.mesh;
        let vertices = m
            .positions
            .iter()
            .zip(&m.normals)
            .zip(&m.uvs)
            .map(|((v, n), uv)| {
                let (pos, normal) = place_unit(&at, *v, *n);
                RVertex { pos, normal, uv0: *uv, uv1: *uv, color: [0.0, 0.0, 0.0, 1.0] }
            })
            .collect();
        Some(DynMesh { texture: Some(key), vertices, indices: m.indices.clone(), tint, cutout: false, particle: false, additive: false })
    }

    fn champion(&mut self, data: &GameData, name: &str) -> Option<Arc<Champion>> {
        self.champs.entry(name.to_ascii_lowercase()).or_insert_with(|| Champion::load(&data.root, name).ok().map(Arc::new)).clone()
    }

    /// A monster playing its idle animation at `time` (seconds).
    pub fn animated(&mut self, gl: &mut GlScene, data: &GameData, name: &str, at: Place, time: f32) -> Option<DynMesh> {
        let Some(c) = self.champion(data, name) else { return self.unit(gl, data, name, true, at, [1.0; 4]) };
        let key = format!("anim:{}", name.to_ascii_lowercase());
        if !gl.has_texture(&key) {
            match &c.texture {
                Some(t) => gl.add_texture(&key, t.width, t.height, t.pixels.clone()),
                None => gl.add_texture(&key, 1, 1, vec![180, 180, 180, 255]),
            }
        }
        let anim = c.find_anim(&["idle1", "idle", "idle_base"]);
        let mats = c.pose(anim.as_deref(), time, true);
        let (pos, nor) = c.skin(&mats);
        let at = Place { scale: at.scale * c.scale, ..at };
        let vertices = pos
            .iter()
            .zip(&nor)
            .zip(&c.mesh.uvs)
            .map(|((v, n), uv)| {
                let (pos, normal) = place_unit(&at, *v, *n);
                RVertex { pos, normal, uv0: *uv, uv1: *uv, color: [0.03, 0.03, 0.03, 1.0] }
            })
            .collect();
        Some(DynMesh { texture: Some(key), vertices, indices: c.mesh.indices.clone(), tint: [1.0; 4], cutout: false, particle: false, additive: false })
    }

    /// Every jungle monster of a map, animated (each with its own phase).
    pub fn monsters(&mut self, gl: &mut GlScene, data: &GameData, monsters: &[lol_map_studio::jungle::Monster], height: &dyn Fn(f32, f32) -> f32, time: f32) -> Vec<DynMesh> {
        monsters
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let y = if m.pos[1].is_nan() { height(m.pos[0], m.pos[2]) } else { height(m.pos[0], m.pos[2]).max(m.pos[1] - 60.0) };
                self.animated(gl, data, &m.name, Place { x: m.pos[0], y, z: m.pos[2], yaw: m.yaw, scale: 1.0 }, time + i as f32 * 0.37)
            })
            .collect()
    }
}
