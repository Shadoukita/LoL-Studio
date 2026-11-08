//! Opened maps as editable layouts. The map's own scene art, textures and rules stay as they are;
//! what can be edited is read into a `Layout`: walls and brush from the navigation grid, ground
//! heights from its sampled heights, structures (with their original names and skins), lanes,
//! jungle camps, particle effects and lights. `layout_build::build` merges the edits back.

use crate::create::grid_height;
use crate::formats::{luaobj, mob::ObjectType, navgrid::flags};
use crate::layout::{BaseMap, CELL, EffectPlace, Lane, LanePath, Layout, LightPlace, ObjKind, Objective, Surface, Symmetry, TurretTier};
use crate::project::{Doc, MapProject};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

fn lane_of(c: char) -> Lane {
    match c {
        'L' => Lane::Top,
        'R' => Lane::Bot,
        _ => Lane::Mid,
    }
}

fn team_of(name: &str) -> u8 {
    let l = name.to_ascii_lowercase();
    if l.contains("t2") || l.contains("chaos") { 2 } else { 1 }
}

/// What a map object or scene mesh is, from Riot's naming.
pub fn classify(name: &str) -> Option<(ObjKind, u8, Lane)> {
    let l = name.to_ascii_lowercase();
    let team = team_of(name);
    if l.starts_with("turret_") {
        if l.contains("shrine") {
            return Some((ObjKind::FountainTurret, team, Lane::Mid));
        }
        // Turret_T1_C_05
        let lane = name.split('_').nth(2).and_then(|s| s.chars().next()).map_or(Lane::Mid, lane_of);
        return Some((ObjKind::Turret, team, lane));
    }
    if l.starts_with("hq_t") {
        return Some((ObjKind::Nexus, team, Lane::Mid));
    }
    if l.starts_with("barracks_t") {
        let lane = name.split('_').nth(2).and_then(|s| s.chars().next()).map_or(Lane::Mid, lane_of);
        return Some((ObjKind::Inhibitor, team, lane));
    }
    if l.starts_with("__spawn_t") {
        return Some((ObjKind::SpawnPoint, team, Lane::Mid));
    }
    if l.starts_with("__p_") && l.contains("_spawn_barracks__") {
        let lane = name.rsplit("__").next().and_then(|s| s.chars().next()).map_or(Lane::Mid, lane_of);
        return Some((ObjKind::MinionSpawn, team, lane));
    }
    if l.contains("shop") {
        return Some((ObjKind::Shop, if l.starts_with("chaos") { 2 } else { 1 }, Lane::Mid));
    }
    if l.starts_with("info_point") {
        return Some((ObjKind::CapturePoint, 0, Lane::Mid));
    }
    if l.starts_with("info_speedshrine") {
        return Some((ObjKind::SpeedShrine, 0, Lane::Mid));
    }
    if l.starts_with("info_largerelic") {
        return Some((ObjKind::CenterRelic, 0, Lane::Mid));
    }
    if l.starts_with("info_healthpack") || l.starts_with("info_healthrelic") {
        return Some((ObjKind::HealthRelic, 0, Lane::Mid));
    }
    if l.starts_with("info_spawn_") && l.contains("crystal") {
        return Some((ObjKind::Crystal, team, Lane::Mid));
    }
    None
}

/// Walls and brush of a navigation grid, per layout cell (CELL units from the origin).
pub fn grid_surface(g: &crate::formats::navgrid::NavGrid, nx: usize, ny: usize) -> Vec<u8> {
    let mut out = vec![Surface::Void as u8; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let (x, z) = ((i as f32 + 0.5) * CELL, (j as f32 + 0.5) * CELL);
            if let Some((ci, cj)) = g.cell_of(x, z) {
                let f = g.flags(cj * g.nx + ci);
                out[j * nx + i] = if f & flags::WALL != 0 {
                    Surface::Wall as u8
                } else if f & flags::BRUSH != 0 {
                    Surface::Brush as u8
                } else {
                    Surface::Ground as u8
                };
            }
        }
    }
    out
}

/// Effects from a Particles.dat (sound emitters included, so nothing is lost on export).
pub fn read_particles(text: &str, height: &dyn Fn(f32, f32) -> f32, next_id: &mut u32) -> Vec<EffectPlace> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 4 {
                return None;
            }
            let n = |i: usize| f.get(i).and_then(|v| v.parse::<f32>().ok());
            let (x, y, z) = (n(1)?, n(2)?, n(3)?);
            *next_id += 1;
            Some(EffectPlace { id: *next_id - 1, name: f[0].to_string(), pos: [x, z], lift: y - height(x, z), yaw: n(6).unwrap_or(0.0) })
        })
        .collect()
}

/// Lights from Light_Env.dat (versions 1 to 3) or Light.dat.
pub fn read_lights(env: Option<&str>, old: Option<&str>, height: &dyn Fn(f32, f32) -> f32, next_id: &mut u32) -> Vec<LightPlace> {
    let mut out = Vec::new();
    let mut push = |x: f32, y: f32, z: f32, c: [f32; 3], inner: f32, outer: f32, intensity: f32| {
        out.push(LightPlace { id: *next_id, pos: [x, z], lift: y - height(x, z), color: c.map(|v| v.clamp(0.0, 255.0) as u8), inner, outer, intensity });
        *next_id += 1;
    };
    if let Some(t) = env {
        let mut lines = t.lines();
        let version: u32 = lines.next().and_then(|v| v.trim().parse().ok()).unwrap_or(3);
        for l in lines {
            let v: Vec<f32> = l.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            match version {
                1 if v.len() >= 9 => push(v[0], v[1], v[2], [v[3], v[4], v[5]], v[6], v[7], 1.0),
                2 if v.len() >= 12 => push(v[0], v[1], v[2], [v[3], v[4], v[5]], v[9], v[10], 1.0),
                _ if v.len() >= 13 => push(v[0], v[1], v[2], [v[3], v[4], v[5]], v[9], v[10], v[12]),
                _ => {}
            }
        }
    } else if let Some(t) = old {
        for l in t.lines() {
            let v: Vec<f32> = l.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            if v.len() >= 7 {
                push(v[0], v[1], v[2], [v[3], v[4], v[5]], 20.0, v[6], 1.0);
            }
        }
    }
    out
}

/// Reads an opened map (its folder at `path`) into an editable layout.
pub fn import(p: &MapProject, path: &Path) -> Result<Layout> {
    let g = p.navgrid().context("the map has no navigation grid (AIPath.aimesh_ngrid)")?;
    let size = [g.max[0].max(g.min[0] + g.nx as f32 * g.cell_size), g.max[2].max(g.min[2] + g.ny as f32 * g.cell_size)];
    let digits: String = p.name.chars().filter(|c| c.is_ascii_digit()).collect();
    let map_id = digits.parse().unwrap_or(1);
    let mut l = Layout::new(&p.name, map_id, size);
    // New scenery for an opened map comes from that map's own art by default.
    if let Some(n) = path.file_name() {
        l.asset_maps = vec![n.to_string_lossy().to_string()];
    }
    l.symmetry = Symmetry::None;
    // Heights are the map's own; generated walls, rivers and trees are off.
    l.theme.base_height = 0.0;
    l.theme.wall_height = 0.0;
    l.theme.river_depth = 0.0;
    l.theme.forest.enabled = false;
    let h = |x: f32, z: f32| grid_height(g, x, z);
    l.surface = grid_surface(g, l.nx, l.ny);
    for j in 0..l.ny {
        for i in 0..l.nx {
            l.height[j * l.nx + i] = h((i as f32 + 0.5) * CELL, (j as f32 + 0.5) * CELL);
        }
    }

    // Turret arguments from the map's CreateLevelProps (name -> skin, tier, lane).
    let mut turrets: HashMap<String, (String, u8, u8)> = HashMap::new();
    // Riot's maps call CreateChildTurret directly or through a SpawnTurret helper (map11).
    if let Some(f) = p.file("Scripts/CreateLevelProps.luaobj") {
        let mut calls = Vec::new();
        for func in ["CreateChildTurret", "SpawnTurret"] {
            calls.extend(luaobj::call_args(&f.original, func).unwrap_or_default());
        }
        for c in calls {
            if let (Some(n), Some(s), Some(t), Some(ln)) = (c.first().and_then(|v| v.as_str()), c.get(1).and_then(|v| v.as_str()), c.get(3).and_then(|v| v.as_num()), c.get(4).and_then(|v| v.as_num())) {
                turrets.insert(n.to_ascii_lowercase(), (s.to_string(), t as u8, ln as u8));
            }
        }
    }

    // Structures: map objects, and Map1-style scene meshes (their central point is the position).
    let mut seen = std::collections::HashSet::new();
    let mut found: Vec<(String, [f32; 2], f32)> = Vec::new();
    if let Some(m) = p.mob() {
        for o in &m.objects {
            if matches!(o.kind, ObjectType::Turret | ObjectType::Nexus | ObjectType::Inhibitor | ObjectType::SpawnPoint | ObjectType::Barracks | ObjectType::Shop | ObjectType::InfoPoint) && seen.insert(o.name.to_ascii_lowercase()) {
                found.push((o.name.clone(), [o.position[0], o.position[2]], o.pitch_yaw_roll[1]));
            }
        }
    }
    for e in p.dsc_entries() {
        let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0).to_string();
        if classify(&stem).is_none() || !seen.insert(stem.to_ascii_lowercase()) {
            continue;
        }
        let c = p.files.iter().filter(|f| f.stem().eq_ignore_ascii_case(&stem)).find_map(|f| match &f.doc {
            Doc::Scb(s) => Some(s.central),
            Doc::Sco(s) => Some(s.central),
            _ => None,
        });
        if let Some(c) = c {
            found.push((stem, [c[0], c[2]], 0.0));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, pos, yaw) in found {
        let Some((kind, team, mut lane)) = classify(&name) else { continue };
        let mut tier = TurretTier::Outer;
        let mut skin = None;
        if let Some((s, t, ln)) = turrets.get(&name.to_ascii_lowercase()) {
            skin = Some(s.clone());
            tier = TurretTier::from_code(*t);
            lane = match ln {
                0 => Lane::Bot,
                2 => Lane::Top,
                _ => Lane::Mid,
            };
        }
        let id = l.next_id;
        l.next_id += 1;
        l.objectives.push(Objective { id, kind, team, lane, tier, pos, yaw, mirror: None, name: Some(name), skin });
    }

    // Jungle camps (one objective per camp, at its first monster).
    let mut camps: Vec<(usize, String, [f32; 2])> = Vec::new();
    for m in crate::jungle::monsters(p) {
        if !camps.iter().any(|c| c.0 == m.camp) {
            camps.push((m.camp, m.name.clone(), [m.pos[0], m.pos[2]]));
        }
    }
    let mid = (size[0] + size[1]) / 2.0;
    for (_, name, pos) in camps {
        let kind = if name.eq_ignore_ascii_case("HA_AP_HealthRelic") { Some(ObjKind::HealthRelic) } else { crate::jungle::kind_of(&name) };
        let Some(kind) = kind else { continue };
        let team = if kind.is_epic() || kind == ObjKind::HealthRelic { 0 } else if pos[0] + pos[1] < mid { 1 } else { 2 };
        let id = l.next_id;
        l.next_id += 1;
        l.objectives.push(Objective { id, kind, team, lane: Lane::Mid, tier: TurretTier::Outer, pos, yaw: 0.0, mirror: None, name: None, skin: None });
    }

    // Lanes from the minions' nav points (__NAV_L02, __NAV_C03, ...).
    if let Some(m) = p.mob() {
        let mut by: HashMap<char, Vec<(u32, [f32; 2])>> = HashMap::new();
        for o in m.objects.iter().filter(|o| o.kind == ObjectType::NavPoint) {
            let rest = o.name.trim_start_matches('_').trim_start_matches("NAV_");
            let mut ch = rest.chars();
            let (Some(c), num) = (ch.next(), ch.as_str()) else { continue };
            let n: u32 = num.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
            by.entry(c).or_default().push((n, [o.position[0], o.position[2]]));
        }
        for (c, mut pts) in by {
            pts.sort_by_key(|x| x.0);
            if pts.len() >= 2 {
                l.lanes.push(LanePath { lane: lane_of(c), points: pts.into_iter().map(|x| x.1).collect() });
            }
        }
        l.lanes.sort_by_key(|lp| lp.lane.index());
    }

    // Effects and lights.
    let mut next = l.next_id;
    l.effects = p.text("Particles.dat").map(|t| read_particles(t, &h, &mut next)).unwrap_or_default();
    l.lights = read_lights(p.text("Light_Env.dat"), p.text("Light.dat"), &h, &mut next);
    l.next_id = next;

    // A compiled camp script whose camps could not be read is kept as it is.
    let keep_camps = p.file("Scripts/NeutralMinionSpawn.luaobj").is_some() && !l.objectives.iter().any(|o| crate::layout_build::camp_def_exists(o.kind));
    l.base = Some(BaseMap { path: path.to_string_lossy().to_string(), objectives: l.objectives.clone(), lanes: l.lanes.clone(), effects: l.effects.clone(), lights: l.lights.clone(), keep_camps });
    Ok(l)
}
