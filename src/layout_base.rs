//! Export of an opened map that was edited as a layout (`Layout::base`): the original folder is
//! the starting point and only what changed is rewritten, so an unchanged import exports the map
//! byte for byte.
//!
//! - structures changed: their map objects, scene meshes, ObjectCFG sections and the turret
//!   script are replaced by generated ones (original names and skins are kept);
//! - lanes changed: the minions' nav points; camps changed: NeutralMinionSpawn.lua;
//! - effects / lights changed: Particles.dat / Light_Env.dat and Light.dat;
//! - painted walls and brush, and props: the navigation grid; props and new walls are added to
//!   room.nvr (maps with a baked room.wgeo keep their scene).

use crate::create::grid_height;
use crate::formats::{
    mob::{MapObjects, ObjectType},
    navgrid::{NavGrid, flags},
    nvr::{Geometry, Material, material_type},
};
use crate::layout::{CELL, Layout, Objective, Surface};
use crate::layout_build::{BuildOptions, add_props, camp_def_exists, create_level_props_pub, lane_points, neutral_script, object_names, resolve_texture, world_files, write_structures};
use crate::layout_import::{classify, grid_surface};
use crate::project::{Doc, MapFile, MapProject};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// Opened base maps, kept while the app runs (opening Summoner's Rift takes a moment).
pub fn base_project(path: &str) -> Result<Arc<MapProject>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<MapProject>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(p) = cache.lock().unwrap().get(path) {
        return Ok(p.clone());
    }
    let p = Arc::new(MapProject::open(&PathBuf::from(path)).with_context(|| format!("cannot open the original map {path}"))?);
    cache.lock().unwrap().insert(path.to_string(), p.clone());
    Ok(p)
}

fn split<'a>(v: &'a [Objective], camps: bool) -> Vec<&'a Objective> {
    v.iter().filter(|o| camp_def_exists(o.kind) == camps).collect()
}

/// Path of the first file of a kind (case kept as in the map).
fn path_of(p: &MapProject, pred: impl Fn(&MapFile) -> bool) -> Option<String> {
    p.files.iter().find(|f| pred(f)).map(|f| f.path.clone())
}

/// Removes `[name]` sections from an ObjectCFG text.
fn drop_sections(cfg: &mut String, names: &[String]) {
    for n in names {
        while let Some((a, b)) = crate::edit::section_range(cfg, n) {
            // Include the header line.
            let start = cfg[..a].rfind('[').unwrap_or(a);
            cfg.replace_range(start..b, "");
        }
    }
}

/// Rock walls for cells painted into walls on the opened map: a raised block per new wall area.
fn wall_blocks(l: &Layout, was: &[u8], height: &dyn Fn(f32, f32) -> f32) -> Geometry {
    let mut g = Geometry::default();
    let is_new = |i: i64, j: i64| i >= 0 && j >= 0 && (i as usize) < l.nx && (j as usize) < l.ny && {
        let k = j as usize * l.nx + i as usize;
        l.surface[k] == Surface::Wall as u8 && was[k] != Surface::Wall as u8
    };
    let wall_h = 260.0;
    let mut quad = |p: [[f32; 3]; 4], n: [f32; 3]| {
        let b = g.positions.len() as u32;
        for (k, v) in p.iter().enumerate() {
            g.positions.push(*v);
            g.normals.push(n);
            let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]][k];
            let uv = [uv[0] + v[0] / 256.0, uv[1] + (v[1] + v[2]) / 256.0];
            g.uvs.push(uv);
            g.uvs1.push(uv);
            g.colors.push(0xff000000);
        }
        g.indices.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
    };
    for j in 0..l.ny as i64 {
        for i in 0..l.nx as i64 {
            if !is_new(i, j) {
                continue;
            }
            let (x0, z0, x1, z1) = (i as f32 * CELL, j as f32 * CELL, (i + 1) as f32 * CELL, (j + 1) as f32 * CELL);
            let base = height((x0 + x1) / 2.0, (z0 + z1) / 2.0);
            let top = base + wall_h;
            quad([[x0, top, z0], [x1, top, z0], [x1, top, z1], [x0, top, z1]], [0.0, 1.0, 0.0]);
            let lo = base - 40.0;
            if !is_new(i, j - 1) {
                quad([[x0, lo, z0], [x1, lo, z0], [x1, top, z0], [x0, top, z0]], [0.0, 0.0, -1.0]);
            }
            if !is_new(i, j + 1) {
                quad([[x1, lo, z1], [x0, lo, z1], [x0, top, z1], [x1, top, z1]], [0.0, 0.0, 1.0]);
            }
            if !is_new(i - 1, j) {
                quad([[x0, lo, z1], [x0, lo, z0], [x0, top, z0], [x0, top, z1]], [-1.0, 0.0, 0.0]);
            }
            if !is_new(i + 1, j) {
                quad([[x1, lo, z0], [x1, lo, z1], [x1, top, z1], [x1, top, z0]], [1.0, 0.0, 0.0]);
            }
        }
    }
    g
}

pub fn build_on_base(l: &Layout, opt: &BuildOptions) -> Result<MapProject> {
    let base = l.base.as_ref().context("not an opened map")?;
    let orig = base_project(&base.path)?;
    let mut p = (*orig).clone();
    p.name = format!("Map{}", l.map_id);
    let g0: NavGrid = orig.navgrid().context("the original map has no navigation grid")?.clone();
    let h = |x: f32, z: f32| grid_height(&g0, x, z);

    let structures_changed = split(&l.objectives, false) != split(&base.objectives, false);
    let camps_changed = split(&l.objectives, true) != split(&base.objectives, true);
    let lanes_changed = l.lanes != base.lanes;

    // Structures and lanes (map objects, scene meshes, ObjectCFG, turret script).
    if structures_changed || lanes_changed {
        let mob_path = path_of(&p, |f| matches!(f.doc, Doc::Mob(_))).unwrap_or_else(|| "Scene/MapObjects.mob".into());
        let mut mob = p.mob().cloned().unwrap_or(MapObjects { reserved: 0, objects: Vec::new() });
        if structures_changed {
            let structural = |k: ObjectType| matches!(k, ObjectType::Turret | ObjectType::Nexus | ObjectType::Inhibitor | ObjectType::SpawnPoint | ObjectType::Barracks | ObjectType::Shop | ObjectType::InfoPoint);
            let mut old: Vec<String> = mob.objects.iter().filter(|o| structural(o.kind)).map(|o| o.name.clone()).collect();
            mob.objects.retain(|o| !structural(o.kind));
            let mut dsc = p.dsc_entries();
            let mut stems = Vec::new();
            dsc.retain(|e| {
                let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0).to_string();
                if classify(&stem).is_some() {
                    stems.push(stem);
                    false
                } else {
                    true
                }
            });
            for stem in &stems {
                let paths: Vec<String> = p.files.iter().filter(|f| f.stem().eq_ignore_ascii_case(stem) && f.path.to_ascii_lowercase().starts_with("scene/") && !f.path.to_ascii_lowercase().contains("/cfg/")).map(|f| f.path.clone()).collect();
                for q in paths {
                    p.remove(&q);
                }
            }
            old.extend(stems.iter().cloned());
            let cfg_path = path_of(&p, |f| f.path.to_ascii_lowercase().ends_with("objectcfg.cfg")).unwrap_or_else(|| "Scene/CFG/ObjectCFG.cfg".into());
            let mut cfg = p.text(&cfg_path).unwrap_or("").to_string();
            drop_sections(&mut cfg, &old);
            let named = object_names(l);
            let mut cfg_new = String::new();
            write_structures(&named, &h, &mut mob.objects, &mut dsc, &mut cfg_new, &mut p)?;
            cfg += &cfg_new;
            p.set_dsc_entries(&dsc);
            p.add_or_replace(MapFile::generated(&cfg_path, crate::formats::bin::latin1_encode(&cfg)));
            if let Some(q) = path_of(&p, |f| f.path.eq_ignore_ascii_case("Scripts/CreateLevelProps.luaobj")) {
                p.remove(&q);
            }
            p.add_or_replace(MapFile::generated("Scripts/CreateLevelProps.lua", crate::formats::bin::latin1_encode(&create_level_props_pub(&named))));
        }
        if lanes_changed {
            mob.objects.retain(|o| o.kind != ObjectType::NavPoint);
            mob.objects.extend(lane_points(l, &h));
        }
        p.add_or_replace(MapFile::generated(&mob_path, mob.write()?));
    }

    // Jungle camps (and ARAM relics).
    if camps_changed && !base.keep_camps {
        if let Some(q) = path_of(&p, |f| f.path.eq_ignore_ascii_case("Scripts/NeutralMinionSpawn.luaobj")) {
            p.remove(&q);
        }
        p.add_or_replace(MapFile::generated("Scripts/NeutralMinionSpawn.lua", crate::formats::bin::latin1_encode(&neutral_script(l, &h))));
    }

    // Effects and lights.
    if l.effects != base.effects || l.lights != base.lights {
        let (particles, env, old) = world_files(l, &h);
        if l.effects != base.effects {
            p.add_or_replace(MapFile::generated("Particles.dat", particles.into_bytes()));
        }
        if l.lights != base.lights {
            p.add_or_replace(MapFile::generated("Light_Env.dat", env.into_bytes()));
            p.add_or_replace(MapFile::generated("Light.dat", old.into_bytes()));
        }
    }

    // Props and new walls: scene geometry (room.nvr maps) and walkability.
    let was = grid_surface(&g0, l.nx, l.ny);
    let painted: Vec<usize> = (0..l.surface.len()).filter(|&k| l.surface[k] != was[k]).collect();
    let mut mats: Vec<Material> = Vec::new();
    let mut geo: Vec<(u32, Geometry)> = Vec::new();
    // New props, and newly painted walls dressed with rocks and trees (League style) or a raised block.
    let blockers = add_props(l, opt, Some(&was), &h, &mut mats, &mut geo, &mut p)?;
    let walls = if l.theme.forest.league_walls && opt.props.is_some() { Geometry::default() } else { wall_blocks(l, &was, &h) };
    if !walls.indices.is_empty() {
        let (name, data) = resolve_texture(&l.theme.cliff)?;
        if p.find(&format!("Scene/Textures/{name}")).is_none() {
            p.add_or_replace(MapFile::generated(&format!("Scene/Textures/{name}"), data));
        }
        mats.push(Material::new("lms_wall", &name, material_type::DEFAULT, 0x10));
        geo.push(((mats.len() - 1) as u32, walls));
    }
    if !geo.is_empty()
        && let Some(nvr_path) = path_of(&p, |f| matches!(f.doc, Doc::Nvr(_)))
        && let Some(Doc::Nvr(n)) = p.file(&nvr_path).map(|f| &f.doc)
    {
        let mut nvr = (**n).clone();
        let first = nvr.materials.len() as u32;
        nvr.materials.extend(mats.iter().cloned());
        for (m, g) in &geo {
            if !g.indices.is_empty() {
                nvr.append_geometry(first + m, g, -100)?;
            }
        }
        p.add_or_replace(MapFile::generated(&nvr_path, nvr.write()?));
    } else if !geo.is_empty()
        && let Some(wgeo_path) = path_of(&p, |f| matches!(f.doc, Doc::Wgeo(_)))
        && let Some(Doc::Wgeo(w)) = p.file(&wgeo_path).map(|f| &f.doc)
    {
        // Baked scenes (room.wgeo): the new meshes are drawn with their texture only, like the
        // rest of the baked map. The ground buckets stay as they are.
        let mut w = (**w).clone();
        for (m, g) in &geo {
            if g.indices.is_empty() {
                continue;
            }
            let mat = &mats[*m as usize];
            w.meshes.push(crate::formats::wgeo::WgeoMesh::new(&mat.channels[0].texture, &mat.name, g.positions.clone(), g.uvs.clone(), g.indices.clone()));
        }
        w.total_triangles = w.meshes.iter().map(|m| m.indices.len() as u32 / 3).sum();
        p.add_or_replace(MapFile::generated(&wgeo_path, w.write()?));
    }
    if !painted.is_empty() || !blockers.is_empty() {
        let mut g = g0.clone();
        for &k in &painted {
            let (i, j) = (k % l.nx, k / l.nx);
            let Some((ci, cj)) = g.cell_of((i as f32 + 0.5) * CELL, (j as f32 + 0.5) * CELL) else { continue };
            let c = cj * g.nx + ci;
            let f = g.flags(c) & !(flags::WALL | flags::BRUSH);
            let s = l.surface[k];
            let f = if s == Surface::Wall as u8 || s == Surface::Void as u8 || s == Surface::DeepWater as u8 {
                f | flags::WALL
            } else if s == Surface::Brush as u8 {
                f | flags::BRUSH
            } else {
                f
            };
            g.set_flags(c, f);
        }
        for (x, z, r) in &blockers {
            let n = (r / g.cell_size).ceil() as i64;
            let Some((ci, cj)) = g.cell_of(*x, *z) else { continue };
            for dj in -n..=n {
                for di in -n..=n {
                    let (i, j) = (ci as i64 + di, cj as i64 + dj);
                    if i < 0 || j < 0 || i as usize >= g.nx || j as usize >= g.ny {
                        continue;
                    }
                    let c = g.cell_center(i as usize, j as usize);
                    if (c[0] - x).hypot(c[1] - z) <= *r {
                        let k = j as usize * g.nx + i as usize;
                        g.set_flags(k, (g.flags(k) | flags::WALL) & !flags::BRUSH);
                    }
                }
            }
        }
        if !opt.preview {
            g.rebuild_hints();
        }
        let grid_path = path_of(&p, |f| matches!(f.doc, Doc::NavGrid(_))).unwrap_or_else(|| "AIPath.aimesh_ngrid".into());
        p.add_or_replace(MapFile::generated(&grid_path, g.write()));
    }
    crate::map_settings::apply(&mut p, &l.settings);
    Ok(p)
}
