//! The game's shared DATA folder (characters, shared textures). The editor uses it to draw
//! structures with their real models; maps never copy from it.

use crate::formats::{dds, inibin::{Inibin, Value, hash_key}, skn::Skn};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct Model {
    pub mesh: Skn,
    pub texture: Option<dds::Image>,
    pub scale: f32,
}

#[derive(Clone)]
pub struct GameData {
    pub root: PathBuf,
    models: Arc<Mutex<HashMap<String, Option<Arc<Model>>>>>,
}

impl GameData {
    pub fn new(root: PathBuf) -> Self {
        Self { root, models: Default::default() }
    }

    /// Looks for DATA next to a map's LEVELS folder, then beside the examples and the program.
    pub fn locate(map_dir: Option<&Path>, configured: Option<&Path>) -> Option<Self> {
        let mut cands: Vec<PathBuf> = Vec::new();
        if let Some(c) = configured {
            cands.push(c.to_path_buf());
        }
        if let Some(d) = crate::paths::data_dir() {
            cands.push(d);
        }
        if let Some(m) = map_dir {
            let mut d = m.parent();
            while let Some(p) = d {
                cands.push(p.join("DATA"));
                d = p.parent();
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            let mut d = exe.parent();
            while let Some(p) = d {
                cands.push(p.join("Example Maps").join("DATA"));
                cands.push(p.join("DATA"));
                d = p.parent();
            }
        }
        cands.into_iter().find(|p| p.join("Characters").is_dir()).map(Self::new)
    }

    pub fn read(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(self.root.join(rel.replace('\\', "/"))).ok()
    }

    /// A character's mesh and texture in bind pose (cached).
    pub fn model(&self, skin: &str) -> Option<Arc<Model>> {
        let key = skin.to_ascii_lowercase();
        if let Some(m) = self.models.lock().unwrap().get(&key) {
            return m.clone();
        }
        let m = self.load_model(skin).map(Arc::new);
        self.models.lock().unwrap().insert(key, m.clone());
        m
    }

    /// A character standing in its idle pose (first frame), for monsters and other units.
    pub fn posed_model(&self, name: &str) -> Option<Arc<Model>> {
        let key = format!("posed:{}", name.to_ascii_lowercase());
        if let Some(m) = self.models.lock().unwrap().get(&key) {
            return m.clone();
        }
        let m = crate::champion::Champion::load(&self.root, name)
            .ok()
            .map(|c| {
                let anim = c.find_anim(&["idle1", "idle", "idle_base"]);
                let mats = c.pose(anim.as_deref(), 0.0, true);
                let (positions, normals) = c.skin(&mats);
                let mesh = crate::formats::skn::Skn { positions, normals, uvs: c.mesh.uvs.clone(), indices: c.mesh.indices.clone(), submeshes: c.mesh.submeshes.clone(), bones: Vec::new(), weights: Vec::new() };
                Arc::new(Model { mesh, texture: c.texture, scale: c.scale })
            })
            .or_else(|| self.model(name));
        self.models.lock().unwrap().insert(key, m.clone());
        m
    }

    fn load_model(&self, skin: &str) -> Option<Model> {
        let dir = self.root.join("Characters").join(skin);
        for ini_path in [dir.join(format!("{skin}.inibin")), dir.join("Skins").join("Base").join("Base.inibin")] {
            let Some(ini) = std::fs::read(&ini_path).ok().and_then(|b| Inibin::parse(&b).ok()) else { continue };
            let s = |k: &str| match ini.get(hash_key("MeshSkin", k)) {
                Some(Value::Str(v)) if !v.is_empty() => Some(v.clone()),
                _ => None,
            };
            let Some(mesh_name) = s("SimpleSkin") else { continue };
            let base = ini_path.parent().unwrap();
            let find = |name: &str| -> Option<PathBuf> {
                let direct = base.join(name);
                if direct.is_file() {
                    return Some(direct);
                }
                let lower = name.to_ascii_lowercase();
                std::fs::read_dir(base).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.file_name().is_some_and(|f| f.to_string_lossy().to_ascii_lowercase() == lower))
            };
            let Some(mesh) = find(&mesh_name).and_then(|p| std::fs::read(p).ok()).and_then(|b| Skn::parse(&b).ok()) else { continue };
            let texture = s("Texture").and_then(|t| find(&t)).and_then(|p| std::fs::read(p).ok()).and_then(|b| dds::decode(&b, 1024).ok());
            let scale = match ini.get(hash_key("MeshSkin", "SkinScale")) {
                Some(Value::F32(v)) if *v > 0.0 => *v,
                _ => 1.0,
            };
            return Some(Model { mesh, texture, scale });
        }
        None
    }
}

/// The character skin the game uses for a structure name.
pub fn structure_skin(name: &str, cfg_skin: Option<&str>, script_skin: Option<&str>) -> Option<String> {
    if let Some(s) = script_skin.or(cfg_skin) {
        return Some(s.to_string());
    }
    let n = name.to_ascii_lowercase();
    let order = n.contains("_t1") || n.contains("order");
    let side = if order { "Order" } else { "Chaos" };
    if n.starts_with("hq_") {
        Some(format!("{side}Nexus"))
    } else if n.starts_with("barracks_t") {
        Some(format!("{side}Inhibitor"))
    } else if n.contains("turretshrine") {
        Some(format!("{side}TurretShrine"))
    } else if n.starts_with("turret_") {
        Some(if order { "OrderTurretNormal".into() } else { "ChaosTurretWorm".into() })
    } else {
        None
    }
}
