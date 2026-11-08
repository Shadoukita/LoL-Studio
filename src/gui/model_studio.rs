//! Model Studio: a Blender-style editor for custom League assets.
//!
//! Object mode works on whole parts, Edit mode on the active part's vertices. Blender keys:
//! G / R / S move, rotate, scale (then X / Y / Z to lock an axis, Ctrl to snap, click or Enter
//! to confirm, right-click or Esc to cancel), Tab switches mode, A selects all (Alt+A none),
//! Shift+D duplicates, X / Delete deletes, E extrudes, L selects linked, P separates, Ctrl+J joins,
//! H hides (Alt+H shows), F frames the selection, Home everything, numpad 1 / 3 / 7 front / side /
//! top. Middle-drag (or right-drag) orbits, Shift+middle-drag pans, the wheel zooms.

use super::gl_scene::{self, DynMesh, Mat4, Shared};
use super::view3d::View3D;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use lol_map_studio::asset::{self, Asset, AssetPart};
use lol_map_studio::formats::dds::{self, Image};
use lol_map_studio::meshops::{self, Mesh};
use lol_map_studio::props::PropLibrary;
use lol_map_studio::render::RVertex;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Object,
    Edit,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum OpKind {
    Grab,
    Rotate,
    Scale,
}

#[derive(Clone)]
struct Op {
    kind: OpKind,
    axis: Option<usize>,
    start: Pos2,
    pivot: [f32; 3],
    before: Asset,
    /// A value typed while transforming (Blender: G 50 Enter).
    typed: String,
}

/// What the app should do after the studio's frame.
pub enum StudioAction {
    Close,
    /// Place this library prop on the map (Scenery tool).
    UseInMap(String),
    /// The asset library changed (reload the scenery library).
    LibraryChanged,
}

pub struct ModelStudio {
    pub asset: Asset,
    textures: HashMap<String, Image>,
    /// Bumped when textures change (new GL texture keys).
    tex_gen: u64,
    saved: Option<Asset>,
    undo: Vec<Asset>,
    redo: Vec<Asset>,
    mode: Mode,
    sel: BTreeSet<usize>,
    active: Option<usize>,
    sel_verts: BTreeSet<u32>,
    cam: View3D,
    gl: Shared,
    op: Option<Op>,
    box_start: Option<Pos2>,
    status: String,
    log: Vec<String>,
    search: String,
    source_tab: u8,
    characters: Vec<String>,
    show_wire: bool,
    show_grid: bool,
    xray: bool,
    extrude_dist: f32,
    decimate_ratio: f32,
    num_t: [f32; 3],
    num_r: [f32; 3],
    num_s: [f32; 3],
    rename: Option<(usize, String)>,
    fitted: bool,
    /// Set by the app when Tab was pressed (taken out of egui's focus handling).
    pub tab_pressed: bool,
    last_pointer: Option<Pos2>,
    /// The library changed inside a panel (tell the app).
    library_changed: bool,
    /// The library list, read when it changes (not every frame).
    library: Option<Vec<(String, std::path::PathBuf)>>,
    target_height: f32,
    uv_offset: [f32; 2],
    uv_scale: [f32; 2],
    uv_rot: f32,
}

fn mesh_from_geo(g: &lol_map_studio::formats::nvr::Geometry) -> Mesh {
    Mesh { positions: g.positions.clone(), normals: g.normals.clone(), uvs: g.uvs.clone(), colors: Vec::new(), indices: g.indices.clone() }
}

fn invert(m: &Mat4) -> Option<Mat4> {
    super::play_view::invert_pub(m)
}

fn decode_image(path: &std::path::Path) -> Option<Image> {
    let b = std::fs::read(path).ok()?;
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dds")) {
        return dds::decode(&b, 2048).ok();
    }
    let img = image::load_from_memory(&b).ok()?.to_rgba8();
    Some(Image { width: img.width() as usize, height: img.height() as usize, pixels: img.into_raw() })
}

impl ModelStudio {
    pub fn new() -> Self {
        let mut s = Self {
            asset: Asset::new("New asset"),
            textures: HashMap::new(),
            tex_gen: 0,
            saved: None,
            undo: Vec::new(),
            redo: Vec::new(),
            mode: Mode::Object,
            sel: BTreeSet::new(),
            active: None,
            sel_verts: BTreeSet::new(),
            cam: View3D::default(),
            gl: gl_scene::new_shared(),
            op: None,
            box_start: None,
            status: String::new(),
            log: Vec::new(),
            search: String::new(),
            source_tab: 0,
            characters: Vec::new(),
            show_wire: true,
            show_grid: true,
            xray: false,
            extrude_dist: 50.0,
            decimate_ratio: 0.5,
            num_t: [0.0; 3],
            num_r: [0.0; 3],
            num_s: [1.0; 3],
            rename: None,
            fitted: false,
            tab_pressed: false,
            last_pointer: None,
            library_changed: false,
            library: None,
            target_height: 300.0,
            uv_offset: [0.0; 2],
            uv_scale: [1.0; 2],
            uv_rot: 0.0,
        };
        s.cam.yaw = -2.2;
        s.cam.pitch = 0.45;
        s.cam.distance = 900.0;
        s.cam.target = [0.0, 100.0, 0.0];
        s
    }

    pub fn dirty(&self) -> bool {
        self.saved.as_ref() != Some(&self.asset) && !self.asset.parts.is_empty()
    }

    fn push_undo(&mut self) {
        self.undo.push(self.asset.clone());
        if self.undo.len() > 60 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn info(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.status = s.clone();
        self.log.push(s);
    }

    fn load_asset(&mut self, a: Asset, textures: HashMap<String, Image>, saved: bool) {
        self.asset = a;
        self.textures = textures;
        self.tex_gen += 1;
        self.saved = if saved { Some(self.asset.clone()) } else { None };
        self.undo.clear();
        self.redo.clear();
        self.sel.clear();
        self.sel_verts.clear();
        self.active = if self.asset.parts.is_empty() { None } else { Some(0) };
        self.mode = Mode::Object;
        self.op = None;
        self.frame_all();
    }

    /// Opens a model file of any supported format as a new asset.
    pub fn import_path(&mut self, path: &std::path::Path) {
        match lol_map_studio::import3d::import_file(path) {
            Ok(m) => {
                let mut a = Asset::new(&m.name);
                a.source = path.to_string_lossy().to_string();
                let mut textures = HashMap::new();
                for p in m.parts {
                    let texture = p.texture.map(|t| {
                        let mut name = t.name.clone();
                        let mut k = 2;
                        while textures.contains_key(&name) {
                            name = format!("{}_{k}", t.name);
                            k += 1;
                        }
                        textures.insert(name.clone(), t.image);
                        name
                    });
                    a.parts.push(AssetPart { name: p.name, mesh: Mesh { positions: p.positions, normals: p.normals, uvs: p.uvs, colors: Vec::new(), indices: p.indices }, texture, color: p.color, visible: true });
                }
                for w in &m.warnings {
                    self.log.push(format!("Import: {w}"));
                }
                // League props stand on the ground at their centre.
                Self::recenter_all(&mut a);
                let (v, t) = a.stats();
                self.load_asset(a, textures, false);
                self.info(format!("Imported {} ({} parts, {} vertices, {} triangles)", path.display(), self.asset.parts.len(), v, t));
            }
            Err(e) => self.info(format!("Could not import {}: {e:#}", path.display())),
        }
    }

    fn recenter_all(a: &mut Asset) {
        let (lo, hi) = a.bounds();
        let off = [-(lo[0] + hi[0]) / 2.0, -lo[1], -(lo[2] + hi[2]) / 2.0];
        let m = meshops::trs(off, [0.0; 3], [1.0; 3]);
        for p in &mut a.parts {
            meshops::transform(&mut p.mesh, &m);
        }
    }

    /// Opens a map prop (from the scenery library) as an editable asset.
    /// Opens a library prop for editing: your assets as saved, map props as a new asset.
    pub fn open_key(&mut self, key: &str, lib: Option<&PropLibrary>) {
        if let Some(name) = key.strip_prefix("asset:") {
            if let Ok((a, t)) = asset::load(&asset::asset_dir(name)) {
                self.load_asset(a, t, true);
                self.info(format!("Editing {name}: saving it updates every copy placed on your maps"));
                return;
            }
        }
        match lib.and_then(|l| l.get(key)) {
            Some(d) => {
                let d = d.clone();
                self.open_prop(&d);
            }
            None => self.info(format!("{key} is not in the library")),
        }
    }

    fn open_prop(&mut self, d: &lol_map_studio::props::PropDef) {
        let mut a = Asset::new(&d.name);
        a.source = format!("prop {}", d.key);
        a.category = d.category.clone();
        a.blocks = d.blocks();
        let mut textures = HashMap::new();
        for (i, p) in d.parts.iter().enumerate() {
            let tex = p.material.channels[0].texture.clone();
            let name = tex.rsplit(['/', '\\']).next().unwrap_or(&tex).trim_end_matches(".dds").trim_end_matches(".tga").to_string();
            if !textures.contains_key(&name) {
                let file = std::fs::read_dir(&d.texture_dir).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|q| q.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(&tex))));
                if let Some(img) = file.as_deref().and_then(decode_image) {
                    textures.insert(name.clone(), img);
                }
            }
            a.parts.push(AssetPart { name: format!("{} {}", p.material.name, i + 1), mesh: mesh_from_geo(&p.geo), texture: textures.contains_key(&name).then_some(name), color: [1.0; 4], visible: true });
        }
        self.load_asset(a, textures, false);
        self.info(format!("Opened {} for editing; save it to keep it in your library", d.name));
    }

    fn frame_all(&mut self) {
        let (lo, hi) = self.asset.bounds();
        self.frame_box(lo, hi);
    }

    fn frame_box(&mut self, lo: [f32; 3], hi: [f32; 3]) {
        if lo[0] > hi[0] {
            return;
        }
        self.cam.target = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let r = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
        self.cam.distance = (r * 1.4).max(60.0);
        self.fitted = true;
    }

    fn frame_selection(&mut self) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        let mut grow = |v: &[f32; 3]| {
            for a in 0..3 {
                lo[a] = lo[a].min(v[a]);
                hi[a] = hi[a].max(v[a]);
            }
        };
        match (self.mode, self.active) {
            (Mode::Edit, Some(i)) if !self.sel_verts.is_empty() => {
                for &v in &self.sel_verts {
                    if let Some(p) = self.asset.parts[i].mesh.positions.get(v as usize) {
                        grow(p);
                    }
                }
            }
            _ => {
                for &i in &self.sel {
                    for p in &self.asset.parts[i].mesh.positions {
                        grow(p);
                    }
                }
            }
        }
        if lo[0] <= hi[0] {
            self.frame_box(lo, hi);
        } else {
            self.frame_all();
        }
    }

    /// Pivot of the current selection (centre of its bounds).
    fn pivot(&self) -> [f32; 3] {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        let mut any = false;
        let mut grow = |v: &[f32; 3]| {
            any = true;
            for a in 0..3 {
                lo[a] = lo[a].min(v[a]);
                hi[a] = hi[a].max(v[a]);
            }
        };
        if self.mode == Mode::Edit
            && let Some(i) = self.active
        {
            for &v in &self.sel_verts {
                if let Some(p) = self.asset.parts[i].mesh.positions.get(v as usize) {
                    grow(p);
                }
            }
        } else {
            for &i in &self.sel {
                for p in &self.asset.parts[i].mesh.positions {
                    grow(p);
                }
            }
        }
        if any { [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0] } else { [0.0; 3] }
    }

    fn has_selection(&self) -> bool {
        match self.mode {
            Mode::Object => !self.sel.is_empty(),
            Mode::Edit => !self.sel_verts.is_empty(),
        }
    }

    /// Applies a matrix to the selection (whole parts, or the active part's selected vertices).
    fn apply_to_selection(&mut self, m: &Mat4) {
        match self.mode {
            Mode::Object => {
                for &i in &self.sel {
                    meshops::transform(&mut self.asset.parts[i].mesh, m);
                }
            }
            Mode::Edit => {
                if let Some(i) = self.active {
                    let v = self.with_seam_twins(i);
                    meshops::transform_selected(&mut self.asset.parts[i].mesh, &v, m);
                }
            }
        }
    }

    /// The selected vertices plus their copies along UV / normal seams (same position), so moving
    /// a vertex does not tear the mesh open.
    fn with_seam_twins(&self, i: usize) -> Vec<u32> {
        let m = &self.asset.parts[i].mesh;
        let key = |p: &[f32; 3]| [(p[0] * 1000.0).round() as i64, (p[1] * 1000.0).round() as i64, (p[2] * 1000.0).round() as i64];
        let wanted: std::collections::HashSet<[i64; 3]> = self.sel_verts.iter().filter_map(|&v| m.positions.get(v as usize)).map(key).collect();
        (0..m.positions.len() as u32).filter(|&v| self.sel_verts.contains(&v) || wanted.contains(&key(&m.positions[v as usize]))).collect()
    }

    fn selected_faces(&self) -> Vec<u32> {
        match self.active {
            Some(i) => {
                let m = &self.asset.parts[i].mesh;
                let v: Vec<u32> = self.sel_verts.iter().copied().collect();
                // Faces whose three corners are all selected.
                meshops::faces_using(m, &v).into_iter().filter(|&f| (0..3).all(|k| m.indices.get(f as usize * 3 + k).is_some_and(|x| self.sel_verts.contains(x)))).collect()
            }
            None => Vec::new(),
        }
    }

    // ------------------------------------------------------------------ operations

    fn delete(&mut self) {
        self.push_undo();
        match self.mode {
            Mode::Object => {
                let mut idx: Vec<usize> = self.sel.iter().copied().collect();
                idx.sort_unstable_by(|a, b| b.cmp(a));
                for i in idx {
                    self.asset.parts.remove(i);
                }
                self.sel.clear();
                self.active = None;
            }
            Mode::Edit => {
                let faces = self.selected_faces();
                if let Some(i) = self.active {
                    meshops::remove_faces(&mut self.asset.parts[i].mesh, &faces);
                }
                self.sel_verts.clear();
            }
        }
    }

    fn duplicate(&mut self) {
        self.push_undo();
        match self.mode {
            Mode::Object => {
                let mut new = BTreeSet::new();
                for &i in &self.sel.clone() {
                    let mut p = self.asset.parts[i].clone();
                    p.name = format!("{} copy", p.name);
                    self.asset.parts.push(p);
                    new.insert(self.asset.parts.len() - 1);
                }
                self.sel = new;
                self.active = self.sel.iter().next().copied();
            }
            Mode::Edit => {
                let faces = self.selected_faces();
                if let Some(i) = self.active {
                    let extra = meshops::extract_faces(&self.asset.parts[i].mesh, &faces);
                    let base = self.asset.parts[i].mesh.positions.len() as u32;
                    let n = extra.positions.len() as u32;
                    self.asset.parts[i].mesh = meshops::merge(&self.asset.parts[i].mesh, &extra);
                    self.sel_verts = (base..base + n).collect();
                }
            }
        }
        self.begin_op(OpKind::Grab, None);
    }

    fn separate(&mut self) {
        let Some(i) = self.active else { return };
        let faces = self.selected_faces();
        if faces.is_empty() {
            return;
        }
        self.push_undo();
        let part = &self.asset.parts[i];
        let new_mesh = meshops::extract_faces(&part.mesh, &faces);
        let mut np = part.clone();
        np.mesh = new_mesh;
        np.name = format!("{} part", part.name);
        meshops::remove_faces(&mut self.asset.parts[i].mesh, &faces);
        self.asset.parts.push(np);
        self.sel_verts.clear();
        self.info("Separated the selection into a new part");
    }

    fn join(&mut self) {
        if self.sel.len() < 2 {
            return;
        }
        self.push_undo();
        let mut idx: Vec<usize> = self.sel.iter().copied().collect();
        let first = idx[0];
        let mut m = self.asset.parts[first].mesh.clone();
        for &i in &idx[1..] {
            m = meshops::merge(&m, &self.asset.parts[i].mesh);
        }
        self.asset.parts[first].mesh = m;
        idx.remove(0);
        idx.sort_unstable_by(|a, b| b.cmp(a));
        for i in idx {
            self.asset.parts.remove(i);
        }
        self.sel = [first].into();
        self.active = Some(first);
        self.info("Joined the selected parts (they share the first part's material)");
    }

    fn extrude(&mut self) -> bool {
        let Some(i) = self.active else { return false };
        let faces = self.selected_faces();
        if faces.is_empty() {
            self.info("Select faces (all their corners) to extrude");
            return false;
        }
        self.push_undo();
        let new = meshops::extrude_faces(&mut self.asset.parts[i].mesh, &faces, self.extrude_dist);
        self.sel_verts = new.into_iter().collect();
        self.info("Extruded; move the new faces (G) or press Enter");
        true
    }

    fn mesh_tool(&mut self, f: impl Fn(&mut Mesh)) {
        let parts: Vec<usize> = if self.sel.is_empty() { self.active.into_iter().collect() } else { self.sel.iter().copied().collect() };
        if parts.is_empty() {
            self.info("Select a part first");
            return;
        }
        self.push_undo();
        for i in parts {
            f(&mut self.asset.parts[i].mesh);
        }
        self.sel_verts.clear();
    }

    fn add_primitive(&mut self, name: &str, m: Mesh) {
        self.push_undo();
        let at = self.cam.target;
        let mut m = m;
        meshops::transform(&mut m, &meshops::trs([at[0], 0.0, at[2]], [0.0; 3], [1.0; 3]));
        self.asset.parts.push(AssetPart { name: name.into(), mesh: m, texture: None, color: [0.8, 0.8, 0.8, 1.0], visible: true });
        let i = self.asset.parts.len() - 1;
        self.sel = [i].into();
        self.active = Some(i);
        self.mode = Mode::Object;
        if self.asset.parts.len() == 1 {
            self.frame_all();
        }
    }

    fn begin_op(&mut self, kind: OpKind, start: Option<Pos2>) {
        if !self.has_selection() {
            self.info("Nothing selected");
            return;
        }
        let start = start.or(self.last_pointer).unwrap_or(Pos2::ZERO);
        self.op = Some(Op { kind, axis: None, start, pivot: self.pivot(), before: self.asset.clone(), typed: String::new() });
    }

    fn save(&mut self) -> bool {
        if self.asset.parts.is_empty() {
            self.info("Nothing to save");
            return false;
        }
        match asset::save(&self.asset, &self.textures) {
            Ok(dir) => {
                self.saved = Some(self.asset.clone());
                self.library = None;
                self.info(format!("Saved to {}", dir.display()));
                true
            }
            Err(e) => {
                self.info(format!("Could not save: {e:#}"));
                false
            }
        }
    }

    fn undo_step(&mut self) {
        if let Some(a) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.asset, a));
            self.clamp_selection();
        }
    }

    fn redo_step(&mut self) {
        if let Some(a) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.asset, a));
            self.clamp_selection();
        }
    }

    fn clamp_selection(&mut self) {
        let n = self.asset.parts.len();
        self.sel.retain(|&i| i < n);
        if self.active.is_some_and(|i| i >= n) {
            self.active = None;
        }
        if let Some(i) = self.active {
            let nv = self.asset.parts[i].mesh.positions.len() as u32;
            self.sel_verts.retain(|&v| v < nv);
        } else {
            self.sel_verts.clear();
        }
    }

    // ------------------------------------------------------------------ UI

    pub fn ui(&mut self, ui: &mut egui::Ui, lib: Option<&PropLibrary>, data_root: Option<&std::path::Path>) -> Option<StudioAction> {
        let mut action = None;
        let idle_top = self.op.is_none();
        egui::Panel::top("ms_top").show(ui, |ui| {
            ui.add_enabled_ui(idle_top, |ui| ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Model Studio").strong().color(Color32::from_rgb(140, 190, 255)));
                ui.separator();
                ui.menu_button("File", |ui| {
                    if ui.button("New asset").clicked() {
                        ui.close();
                        self.load_asset(Asset::new("New asset"), HashMap::new(), false);
                    }
                    if ui.button("Import a 3D model…").clicked() {
                        ui.close();
                        let exts = lol_map_studio::import3d::supported_extensions();
                        if let Some(p) = rfd::FileDialog::new().add_filter("3D models", exts).pick_file() {
                            self.import_path(&p);
                        }
                    }
                    if ui.button("Save to library  Ctrl+S").clicked() {
                        ui.close();
                        if self.save() {
                            action = Some(StudioAction::LibraryChanged);
                        }
                    }
                    if ui.button("Export as OBJ…").clicked() {
                        ui.close();
                        self.export_obj();
                    }
                    ui.separator();
                    if ui.button("Close Model Studio").clicked() {
                        ui.close();
                        action = Some(StudioAction::Close);
                    }
                });
                ui.menu_button("Add", |ui| {
                    let mut add = |ui: &mut egui::Ui, label: &str, m: fn() -> Mesh| {
                        if ui.button(label).clicked() {
                            ui.close();
                            self.add_primitive(label, m());
                        }
                    };
                    add(ui, "Cube", || meshops::cube([100.0, 100.0, 100.0]));
                    add(ui, "Cylinder", || meshops::cylinder(50.0, 120.0, 24));
                    add(ui, "Cone", || meshops::cone(50.0, 120.0, 24));
                    add(ui, "Sphere", || meshops::sphere(60.0, 12, 24));
                    add(ui, "Plane", || meshops::plane([200.0, 200.0], 4));
                    add(ui, "Torus", || meshops::torus(80.0, 20.0, 32, 12));
                    add(ui, "Ramp", || meshops::ramp([100.0, 60.0, 160.0]));
                    add(ui, "Rock", || meshops::rock(80.0, 7));
                });
                ui.separator();
                ui.selectable_value(&mut self.mode, Mode::Object, "Object").on_hover_text("Tab");
                if ui.selectable_value(&mut self.mode, Mode::Edit, "Edit").on_hover_text("Tab: vertices of the active part").clicked() && self.active.is_none() {
                    self.active = self.sel.iter().next().copied().or(if self.asset.parts.is_empty() { None } else { Some(0) });
                }
                ui.separator();
                ui.checkbox(&mut self.show_wire, "Wireframe");
                ui.checkbox(&mut self.xray, "X-ray").on_hover_text("Select through the model in Edit mode");
                ui.checkbox(&mut self.show_grid, "Grid");
                ui.separator();
                if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo")).clicked() {
                    self.undo_step();
                }
                if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo")).clicked() {
                    self.redo_step();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        action = Some(StudioAction::Close);
                    }
                    if ui.add(egui::Button::new(RichText::new("Use in map").strong().color(Color32::from_rgb(120, 230, 140)))).on_hover_text("Save, then place it with the Scenery tool").clicked() && self.save() {
                        action = Some(StudioAction::UseInMap(format!("asset:{}", self.asset.name)));
                    }
                    if ui.button("Save  Ctrl+S").clicked() && self.save() {
                        action = Some(StudioAction::LibraryChanged);
                    }
                    let (v, t) = self.asset.stats();
                    ui.label(RichText::new(format!("{} parts · {} vertices · {} triangles{}", self.asset.parts.len(), v, t, if self.dirty() { " · unsaved" } else { "" })).weak());
                });
            }));
        });
        egui::Panel::bottom("ms_status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
            });
        });
        // While G / R / S runs, the panels wait (their edits would be undone by the running transform).
        let idle = self.op.is_none();
        egui::Panel::left("ms_left").default_size(260.0).show(ui, |ui| {
            ui.add_enabled_ui(idle, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.left_panel(ui, lib, data_root));
            });
        });
        egui::Panel::right("ms_right").default_size(330.0).show(ui, |ui| {
            ui.add_enabled_ui(idle, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.right_panel(ui));
            });
        });
        egui::CentralPanel::no_frame().show(ui, |ui| self.viewport(ui));
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::S)) && self.save() {
            action = Some(StudioAction::LibraryChanged);
        }
        if std::mem::take(&mut self.library_changed) && action.is_none() {
            action = Some(StudioAction::LibraryChanged);
        }
        action
    }

    fn left_panel(&mut self, ui: &mut egui::Ui, lib: Option<&PropLibrary>, data_root: Option<&std::path::Path>) {
        ui.heading("Outliner");
        ui.horizontal(|ui| {
            ui.label("Asset");
            ui.text_edit_singleline(&mut self.asset.name);
        });
        let mut toggle_vis = None;
        let mut select = None;
        for (i, p) in self.asset.parts.iter().enumerate() {
            ui.horizontal(|ui| {
                let mut v = p.visible;
                if ui.add(egui::Checkbox::without_text(&mut v)).on_hover_text("Show / hide (H, Alt+H)").changed() {
                    toggle_vis = Some(i);
                }
                let sel = self.sel.contains(&i);
                let label = format!("{}{}  ({} tris)", if self.active == Some(i) { "> " } else { "" }, p.name, p.mesh.indices.len() / 3);
                let r = ui.selectable_label(sel, label);
                if r.clicked() {
                    select = Some((i, ui.input(|x| x.modifiers.shift)));
                }
                if r.double_clicked() {
                    self.rename = Some((i, p.name.clone()));
                }
            });
        }
        if let Some(i) = toggle_vis {
            self.push_undo();
            self.asset.parts[i].visible = !self.asset.parts[i].visible;
        }
        if let Some((i, add)) = select {
            if !add {
                self.sel.clear();
            }
            if !self.sel.insert(i) && add {
                self.sel.remove(&i);
            }
            self.active = Some(i);
            self.sel_verts.clear();
        }
        if let Some((i, mut n)) = self.rename.take() {
            ui.horizontal(|ui| {
                let r = ui.text_edit_singleline(&mut n);
                if r.lost_focus() || ui.input(|x| x.key_pressed(egui::Key::Enter)) {
                    if i < self.asset.parts.len() {
                        self.push_undo();
                        self.asset.parts[i].name = n.clone();
                    }
                } else {
                    self.rename = Some((i, n.clone()));
                    r.request_focus();
                }
            });
        }
        if self.asset.parts.is_empty() {
            ui.label(RichText::new("Empty: import a model, open one below, or use Add.").weak());
        }
        ui.separator();
        ui.heading("Open");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.source_tab, 0, "My assets");
            ui.selectable_value(&mut self.source_tab, 1, "Map props");
            ui.selectable_value(&mut self.source_tab, 2, "Characters");
        });
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search…"));
        let q = self.search.to_ascii_lowercase();
        egui::ScrollArea::vertical().max_height(420.0).id_salt("ms_sources").show(ui, |ui| match self.source_tab {
            0 => {
                if self.library.is_none() {
                    self.library = Some(asset::list());
                }
                let list = self.library.clone().unwrap_or_default();
                if list.is_empty() {
                    ui.label(RichText::new("No saved assets yet.").weak());
                }
                for (name, dir) in list.into_iter().filter(|x| q.is_empty() || x.0.to_ascii_lowercase().contains(&q)) {
                    ui.horizontal(|ui| {
                        if ui.button(&name).clicked() {
                            match asset::load(&dir) {
                                Ok((a, t)) => {
                                    self.load_asset(a, t, true);
                                    self.info(format!("Opened {name}"));
                                }
                                Err(e) => self.info(format!("Could not open {name}: {e:#}")),
                            }
                        }
                        if ui.small_button("Delete").on_hover_text("Delete from the library").clicked() {
                            let _ = std::fs::remove_dir_all(&dir);
                            self.library = None;
                            self.library_changed = true;
                            self.info(format!("Deleted {name} from the library"));
                        }
                    });
                }
            }
            1 => match lib {
                None => {
                    ui.spinner();
                }
                Some(lb) => {
                    let mut open = None;
                    for d in lb.props.iter().filter(|d| q.is_empty() || d.name.to_ascii_lowercase().contains(&q) || d.category.to_ascii_lowercase().contains(&q)).take(300) {
                        if ui.button(format!("{}  ·  {}", d.name, d.category)).clicked() {
                            open = Some(d.clone());
                        }
                    }
                    if let Some(d) = open {
                        self.open_prop(&d);
                    }
                }
            },
            _ => {
                if self.characters.is_empty()
                    && let Some(root) = data_root
                {
                    let mut v: Vec<String> = std::fs::read_dir(root.join("Characters")).into_iter().flatten().flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
                    v.sort_by_key(|s| s.to_ascii_lowercase());
                    self.characters = v;
                }
                let mut open = None;
                for c in self.characters.iter().filter(|c| q.is_empty() || c.to_ascii_lowercase().contains(&q)).take(400) {
                    if ui.button(c).clicked() {
                        open = Some(c.clone());
                    }
                }
                if let (Some(c), Some(root)) = (open, data_root) {
                    let dir = root.join("Characters").join(&c);
                    // The base skin's model: in the folder itself or under Skins/Base (newer characters).
                    let find = |d: &std::path::Path| -> Option<std::path::PathBuf> {
                        let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(d).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("skn"))).collect();
                        // Prefer the model named like the character, then the biggest.
                        v.sort_by_key(|p| (!p.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(&c)), std::cmp::Reverse(p.metadata().map(|m| m.len()).unwrap_or(0))));
                        v.into_iter().next()
                    };
                    let skn = find(&dir).or_else(|| find(&dir.join("Skins").join("Base"))).or_else(|| std::fs::read_dir(dir.join("Skins")).ok()?.flatten().find_map(|e| find(&e.path())));
                    match skn {
                        Some(p) => self.import_path(&p),
                        None => self.info(format!("{c} has no model (.skn)")),
                    }
                }
            }
        });
    }

    fn right_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Asset");
        egui::Grid::new("ms_asset").num_columns(2).show(ui, |ui| {
            ui.label("Category");
            ui.add(egui::TextEdit::singleline(&mut self.asset.category).hint_text("My assets"));
            ui.end_row();
            ui.label("Blocks walking");
            ui.checkbox(&mut self.asset.blocks, "units walk around it");
            ui.end_row();
            let (lo, hi) = self.asset.bounds();
            ui.label("Size");
            ui.label(format!("{:.0} × {:.0} × {:.0} units", hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]));
            ui.end_row();
        });
        ui.label(RichText::new("League sizes: a champion is about 180 units tall, a turret 500, a tree 300 to 600.").weak());
        // League budgets (the client draws thousands of props).
        let (v, t) = self.asset.stats();
        let warn = Color32::from_rgb(255, 190, 110);
        if t > 3000 {
            ui.label(RichText::new(format!("{t} triangles: Summoner's Rift props use 100 to 3000. Decimate to keep the map fast.")).color(warn));
        }
        if self.asset.parts.iter().any(|p| p.mesh.positions.len() > 65000) {
            ui.label(RichText::new("A part has more than 65000 vertices; split it (Separate loose parts).").color(warn));
        }
        if self.textures.values().any(|t| t.width.max(t.height) > 1024) {
            ui.label(RichText::new("Textures over 1024 px are heavy for the 4.x client; they are kept but consider smaller ones.").color(warn));
        }
        if self.asset.parts.len() > 8 {
            ui.label(RichText::new(format!("{} parts = {} materials; Join parts that share a texture.", self.asset.parts.len(), self.asset.parts.len())).color(warn));
        }
        let _ = v;
        ui.horizontal(|ui| {
            if ui.button("Stand on the ground").on_hover_text("Centre it and put its base at height 0 (how map props are placed)").clicked() {
                self.push_undo();
                Self::recenter_all(&mut self.asset);
                self.frame_all();
            }
            ui.add(egui::DragValue::new(&mut self.target_height).range(1.0..=10000.0).speed(5.0));
            if ui.button("Scale to height").on_hover_text("Uniformly scale the whole asset to this height in units").clicked() {
                let (lo, hi) = self.asset.bounds();
                let h = hi[1] - lo[1];
                if h > 0.0 {
                    self.push_undo();
                    let k = self.target_height / h;
                    let m = meshops::trs([0.0; 3], [0.0; 3], [k; 3]);
                    for p in &mut self.asset.parts {
                        meshops::transform(&mut p.mesh, &m);
                    }
                    self.frame_all();
                    let th = self.target_height;
                    self.info(format!("Scaled to {th:.0} units tall (fine-tune with S)"));
                }
            }
        });
        ui.separator();

        // Transform the selection numerically.
        ui.heading("Transform");
        ui.label(RichText::new(if self.mode == Mode::Object { "Selected parts" } else { "Selected vertices" }).weak());
        egui::Grid::new("ms_tf").num_columns(4).show(ui, |ui| {
            for (label, v, speed) in [("Move", &mut self.num_t, 1.0), ("Rotate °", &mut self.num_r, 0.5), ("Scale", &mut self.num_s, 0.01)] {
                ui.label(label);
                for a in v.iter_mut() {
                    ui.add(egui::DragValue::new(a).speed(speed));
                }
                ui.end_row();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() && self.has_selection() {
                self.push_undo();
                let p = self.pivot();
                let m = meshops::mat_mul(&meshops::trs([p[0] + self.num_t[0], p[1] + self.num_t[1], p[2] + self.num_t[2]], self.num_r, self.num_s), &meshops::trs([-p[0], -p[1], -p[2]], [0.0; 3], [1.0; 3]));
                self.apply_to_selection(&m);
                self.num_t = [0.0; 3];
                self.num_r = [0.0; 3];
                self.num_s = [1.0; 3];
            }
            for (k, name) in ["X", "Y", "Z"].iter().enumerate() {
                if ui.button(format!("Mirror {name}")).clicked() {
                    self.mesh_tool(|m| meshops::mirror(m, k));
                }
            }
        });
        ui.separator();

        // Part: material and mesh tools.
        if let Some(i) = self.active.filter(|&i| i < self.asset.parts.len()) {
            ui.heading(format!("Part: {}", self.asset.parts[i].name));
            ui.horizontal(|ui| {
                ui.label("Texture");
                ui.label(self.asset.parts[i].texture.clone().unwrap_or_else(|| "(none)".into()));
            });
            ui.horizontal(|ui| {
                if ui.button("Load image…").on_hover_text("PNG, JPG, TGA, BMP or DDS").clicked()
                    && let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "tga", "bmp", "dds"]).pick_file()
                {
                    match decode_image(&p) {
                        Some(img) => {
                            self.push_undo();
                            let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "texture".into());
                            self.textures.insert(name.clone(), img);
                            self.tex_gen += 1;
                            self.asset.parts[i].texture = Some(name);
                        }
                        None => self.info("Could not read that image"),
                    }
                }
                if self.asset.parts[i].texture.is_some() && ui.button("Remove").clicked() {
                    self.push_undo();
                    self.asset.parts[i].texture = None;
                }
                let mut names: Vec<String> = self.textures.keys().cloned().collect();
                names.sort();
                if !names.is_empty() {
                    egui::ComboBox::from_id_salt("ms_tex").selected_text("Use…").show_ui(ui, |ui| {
                        for n in names {
                            if ui.selectable_label(false, &n).clicked() {
                                self.undo.push(self.asset.clone());
                                self.asset.parts[i].texture = Some(n);
                            }
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.label("Tint");
                let mut c = self.asset.parts[i].color;
                let r = ui.color_edit_button_rgba_unmultiplied(&mut c);
                if r.changed() {
                    if self.undo.last().is_none_or(|u| u.parts.get(i).is_none_or(|p| p.color == self.asset.parts[i].color && p.texture == self.asset.parts[i].texture) || u.parts.len() != self.asset.parts.len()) {
                        self.undo.push(self.asset.clone());
                    }
                    self.asset.parts[i].color = c;
                }
            });
            ui.label(RichText::new("The tint is baked into the game texture when saved.").weak());
            ui.horizontal(|ui| {
                ui.label("UV offset");
                ui.add(egui::DragValue::new(&mut self.uv_offset[0]).speed(0.01));
                ui.add(egui::DragValue::new(&mut self.uv_offset[1]).speed(0.01));
            });
            ui.horizontal(|ui| {
                ui.label("UV scale");
                ui.add(egui::DragValue::new(&mut self.uv_scale[0]).speed(0.01));
                ui.add(egui::DragValue::new(&mut self.uv_scale[1]).speed(0.01));
                ui.label("turn");
                ui.add(egui::DragValue::new(&mut self.uv_rot).speed(1.0).suffix("°"));
            });
            if ui.button("Apply to UVs").on_hover_text("Move, tile or turn the texture on this part").clicked() {
                let (o, sc, r) = (self.uv_offset, self.uv_scale, self.uv_rot);
                self.push_undo();
                meshops::uv_transform(&mut self.asset.parts[i].mesh, o, sc, r);
                self.uv_offset = [0.0; 2];
                self.uv_scale = [1.0; 2];
                self.uv_rot = 0.0;
            }
        }
        ui.separator();
        ui.heading("Mesh tools");
        ui.label(RichText::new("On the selected parts (or the active part).").weak());
        egui::Grid::new("ms_tools").num_columns(2).show(ui, |ui| {
            if ui.button("Smooth normals").clicked() {
                self.mesh_tool(|m| meshops::recompute_normals(m, None));
            }
            if ui.button("Auto smooth 40°").clicked() {
                self.mesh_tool(|m| meshops::recompute_normals(m, Some(40.0)));
            }
            ui.end_row();
            if ui.button("Flat shading").clicked() {
                self.mesh_tool(meshops::flat_shade);
            }
            if ui.button("Flip faces").clicked() {
                self.mesh_tool(meshops::flip);
            }
            ui.end_row();
            if ui.button("Merge by distance").clicked() {
                self.mesh_tool(|m| {
                    meshops::weld(m, 0.5);
                });
            }
            if ui.button("Subdivide").clicked() {
                self.mesh_tool(meshops::subdivide);
            }
            ui.end_row();
            if ui.button("Box UVs").on_hover_text("Project the texture from six sides (good for rocks and walls)").clicked() {
                self.mesh_tool(|m| meshops::uv_box(m, 0.005));
            }
            if ui.button("Top-down UVs").clicked() {
                self.mesh_tool(|m| meshops::uv_planar(m, 1, 0.005));
            }
            ui.end_row();
            if ui.button("Separate loose parts").clicked() {
                self.split_selected();
            }
            if ui.button("Join  Ctrl+J").clicked() {
                self.join();
            }
            ui.end_row();
        });
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.decimate_ratio, 0.05..=0.95).text("keep"));
            if ui.button("Decimate").on_hover_text("Fewer triangles (keep this fraction)").clicked() {
                let r = self.decimate_ratio;
                self.mesh_tool(|m| meshops::decimate(m, r));
            }
        });
        if self.mode == Mode::Edit {
            ui.separator();
            ui.heading("Edit mode");
            ui.label(RichText::new(format!("{} vertices selected", self.sel_verts.len())).weak());
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.extrude_dist).speed(1.0).prefix("extrude "));
                if ui.button("Extrude  E").clicked() {
                    let _ = self.extrude();
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Select linked  L").clicked() {
                    self.select_linked();
                }
                if ui.button("Separate  P").clicked() {
                    self.separate();
                }
                if ui.button("Delete faces  X").clicked() {
                    self.delete();
                }
            });
        }
        ui.separator();
        ui.collapsing("Log", |ui| {
            for l in self.log.iter().rev().take(30) {
                ui.label(RichText::new(l).weak());
            }
        });
        ui.collapsing("Keys", |ui| {
            ui.label("G move · R rotate · S scale, then X/Y/Z to lock an axis, Ctrl to snap, click/Enter to confirm, right-click/Esc to cancel");
            ui.label("Tab Object/Edit · A all · Alt+A none · Shift+click add · drag: box select");
            ui.label("Shift+D duplicate · X/Del delete · E extrude · L linked · P separate · Ctrl+J join · H hide · Alt+H show all");
            ui.label("Middle or right drag orbit · Shift+middle pan · wheel zoom · F frame selection · Home everything · numpad 1/3/7 views");
            ui.label("Ctrl+Z / Ctrl+Y undo / redo · Ctrl+S save");
        });
    }

    fn split_selected(&mut self) {
        let parts: Vec<usize> = self.sel.iter().copied().collect();
        if parts.is_empty() {
            return;
        }
        self.push_undo();
        let mut idx = parts.clone();
        idx.sort_unstable_by(|a, b| b.cmp(a));
        let mut added = 0;
        for i in idx {
            let p = self.asset.parts.remove(i);
            let pieces = meshops::split_loose(&p.mesh);
            added += pieces.len();
            for (k, m) in pieces.into_iter().enumerate() {
                let mut np = p.clone();
                np.mesh = m;
                np.name = format!("{} {}", p.name, k + 1);
                self.asset.parts.push(np);
            }
        }
        self.sel.clear();
        self.active = None;
        self.info(format!("Separated into {added} parts"));
    }

    fn select_linked(&mut self) {
        let Some(i) = self.active else { return };
        let seed: Vec<u32> = self.sel_verts.iter().copied().collect();
        self.sel_verts = meshops::linked_vertices(&self.asset.parts[i].mesh, &seed).into_iter().collect();
    }

    fn export_obj(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Wavefront OBJ", &["obj"]).set_file_name(format!("{}.obj", asset::safe_name(&self.asset.name))).save_file() else { return };
        let mut s = String::from("# Exported by LoL Map Studio (League coordinates: X mirrored back for other tools)\n");
        let mut base = 1usize;
        for p in &self.asset.parts {
            s += &format!("o {}\n", p.name.replace(' ', "_"));
            for v in &p.mesh.positions {
                s += &format!("v {} {} {}\n", -v[0], v[1], v[2]);
            }
            for t in &p.mesh.uvs {
                s += &format!("vt {} {}\n", t[0], 1.0 - t[1]);
            }
            for n in &p.mesh.normals {
                s += &format!("vn {} {} {}\n", -n[0], n[1], n[2]);
            }
            let has_uv = p.mesh.uvs.len() == p.mesh.positions.len();
            for f in p.mesh.indices.chunks_exact(3) {
                // Mirrored back: reverse the winding.
                let c: Vec<String> = [f[0], f[2], f[1]].iter().map(|&i| {
                    let k = i as usize + base;
                    if has_uv { format!("{k}/{k}/{k}") } else { format!("{k}//{k}") }
                }).collect();
                s += &format!("f {}\n", c.join(" "));
            }
            base += p.mesh.positions.len();
        }
        match std::fs::write(&path, s) {
            Ok(_) => self.info(format!("Exported {}", path.display())),
            Err(e) => self.info(format!("Could not write {}: {e}", path.display())),
        }
    }

    // ------------------------------------------------------------------ viewport

    fn keys(&mut self, ui: &egui::Ui, hover: Option<Pos2>) {
        if ui.ctx().egui_wants_keyboard_input() {
            self.tab_pressed = false;
            return;
        }
        use egui::Key::*;
        let mods = ui.input(|i| i.modifiers);
        let key = |k| ui.input(|i| i.key_pressed(k));
        if mods.ctrl && key(Z) && self.op.is_some() {
            // Undo while transforming cancels the transform.
            if let Some(op) = self.op.take() {
                self.asset = op.before;
            }
            return;
        }
        if mods.ctrl && key(Z) {
            if mods.shift { self.redo_step() } else { self.undo_step() }
            return;
        }
        if mods.ctrl && key(Y) {
            self.redo_step();
            return;
        }
        if self.op.is_some() {
            // Axis locks and typed values while transforming.
            for (k, a) in [(X, 0), (Y, 1), (Z, 2)] {
                if key(k)
                    && let Some(op) = &mut self.op
                {
                    op.axis = if op.axis == Some(a) { None } else { Some(a) };
                }
            }
            let typed: String = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Text(t) => Some(t.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect::<String>()),
                        _ => None,
                    })
                    .collect()
            });
            if let Some(op) = &mut self.op {
                op.typed.push_str(&typed);
                if key(Backspace) {
                    op.typed.pop();
                }
            }
            self.tab_pressed = false;
            return;
        }
        if std::mem::take(&mut self.tab_pressed) || key(Tab) {
            self.mode = if self.mode == Mode::Object { Mode::Edit } else { Mode::Object };
            if self.mode == Mode::Edit && self.active.is_none() {
                self.active = self.sel.iter().next().copied().or(if self.asset.parts.is_empty() { None } else { Some(0) });
            }
        }
        if mods.ctrl && key(J) {
            self.join();
            return;
        }
        if mods.shift && key(D) {
            self.duplicate();
            return;
        }
        if mods.alt && key(A) {
            self.sel.clear();
            self.sel_verts.clear();
            return;
        }
        if mods.alt && key(H) {
            self.push_undo();
            for p in &mut self.asset.parts {
                p.visible = true;
            }
            return;
        }
        if mods.any() {
            return;
        }
        let start = hover.unwrap_or(Pos2::ZERO);
        if key(G) {
            self.begin_op(OpKind::Grab, Some(start));
        } else if key(R) {
            self.begin_op(OpKind::Rotate, Some(start));
        } else if key(S) {
            self.begin_op(OpKind::Scale, Some(start));
        } else if key(A) {
            match self.mode {
                Mode::Object => self.sel = (0..self.asset.parts.len()).collect(),
                Mode::Edit => {
                    if let Some(i) = self.active {
                        self.sel_verts = (0..self.asset.parts[i].mesh.positions.len() as u32).collect();
                    }
                }
            }
        } else if key(X) || key(Delete) {
            self.delete();
        } else if key(E) && self.mode == Mode::Edit {
            if self.extrude() {
                self.begin_op(OpKind::Grab, Some(start));
            }
        } else if key(L) && self.mode == Mode::Edit {
            self.select_linked();
        } else if key(P) && self.mode == Mode::Edit {
            self.separate();
        } else if key(H) && self.mode == Mode::Object {
            self.push_undo();
            for &i in &self.sel {
                self.asset.parts[i].visible = false;
            }
        } else if key(F) {
            self.frame_selection();
        } else if key(Home) {
            self.frame_all();
        } else if key(Num1) || key(Num3) || key(Num7) {
            // Front / side / top views.
            if key(Num1) {
                self.cam.yaw = -std::f32::consts::FRAC_PI_2;
                self.cam.pitch = 0.05;
            } else if key(Num3) {
                self.cam.yaw = 0.0;
                self.cam.pitch = 0.05;
            } else {
                self.cam.pitch = 1.55;
            }
        }
    }

    fn part_meshes(&self, gl: &mut gl_scene::GlScene) -> Vec<DynMesh> {
        let mut out = Vec::new();
        for (i, p) in self.asset.parts.iter().enumerate() {
            if !p.visible || p.mesh.indices.is_empty() {
                continue;
            }
            let key = p.texture.as_ref().map(|t| format!("studio:{}:{}", self.tex_gen, t));
            if let (Some(k), Some(t)) = (&key, p.texture.as_ref()) {
                if !gl.has_texture(k) {
                    match self.textures.get(t) {
                        Some(img) => gl.add_texture(k, img.width, img.height, img.pixels.clone()),
                        None => gl.add_texture(k, 1, 1, vec![200, 200, 200, 255]),
                    }
                }
            }
            let n = p.mesh.positions.len();
            let vertices: Vec<RVertex> = (0..n)
                .map(|k| {
                    let uv = p.mesh.uvs.get(k).copied().unwrap_or([0.0; 2]);
                    RVertex { pos: p.mesh.positions[k], normal: p.mesh.normals.get(k).copied().unwrap_or([0.0, 1.0, 0.0]), uv0: uv, uv1: uv, color: [0.09, 0.09, 0.1, 1.0] }
                })
                .collect();
            let sel = self.sel.contains(&i) && self.mode == Mode::Object;
            let c = p.color;
            let tint = if sel { [c[0] * 1.25 + 0.15, c[1] * 1.15 + 0.1, c[2] * 0.9, 1.0] } else { [c[0], c[1], c[2], 1.0] };
            out.push(DynMesh { texture: key, vertices, indices: p.mesh.indices.clone(), tint, cutout: false, particle: false, additive: false });
        }
        out
    }

    fn viewport(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if !self.fitted && !self.asset.parts.is_empty() {
            self.frame_all();
        }
        let hover = resp.hover_pos();
        if hover.is_some() {
            self.last_pointer = hover;
        }
        // Files dropped on the window: models are imported, images become the part's texture.
        let dropped = ui.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let path = f.path().to_path_buf();
            if path.as_os_str().is_empty() {
                continue;
            }
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if lol_map_studio::import3d::supported_extensions().contains(&ext.as_str()) {
                self.import_path(&path);
            } else if let (Some(i), Some(img)) = (self.active.filter(|&i| i < self.asset.parts.len()), decode_image(&path)) {
                self.push_undo();
                let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "texture".into());
                self.textures.insert(name.clone(), img);
                self.tex_gen += 1;
                self.asset.parts[i].texture = Some(name);
                self.info(format!("Texture {} on {}", path.display(), self.asset.parts[i].name));
            }
        }
        if ui.input(|i| !i.raw.hovered_files.is_empty()) {
            ui.painter().rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(60, 120, 220, 60));
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Drop a 3D model to import it, or an image to texture the selected part", FontId::proportional(18.0), Color32::WHITE);
        }
        self.keys(ui, hover);
        // Camera.
        let (mods, scroll) = ui.input(|i| (i.modifiers, i.smooth_scroll_delta.y));
        let orbit = resp.dragged_by(egui::PointerButton::Middle) && !mods.shift || (resp.dragged_by(egui::PointerButton::Secondary) && self.op.is_none());
        if orbit {
            let d = resp.drag_delta();
            self.cam.yaw -= d.x * 0.008;
            self.cam.pitch = (self.cam.pitch + d.y * 0.008).clamp(-1.5, 1.55);
        }
        if resp.dragged_by(egui::PointerButton::Middle) && mods.shift {
            let d = resp.drag_delta();
            let (cy, sy) = (self.cam.yaw.cos(), self.cam.yaw.sin());
            let k = self.cam.distance * 0.0015;
            // Screen right is (sy, -cy) on the ground; screen up follows the camera's pitch.
            let (cp, sp) = (self.cam.pitch.cos(), self.cam.pitch.sin());
            self.cam.target[0] += (-sy * d.x + cy * sp * d.y) * k;
            self.cam.target[2] += (cy * d.x + sy * sp * d.y) * k;
            self.cam.target[1] += cp * d.y * k;
        }
        if resp.hovered() && scroll != 0.0 {
            self.cam.distance = (self.cam.distance * (-scroll * 0.0015).exp()).clamp(5.0, 50000.0);
        }
        let aspect = rect.width() / rect.height().max(1.0);
        let mvp = self.cam.mvp(aspect);
        let Some(inv) = invert(&mvp) else { return };
        let to_ndc = |p: Pos2| [(p.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.top()) / rect.height() * 2.0];
        let project = |p: [f32; 3]| -> Option<Pos2> {
            let c = gl_scene::transform(&mvp, p);
            (c[3] > 1e-3).then(|| Pos2::new(rect.left() + (c[0] / c[3] + 1.0) / 2.0 * rect.width(), rect.top() + (1.0 - c[1] / c[3]) / 2.0 * rect.height()))
        };
        let ray = |p: Pos2| {
            let n = to_ndc(p);
            let un = |z: f32| {
                let v = gl_scene::transform(&inv, [n[0], n[1], z]);
                [v[0] / v[3], v[1] / v[3], v[2] / v[3]]
            };
            let (a, b) = (un(-1.0), un(1.0));
            let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-9);
            (a, [d[0] / l, d[1] / l, d[2] / l])
        };

        // Modal transform.
        if let Some(op) = self.op.clone() {
            let cur = hover.or(self.last_pointer).unwrap_or(op.start);
            let typed: Option<f32> = op.typed.parse().ok();
            let m;
            let ps = project(op.pivot).unwrap_or(rect.center());
            let snap = mods.ctrl;
            match op.kind {
                OpKind::Grab => {
                    // Mouse movement in the plane through the pivot facing the camera.
                    let c = gl_scene::transform(&mvp, op.pivot);
                    let depth = c[2] / c[3];
                    let un = |p: Pos2| {
                        let n = to_ndc(p);
                        let v = gl_scene::transform(&inv, [n[0], n[1], depth]);
                        [v[0] / v[3], v[1] / v[3], v[2] / v[3]]
                    };
                    let (a, b) = (un(op.start), un(cur));
                    let mut d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                    if let Some(ax) = op.axis {
                        // Along the axis: project the mouse ray onto it.
                        for k in 0..3 {
                            if k != ax {
                                d[k] = 0.0;
                            }
                        }
                        if d[ax].abs() < 1e-3 {
                            let len = (b[0] - a[0]).hypot(b[2] - a[2]).max((b[1] - a[1]).abs());
                            let sgn = if (cur.x - op.start.x) - (cur.y - op.start.y) >= 0.0 { 1.0 } else { -1.0 };
                            d[ax] = len * sgn;
                        }
                    }
                    if snap {
                        d = d.map(|v| (v / 10.0).round() * 10.0);
                    }
                    if let Some(t) = typed {
                        d = [0.0; 3];
                        d[op.axis.unwrap_or(0)] = t;
                    }
                    m = meshops::trs(d, [0.0; 3], [1.0; 3]);
                    self.status = format!("Move {:.1} {:.1} {:.1}{}{}", d[0], d[1], d[2], op.axis.map_or(String::new(), |a| format!("  along {}", ["X", "Y", "Z"][a])), if op.typed.is_empty() { "   (type a number for exact values)".to_string() } else { format!("   typed: {}", op.typed) });
                }
                OpKind::Rotate => {
                    let a0 = (op.start.y - ps.y).atan2(op.start.x - ps.x);
                    let a1 = (cur.y - ps.y).atan2(cur.x - ps.x);
                    let mut deg = (a1 - a0).to_degrees();
                    if snap {
                        deg = (deg / 15.0).round() * 15.0;
                    }
                    if let Some(t) = typed {
                        deg = t;
                    }
                    let ax = op.axis.unwrap_or(1);
                    let mut r = [0.0; 3];
                    r[ax] = -deg;
                    let p = op.pivot;
                    m = meshops::mat_mul(&meshops::trs(p, r, [1.0; 3]), &meshops::trs([-p[0], -p[1], -p[2]], [0.0; 3], [1.0; 3]));
                    self.status = format!("Rotate {:.1}° around {}", deg, ["X", "Y", "Z"][ax]);
                }
                OpKind::Scale => {
                    let d0 = (op.start - ps).length().max(1.0);
                    let mut f = (cur - ps).length() / d0;
                    if snap {
                        f = (f * 10.0).round() / 10.0;
                    }
                    if let Some(t) = typed {
                        f = t;
                    }
                    let s = match op.axis {
                        Some(ax) => {
                            let mut s = [1.0; 3];
                            s[ax] = f;
                            s
                        }
                        None => [f; 3],
                    };
                    let p = op.pivot;
                    m = meshops::mat_mul(&meshops::trs(p, [0.0; 3], s), &meshops::trs([-p[0], -p[1], -p[2]], [0.0; 3], [1.0; 3]));
                    self.status = format!("Scale {:.2}{}", f, op.axis.map_or(String::new(), |a| format!(" on {}", ["X", "Y", "Z"][a])));
                }
            }
            self.asset = op.before.clone();
            self.apply_to_selection(&m);
            let confirm = resp.clicked_by(egui::PointerButton::Primary) || ui.input(|i| i.key_pressed(egui::Key::Enter));
            let cancel = resp.clicked_by(egui::PointerButton::Secondary) || ui.input(|i| i.key_pressed(egui::Key::Escape));
            if cancel {
                self.asset = op.before.clone();
                self.op = None;
                self.status = "Cancelled".into();
            } else if confirm {
                self.undo.push(op.before.clone());
                self.redo.clear();
                self.op = None;
            }
        } else {
            // Selection: click picks, drag draws a box.
            if resp.drag_started_by(egui::PointerButton::Primary) {
                self.box_start = ui.input(|i| i.pointer.press_origin());
            }
            if let (Some(a), Some(b)) = (self.box_start, hover)
                && resp.dragged_by(egui::PointerButton::Primary)
            {
                ui.painter().rect_stroke(Rect::from_two_pos(a, b), 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Middle);
            }
            if resp.drag_stopped_by(egui::PointerButton::Primary)
                && let (Some(a), Some(b)) = (self.box_start.take(), hover)
            {
                let (na, nb) = (to_ndc(a), to_ndc(b));
                let r = [na[0].min(nb[0]), na[1].min(nb[1]), na[0].max(nb[0]), na[1].max(nb[1])];
                if !mods.shift {
                    match self.mode {
                        Mode::Object => self.sel.clear(),
                        Mode::Edit => self.sel_verts.clear(),
                    }
                }
                match self.mode {
                    Mode::Object => {
                        for (i, p) in self.asset.parts.iter().enumerate() {
                            if p.visible && !meshops::select_by_box(&p.mesh, &mvp, r).is_empty() {
                                self.sel.insert(i);
                                if self.active != Some(i) {
                                    self.sel_verts.clear();
                                }
                                self.active = Some(i);
                            }
                        }
                    }
                    Mode::Edit => {
                        if let Some(i) = self.active {
                            let mut hits = meshops::select_by_box(&self.asset.parts[i].mesh, &mvp, r);
                            if !self.xray {
                                hits.retain(|&v| self.visible_vertex(i, v, &mvp, &ray, &project));
                            }
                            self.sel_verts.extend(hits);
                        }
                    }
                }
            }
            if resp.clicked_by(egui::PointerButton::Primary)
                && let Some(p) = hover
            {
                let (o, d) = ray(p);
                match self.mode {
                    Mode::Object => {
                        let hit = self.asset.parts.iter().enumerate().filter(|(_, q)| q.visible).filter_map(|(i, q)| meshops::pick_face(&q.mesh, o, d).map(|(_, t)| (i, t))).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                        if !mods.shift {
                            self.sel.clear();
                        }
                        if let Some((i, _)) = hit {
                            if mods.shift && self.sel.contains(&i) {
                                self.sel.remove(&i);
                            } else {
                                self.sel.insert(i);
                            }
                            if self.active != Some(i) {
                                self.sel_verts.clear();
                            }
                            self.active = Some(i);
                        }
                    }
                    Mode::Edit => {
                        if let Some(i) = self.active {
                            let hit = meshops::pick_vertex(&self.asset.parts[i].mesh, &mvp, to_ndc(p), 12.0 / rect.width() * 2.0);
                            if !mods.shift {
                                self.sel_verts.clear();
                            }
                            if let Some(v) = hit {
                                if mods.shift && self.sel_verts.contains(&v) {
                                    self.sel_verts.remove(&v);
                                } else {
                                    self.sel_verts.insert(v);
                                }
                            }
                        }
                    }
                }
            }
        }

        // Draw.
        {
            let mut g = self.gl.lock().unwrap();
            g.extras = self.part_meshes(&mut g);
        }
        let eye = self.cam.eye_world();
        let gl = self.gl.clone();
        let cb = egui_glow::CallbackFn::new(move |info, p| {
            let mut s = gl.lock().unwrap();
            unsafe { s.paint(p.gl(), &info, &mvp, eye, [0.16, 0.17, 0.19, 1.0]) };
        });
        ui.painter().add(egui::PaintCallback { rect, callback: Arc::new(cb) });
        let painter = ui.painter_at(rect);
        if self.show_grid {
            let (lo, hi) = self.asset.bounds();
            let ext = ((hi[0] - lo[0]).max(hi[2] - lo[2]).max(200.0) * 1.5 / 50.0).ceil() * 50.0;
            let step = if ext > 2000.0 { 250.0 } else if ext > 500.0 { 50.0 } else { 10.0 };
            let n = (ext / step) as i32;
            for k in -n..=n {
                let v = k as f32 * step;
                let col = if k == 0 { Color32::from_rgba_unmultiplied(120, 120, 120, 160) } else { Color32::from_white_alpha(28) };
                if let (Some(a), Some(b)) = (project([v, 0.0, -ext]), project([v, 0.0, ext])) {
                    painter.line_segment([a, b], Stroke::new(1.0, col));
                }
                if let (Some(a), Some(b)) = (project([-ext, 0.0, v]), project([ext, 0.0, v])) {
                    painter.line_segment([a, b], Stroke::new(1.0, col));
                }
            }
            // Axes.
            for (ax, col) in [([200.0, 0.0, 0.0], Color32::from_rgb(230, 70, 70)), ([0.0, 200.0, 0.0], Color32::from_rgb(90, 220, 90)), ([0.0, 0.0, 200.0], Color32::from_rgb(80, 140, 255))] {
                if let (Some(a), Some(b)) = (project([0.0; 3]), project(ax)) {
                    painter.line_segment([a, b], Stroke::new(2.0, col));
                }
            }
        }
        // Wireframe of the selected parts (Object) or the active part (Edit), and vertices.
        let wire_parts: Vec<usize> = match self.mode {
            Mode::Object => self.sel.iter().copied().collect(),
            Mode::Edit => self.active.into_iter().collect(),
        };
        if self.show_wire {
            for i in wire_parts {
                let p = &self.asset.parts[i];
                if p.mesh.indices.len() / 3 > 40000 {
                    continue;
                }
                let sp: Vec<Option<Pos2>> = p.mesh.positions.iter().map(|v| project(*v)).collect();
                let col = if self.mode == Mode::Edit { Color32::from_rgba_unmultiplied(20, 20, 20, 150) } else { Color32::from_rgba_unmultiplied(255, 170, 60, 110) };
                for f in p.mesh.indices.chunks_exact(3) {
                    for (a, b) in [(f[0], f[1]), (f[1], f[2]), (f[2], f[0])] {
                        if let (Some(Some(x)), Some(Some(y))) = (sp.get(a as usize), sp.get(b as usize)) {
                            painter.line_segment([*x, *y], Stroke::new(0.6, col));
                        }
                    }
                }
            }
        }
        if self.mode == Mode::Edit
            && let Some(i) = self.active
        {
            let p = &self.asset.parts[i];
            if p.mesh.positions.len() < 60000 {
                for (k, v) in p.mesh.positions.iter().enumerate() {
                    if let Some(s) = project(*v) {
                        let sel = self.sel_verts.contains(&(k as u32));
                        painter.circle_filled(s, if sel { 3.0 } else { 1.8 }, if sel { Color32::from_rgb(255, 160, 40) } else { Color32::from_gray(20) });
                    }
                }
            }
        }
        if let Some(op) = &self.op {
            if let Some(ps) = project(op.pivot) {
                painter.circle_stroke(ps, 6.0, Stroke::new(1.5, Color32::WHITE));
                if let Some(h) = hover {
                    painter.line_segment([ps, h], Stroke::new(1.0, Color32::from_white_alpha(90)));
                }
            }
            if let Some(ax) = op.axis {
                let p = op.pivot;
                let mut a = p;
                let mut b = p;
                a[ax] -= 5000.0;
                b[ax] += 5000.0;
                if let (Some(x), Some(y)) = (project(a), project(b)) {
                    painter.line_segment([x, y], Stroke::new(1.5, [Color32::from_rgb(230, 70, 70), Color32::from_rgb(90, 220, 90), Color32::from_rgb(80, 140, 255)][ax]));
                }
            }
        }
        let mode = match self.mode {
            Mode::Object => "Object mode",
            Mode::Edit => "Edit mode (vertices)",
        };
        painter.text(rect.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, mode, FontId::proportional(14.0), Color32::from_gray(230));
        painter.text(rect.left_bottom() + Vec2::new(10.0, -8.0), Align2::LEFT_BOTTOM, "G move · R rotate · S scale (X/Y/Z lock, Ctrl snap) · Tab edit · drag box select · middle/right drag orbit · wheel zoom · F frame", FontId::proportional(12.0), Color32::from_gray(190));
        if self.asset.parts.is_empty() {
            painter.text(rect.center(), Align2::CENTER_CENTER, "Import a 3D model (File), open one on the left, or Add a shape", FontId::proportional(16.0), Color32::from_gray(170));
        }
        ui.ctx().request_repaint();
    }

    /// Whether a vertex is in front (not hidden by the part itself) for X-ray-off selection.
    fn visible_vertex(&self, i: usize, v: u32, mvp: &Mat4, ray: &dyn Fn(Pos2) -> ([f32; 3], [f32; 3]), project: &dyn Fn([f32; 3]) -> Option<Pos2>) -> bool {
        let m = &self.asset.parts[i].mesh;
        let Some(p) = m.positions.get(v as usize) else { return false };
        let Some(s) = project(*p) else { return false };
        let (o, d) = ray(s);
        let dist = ((p[0] - o[0]).powi(2) + (p[1] - o[1]).powi(2) + (p[2] - o[2]).powi(2)).sqrt();
        let _ = mvp;
        match meshops::pick_face(m, o, d) {
            Some((_, t)) => t >= dist - dist * 0.01 - 0.5,
            None => true,
        }
    }
}

impl Default for ModelStudio {
    fn default() -> Self {
        Self::new()
    }
}
