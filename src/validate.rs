//! Checks that a map will load in the game: the files the loader opens, consistent scene format,
//! object names, positions on the navigation grid, texture references and script expectations.

use crate::formats::{luaobj, mob::ObjectType, navgrid::flags};
use crate::project::{Doc, MapProject, SceneFormat};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub severity: Severity,
    pub message: String,
    pub file: Option<String>,
}

fn issue(v: &mut Vec<Issue>, severity: Severity, file: Option<&str>, message: impl Into<String>) {
    v.push(Issue { severity, message: message.into(), file: file.map(|s| s.to_string()) });
}

/// World objects with positions: map objects and room.dsc meshes.
pub struct Placed {
    pub name: String,
    pub kind: String,
    pub position: [f32; 3],
}

pub fn placed_objects(p: &MapProject) -> Vec<Placed> {
    let mut out = Vec::new();
    if let Some(m) = p.mob() {
        for o in &m.objects {
            out.push(Placed { name: o.name.clone(), kind: o.kind.label(), position: o.position });
        }
    }
    for e in p.dsc_entries() {
        let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0).to_string();
        let central = [format!("Scene/{stem}.SCB"), format!("Scene/{}", e.file)].iter().find_map(|path| match p.file(path).map(|f| &f.doc) {
            Some(Doc::Scb(s)) => Some(s.central),
            Some(Doc::Sco(s)) => Some(s.central),
            _ => None,
        });
        if let Some(c) = central {
            out.push(Placed { name: stem, kind: "Scene object".into(), position: c });
        }
    }
    out
}

pub fn validate(p: &MapProject) -> Vec<Issue> {
    use Severity::*;
    let mut v = Vec::new();

    let digits = p.name.strip_prefix("Map").or_else(|| p.name.strip_prefix("map"));
    if !digits.is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())) {
        issue(&mut v, Warning, None, format!("the folder is called \"{}\"; the game looks for LEVELS/Map<ID> (for example Map30)", p.name));
    }

    for f in &p.files {
        if let Some(e) = &f.parse_error {
            issue(&mut v, Warning, Some(&f.path), format!("could not be read ({e}); it will be copied unchanged"));
        }
    }

    let renderer = p.renderer_setting();
    let has_nvr = p.find("Scene/room.nvr").is_some();
    let has_wgeo = p.find("Scene/room.wgeo").is_some();
    let baked = renderer.as_deref().is_some_and(|r| r.eq_ignore_ascii_case("baked"));
    match (baked, has_nvr, has_wgeo) {
        (true, _, false) => issue(&mut v, Error, Some("terrain.inibin"), "Renderer is \"baked\" but there is no Scene/room.wgeo; the game will stop with \"There was a problem loading the map\""),
        (false, false, true) => issue(&mut v, Error, Some("terrain.inibin"), "there is only Scene/room.wgeo, but terrain.inibin does not set Renderer = baked; the game will look for room.nvr"),
        (false, false, false) => issue(&mut v, Error, None, "no scene geometry (Scene/room.nvr or Scene/room.wgeo)"),
        _ => {}
    }
    if p.scene_format() == Some(SceneFormat::Legacy) && p.find("Scene/room.mat").is_none() {
        issue(&mut v, Warning, None, "no Scene/room.mat: object meshes (turrets, shops) will have no materials");
    }
    for (path, why) in [
        ("AIPath.aimesh_ngrid", "units cannot move without the navigation grid"),
        ("Scene/MapObjects.mob", "spawn points and map objects come from it"),
        ("Scripts/LevelScript.luaobj", "the level script drives minions, structures and victory"),
        ("Mission.inibin", "mission settings"),
        ("Scene/CFG/ObjectCFG.cfg", "turret, inhibitor and nexus settings"),
    ] {
        // The game loads Lua as compiled .luaobj or as plain .lua text.
        let lua_text = path.ends_with(".luaobj") && p.find(&path.replace(".luaobj", ".lua")).is_some();
        if p.find(path).is_none() && !lua_text {
            issue(&mut v, if path.ends_with("ngrid") || path.ends_with(".mob") { Error } else { Warning }, None, format!("missing {path}: {why}"));
        }
    }

    let grid = p.navgrid();
    let mut names = std::collections::HashSet::new();
    if let Some(m) = p.mob() {
        let mut seen = std::collections::HashSet::new();
        for o in &m.objects {
            if !seen.insert(o.name.as_str()) {
                issue(&mut v, Error, Some("Scene/MapObjects.mob"), format!("two map objects are called {}", o.name));
            }
            if o.name.len() > crate::formats::mob::MAX_NAME {
                issue(&mut v, Error, Some("Scene/MapObjects.mob"), format!("{} is longer than 61 characters", o.name));
            }
            if o.position.iter().chain(o.scale.iter()).any(|x| !x.is_finite()) {
                issue(&mut v, Error, Some("Scene/MapObjects.mob"), format!("{} has an invalid position or scale", o.name));
            }
            names.insert(o.name.clone());
        }
        for team in ["__Spawn_T1", "__Spawn_T2"] {
            if m.find(team).is_none() {
                issue(&mut v, Error, Some("Scene/MapObjects.mob"), format!("no {team}: champions of that team have nowhere to spawn"));
            }
        }
        if !m.objects.iter().any(|o| o.kind == ObjectType::LevelSize) {
            issue(&mut v, Warning, Some("Scene/MapObjects.mob"), "no __LevelSize_ object");
        }
    }
    for e in p.dsc_entries() {
        let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0).to_string();
        let has_sco = p.find(&format!("Scene/{}", e.file)).is_some();
        let has_scb = p.find(&format!("Scene/{stem}.SCB")).is_some();
        if !has_sco && !stem.eq_ignore_ascii_case("AIPath") {
            issue(&mut v, Warning, Some("Scene/room.dsc"), format!("{} is listed but Scene/{} does not exist, so the game skips it", e.file, e.file));
        } else if !has_scb && !has_sco {
            issue(&mut v, Warning, Some("Scene/room.dsc"), format!("{stem} has no mesh"));
        }
        names.insert(stem);
    }

    if let Some(g) = grid {
        for o in placed_objects(p) {
            let important = o.name.starts_with("__Spawn") || o.name.starts_with("Turret") || o.name.starts_with("HQ_") || o.name.starts_with("Barracks") || o.name.starts_with("__P_");
            if !important {
                continue;
            }
            match g.cell_of(o.position[0], o.position[2]) {
                None => issue(&mut v, Error, None, format!("{} ({}) is outside the navigation grid", o.name, o.kind)),
                Some((i, j)) => {
                    let k = j * g.nx + i;
                    if g.flags(k) & flags::WALL != 0 && (o.name.starts_with("__Spawn") || o.name.starts_with("__P_")) {
                        issue(&mut v, Warning, None, format!("{} stands on a wall cell; units spawned there may be stuck", o.name));
                    }
                }
            }
        }
        if g.cell_count() > 0 && !(0..g.cell_count()).any(|k| g.flags(k) & flags::WALL == 0) {
            issue(&mut v, Error, Some("AIPath.aimesh_ngrid"), "no cell is walkable");
        }
    }

    // Textures referenced by the scene.
    let tex_exists = |name: &str| {
        let n = name.replace('\\', "/");
        let base = n.rsplit('/').next().unwrap_or(&n);
        ["Scene/Textures/", "Scene/textures/", "Scene/", ""].iter().any(|d| p.find(&format!("{d}{base}")).is_some() || p.find(&format!("{d}{n}")).is_some())
    };
    let mut missing = std::collections::BTreeSet::new();
    if let Some(Doc::Nvr(n)) = p.file("Scene/room.nvr").map(|f| &f.doc) {
        for m in &n.materials {
            for c in &m.channels {
                if !c.texture.is_empty() && !tex_exists(&c.texture) {
                    missing.insert(c.texture.clone());
                }
            }
        }
    }
    if let Some(Doc::Wgeo(w)) = p.file("Scene/room.wgeo").map(|f| &f.doc) {
        for m in &w.meshes {
            if !m.texture.is_empty() && !tex_exists(&m.texture) {
                missing.insert(m.texture.clone());
            }
        }
    }
    let mut shared = 0;
    if let Some(t) = p.text("Scene/room.mat") {
        for tex in crate::create::room_mat_textures(t) {
            if !tex_exists(&tex) && !missing.contains(&tex) {
                shared += 1;
            }
        }
    }
    if shared > 0 {
        issue(&mut v, Info, Some("Scene/room.mat"), format!("{shared} object textures are not in the map folder; the shipped maps load these from the game's shared data"));
    }
    for t in missing.iter().take(25) {
        issue(&mut v, Warning, None, format!("texture {t} is referenced but not in the map (the game draws its default texture)"));
    }
    if missing.len() > 25 {
        issue(&mut v, Warning, None, format!("... and {} more missing textures", missing.len() - 25));
    }

    // Object names the scripts use.
    let mut wanted = std::collections::BTreeSet::new();
    for f in p.files.iter().filter(|f| f.kind == crate::project::FileKind::Lua) {
        if let Ok(info) = luaobj::inspect(&f.original) {
            for s in info.strings {
                let object_like = ["Turret_T", "Barracks_T", "HQ_T", "__P_", "__Spawn_T", "__NAV_"].iter().any(|pre| s.starts_with(pre));
                if object_like && !s.contains(' ') && !names.contains(&s) {
                    wanted.insert((s, f.path.clone()));
                }
            }
        }
    }
    for (s, file) in wanted.iter().take(30) {
        issue(&mut v, Info, Some(file), format!("the script mentions \"{s}\" but the map has no object with that name"));
    }

    if v.is_empty() {
        issue(&mut v, Info, None, "no problems found");
    }
    v.sort_by_key(|i| i.severity);
    v
}
