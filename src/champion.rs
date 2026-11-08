//! A champion for Play mode: skinned mesh, skeleton, texture and animations from the game's
//! DATA folder, posed on the CPU each frame.

use crate::formats::{anm::{Animation, slerp}, dds::{self, Image}, inibin::{Inibin, Value, hash_key}, skl::{self, Mat4, Skeleton}, skn::Skn};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Champion {
    pub name: String,
    pub mesh: Skn,
    pub skeleton: Skeleton,
    pub texture: Option<Image>,
    pub portrait: Option<Image>,
    /// Animations by lowercase file stem.
    pub anims: HashMap<String, Animation>,
    pub scale: f32,
    pub move_speed: f32,
    pub radius: f32,
    /// Level-1 stats for the HUD.
    pub stats: Stats,
    /// Q, W, E, R icons, the passive icon and the minimap circle (from DATA/Spells/Icons2D and Info).
    pub spell_icons: [Option<Image>; 4],
    pub passive_icon: Option<Image>,
    pub circle: Option<Image>,
    /// Spell names (Data*Spell1..4) for diagnostics.
    pub spells: [String; 4],
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub hp: f32,
    pub mana: f32,
    /// False for champions without a resource bar (Katarina in 4.x).
    pub has_mana: bool,
    pub ad: f32,
    pub armor: f32,
    pub mr: f32,
    pub attack_speed: f32,
}

fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let lower = name.to_ascii_lowercase();
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|f| f.to_string_lossy().to_ascii_lowercase() == lower))
}

impl Champion {
    /// Loads `DATA/Characters/<name>` (base skin).
    pub fn load(data_root: &Path, name: &str) -> Result<Self> {
        let dir = data_root.join("Characters").join(name);
        let ini_bytes = std::fs::read(dir.join(format!("{name}.inibin"))).with_context(|| format!("no {name}.inibin"))?;
        let ini = Inibin::parse(&ini_bytes)?;
        let s = |sec: &str, k: &str| match ini.get(hash_key(sec, k)) {
            Some(Value::Str(v)) if !v.is_empty() => Some(v.clone()),
            _ => None,
        };
        let f = |sec: &str, k: &str, d: f32| match ini.get(hash_key(sec, k)) {
            Some(Value::F32(v)) => *v,
            Some(Value::I32(v)) => *v as f32,
            Some(Value::Str(v)) => v.trim().parse().unwrap_or(d),
            _ => d,
        };
        let mesh_name = s("MeshSkin", "SimpleSkin").context("the character has no mesh")?;
        let skl_name = s("MeshSkin", "Skeleton").context("the character has no skeleton")?;
        let mesh = Skn::parse(&std::fs::read(find_ci(&dir, &mesh_name).context("mesh file missing")?)?)?;
        let skeleton = Skeleton::parse(&std::fs::read(find_ci(&dir, &skl_name).context("skeleton file missing")?)?)?;
        let texture = s("MeshSkin", "Texture").and_then(|t| find_ci(&dir, &t)).and_then(|p| std::fs::read(p).ok()).and_then(|b| dds::decode(&b, 1024).ok());
        let portrait = find_ci(&dir.join("Info"), &format!("{}_Square.dds", name.trim_end_matches(|c: char| c == 'V' || c.is_ascii_digit())))
            .or_else(|| std::fs::read_dir(dir.join("Info")).ok()?.flatten().map(|e| e.path()).find(|p| p.to_string_lossy().to_ascii_lowercase().ends_with("_square.dds")))
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| dds::decode(&b, 128).ok());
        let mut anims = HashMap::new();
        for sub in ["animations", "Animations"] {
            let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("anm")) {
                    continue;
                }
                let stem = p.file_stem().unwrap().to_string_lossy().to_ascii_lowercase();
                if let Ok(a) = std::fs::read(&p).map_err(anyhow::Error::from).and_then(|b| Animation::parse(&b)) {
                    anims.insert(stem, a);
                }
            }
            break;
        }
        if anims.is_empty() {
            bail!("the character has no animations");
        }
        let icons = data_root.join("Spells").join("Icons2D");
        let load_icon = |n: &str| -> Option<Image> {
            let n = n.trim();
            if n.is_empty() {
                return None;
            }
            let stem = n.rsplit_once('.').map_or(n, |x| x.0);
            [icons.clone(), dir.join("Info"), dir.clone()]
                .iter()
                .find_map(|d| find_ci(d, &format!("{stem}.dds")))
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| dds::decode(&b, 128).ok())
        };
        let spells: [String; 4] = std::array::from_fn(|i| s("Data", &format!("Spell{}", i + 1)).unwrap_or_default());
        let spell_icons: [Option<Image>; 4] = std::array::from_fn(|i| {
            let sp = &spells[i];
            let ini = std::fs::read(data_root.join("Spells").join(format!("{sp}.inibin"))).ok().and_then(|b| Inibin::parse(&b).ok());
            let key = ini.as_ref().and_then(|ini| {
                [("SpellData", "InventoryIcon"), ("SpellData", "InventoryIcon1")].iter().find_map(|(a, b)| match ini.get(hash_key(a, b)) {
                    Some(Value::Str(v)) if !v.is_empty() => Some(v.clone()),
                    _ => None,
                })
            });
            key.and_then(|k| load_icon(&k))
        });
        let passive_icon = [("Data", "PassLev1Icon"), ("Data", "Passive1Icon"), ("Data", "PassiveIcon")].iter().find_map(|(a, b)| s(a, b)).and_then(|k| load_icon(&k));
        let base = name.trim_end_matches(|c: char| c == 'V' || c.is_ascii_digit());
        let circle = load_icon(&format!("{base}_Circle"));
        let par = s("Data", "PARType").unwrap_or_default().to_ascii_lowercase();
        let mana = f("Data", "BaseMP", 0.0);
        let stats = Stats {
            hp: f("Data", "BaseHP", 500.0) + f("Data", "HPPerLevel", 0.0),
            mana: mana + f("Data", "MPPerLevel", 0.0),
            has_mana: mana > 0.0 && !matches!(par.as_str(), "none" | ""),
            ad: f("Data", "BaseDamage", 50.0) + f("Data", "DamagePerLevel", 0.0),
            armor: f("Data", "Armor", 20.0) + f("Data", "ArmorPerLevel", 0.0),
            mr: f("Data", "SpellBlock", 30.0) + f("Data", "SpellBlockPerLevel", 0.0),
            attack_speed: 0.625 / (1.0 + f("Data", "AttackDelayOffsetPercent", 0.0)),
        };
        Ok(Self {
            name: name.to_string(),
            mesh,
            skeleton,
            texture,
            portrait,
            anims,
            scale: f("MeshSkin", "SkinScale", 1.0).max(0.01),
            move_speed: f("Data", "MoveSpeed", 345.0),
            radius: f("Data", "PathfindingCollisionRadius", 35.0),
            stats,
            spell_icons,
            passive_icon,
            circle,
            spells,
        })
    }

    /// The animation whose name ends with one of `keys` (base skin animations first).
    pub fn find_anim(&self, keys: &[&str]) -> Option<String> {
        let base = self.name.trim_end_matches(|c: char| c == 'V' || c.is_ascii_digit()).to_ascii_lowercase();
        for k in keys {
            let mut cands: Vec<&String> = self.anims.keys().filter(|n| n.ends_with(k) || n.ends_with(&format!("_{k}"))).collect();
            cands.sort_by_key(|n| (!n.starts_with(&base) || n.len() > base.len() + k.len() + 2, n.len()));
            if let Some(c) = cands.first() {
                return Some((*c).clone());
            }
        }
        None
    }

    /// Skinning matrices (one per skin influence) for an animation at a time (looping).
    pub fn pose(&self, anim: Option<&str>, time: f32, looping: bool) -> Vec<Mat4> {
        let a = anim.and_then(|n| self.anims.get(n));
        let joints = &self.skeleton.joints;
        let mut global: Vec<Option<Mat4>> = vec![None; joints.len()];
        let local = |i: usize| -> Mat4 {
            let j = &joints[i];
            if let Some(a) = a
                && let Some(tr) = a.tracks.get(&j.hash)
                && !tr.r.is_empty()
            {
                let n = tr.r.len();
                let ft = (time / a.frame_time).max(0.0);
                let ft = if looping { ft % n as f32 } else { ft.min((n - 1) as f32) };
                let f0 = (ft.floor() as usize).min(n - 1);
                let f1 = if looping { (f0 + 1) % n } else { (f0 + 1).min(n - 1) };
                let w = ft - ft.floor();
                let lerp3 = |x: [f32; 3], y: [f32; 3]| [x[0] + (y[0] - x[0]) * w, x[1] + (y[1] - x[1]) * w, x[2] + (y[2] - x[2]) * w];
                return skl::compose(lerp3(tr.t[f0], tr.t[f1]), slerp(tr.r[f0], tr.r[f1], w), lerp3(tr.s[f0], tr.s[f1]));
            }
            skl::compose(j.local_t, j.local_r, j.local_s)
        };
        fn resolve(i: usize, joints: &[skl::Joint], global: &mut Vec<Option<Mat4>>, local: &dyn Fn(usize) -> Mat4, depth: usize) -> Mat4 {
            if let Some(g) = global[i] {
                return g;
            }
            let l = local(i);
            let p = joints[i].parent;
            let g = if p >= 0 && (p as usize) < joints.len() && depth < 256 { skl::mul(&resolve(p as usize, joints, global, local, depth + 1), &l) } else { l };
            global[i] = Some(g);
            g
        }
        for i in 0..joints.len() {
            resolve(i, joints, &mut global, &local, 0);
        }
        let skin = |ji: usize| skl::mul(&global[ji].unwrap_or_else(skl::identity), &joints[ji].inv_bind);
        if self.skeleton.influences.is_empty() {
            (0..joints.len()).map(skin).collect()
        } else {
            self.skeleton.influences.iter().map(|&ji| skin((ji as usize).min(joints.len() - 1))).collect()
        }
    }

    /// Skinned positions and normals in model space.
    pub fn skin(&self, mats: &[Mat4]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
        let n = self.mesh.positions.len();
        let mut pos = Vec::with_capacity(n);
        let mut nor = Vec::with_capacity(n);
        for v in 0..n {
            let (p, nn) = (self.mesh.positions[v], self.mesh.normals[v]);
            let (mut op, mut on) = ([0.0f32; 3], [0.0f32; 3]);
            for k in 0..4 {
                let w = self.mesh.weights[v][k];
                if w <= 0.0 {
                    continue;
                }
                let Some(m) = mats.get(self.mesh.bones[v][k] as usize) else { continue };
                for a in 0..3 {
                    op[a] += w * (m[a] * p[0] + m[4 + a] * p[1] + m[8 + a] * p[2] + m[12 + a]);
                    on[a] += w * (m[a] * nn[0] + m[4 + a] * nn[1] + m[8 + a] * nn[2]);
                }
            }
            pos.push(op);
            nor.push(on);
        }
        (pos, nor)
    }
}
