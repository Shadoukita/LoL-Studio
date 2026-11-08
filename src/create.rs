//! New maps from scratch, and terrain rebuilt from the navigation grid.
//!
//! A generated map uses its navigation grid as the terrain layout: walkable cells are ground at the
//! cell's height, wall cells rise into cliffs, brush cells get grass. The scene geometry
//! (`room.nvr` or `room.wgeo`), sampled heights and the minimap are rebuilt from it, so painting
//! the grid in the editor reshapes the map.
//!
//! Gameplay data (level scripts, inibins, object settings) comes from a template map: the level
//! scripts address structures by name, so a new map keeps the template's turrets, inhibitors,
//! nexuses, spawns and lane nav points, moved to proportional positions.

use crate::formats::{
    aimesh::AiMesh,
    dds::{self, Image},
    inibin::{Value, hash_key},
    mob::{MapObjects, ObjectType},
    navgrid::{NavGrid, flags},
    nvr::{Geometry, Material, Nvr, material_type},
    scb::{Sco, Scb},
    wgeo::{Wgeo, WgeoMesh},
};
use crate::project::{Doc, MapFile, MapProject, SceneFormat, TERRAIN_RENDERER_KEY, parse_dsc};
use anyhow::{Context, Result, bail};

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainStyle {
    /// Texture files (in the scene texture folder) for each surface.
    pub ground_texture: String,
    pub cliff_texture: String,
    pub brush_texture: String,
    pub water_texture: String,
    /// How far wall cells rise above the neighbouring ground.
    pub wall_height: f32,
    /// World units per texture repeat.
    pub texture_scale: f32,
    pub water_level: Option<f32>,
    pub brush_height: f32,
}

impl Default for TerrainStyle {
    fn default() -> Self {
        Self {
            ground_texture: GEN_GROUND.into(),
            cliff_texture: GEN_CLIFF.into(),
            brush_texture: GEN_BRUSH.into(),
            water_texture: GEN_WATER.into(),
            wall_height: 260.0,
            texture_scale: 600.0,
            water_level: None,
            brush_height: 140.0,
        }
    }
}

pub const GEN_GROUND: &str = "lms_ground.dds";
pub const GEN_CLIFF: &str = "lms_cliff.dds";
pub const GEN_BRUSH: &str = "lms_brush.dds";
pub const GEN_WATER: &str = "lms_water.dds";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartLayout {
    /// The template's walls, brush and heights, scaled to the new size.
    TemplateScaled,
    /// Open ground with a wall border.
    Blank,
}

#[derive(Clone, Debug)]
pub struct NewMapOptions {
    pub folder_name: String,
    pub size: [f32; 2],
    pub cell_size: f32,
    pub layout: StartLayout,
    pub base_height: f32,
    /// Amplitude of gentle rolling hills on blank layouts (0 = flat).
    pub hills: f32,
    pub border_cells: usize,
    pub style: TerrainStyle,
    /// Keep the template's decorative level props, particles and lights (moved proportionally).
    pub keep_decor: bool,
}

impl Default for NewMapOptions {
    fn default() -> Self {
        Self {
            folder_name: "Map30".into(),
            size: [12000.0, 12000.0],
            cell_size: 50.0,
            layout: StartLayout::TemplateScaled,
            base_height: 50.0,
            hills: 0.0,
            border_cells: 6,
            style: TerrainStyle::default(),
            keep_decor: true,
        }
    }
}

/// Files copied from the template unchanged (everything not regenerated).
fn is_regenerated(path: &str) -> bool {
    let l = path.to_ascii_lowercase();
    l.starts_with("scene/") && !l.starts_with("scene/cfg/")
        || l == "aipath.aimesh_ngrid"
        || l == "aipath.aimesh"
        || l == "info/2dlevelminimap.dds"
        || l == "particles.dat"
        || l == "light.dat"
}

struct Mapping {
    t_min: [f32; 2],
    t_size: [f32; 2],
    size: [f32; 2],
}

impl Mapping {
    fn xz(&self, x: f32, z: f32) -> (f32, f32) {
        ((x - self.t_min[0]) / self.t_size[0] * self.size[0], (z - self.t_min[1]) / self.t_size[1] * self.size[1])
    }
}

/// Ground height from a navigation grid (bilinear over cell centres).
pub fn grid_height(g: &NavGrid, x: f32, z: f32) -> f32 {
    let fx = ((x - g.min[0]) / g.cell_size - 0.5).clamp(0.0, (g.nx - 1) as f32);
    let fz = ((z - g.min[2]) / g.cell_size - 0.5).clamp(0.0, (g.ny - 1) as f32);
    let (i0, j0) = (fx.floor() as usize, fz.floor() as usize);
    let (i1, j1) = ((i0 + 1).min(g.nx - 1), (j0 + 1).min(g.ny - 1));
    let (tx, tz) = (fx - i0 as f32, fz - j0 as f32);
    let h = |i: usize, j: usize| g.height(j * g.nx + i);
    let a = h(i0, j0) * (1.0 - tx) + h(i1, j0) * tx;
    let b = h(i0, j1) * (1.0 - tx) + h(i1, j1) * tx;
    a * (1.0 - tz) + b * tz
}

pub fn new_map(template: &MapProject, opt: &NewMapOptions) -> Result<MapProject> {
    let format = template.scene_format().context("the template has no room.nvr or room.wgeo")?;
    let tgrid = template.navgrid().context("the template has no AIPath.aimesh_ngrid")?;
    if opt.size[0] < 2000.0 || opt.size[1] < 2000.0 || opt.size[0] > 30000.0 || opt.size[1] > 30000.0 {
        bail!("map size must be between 2000 and 30000 units per side");
    }
    let mut p = MapProject::empty(&opt.folder_name);
    for f in &template.files {
        if !is_regenerated(&f.path) {
            p.add_or_replace(MapFile::new(&f.path, f.original.to_vec()));
        }
    }
    for f in &mut p.files {
        f.dirty = false;
    }

    // Navigation grid: the terrain layout.
    let cs = opt.cell_size;
    let nx = (opt.size[0] / cs).ceil() as usize;
    let ny = (opt.size[1] / cs).ceil() as usize;
    let size = [nx as f32 * cs, ny as f32 * cs];
    let mut grid = NavGrid::new_blank(tgrid.version.max(3), [0.0, opt.base_height, 0.0], [size[0], opt.base_height, size[1]], cs, nx, ny);
    let map = Mapping {
        t_min: [tgrid.min[0], tgrid.min[2]],
        t_size: [tgrid.max[0] - tgrid.min[0], tgrid.max[2] - tgrid.min[2]],
        size,
    };
    for j in 0..ny {
        for i in 0..nx {
            let k = j * nx + i;
            let [x, z] = grid.cell_center(i, j);
            let border = i < opt.border_cells || j < opt.border_cells || i + opt.border_cells >= nx || j + opt.border_cells >= ny;
            let (mut f, h) = match opt.layout {
                StartLayout::TemplateScaled => {
                    let tx = map.t_min[0] + x / size[0] * map.t_size[0];
                    let tz = map.t_min[1] + z / size[1] * map.t_size[1];
                    match tgrid.cell_of(tx, tz) {
                        Some((ti, tj)) => {
                            let tk = tj * tgrid.nx + ti;
                            (tgrid.flags(tk) & (flags::WALL | flags::BRUSH | flags::SEE_THROUGH), tgrid.height(tk))
                        }
                        None => (flags::WALL, opt.base_height),
                    }
                }
                StartLayout::Blank => {
                    let hill = if opt.hills > 0.0 { opt.hills * (smooth_noise(x / 2400.0, z / 2400.0, 7) - 0.5) * 2.0 } else { 0.0 };
                    (0, opt.base_height + hill)
                }
            };
            if border {
                f |= flags::WALL;
            }
            grid.set_flags(k, f);
            grid.set_height(k, h);
        }
    }
    let tg_height = |x: f32, z: f32| grid_height(tgrid, x, z);

    // Map objects: structures, spawns and lane points moved proportionally.
    let mut mob = template.mob().cloned().unwrap_or(MapObjects { reserved: 0, objects: Vec::new() });
    mob.objects.retain(|o| {
        let brush = o.name.starts_with("LevelProp_brush");
        match o.kind {
            ObjectType::LevelProp => opt.keep_decor && !(brush && opt.layout == StartLayout::Blank),
            _ => true,
        }
    });
    let moved: Vec<(usize, [f32; 3])> = mob
        .objects
        .iter()
        .enumerate()
        .filter(|(_, o)| o.kind != ObjectType::LevelSize)
        .map(|(i, o)| {
            let (x, z) = map.xz(o.position[0], o.position[2]);
            let lift = o.position[1] - tg_height(o.position[0], o.position[2]);
            (i, [x, grid_height(&grid, x, z) + lift, z])
        })
        .collect();
    for (i, np) in moved {
        let o = &mut mob.objects[i];
        let d = [np[0] - o.position[0], np[1] - o.position[1], np[2] - o.position[2]];
        o.position = np;
        if o.bbox_min != [0.0; 3] || o.bbox_max != [0.0; 3] {
            for a in 0..3 {
                o.bbox_min[a] += d[a];
                o.bbox_max[a] += d[a];
            }
        }
    }
    // Keep structures on walkable ground.
    for o in &mob.objects {
        if matches!(o.kind, ObjectType::Turret | ObjectType::Nexus | ObjectType::Inhibitor | ObjectType::SpawnPoint | ObjectType::Barracks | ObjectType::NavPoint | ObjectType::Shop) {
            clear_walls(&mut grid, o.position[0], o.position[2], 250.0);
        }
    }

    // Object meshes in Scene/ (Map1-style turrets, inhibitors, nexus, shops, lake, and the meshes
    // Map1 keeps for its map objects): copied and moved with their objects.
    let mut stems: Vec<String> = template
        .files
        .iter()
        .filter(|f| f.path.to_ascii_lowercase().starts_with("scene/") && !f.path[6..].contains('/') && matches!(f.doc, Doc::Scb(_) | Doc::Sco(_)))
        .map(|f| f.stem().to_string())
        .collect();
    stems.sort_by_key(|s| s.to_ascii_lowercase());
    stems.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let template_mob = template.mob();
    let listed = parse_dsc(template.text("Scene/room.dsc").unwrap_or(""));
    for stem in stems {
        if stem.eq_ignore_ascii_case("AIPath") {
            continue;
        }
        let is_lake = stem.to_ascii_lowercase().starts_with("lake");
        if is_lake && opt.style.water_level.is_none() {
            continue;
        }
        let paths: Vec<String> = template.files.iter().filter(|f| f.path.len() > 6 && f.path[6..].contains('.') && f.path.to_ascii_lowercase().starts_with("scene/") && f.stem().eq_ignore_ascii_case(&stem) && !f.path[6..].contains('/')).map(|f| f.path.clone()).collect();
        let central = paths.iter().find_map(|path| match template.file(path).map(|f| &f.doc) {
            Some(Doc::Scb(s)) => Some(s.central),
            Some(Doc::Sco(s)) => Some(s.central),
            _ => None,
        });
        let Some(central) = central else { continue };
        let xform: Box<dyn Fn([f32; 3]) -> [f32; 3]> = if is_lake {
            let wl = opt.style.water_level.unwrap();
            let m = Mapping { t_min: map.t_min, t_size: map.t_size, size };
            Box::new(move |p: [f32; 3]| {
                let (x, z) = m.xz(p[0], p[2]);
                [x, wl, z]
            })
        } else {
            let old_obj = template_mob.and_then(|m| m.find(&stem).map(|i| m.objects[i].position));
            let new_obj = mob.find(&stem).map(|i| mob.objects[i].position);
            let d = match (old_obj, new_obj) {
                (Some(o), Some(n)) => [n[0] - o[0], n[1] - o[1], n[2] - o[2]],
                _ => {
                    let (x, z) = map.xz(central[0], central[2]);
                    let lift = central[1] - tg_height(central[0], central[2]);
                    clear_walls(&mut grid, x, z, 250.0);
                    [x - central[0], grid_height(&grid, x, z) + lift - central[1], z - central[2]]
                }
            };
            Box::new(move |p: [f32; 3]| [p[0] + d[0], p[1] + d[1], p[2] + d[2]])
        };
        for path in &paths {
            match template.file(path).map(|f| &f.doc) {
                Some(Doc::Scb(s)) => {
                    let mut s = s.clone();
                    s.transform(&xform);
                    p.add_or_replace(MapFile::generated(path, s.write()?));
                }
                Some(Doc::Sco(s)) => {
                    let mut s = s.clone();
                    s.transform(&xform);
                    p.add_or_replace(MapFile::generated(path, s.write()));
                }
                _ => {}
            }
        }
    }
    let dsc: Vec<_> = listed
        .into_iter()
        .filter(|e| {
            let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0);
            !stem.eq_ignore_ascii_case("AIPath") && (p.find(&format!("Scene/{}", e.file)).is_some() || p.find(&format!("Scene/{stem}.SCB")).is_some())
        })
        .collect();
    p.set_dsc_entries(&dsc);

    // Level size: the playable rectangle.
    if let Some(ls) = mob.objects.iter_mut().find(|o| o.kind == ObjectType::LevelSize) {
        ls.position = [size[0] / 2.0, ls.position[1], size[1] / 2.0];
        ls.bbox_min = [0.0, ls.bbox_min[1], 0.0];
        ls.bbox_max = [size[0], ls.bbox_max[1], size[1]];
    }
    p.add_or_replace(MapFile::generated("Scene/MapObjects.mob", mob.write()?));

    // Object materials (room.mat) and the textures they use.
    if let Some(mat) = template.file("Scene/room.mat") {
        p.add_or_replace(MapFile::generated(&mat.path, mat.original.to_vec()));
        for tex in room_mat_textures(template.text("Scene/room.mat").unwrap_or("")) {
            copy_texture(template, &mut p, &tex);
        }
    }

    // Decor: particles and lights moved proportionally (or empty).
    for name in ["Particles.dat", "Light.dat"] {
        let text = if opt.keep_decor { template.text(name).map(|t| move_dat_lines(t, name == "Particles.dat", &map, tgrid, &grid)) } else { None };
        let text = text.unwrap_or_default();
        if template.file(name).is_some() || !text.is_empty() {
            p.set_text(&template.file(name).map_or(name.to_string(), |f| f.path.clone()), text);
        }
    }

    // Terrain renderer must match the scene format.
    set_renderer(&mut p, format)?;

    let ver = tgrid.version;
    let had_aimesh = template.find("AIPath.aimesh").is_some();
    let mini_size = template
        .file("INFO/2DLevelMiniMap.dds")
        .and_then(|f| dds::info(&f.original).ok())
        .map_or(512, |i| i.width.max(i.height).clamp(128, 1024));
    p.add_or_replace(MapFile::generated("AIPath.aimesh_ngrid", grid.write()));
    rebuild_scene(&mut p, &opt.style, format, BuildExtras { aimesh: had_aimesh, minimap: Some(mini_size), navgrid_version: ver })?;
    p.name = opt.folder_name.clone();
    Ok(p)
}

fn clear_walls(g: &mut NavGrid, x: f32, z: f32, r: f32) {
    let n = (r / g.cell_size).ceil() as i64;
    let Some((ci, cj)) = g.cell_of(x, z) else { return };
    for dj in -n..=n {
        for di in -n..=n {
            let (i, j) = (ci as i64 + di, cj as i64 + dj);
            if i < 0 || j < 0 || i as usize >= g.nx || j as usize >= g.ny {
                continue;
            }
            let [cx, cz] = g.cell_center(i as usize, j as usize);
            if (cx - x).hypot(cz - z) <= r {
                let k = j as usize * g.nx + i as usize;
                let f = g.flags(k);
                g.set_flags(k, f & !flags::WALL);
            }
        }
    }
}

fn move_dat_lines(text: &str, named: bool, m: &Mapping, tg: &NavGrid, g: &NavGrid) -> String {
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let end = &line[body.len()..];
        let toks: Vec<&str> = body.split(' ').collect();
        let at = if named { 1 } else { 0 };
        let nums: Option<Vec<f32>> = toks.get(at..at + 3).and_then(|t| t.iter().map(|s| s.parse().ok()).collect());
        match nums {
            Some(v) if v.len() == 3 => {
                let (x, z) = m.xz(v[0], v[2]);
                let y = grid_height(g, x, z) + (v[1] - grid_height(tg, v[0], v[2]));
                let mut t: Vec<String> = toks.iter().map(|s| s.to_string()).collect();
                let fmt = |v: f32| if named { format!("{}", (v * 100.0).round() / 100.0) } else { format!("{}", v.round() as i64) };
                t[at] = fmt(x);
                t[at + 1] = fmt(y);
                t[at + 2] = fmt(z);
                out.push_str(&t.join(" "));
                out.push_str(end);
            }
            _ => out.push_str(line),
        }
    }
    out
}

pub fn room_mat_textures(text: &str) -> Vec<String> {
    let mut v: Vec<String> = text
        .lines()
        .filter_map(|l| {
            let (k, val) = l.split_once('=')?;
            if !k.trim().eq_ignore_ascii_case("Texture") {
                return None;
            }
            let t = val.trim();
            let stem = t.rsplit_once('.').map_or(t, |x| x.0);
            Some(format!("{stem}.dds"))
        })
        .collect();
    v.sort_by_key(|s| s.to_ascii_lowercase());
    v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    v
}

fn texture_dir(p: &MapProject) -> String {
    p.files
        .iter()
        .find_map(|f| {
            let l = f.path.to_ascii_lowercase();
            l.starts_with("scene/textures/").then(|| f.path[..14].to_string())
        })
        .unwrap_or_else(|| "Scene/Textures".into())
}

fn copy_texture(from: &MapProject, to: &mut MapProject, name: &str) {
    let dir = texture_dir(from);
    if let Some(f) = from.file(&format!("{dir}/{name}")) {
        to.add_or_replace(MapFile::new(&f.path, f.original.to_vec()));
    }
}

pub fn set_renderer(p: &mut MapProject, format: SceneFormat) -> Result<()> {
    let key = hash_key(TERRAIN_RENDERER_KEY.0, TERRAIN_RENDERER_KEY.1);
    let want = match format {
        SceneFormat::Baked => Some("baked"),
        SceneFormat::Legacy => None,
    };
    if p.find("terrain.inibin").is_none() {
        let mut ini = crate::formats::inibin::Inibin::new();
        if let Some(w) = want {
            ini.set(key, Value::Str(w.into()));
        }
        p.add_or_replace(MapFile::generated("terrain.inibin", ini.write()?));
        return Ok(());
    }
    let f = p.file_mut("terrain.inibin").unwrap();
    let Doc::Inibin(ini) = &mut f.doc else { bail!("terrain.inibin could not be read") };
    let cur = ini.get(key).cloned();
    match (want, cur) {
        (Some(w), Some(Value::Str(s))) if s.eq_ignore_ascii_case(w) => {}
        (Some(w), _) => {
            ini.set(key, Value::Str(w.into()));
            f.dirty = true;
        }
        (None, Some(Value::Str(s))) if !s.eq_ignore_ascii_case("legacy") => {
            ini.set(key, Value::Str("legacy".into()));
            f.dirty = true;
        }
        _ => {}
    }
    Ok(())
}

pub struct BuildExtras {
    pub aimesh: bool,
    pub minimap: Option<usize>,
    pub navgrid_version: u8,
}

/// Rebuilds the scene geometry, sampled heights, pathfinding hints, AI mesh and minimap from the
/// navigation grid. Generated textures are added when the style uses them.
pub fn rebuild_scene(p: &mut MapProject, style: &TerrainStyle, format: SceneFormat, extras: BuildExtras) -> Result<()> {
    let mut grid = p.navgrid().cloned().context("the map has no navigation grid")?;
    let _ = extras.navgrid_version;
    let corners = corner_heights(&grid, style.wall_height);
    // Sampled heights follow the rendered ground.
    let (hx, hy) = (grid.hx, grid.hy);
    for j in 0..hy {
        for i in 0..hx {
            let x = grid.min[0] + i as f32 * grid.hdx;
            let z = grid.min[2] + j as f32 * grid.hdy;
            grid.set_sampled_height(i, j, corner_sample(&grid, &corners, x, z));
        }
    }
    let (lo, hi) = (0..grid.cell_count()).fold((f32::MAX, f32::MIN), |(a, b), k| (a.min(grid.height(k)), b.max(grid.height(k))));
    let mut header_fix = grid.data.clone();
    if grid.version >= 3 {
        crate::formats::bin::put_f32(&mut header_fix, 3 + 4, lo);
        crate::formats::bin::put_f32(&mut header_fix, 3 + 16, hi.max(lo));
    }
    grid = NavGrid::parse(header_fix)?;
    grid.rebuild_hints();

    let parts = terrain_parts(&grid, &corners, style);
    let tex_dir = texture_dir(p);
    for (name, kind) in [(&style.ground_texture, 0), (&style.cliff_texture, 1), (&style.brush_texture, 2), (&style.water_texture, 3)] {
        let path = format!("{tex_dir}/{name}");
        if p.find(&path).is_none() {
            let img = procedural_texture(kind);
            p.add_or_replace(MapFile::generated(&path, dds::encode_argb(&img, true)));
            if format == SceneFormat::Baked {
                for (pre, div) in [("2x_", 2), ("4x_", 4)] {
                    let mut im = img_clone(&img);
                    for _ in 0..div / 2 {
                        im = dds::half(&im);
                    }
                    p.add_or_replace(MapFile::generated(&format!("{tex_dir}/{pre}{name}"), dds::encode_argb(&im, true)));
                }
            }
        }
    }
    match format {
        SceneFormat::Legacy => {
            let mats = vec![
                Material::new("lms_ground", &style.ground_texture, material_type::DEFAULT, 0x11),
                Material::new("lms_cliff", &style.cliff_texture, material_type::DEFAULT, 0x10),
                Material::new("lms_brush", &style.brush_texture, material_type::WALL_OF_GRASS, 0x10),
                Material::new("lms_water", &style.water_texture, material_type::DEFAULT, 0x10),
            ];
            let parts: Vec<(u32, Geometry)> = parts.into_iter().filter(|(_, g)| !g.indices.is_empty()).collect();
            let nvr = Nvr::build(mats, &parts, 900.0)?;
            p.add_or_replace(MapFile::generated("Scene/room.nvr", nvr.write()?));
            p.remove("Scene/room.wgeo");
        }
        SceneFormat::Baked => {
            let tex = [&style.ground_texture, &style.cliff_texture, &style.brush_texture, &style.water_texture];
            let names = ["lms_ground", "lms_cliff", "lms_brush", "lms_water"];
            let mut w = Wgeo { version: 5, meshes: Vec::new(), buckets: Default::default(), total_triangles: 0 };
            let mut ground_tris: Vec<[f32; 3]> = Vec::new();
            for (k, g) in parts {
                if g.indices.is_empty() {
                    continue;
                }
                for piece in tile_geometry(&g, 2000.0) {
                    if k <= 1 {
                        for &i in &piece.indices {
                            ground_tris.push(piece.positions[i as usize]);
                        }
                    }
                    w.meshes.push(WgeoMesh::new(tex[k as usize], names[k as usize], piece.positions, piece.uvs, piece.indices));
                }
            }
            w.buckets = crate::formats::wgeo::build_buckets(&ground_tris, 128);
            w.total_triangles = w.triangle_count();
            p.add_or_replace(MapFile::generated("Scene/room.wgeo", w.write()?));
            p.remove("Scene/room.nvr");
        }
    }
    let walk = grid.walkable();
    let heights: Vec<f32> = (0..grid.cell_count()).map(|k| grid.height(k)).collect();
    if extras.aimesh {
        match coarse_aimesh(&walk, &heights, grid.nx, grid.ny, grid.cell_size, [grid.min[0], grid.min[2]]) {
            Ok(b) => {
                p.add_or_replace(MapFile::generated("AIPath.aimesh", b));
            }
            Err(_) => {
                // The client loads only the grid (LoadAIMesh reads AIPath.aimesh_ngrid).
                p.remove("AIPath.aimesh");
            }
        }
    }
    if let Some(side) = extras.minimap {
        let img = minimap(p, &grid, style, side);
        p.add_or_replace(MapFile::generated("INFO/2DLevelMiniMap.dds", dds::encode_argb(&img, false)));
    }
    p.add_or_replace(MapFile::generated("AIPath.aimesh_ngrid", grid.write()));
    set_renderer(p, format)?;
    Ok(())
}

/// The AI mesh on blocks of cells, as coarse as needed to stay under 16-bit triangle indices
/// (Map1's has about 2500 triangles). A block is walkable when most of its cells are.
fn coarse_aimesh(walk: &[bool], heights: &[f32], nx: usize, ny: usize, cs: f32, origin: [f32; 2]) -> Result<Vec<u8>> {
    let mut k = 1;
    loop {
        let (bx, by) = (nx.div_ceil(k), ny.div_ceil(k));
        let mut bw = vec![false; bx * by];
        let mut bh = vec![0.0f32; bx * by];
        for j in 0..by {
            for i in 0..bx {
                let (mut n, mut w, mut h) = (0, 0, 0.0);
                for jj in j * k..((j + 1) * k).min(ny) {
                    for ii in i * k..((i + 1) * k).min(nx) {
                        n += 1;
                        if walk[jj * nx + ii] {
                            w += 1;
                            h += heights[jj * nx + ii];
                        }
                    }
                }
                bw[j * bx + i] = w * 2 > n;
                bh[j * bx + i] = if w > 0 { h / w as f32 } else { 0.0 };
            }
        }
        if bw.iter().filter(|&&b| b).count() * 2 < 0xffff {
            return AiMesh::from_walkable(&bw, &bh, bx, by, cs * k as f32, origin);
        }
        k += 1;
    }
}

fn img_clone(i: &Image) -> Image {
    Image { width: i.width, height: i.height, pixels: i.pixels.clone() }
}

/// Heights at cell corners: walkable cells stay flat at their height, walls rise inside wall cells.
fn corner_heights(g: &NavGrid, wall: f32) -> Vec<f32> {
    let (nx, ny) = (g.nx, g.ny);
    let mut c = vec![0.0f32; (nx + 1) * (ny + 1)];
    for j in 0..=ny {
        for i in 0..=nx {
            let mut walk = (0.0, 0);
            let mut wallsum = (0.0, 0);
            for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                if i < di || j < dj {
                    continue;
                }
                let (ci, cj) = (i - di, j - dj);
                if ci >= nx || cj >= ny {
                    continue;
                }
                let k = cj * nx + ci;
                if g.flags(k) & flags::WALL == 0 {
                    walk.0 += g.height(k);
                    walk.1 += 1;
                } else {
                    wallsum.0 += g.height(k) + wall;
                    wallsum.1 += 1;
                }
            }
            c[j * (nx + 1) + i] = if walk.1 > 0 { walk.0 / walk.1 as f32 } else { wallsum.0 / wallsum.1.max(1) as f32 };
        }
    }
    c
}

fn corner_sample(g: &NavGrid, c: &[f32], x: f32, z: f32) -> f32 {
    let fx = ((x - g.min[0]) / g.cell_size).clamp(0.0, g.nx as f32);
    let fz = ((z - g.min[2]) / g.cell_size).clamp(0.0, g.ny as f32);
    let (i0, j0) = ((fx.floor() as usize).min(g.nx - 1), (fz.floor() as usize).min(g.ny - 1));
    let (tx, tz) = (fx - i0 as f32, fz - j0 as f32);
    let w = g.nx + 1;
    let a = c[j0 * w + i0] * (1.0 - tx) + c[j0 * w + i0 + 1] * tx;
    let b = c[(j0 + 1) * w + i0] * (1.0 - tx) + c[(j0 + 1) * w + i0 + 1] * tx;
    a * (1.0 - tz) + b * tz
}

/// Terrain geometry by surface: 0 ground, 1 cliff, 2 brush, 3 water.
fn terrain_parts(g: &NavGrid, c: &[f32], style: &TerrainStyle) -> Vec<(u32, Geometry)> {
    let (nx, ny, cs) = (g.nx, g.ny, g.cell_size);
    let w = nx + 1;
    let mut parts: Vec<Geometry> = (0..4).map(|_| Geometry::default()).collect();
    let corner = |i: usize, j: usize| [g.min[0] + i as f32 * cs, c[j * w + i], g.min[2] + j as f32 * cs];
    let ts = style.texture_scale;
    for j in 0..ny {
        for i in 0..nx {
            let k = j * nx + i;
            let wall = g.flags(k) & flags::WALL != 0;
            let q = [corner(i, j), corner(i + 1, j), corner(i + 1, j + 1), corner(i, j + 1)];
            let part = &mut parts[if wall { 1 } else { 0 }];
            let base = part.positions.len() as u32;
            let n = quad_normal(&q);
            for p in q {
                part.positions.push(p);
                part.normals.push(n);
                part.uvs.push(if wall { [(p[0] + p[2]) / ts, -p[1] / (ts * 0.5)] } else { [p[0] / ts, p[2] / ts] });
                part.colors.push(0xff000000);
            }
            part.indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
            if !wall && g.flags(k) & flags::BRUSH != 0 {
                let [x, z] = g.cell_center(i, j);
                let y = g.height(k);
                let r = cs * 0.7;
                let h = style.brush_height * (0.85 + 0.3 * hash01(i as u32, j as u32));
                let b = &mut parts[2];
                for (dx, dz) in [(1.0f32, 0.0f32), (0.0, 1.0)] {
                    let base = b.positions.len() as u32;
                    let quad = [[x - dx * r, y, z - dz * r], [x + dx * r, y, z + dz * r], [x + dx * r, y + h, z + dz * r], [x - dx * r, y + h, z - dz * r]];
                    for (vi, p) in quad.iter().enumerate() {
                        b.positions.push(*p);
                        b.normals.push([0.0, 1.0, 0.0]);
                        b.uvs.push([if vi == 0 || vi == 3 { 0.0 } else { 1.0 }, if vi < 2 { 1.0 } else { 0.0 }]);
                        b.colors.push(0xff000000);
                    }
                    b.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3, base, base + 2, base + 1, base, base + 3, base + 2]);
                }
            }
        }
    }
    if let Some(wl) = style.water_level {
        let wpart = &mut parts[3];
        let step = 2000.0;
        let (sx, sz) = (g.max[0] - g.min[0], g.max[2] - g.min[2]);
        let (mx, mz) = ((sx / step).ceil() as usize, (sz / step).ceil() as usize);
        for b in 0..mz {
            for a in 0..mx {
                let (x0, z0) = (g.min[0] + a as f32 * step, g.min[2] + b as f32 * step);
                let (x1, z1) = ((x0 + step).min(g.max[0]), (z0 + step).min(g.max[2]));
                let base = wpart.positions.len() as u32;
                for p in [[x0, wl, z0], [x1, wl, z0], [x1, wl, z1], [x0, wl, z1]] {
                    wpart.positions.push(p);
                    wpart.normals.push([0.0, 1.0, 0.0]);
                    wpart.uvs.push([p[0] / ts, p[2] / ts]);
                    wpart.colors.push(0xff000000);
                }
                wpart.indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
            }
        }
    }
    parts.into_iter().enumerate().map(|(i, g)| (i as u32, g)).collect()
}

fn quad_normal(q: &[[f32; 3]; 4]) -> [f32; 3] {
    let a = [q[2][0] - q[0][0], q[2][1] - q[0][1], q[2][2] - q[0][2]];
    let b = [q[3][0] - q[1][0], q[3][1] - q[1][1], q[3][2] - q[1][2]];
    let mut n = [a[2] * b[1] - a[1] * b[2], a[0] * b[2] - a[2] * b[0], a[1] * b[0] - a[0] * b[1]];
    if n[1] < 0.0 {
        n = n.map(|v| -v);
    }
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
    n.map(|v| v / l)
}

/// Splits geometry into square tiles (WGEO meshes are culled per mesh).
fn tile_geometry(g: &Geometry, tile: f32) -> Vec<Geometry> {
    let mut buckets: std::collections::BTreeMap<(i64, i64), Vec<u32>> = Default::default();
    for t in g.indices.chunks_exact(3) {
        let cx = (g.positions[t[0] as usize][0] + g.positions[t[1] as usize][0] + g.positions[t[2] as usize][0]) / 3.0;
        let cz = (g.positions[t[0] as usize][2] + g.positions[t[1] as usize][2] + g.positions[t[2] as usize][2]) / 3.0;
        buckets.entry(((cx / tile).floor() as i64, (cz / tile).floor() as i64)).or_default().extend_from_slice(t);
    }
    buckets
        .into_values()
        .flat_map(|idx| crate::formats::nvr::split_geometry(&crate::formats::nvr::subset(g, &idx), 65000))
        .collect()
}

fn hash01(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9e3779b1) ^ b.wrapping_mul(0x85ebca77);
    h ^= h >> 15;
    h = h.wrapping_mul(0xc2b2ae3d);
    h ^= h >> 13;
    (h & 0xffff) as f32 / 65535.0
}

fn smooth_noise(x: f32, y: f32, seed: u32) -> f32 {
    let mut total = 0.0;
    let mut amp = 0.5;
    let mut f = 1.0;
    for o in 0..4 {
        let (px, py) = (x * f, y * f);
        let (ix, iy) = (px.floor(), py.floor());
        let (tx, ty) = (px - ix, py - iy);
        let s = |a: f32, b: f32| hash01((a as i32 as u32).wrapping_add(seed * 31 + o), (b as i32 as u32).wrapping_add(seed * 17));
        let sm = |t: f32| t * t * (3.0 - 2.0 * t);
        let (ux, uy) = (sm(tx), sm(ty));
        let v = (s(ix, iy) * (1.0 - ux) + s(ix + 1.0, iy) * ux) * (1.0 - uy) + (s(ix, iy + 1.0) * (1.0 - ux) + s(ix + 1.0, iy + 1.0) * ux) * uy;
        total += v * amp;
        amp *= 0.5;
        f *= 2.0;
    }
    total / 0.9375
}

/// Tileable procedural textures: 0 ground, 1 cliff, 2 brush (with alpha), 3 water.
pub fn procedural_texture(kind: u32) -> Image {
    let n = 256usize;
    let mut px = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            // Sample on a torus so the texture tiles.
            let (u, v) = (x as f32 / n as f32 * std::f32::consts::TAU, y as f32 / n as f32 * std::f32::consts::TAU);
            let (a, b) = (u.cos() * 1.6 + 8.0, u.sin() * 1.6 + 8.0);
            let (c, d) = (v.cos() * 1.6 + 8.0, v.sin() * 1.6 + 8.0);
            let nz = |s: u32| (smooth_noise(a + c, b + d, s) + smooth_noise(a * 2.0 + d, b * 2.0 + c, s + 5)) * 0.5;
            let (r, g, bl, al) = match kind {
                0 => {
                    let t = nz(1);
                    let dirt = nz(9);
                    let m = ((dirt - 0.55) * 6.0).clamp(0.0, 1.0);
                    let grass = [62.0 + 50.0 * t, 98.0 + 60.0 * t, 40.0 + 25.0 * t];
                    let soil = [112.0 + 40.0 * t, 88.0 + 30.0 * t, 58.0 + 20.0 * t];
                    (grass[0] * (1.0 - m) + soil[0] * m, grass[1] * (1.0 - m) + soil[1] * m, grass[2] * (1.0 - m) + soil[2] * m, 255.0)
                }
                1 => {
                    let t = nz(3);
                    let streak = ((v * 6.0 + t * 4.0).sin() * 0.5 + 0.5) * 0.25;
                    let s = 80.0 + 70.0 * t - 40.0 * streak;
                    (s * 1.05, s * 0.98, s * 0.88, 255.0)
                }
                2 => {
                    let blade = ((x as f32 * 0.9 + nz(4) * 30.0).sin() * 0.5 + 0.5).powf(3.0);
                    let top = 1.0 - y as f32 / n as f32;
                    let alpha = if blade > top * 0.9 + 0.05 { 255.0 } else { 0.0 };
                    let t = nz(6);
                    (40.0 + 40.0 * t, 110.0 + 70.0 * t * (1.0 - top * 0.3), 35.0 + 20.0 * t, alpha)
                }
                _ => {
                    let t = nz(8);
                    (30.0 + 30.0 * t, 80.0 + 50.0 * t, 120.0 + 60.0 * t, 255.0)
                }
            };
            let o = (y * n + x) * 4;
            px[o] = r.clamp(0.0, 255.0) as u8;
            px[o + 1] = g.clamp(0.0, 255.0) as u8;
            px[o + 2] = bl.clamp(0.0, 255.0) as u8;
            px[o + 3] = al as u8;
        }
    }
    Image { width: n, height: n, pixels: px }
}

/// Top-down minimap: the ground shaded by surface and height, with structures marked.
pub fn minimap(p: &MapProject, g: &NavGrid, style: &TerrainStyle, side: usize) -> Image {
    let color_of = |name: &str, fallback: [u8; 3]| {
        let dir = texture_dir(p);
        p.file(&format!("{dir}/{name}")).and_then(|f| dds::average_color(&f.bytes().ok()?)).unwrap_or(fallback)
    };
    let ground = color_of(&style.ground_texture, [80, 120, 50]);
    let cliff = color_of(&style.cliff_texture, [100, 95, 85]);
    let brush = [40, 100, 40];
    let water = [40, 90, 140];
    let (lo, hi) = (0..g.cell_count()).fold((f32::MAX, f32::MIN), |(a, b), k| (a.min(g.height(k)), b.max(g.height(k))));
    let mut px = vec![0u8; side * side * 4];
    for y in 0..side {
        for x in 0..side {
            // Minimap rows run from the top (high Z) down.
            let wx = g.min[0] + (x as f32 + 0.5) / side as f32 * (g.max[0] - g.min[0]);
            let wz = g.max[2] - (y as f32 + 0.5) / side as f32 * (g.max[2] - g.min[2]);
            let (c, h) = match g.cell_of(wx, wz) {
                Some((i, j)) => {
                    let k = j * g.nx + i;
                    let f = g.flags(k);
                    let h = g.height(k);
                    let c = if f & flags::WALL != 0 {
                        cliff
                    } else if style.water_level.is_some_and(|wl| h < wl) {
                        water
                    } else if f & flags::BRUSH != 0 {
                        brush
                    } else {
                        ground
                    };
                    (c, h)
                }
                None => ([0, 0, 0], lo),
            };
            let shade = if hi > lo { 0.75 + 0.35 * (h - lo) / (hi - lo) } else { 1.0 };
            let o = (y * side + x) * 4;
            for k in 0..3 {
                px[o + k] = (c[k] as f32 * shade).clamp(0.0, 255.0) as u8;
            }
            px[o + 3] = 255;
        }
    }
    let mut dot = |wx: f32, wz: f32, col: [u8; 3], r: i64| {
        let x = ((wx - g.min[0]) / (g.max[0] - g.min[0]) * side as f32) as i64;
        let y = ((g.max[2] - wz) / (g.max[2] - g.min[2]) * side as f32) as i64;
        for dy in -r..=r {
            for dx in -r..=r {
                let (xx, yy) = (x + dx, y + dy);
                if dx * dx + dy * dy <= r * r && xx >= 0 && yy >= 0 && (xx as usize) < side && (yy as usize) < side {
                    let o = (yy as usize * side + xx as usize) * 4;
                    px[o..o + 3].copy_from_slice(&col);
                }
            }
        }
    };
    let r = (side / 128).max(2) as i64;
    if let Some(m) = p.mob() {
        for o in &m.objects {
            let order = o.name.contains("T1") || o.name.contains("Order");
            let col = if order { [60, 140, 255] } else { [255, 70, 60] };
            if matches!(o.kind, ObjectType::Turret | ObjectType::Nexus | ObjectType::Inhibitor) {
                dot(o.position[0], o.position[2], col, r);
            }
        }
    }
    for e in p.dsc_entries() {
        let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0);
        if !(stem.starts_with("Turret") || stem.starts_with("HQ") || stem.starts_with("Barracks")) {
            continue;
        }
        let c = match p.file(&format!("Scene/{stem}.SCB")).map(|f| &f.doc) {
            Some(Doc::Scb(s)) => s.central,
            _ => match p.file(&format!("Scene/{}", e.file)).map(|f| &f.doc) {
                Some(Doc::Sco(s)) => s.central,
                _ => continue,
            },
        };
        let col = if stem.contains("T1") { [60, 140, 255] } else { [255, 70, 60] };
        dot(c[0], c[2], col, r);
    }
    Image { width: side, height: side, pixels: px }
}

/// Moves a room.dsc object (both its .SCB and .sco) by `d`.
pub fn move_dsc_object(p: &mut MapProject, stem: &str, d: [f32; 3]) -> Result<()> {
    let mut any = false;
    for ext in ["SCB", "sco"] {
        let path = format!("Scene/{stem}.{ext}");
        if let Some(f) = p.file_mut(&path) {
            match &mut f.doc {
                Doc::Scb(s) => s.translate(d),
                Doc::Sco(s) => s.translate(d),
                _ => continue,
            }
            f.dirty = true;
            any = true;
        }
    }
    if !any {
        bail!("no Scene/{stem}.SCB or .sco");
    }
    Ok(())
}

/// Copies a room.dsc object under a new name (both mesh files) and lists it in room.dsc.
pub fn duplicate_dsc_object(p: &mut MapProject, stem: &str, new_stem: &str, d: [f32; 3]) -> Result<()> {
    if p.find(&format!("Scene/{new_stem}.sco")).is_some() || p.find(&format!("Scene/{new_stem}.SCB")).is_some() {
        bail!("{new_stem} already exists");
    }
    let mut made = false;
    for ext in ["SCB", "sco"] {
        let Some(f) = p.file(&format!("Scene/{stem}.{ext}")) else { continue };
        let out = match &f.doc {
            Doc::Scb(s) => {
                let mut s: Scb = s.clone();
                s.translate(d);
                s.rename(new_stem);
                s.write()?
            }
            Doc::Sco(s) => {
                let mut s: Sco = s.clone();
                s.translate(d);
                s.rename(new_stem);
                s.write()
            }
            _ => continue,
        };
        p.add_or_replace(MapFile::generated(&format!("Scene/{new_stem}.{ext}"), out));
        made = true;
    }
    if !made {
        bail!("no Scene/{stem}.SCB or .sco to copy");
    }
    let mut e = p.dsc_entries();
    let q = e.iter().find(|x| x.file.to_ascii_lowercase().starts_with(&format!("{}.", stem.to_ascii_lowercase()))).map_or(-100, |x| x.quality);
    e.push(crate::project::DscEntry { file: format!("{new_stem}.sco"), quality: q });
    p.set_dsc_entries(&e);
    Ok(())
}
