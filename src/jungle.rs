//! Jungle camps of a map: where each monster stands, read from `Scripts/NeutralMinionSpawn.lua`
//! (as LoL Map Studio writes it), or Summoner's Rift's own camps for maps that ship the
//! compiled script.

use crate::layout::{Lane, ObjKind, Objective, SR_CAMPS, TurretTier};
use crate::project::MapProject;

#[derive(Clone, Debug)]
pub struct Monster {
    /// Character name (DATA/Characters/<name>).
    pub name: String,
    pub pos: [f32; 3],
    /// Facing in degrees (atan2 of the direction to the camp's face point).
    pub yaw: f32,
    /// Index of the camp (1-based, as in the script).
    pub camp: usize,
}

fn points(line: &str) -> Vec<[f32; 3]> {
    line.split("Make3DPoint(")
        .skip(1)
        .filter_map(|s| {
            let v: Vec<f32> = s.split(')').next()?.split(',').filter_map(|t| t.trim().parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        })
        .collect()
}

fn quoted(s: &str) -> Vec<String> {
    s.split('"').skip(1).step_by(2).map(str::to_string).collect()
}

/// Camps from a plain-text NeutralMinionSpawn.lua in this tool's layout.
pub fn parse_script(text: &str) -> Vec<Monster> {
    use std::collections::HashMap;
    let mut spawn: HashMap<usize, Vec<[f32; 3]>> = HashMap::new();
    let mut face: HashMap<usize, Vec<[f32; 3]>> = HashMap::new();
    let mut group: HashMap<usize, String> = HashMap::new();
    let mut predefined: HashMap<String, Vec<String>> = HashMap::new();
    let index = |s: &str, prefix: &str| -> Option<usize> { s.strip_prefix(prefix)?.split(']').next()?.parse().ok() };
    for l in text.lines().map(str::trim) {
        if let Some(n) = index(l, "CampSpawnPoints[") {
            spawn.insert(n, points(l));
        } else if let Some(n) = index(l, "CampFacePoints[") {
            face.insert(n, points(l));
        } else if let Some(n) = index(l, "NeutralMinionCamps[")
            && let Some(rest) = l.split_once("].Groups = {")
        {
            if let Some(t) = rest.1.split("PredefinedCamps[").nth(1).and_then(|s| s.split(']').next()) {
                group.insert(n, t.trim().to_string());
            }
        } else if let Some(t) = l.strip_prefix("PredefinedCamps[").and_then(|s| s.split_once(']')) {
            predefined.insert(t.0.trim().to_string(), quoted(t.1));
        }
    }
    let mut camps: Vec<usize> = spawn.keys().copied().collect();
    camps.sort();
    let mut out = Vec::new();
    for n in camps {
        let Some(names) = group.get(&n).and_then(|g| predefined.get(g)) else { continue };
        let pts = &spawn[&n];
        for (i, name) in names.iter().enumerate() {
            let Some(p) = pts.get(i).or(pts.last()) else { continue };
            let f = face.get(&n).and_then(|f| f.get(i).or(f.last())).copied().unwrap_or([p[0], p[1], p[2] + 1.0]);
            out.push(Monster { name: name.clone(), pos: *p, yaw: (f[0] - p[0]).atan2(f[2] - p[2]).to_degrees(), camp: n });
        }
    }
    out
}

/// Summoner's Rift's camps (for maps that ship Riot's compiled script).
pub fn summoners_rift() -> Vec<Monster> {
    let mut out = Vec::new();
    for (n, (kind, team, pos)) in SR_CAMPS.iter().enumerate() {
        let o = Objective { id: 0, kind: *kind, team: *team, lane: Lane::Mid, tier: TurretTier::Outer, pos: *pos, yaw: 0.0, mirror: None, name: None, skin: None };
        let (dx, dz) = (7435.0 - pos[0], 7435.0 - pos[1]);
        for (name, p) in crate::layout_build::camp_monsters(*kind).iter().zip(crate::layout_build::camp_positions(&o)) {
            out.push(Monster { name: name.to_string(), pos: [p[0], f32::NAN, p[1]], yaw: dx.atan2(dz).to_degrees(), camp: n + 1 });
        }
    }
    out
}

/// The monsters of a map. Heights left NaN are for the caller to put on the ground.
pub fn monsters(p: &MapProject) -> Vec<Monster> {
    if let Some(t) = p.text("Scripts/NeutralMinionSpawn.lua") {
        return parse_script(t);
    }
    match p.file("Scripts/NeutralMinionSpawn.luaobj").and_then(|f| crate::formats::luaobj::inspect(&f.original).ok()) {
        // Riot's compiled script: Summoner's Rift's camps, if its first spawn point is Map1's.
        Some(info) if info.strings.iter().any(|s| s == "CampSpawnPoints") && info.strings.iter().any(|s| s == "CAMPTYPE_NASHOR") && is_map1_jungle(p) => summoners_rift(),
        _ => Vec::new(),
    }
}

/// Camp kind labels for monsters (for display).
pub fn kind_of(name: &str) -> Option<ObjKind> {
    ObjKind::CAMPS.into_iter().find(|k| crate::layout_build::camp_monsters(*k).first().is_some_and(|m| m.eq_ignore_ascii_case(name)))
}

fn is_map1_jungle(p: &MapProject) -> bool {
    let Some(f) = p.file("Scripts/NeutralMinionSpawn.luaobj") else { return false };
    let Ok(calls) = crate::formats::luaobj::call_args(&f.original, "Make3DPoint") else { return false };
    calls.first().and_then(|c| Some((c.first()?.as_num()?, c.get(2)?.as_num()?))).is_some_and(|(x, z)| (x - 3632.7).abs() < 1.0 && (z - 7600.37).abs() < 1.0)
}

/// The monsters of a layout's camps (live while editing; heights left to the caller).
pub fn from_layout(l: &crate::layout::Layout) -> Vec<Monster> {
    let [w, d] = l.size();
    let mut out = Vec::new();
    for (n, o) in l.objectives.iter().filter(|o| o.kind.is_camp()).enumerate() {
        let yaw = if o.yaw != 0.0 { o.yaw } else { (w / 2.0 - o.pos[0]).atan2(d / 2.0 - o.pos[1]).to_degrees() };
        for (name, p) in crate::layout_build::camp_monsters(o.kind).iter().zip(crate::layout_build::camp_positions(o)) {
            out.push(Monster { name: name.to_string(), pos: [p[0], f32::NAN, p[1]], yaw, camp: n + 1 });
        }
    }
    out
}
