//! Editing operations on an open map, with undo. The UI calls these; each records the files it
//! touches before changing them.

use crate::create::{self, TerrainStyle};
use crate::formats::{
    mob::{MapObject, ObjectType},
    navgrid::flags,
};
use crate::project::{Doc, FileKind, MapFile, MapProject, SceneFormat};
use anyhow::{Result, bail};
use std::sync::Arc;

#[derive(Clone)]
struct Snapshot {
    doc: Doc,
    dirty: bool,
    original: Arc<Vec<u8>>,
    kind: FileKind,
    parse_error: Option<String>,
    path: String,
}

struct UndoStep {
    label: String,
    files: Vec<(String, Option<Snapshot>)>,
    nav_walk_changed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Sel {
    None,
    Mob(String),
    Dsc(String),
    File(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NavPaint {
    Wall,
    Walkable,
    Brush,
    RemoveBrush,
    Raise,
    Lower,
    Flatten,
    Smooth,
}

impl NavPaint {
    pub const ALL: [NavPaint; 8] = [NavPaint::Wall, NavPaint::Walkable, NavPaint::Brush, NavPaint::RemoveBrush, NavPaint::Raise, NavPaint::Lower, NavPaint::Flatten, NavPaint::Smooth];
    pub fn label(self) -> &'static str {
        match self {
            NavPaint::Wall => "Wall",
            NavPaint::Walkable => "Walkable",
            NavPaint::Brush => "Brush (grass)",
            NavPaint::RemoveBrush => "Remove brush",
            NavPaint::Raise => "Raise ground",
            NavPaint::Lower => "Lower ground",
            NavPaint::Flatten => "Flatten",
            NavPaint::Smooth => "Smooth",
        }
    }
    pub fn changes_walkability(self) -> bool {
        matches!(self, NavPaint::Wall | NavPaint::Walkable)
    }
}

pub struct Editor {
    pub project: MapProject,
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
    /// Walkable cells changed since the hints were computed: rebuild them before export.
    pub nav_walk_changed: bool,
    /// Generated maps rebuild their scene from the navigation grid.
    pub generated: Option<(TerrainStyle, SceneFormat)>,
    pub terrain_stale: bool,
    pub scene_rev: u64,
    pub nav_rev: u64,
    pub objects_rev: u64,
    pub sel: Sel,
    /// Undo step currently open (for drags and paint strokes).
    open_step: Option<String>,
    pub flatten_height: Option<f32>,
}

const UNDO_LIMIT: usize = 40;

impl Editor {
    pub fn new(project: MapProject) -> Self {
        Self {
            project,
            undo: Vec::new(),
            redo: Vec::new(),
            nav_walk_changed: false,
            generated: None,
            terrain_stale: false,
            scene_rev: 1,
            nav_rev: 1,
            objects_rev: 1,
            sel: Sel::None,
            open_step: None,
            flatten_height: None,
        }
    }

    fn snap(&self, path: &str) -> Option<Snapshot> {
        self.project.file(path).map(|f| Snapshot { doc: f.doc.clone(), dirty: f.dirty, original: f.original.clone(), kind: f.kind, parse_error: f.parse_error.clone(), path: f.path.clone() })
    }

    /// Records the current state of `paths` as one undo step.
    pub fn record(&mut self, label: &str, paths: &[String]) {
        let files = paths.iter().map(|p| (p.clone(), self.snap(p))).collect();
        self.undo.push(UndoStep { label: label.into(), files, nav_walk_changed: self.nav_walk_changed });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Like `record`, but merges into the step already open with the same label (one stroke).
    pub fn record_once(&mut self, label: &str, paths: &[String]) {
        if self.open_step.as_deref() == Some(label) {
            return;
        }
        self.record(label, paths);
        self.open_step = Some(label.into());
    }

    pub fn end_stroke(&mut self) {
        self.open_step = None;
    }

    fn swap(&mut self, step: UndoStep) -> UndoStep {
        let mut back = Vec::new();
        for (path, snap) in step.files {
            back.push((path.clone(), self.snap(&path)));
            match snap {
                Some(s) => {
                    let f = MapFile { path: s.path.clone(), kind: s.kind, original: s.original, doc: s.doc, dirty: s.dirty, parse_error: s.parse_error };
                    self.project.add_or_replace(f);
                    if let Some(f) = self.project.file_mut(&s.path) {
                        f.dirty = s.dirty;
                    }
                }
                None => {
                    self.project.remove(&path);
                }
            }
        }
        let prev = self.nav_walk_changed;
        self.nav_walk_changed = step.nav_walk_changed || prev;
        self.touch_all();
        UndoStep { label: step.label, files: back, nav_walk_changed: prev }
    }

    pub fn undo(&mut self) -> Option<String> {
        self.end_stroke();
        let s = self.undo.pop()?;
        let label = s.label.clone();
        let r = self.swap(s);
        self.redo.push(r);
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        self.end_stroke();
        let s = self.redo.pop()?;
        let label = s.label.clone();
        let r = self.swap(s);
        self.undo.push(r);
        Some(label)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    pub fn touch_all(&mut self) {
        self.scene_rev += 1;
        self.nav_rev += 1;
        self.objects_rev += 1;
    }

    pub fn mob_path(&self) -> String {
        self.project.file("Scene/MapObjects.mob").map_or("Scene/MapObjects.mob".into(), |f| f.path.clone())
    }

    fn dsc_paths(&self, stem: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .project
            .files
            .iter()
            .filter(|f| f.path.to_ascii_lowercase().starts_with("scene/") && f.stem().eq_ignore_ascii_case(stem) && matches!(f.kind, FileKind::Scb | FileKind::Sco))
            .map(|f| f.path.clone())
            .collect();
        v.push("Scene/room.dsc".into());
        v
    }

    pub fn dsc_central(&self, stem: &str) -> Option<[f32; 3]> {
        self.project.files.iter().filter(|f| f.path.to_ascii_lowercase().starts_with("scene/") && f.stem().eq_ignore_ascii_case(stem)).find_map(|f| match &f.doc {
            Doc::Scb(s) => Some(s.central),
            Doc::Sco(s) => Some(s.central),
            _ => None,
        })
    }

    pub fn ground_height(&self, x: f32, z: f32) -> Option<f32> {
        self.project.navgrid().map(|g| create::grid_height(g, x, z))
    }

    // ----- map objects -----

    pub fn set_mob_object(&mut self, name: &str, obj: MapObject, label: &str) {
        let path = self.mob_path();
        self.record_once(label, &[path]);
        if let Some(m) = self.project.mob_mut()
            && let Some(i) = m.find(name)
        {
            // The bounding box follows the object when it moves.
            let old = m.objects[i].position;
            let mut obj = obj;
            if obj.position != old && obj.bbox_min == m.objects[i].bbox_min && (obj.bbox_min != [0.0; 3] || obj.bbox_max != [0.0; 3]) {
                for a in 0..3 {
                    let d = obj.position[a] - old[a];
                    obj.bbox_min[a] += d;
                    obj.bbox_max[a] += d;
                }
            }
            m.objects[i] = obj;
        }
        self.objects_rev += 1;
    }

    pub fn move_object(&mut self, sel: &Sel, d: [f32; 3]) -> Result<()> {
        match sel {
            Sel::Mob(name) => {
                let Some(o) = self.project.mob().and_then(|m| m.find(name).map(|i| m.objects[i].clone())) else { bail!("no object {name}") };
                let mut n = o.clone();
                for a in 0..3 {
                    n.position[a] += d[a];
                }
                self.set_mob_object(name, n, "Move object");
            }
            Sel::Dsc(stem) => {
                let paths = self.dsc_paths(stem);
                self.record_once("Move object", &paths);
                create::move_dsc_object(&mut self.project, stem, d)?;
                self.objects_rev += 1;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn delete_selected(&mut self) -> Result<String> {
        let sel = self.sel.clone();
        match &sel {
            Sel::Mob(name) => {
                let path = self.mob_path();
                self.record("Delete object", &[path]);
                if let Some(m) = self.project.mob_mut()
                    && let Some(i) = m.find(name)
                {
                    m.objects.remove(i);
                }
            }
            Sel::Dsc(stem) => {
                let paths = self.dsc_paths(stem);
                self.record("Delete object", &paths);
                let mut e = self.project.dsc_entries();
                e.retain(|x| !x.file.rsplit_once('.').map_or(x.file.as_str(), |y| y.0).eq_ignore_ascii_case(stem));
                self.project.set_dsc_entries(&e);
                for p in paths.iter().filter(|p| !p.ends_with("room.dsc")) {
                    self.project.remove(p);
                }
            }
            Sel::File(path) => {
                self.record("Delete file", std::slice::from_ref(path));
                self.project.remove(path);
                self.touch_all();
            }
            Sel::None => bail!("nothing is selected"),
        }
        self.sel = Sel::None;
        self.objects_rev += 1;
        Ok(match sel {
            Sel::Mob(n) | Sel::Dsc(n) | Sel::File(n) => n,
            Sel::None => String::new(),
        })
    }

    /// Copies the selected object under a new name at `at` (x, z); its ObjectCFG section is copied
    /// too, so the new structure has the same stats.
    pub fn duplicate_selected(&mut self, at: [f32; 2]) -> Result<Sel> {
        let sel = self.sel.clone();
        match &sel {
            Sel::Mob(name) => {
                let Some(o) = self.project.mob().and_then(|m| m.find(name).map(|i| m.objects[i].clone())) else { bail!("no object {name}") };
                let mobs = self.project.mob().unwrap();
                let mut new_name = next_name(name);
                while mobs.find(&new_name).is_some() {
                    new_name = next_name(&new_name);
                }
                let y = self.ground_height(at[0], at[1]).unwrap_or(o.position[1]);
                let d = [at[0] - o.position[0], y - o.position[1], at[1] - o.position[2]];
                let mut n = MapObject::new(&new_name, o.kind, [at[0], y, at[1]]);
                n.pitch_yaw_roll = o.pitch_yaw_roll;
                n.scale = o.scale;
                n.skin_id = o.skin_id;
                n.ignore_collision_on_placement = o.ignore_collision_on_placement;
                if o.bbox_min != [0.0; 3] || o.bbox_max != [0.0; 3] {
                    for a in 0..3 {
                        n.bbox_min[a] = o.bbox_min[a] + d[a];
                        n.bbox_max[a] = o.bbox_max[a] + d[a];
                    }
                }
                let mut paths = vec![self.mob_path()];
                paths.extend(self.cfg_path());
                self.record("Duplicate object", &paths);
                self.insert_mob(n);
                self.copy_cfg_section(name, &new_name);
                self.objects_rev += 1;
                Ok(Sel::Mob(new_name))
            }
            Sel::Dsc(stem) => {
                let Some(c) = self.dsc_central(stem) else { bail!("{stem} has no mesh") };
                let mut new_stem = next_name(stem);
                while self.project.find(&format!("Scene/{new_stem}.sco")).is_some() || self.project.find(&format!("Scene/{new_stem}.SCB")).is_some() {
                    new_stem = next_name(&new_stem);
                }
                let y = self.ground_height(at[0], at[1]).map_or(c[1], |g| g + (c[1] - self.ground_height(c[0], c[2]).unwrap_or(c[1])));
                let d = [at[0] - c[0], y - c[1], at[1] - c[2]];
                let mut paths = vec!["Scene/room.dsc".to_string(), format!("Scene/{new_stem}.SCB"), format!("Scene/{new_stem}.sco")];
                paths.extend(self.cfg_path());
                self.record("Duplicate object", &paths);
                create::duplicate_dsc_object(&mut self.project, stem, &new_stem, d)?;
                self.copy_cfg_section(stem, &new_stem);
                self.objects_rev += 1;
                Ok(Sel::Dsc(new_stem))
            }
            _ => bail!("select a map object or scene object to duplicate"),
        }
    }

    fn insert_mob(&mut self, o: MapObject) {
        if self.project.mob().is_none() {
            let empty = crate::formats::mob::MapObjects { reserved: 0, objects: Vec::new() };
            self.project.add_or_replace(MapFile::generated("Scene/MapObjects.mob", empty.write().unwrap()));
        }
        let m = self.project.mob_mut().unwrap();
        let sorted = m.objects.windows(2).all(|w| w[0].name.as_bytes() <= w[1].name.as_bytes());
        if sorted {
            m.insert_sorted(o);
        } else {
            m.objects.push(o);
        }
    }

    /// A new map object of `kind` at (x, z).
    pub fn place_object(&mut self, kind: ObjectType, base: &str, at: [f32; 2]) -> Result<Sel> {
        let y = self.ground_height(at[0], at[1]).unwrap_or(0.0);
        let name = match self.project.mob() {
            Some(m) => m.unique_name(base),
            None => base.to_string(),
        };
        if name.len() > crate::formats::mob::MAX_NAME {
            bail!("the name {name} is too long");
        }
        let mut o = MapObject::new(&name, kind, [at[0], y, at[1]]);
        if kind == ObjectType::NavPoint {
            o.scale = [0.5; 3];
        }
        if matches!(kind, ObjectType::Turret | ObjectType::Inhibitor | ObjectType::Nexus | ObjectType::LevelProp) {
            let r = match kind {
                ObjectType::Nexus => 340.0,
                ObjectType::Inhibitor => 210.0,
                ObjectType::Turret => 150.0,
                _ => 120.0,
            };
            o.bbox_min = [at[0] - r, y - 300.0, at[1] - r];
            o.bbox_max = [at[0] + r, y + 700.0, at[1] + r];
        }
        let path = self.mob_path();
        self.record("Place object", &[path]);
        self.insert_mob(o);
        self.objects_rev += 1;
        Ok(Sel::Mob(name))
    }

    pub fn rename_mob(&mut self, old: &str, new: &str) -> Result<()> {
        if new.is_empty() || new.len() > crate::formats::mob::MAX_NAME {
            bail!("names must be 1 to 61 characters");
        }
        if self.project.mob().is_some_and(|m| m.find(new).is_some()) {
            bail!("{new} already exists");
        }
        let mut paths = vec![self.mob_path()];
        paths.extend(self.cfg_path());
        self.record("Rename object", &paths);
        if let Some(m) = self.project.mob_mut()
            && let Some(i) = m.find(old)
        {
            m.objects[i].name = new.to_string();
        }
        self.rename_cfg_section(old, new);
        self.sel = Sel::Mob(new.into());
        self.objects_rev += 1;
        Ok(())
    }

    // ----- ObjectCFG sections -----

    pub fn cfg_path(&self) -> Option<String> {
        self.project.file("Scene/CFG/ObjectCFG.cfg").map(|f| f.path.clone())
    }

    pub fn cfg_section(&self, name: &str) -> Option<String> {
        let t = self.project.text(&self.cfg_path()?)?;
        section_range(t, name).map(|(a, b)| t[a..b].to_string())
    }

    fn copy_cfg_section(&mut self, from: &str, to: &str) {
        let Some(path) = self.cfg_path() else { return };
        let Some(t) = self.project.text(&path).map(|s| s.to_string()) else { return };
        if section_range(&t, to).is_some() {
            return;
        }
        if let Some((a, b)) = section_range(&t, from) {
            let body = &t[a..b];
            let nl = if t.contains("\r\n") { "\r\n" } else { "\n" };
            let renamed = body.replacen(&format!("[{from}]"), &format!("[{to}]"), 1);
            let mut out = t.clone();
            if !out.ends_with('\n') {
                out.push_str(nl);
            }
            out.push_str(&renamed);
            if !renamed.ends_with('\n') {
                out.push_str(nl);
            }
            self.project.set_text(&path, out);
        }
    }

    fn rename_cfg_section(&mut self, from: &str, to: &str) {
        let Some(path) = self.cfg_path() else { return };
        let Some(t) = self.project.text(&path).map(|s| s.to_string()) else { return };
        if let Some((a, _)) = section_range(&t, from) {
            let out = format!("{}{}", &t[..a], t[a..].replacen(&format!("[{from}]"), &format!("[{to}]"), 1));
            self.project.set_text(&path, out);
        }
    }

    pub fn set_cfg_section(&mut self, name: &str, body: &str) {
        let Some(path) = self.cfg_path() else { return };
        let Some(t) = self.project.text(&path).map(|s| s.to_string()) else { return };
        self.record("Edit object settings", std::slice::from_ref(&path));
        let out = match section_range(&t, name) {
            Some((a, b)) => format!("{}{}{}", &t[..a], body, &t[b..]),
            None => format!("{t}{body}"),
        };
        self.project.set_text(&path, out);
    }

    // ----- navigation grid -----

    /// Paints cells within `radius` of (x, z). Returns whether anything changed.
    pub fn paint_nav(&mut self, mode: NavPaint, x: f32, z: f32, radius: f32, strength: f32) -> bool {
        let label = format!("Paint: {}", mode.label());
        let path = self.project.file("AIPath.aimesh_ngrid").map(|f| f.path.clone());
        let Some(path) = path else { return false };
        self.record_once(&label, &[path]);
        if mode == NavPaint::Flatten && self.flatten_height.is_none() {
            self.flatten_height = self.ground_height(x, z);
        }
        let flat = self.flatten_height;
        let Some(g) = self.project.navgrid_mut() else { return false };
        let n = (radius / g.cell_size).ceil() as i64 + 1;
        let Some((ci, cj)) = g.cell_of(x, z).or_else(|| {
            let i = ((x - g.min[0]) / g.cell_size).clamp(0.0, g.nx as f32 - 1.0) as usize;
            let j = ((z - g.min[2]) / g.cell_size).clamp(0.0, g.ny as f32 - 1.0) as usize;
            Some((i, j))
        }) else {
            return false;
        };
        let mut changed = false;
        let mut cells = Vec::new();
        for dj in -n..=n {
            for di in -n..=n {
                let (i, j) = (ci as i64 + di, cj as i64 + dj);
                if i < 0 || j < 0 || i as usize >= g.nx || j as usize >= g.ny {
                    continue;
                }
                let [cx, cz] = g.cell_center(i as usize, j as usize);
                let dist = (cx - x).hypot(cz - z);
                if dist <= radius {
                    cells.push((j as usize * g.nx + i as usize, 1.0 - dist / radius.max(1.0)));
                }
            }
        }
        let snapshot: Vec<f32> = if mode == NavPaint::Smooth { (0..g.cell_count()).map(|k| g.height(k)).collect() } else { Vec::new() };
        for (k, falloff) in cells {
            let f = g.flags(k);
            let h = g.height(k);
            match mode {
                NavPaint::Wall | NavPaint::Walkable | NavPaint::Brush | NavPaint::RemoveBrush => {
                    let nf = match mode {
                        NavPaint::Wall => (f | flags::WALL) & !flags::BRUSH,
                        NavPaint::Walkable => f & !flags::WALL,
                        NavPaint::Brush => (f | flags::BRUSH) & !flags::WALL,
                        _ => f & !flags::BRUSH,
                    };
                    if nf != f {
                        g.set_flags(k, nf);
                        changed = true;
                    }
                }
                NavPaint::Raise | NavPaint::Lower => {
                    let d = strength * (0.35 + 0.65 * falloff) * if mode == NavPaint::Raise { 1.0 } else { -1.0 };
                    g.set_height(k, h + d);
                    changed = true;
                }
                NavPaint::Flatten => {
                    if let Some(t) = flat {
                        let nh = h + (t - h) * (0.5 + 0.5 * falloff).min(1.0);
                        if (nh - h).abs() > 0.01 {
                            g.set_height(k, nh);
                            changed = true;
                        }
                    }
                }
                NavPaint::Smooth => {
                    let (i, j) = ((k % g.nx) as i64, (k / g.nx) as i64);
                    let (mut s, mut c) = (0.0, 0.0);
                    for (di, dj) in [(-1, 0), (1, 0), (0, -1), (0, 1), (0, 0)] {
                        let (ii, jj) = (i + di, j + dj);
                        if ii >= 0 && jj >= 0 && (ii as usize) < g.nx && (jj as usize) < g.ny {
                            s += snapshot[jj as usize * g.nx + ii as usize];
                            c += 1.0;
                        }
                    }
                    g.set_height(k, h + (s / c - h) * 0.5 * falloff.max(0.2));
                    changed = true;
                }
            }
        }
        if changed {
            if mode.changes_walkability() {
                self.nav_walk_changed = true;
            }
            self.terrain_stale = true;
            self.nav_rev += 1;
        }
        changed
    }

    // ----- scene -----

    pub fn rebuild_terrain(&mut self, style: &TerrainStyle, format: SceneFormat) -> Result<()> {
        let mut paths: Vec<String> = ["Scene/room.nvr", "Scene/room.wgeo", "AIPath.aimesh_ngrid", "AIPath.aimesh", "INFO/2DLevelMiniMap.dds", "terrain.inibin"].iter().map(|s| s.to_string()).collect();
        let tex = self.project.files.iter().find(|f| f.path.to_ascii_lowercase().starts_with("scene/textures/")).map_or("Scene/Textures".to_string(), |f| f.path[..14].to_string());
        for t in [&style.ground_texture, &style.cliff_texture, &style.brush_texture, &style.water_texture] {
            paths.push(format!("{tex}/{t}"));
            paths.push(format!("{tex}/2x_{t}"));
            paths.push(format!("{tex}/4x_{t}"));
        }
        self.record("Rebuild terrain", &paths);
        let had_aimesh = self.project.find("AIPath.aimesh").is_some();
        let mini = self.project.file("INFO/2DLevelMiniMap.dds").and_then(|f| crate::formats::dds::info(&f.original).ok()).map_or(512, |i| i.width.clamp(128, 1024));
        let ver = self.project.navgrid().map_or(3, |g| g.version);
        create::rebuild_scene(&mut self.project, style, format, create::BuildExtras { aimesh: had_aimesh, minimap: Some(mini), navgrid_version: ver })?;
        self.generated = Some((style.clone(), format));
        self.terrain_stale = false;
        self.nav_walk_changed = false;
        self.touch_all();
        Ok(())
    }

    /// Steps needed before writing: generated terrain and pathfinding hints.
    pub fn prepare_export(&mut self) -> Result<Vec<String>> {
        let mut done = Vec::new();
        if let Some((style, format)) = self.generated.clone()
            && self.terrain_stale
        {
            self.rebuild_terrain(&style, format)?;
            done.push("rebuilt the terrain from the navigation grid".to_string());
        }
        if self.nav_walk_changed {
            if let Some(g) = self.project.navgrid_mut() {
                g.rebuild_hints();
            }
            self.nav_walk_changed = false;
            done.push("recomputed pathfinding hints".to_string());
        }
        Ok(done)
    }
}

/// "Turret_T1_C_05" -> "Turret_T1_C_06"; "Foo" -> "Foo_2".
pub fn next_name(name: &str) -> String {
    let digits = name.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return format!("{name}_2");
    }
    let (head, num) = name.split_at(name.len() - digits);
    let n: u64 = num.parse().unwrap_or(0) + 1;
    format!("{head}{n:0digits$}")
}

/// Byte range of `[name]` and its lines up to the next section.
pub fn section_range(text: &str, name: &str) -> Option<(usize, usize)> {
    let header = format!("[{name}]");
    let mut pos = 0;
    let mut start = None;
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if let Some(s) = start {
            if t.starts_with('[') && t.ends_with(']') {
                return Some((s, pos));
            }
        } else if t.eq_ignore_ascii_case(&header) {
            start = Some(pos);
        }
        pos += line.len();
    }
    start.map(|s| (s, text.len()))
}
