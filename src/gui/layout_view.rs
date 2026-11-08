//! The custom map editor: paint surfaces, ground textures and heights on a canvas, place
//! objectives and draw minion lanes, with symmetry and undo. A live preview of the built map
//! is rendered in 3D (and in 2D with "Game look").

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use lol_map_studio::layout::{CELL, CustomProp, GroundLayer, Lane, Layout, MAX_LAYERS, ObjKind, Rules, SculptMode, Surface, Symmetry, TurretTier};
use lol_map_studio::props::PropLibrary;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneryMode {
    Place,
    Scatter,
    Erase,
    /// A row of props along a dragged line.
    Path,
}

/// Average colour of a theme texture (for the canvas), cached.
fn texture_color(cache: &mut HashMap<String, [u8; 3]>, r: &str) -> [u8; 3] {
    if let Some(c) = cache.get(r) {
        return *c;
    }
    let c = lol_map_studio::layout_build::resolve_texture(r).ok().and_then(|(_, b)| lol_map_studio::formats::dds::average_color(&b)).unwrap_or([110, 130, 90]);
    cache.insert(r.to_string(), c);
    c
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LTool {
    Select,
    Brush,
    Rect,
    Ellipse,
    Polygon,
    Fill,
    Texture,
    Sculpt,
    Objective,
    Lane,
    Line,
    Scenery,
    /// Particle effects and lights.
    Effects,
    /// Distances and game ranges.
    Measure,
}

impl LTool {
    pub const ALL: [(LTool, &'static str, &'static str); 14] = [
        (LTool::Select, "Select  V", "Select and move objectives and props (drag). Del removes."),
        (LTool::Brush, "Brush  B", "Paint the chosen surface"),
        (LTool::Line, "Line  L", "Drag a straight line of the surface (walls, paths)"),
        (LTool::Rect, "Rectangle  R", "Drag a rectangle of the surface"),
        (LTool::Ellipse, "Ellipse  E", "Drag an ellipse of the surface"),
        (LTool::Polygon, "Polygon  P", "Click corners; double-click or Enter to fill, Esc to cancel"),
        (LTool::Fill, "Fill  F", "Fill a connected area with the surface"),
        (LTool::Texture, "Texture  T", "Paint the ground texture layers"),
        (LTool::Sculpt, "Sculpt  H", "Raise, lower, flatten or smooth the ground"),
        (LTool::Objective, "Objectives  O", "Place nexus, inhibitors, turrets, spawn points, fountain turrets, shops and minion spawns"),
        (LTool::Lane, "Lanes  N", "Draw a minion lane from the blue base to the red base"),
        (LTool::Scenery, "Scenery  G", "Place, scatter or erase trees, rocks and other props"),
        (LTool::Effects, "Effects  X", "Place particle effects (fires, glows, water) and lights"),
        (LTool::Measure, "Measure  M", "Drag to measure distances; see turret, fountain and vision ranges"),
    ];
}

pub struct LayoutDoc {
    pub layout: Layout,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub rev: u64,
    undo: Vec<Layout>,
    redo: Vec<Layout>,
    stroke_open: bool,
}

impl LayoutDoc {
    pub fn new(layout: Layout, path: Option<PathBuf>) -> Self {
        Self { layout, path, dirty: false, rev: 1, undo: Vec::new(), redo: Vec::new(), stroke_open: false }
    }

    /// Records the state before an edit (once per stroke).
    pub fn begin(&mut self) {
        if !self.stroke_open {
            self.undo.push(self.layout.clone());
            if self.undo.len() > 30 {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.stroke_open = true;
        }
        self.dirty = true;
        self.rev += 1;
    }

    pub fn end(&mut self) {
        self.stroke_open = false;
    }

    pub fn undo(&mut self) -> bool {
        self.end();
        match self.undo.pop() {
            Some(l) => {
                self.redo.push(std::mem::replace(&mut self.layout, l));
                self.rev += 1;
                self.dirty = true;
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.end();
        match self.redo.pop() {
            Some(l) => {
                self.undo.push(std::mem::replace(&mut self.layout, l));
                self.rev += 1;
                self.dirty = true;
                true
            }
            None => false,
        }
    }

    pub fn title(&self) -> String {
        format!("{}{}", self.layout.name, if self.dirty { " *" } else { "" })
    }
}

pub struct LayoutView {
    pub tool: LTool,
    pub surface: Surface,
    pub radius: f32,
    pub square: bool,
    pub layer: usize,
    pub strength: f32,
    pub sculpt: SculptMode,
    pub obj_kind: ObjKind,
    pub team: u8,
    pub lane: Lane,
    pub tier: TurretTier,
    pub selected: Option<u32>,
    pub center: [f32; 2],
    pub scale: f32,
    fitted: bool,
    canvas: Option<(TextureHandle, u64)>,
    poly: Vec<[f32; 2]>,
    lane_pts: Vec<[f32; 2]>,
    last_paint: Option<[f32; 2]>,
    flatten_to: Option<f32>,
    pub game_look: bool,
    pub show_grid: bool,
    pub status: Option<String>,
    /// A placed prop to open in the Model Studio (set by the properties panel).
    pub open_in_studio: Option<String>,
    /// Further selected items (box select, Ctrl+click), besides the single selection.
    pub multi: Vec<lol_map_studio::layout::ItemRef>,
    pub clipboard: lol_map_studio::layout::Clipboard,
    /// Placed props lean with the ground's slope.
    pub align_slope: bool,
    left_tab: u8,
    outliner_search: String,
    right_tab: u8,
    show_server_settings: bool,
    /// Objects waiting to be placed (a prefab or a paste), shown at the cursor.
    pub placing: Option<lol_map_studio::layout::Clipboard>,
    pub show_ranges: bool,
    pub show_pathing: bool,
    genopt: lol_map_studio::worldtools::GenOptions,
    prefab_name: String,
    hm_range: [f32; 2],
    hm_walls: f32,
    rand_scale: [f32; 2],
    /// Values of the map the settings come from (path, id -> value).
    settings_src: Option<(String, HashMap<String, String>)>,
    /// Path mode: spacing as a multiple of the prop's width, and extra turn.
    pub path_gap: f32,
    pub path_turn: f32,
    /// Path mode follows the drawn stroke (else a straight line).
    pub path_free: bool,
    /// The stroke being drawn.
    pub path_line: Vec<[f32; 2]>,
    /// The 3D view should look at this point (outliner double-click).
    pub focus_on: Option<[f32; 2]>,
    moving: Option<u32>,
    moving_prop: Option<u32>,
    pub fountain: bool,
    pub scenery: SceneryMode,
    pub prop_key: Option<String>,
    pub prop_cat: String,
    pub prop_search: String,
    pub prop_scale: [f32; 2],
    pub random_turn: bool,
    pub scatter_spacing: f32,
    pub selected_prop: Option<u32>,
    pub line_width: f32,
    pub show_objectives: bool,
    pub show_lanes: bool,
    pub show_props: bool,
    colors: HashMap<String, [u8; 3]>,
    thumbs: HashMap<String, TextureHandle>,
    seed: u32,
    import: Option<CustomProp>,
    /// Effects tool: lights instead of particle effects.
    pub fx_light: bool,
    pub fx_name: Option<String>,
    pub fx_search: String,
    pub fx_lift: f32,
    pub light_color: [u8; 3],
    pub light_inner: f32,
    pub light_outer: f32,
    pub light_intensity: f32,
    /// Selected effect or light (their ids are unique across both).
    pub selected_fx: Option<u32>,
    /// Particle effect names from DATA/Particles (set by the app).
    pub effect_names: Vec<String>,
    /// Where a press-drag-release placement started (kept until the release).
    drag_anchor: Option<[f32; 2]>,
    /// Opened maps: walls and brush as imported, and the overlay of painted differences.
    base_was: Option<(String, std::sync::Arc<Vec<u8>>)>,
    diff_tex: Option<(TextureHandle, u64)>,
}

impl Default for LayoutView {
    fn default() -> Self {
        Self {
            tool: LTool::Brush,
            surface: Surface::Wall,
            radius: 250.0,
            square: false,
            layer: 1,
            strength: 0.5,
            sculpt: SculptMode::Raise,
            obj_kind: ObjKind::Turret,
            team: 1,
            lane: Lane::Mid,
            tier: TurretTier::Outer,
            selected: None,
            center: [0.0, 0.0],
            scale: 0.05,
            fitted: false,
            canvas: None,
            poly: Vec::new(),
            lane_pts: Vec::new(),
            last_paint: None,
            flatten_to: None,
            game_look: false,
            show_grid: false,
            status: None,
            open_in_studio: None,
            multi: Vec::new(),
            clipboard: Default::default(),
            align_slope: false,
            left_tab: 0,
            right_tab: 0,
            show_server_settings: false,
            placing: None,
            show_ranges: false,
            show_pathing: false,
            genopt: Default::default(),
            prefab_name: "My prefab".into(),
            hm_range: [-100.0, 300.0],
            hm_walls: 0.6,
            rand_scale: [0.85, 1.2],
            settings_src: None,
            path_gap: 1.0,
            path_turn: 0.0,
            path_free: true,
            path_line: Vec::new(),
            outliner_search: String::new(),
            focus_on: None,
            moving: None,
            moving_prop: None,
            fountain: false,
            scenery: SceneryMode::Place,
            prop_key: None,
            prop_cat: "Trees".into(),
            prop_search: String::new(),
            prop_scale: [0.9, 1.15],
            random_turn: true,
            scatter_spacing: 220.0,
            selected_prop: None,
            line_width: 200.0,
            show_objectives: true,
            show_lanes: true,
            show_props: true,
            colors: HashMap::new(),
            thumbs: HashMap::new(),
            seed: 1,
            import: None,
            fx_light: false,
            fx_name: Some("Fire_Cauldron.troy".into()),
            fx_search: String::new(),
            fx_lift: 0.0,
            light_color: [255, 170, 80],
            light_inner: 20.0,
            light_outer: 600.0,
            light_intensity: 1.0,
            selected_fx: None,
            effect_names: Vec::new(),
            drag_anchor: None,
            base_was: None,
            diff_tex: None,
        }
    }
}

fn layer_color(i: usize) -> [f32; 3] {
    match i % 4 {
        1 => [128.0, 98.0, 64.0],
        2 => [150.0, 146.0, 136.0],
        3 => [196.0, 176.0, 120.0],
        _ => [96.0, 132.0, 74.0],
    }
}

fn category_color(c: &str) -> Color32 {
    match c {
        "Trees" => Color32::from_rgb(40, 150, 60),
        "Rocks" => Color32::from_rgb(150, 140, 130),
        "Plants & flowers" => Color32::from_rgb(200, 120, 200),
        "Lights" => Color32::from_rgb(255, 170, 60),
        "Statues & ruins" => Color32::from_rgb(190, 190, 220),
        "My props" => Color32::from_rgb(90, 200, 230),
        "Base & walls" => Color32::from_rgb(120, 170, 255),
        "Decals" => Color32::from_rgb(170, 150, 110),
        "Jungle" => Color32::from_rgb(110, 200, 120),
        _ => Color32::from_rgb(200, 180, 120),
    }
}

pub fn team_color(team: u8) -> Color32 {
    match team {
        1 => Color32::from_rgb(70, 150, 255),
        2 => Color32::from_rgb(255, 80, 70),
        _ => Color32::from_rgb(190, 120, 255),
    }
}

fn camp_color(k: ObjKind) -> Color32 {
    match k {
        ObjKind::BlueBuff => Color32::from_rgb(90, 170, 255),
        ObjKind::RedBuff => Color32::from_rgb(255, 110, 70),
        ObjKind::Dragon => Color32::from_rgb(255, 170, 40),
        ObjKind::Baron => Color32::from_rgb(200, 90, 255),
        _ => Color32::from_rgb(230, 230, 150),
    }
}

fn camp_short(k: ObjKind) -> &'static str {
    match k {
        ObjKind::BlueBuff => "Blue",
        ObjKind::RedBuff => "Red",
        ObjKind::Wolves => "Wolves",
        ObjKind::Wraiths => "Wraiths",
        ObjKind::Golems => "Golems",
        ObjKind::Wight => "Wight",
        ObjKind::Dragon => "Dragon",
        ObjKind::Baron => "Baron",
        _ => "",
    }
}

impl LayoutView {
    /// Every object on the map in a searchable tree: click selects, double-click looks at it.
    fn outliner_ui(&mut self, ui: &mut egui::Ui, doc: &mut LayoutDoc, lib: Option<&PropLibrary>) {
        use lol_map_studio::layout::ItemRef;
        ui.add(egui::TextEdit::singleline(&mut self.outliner_search).hint_text("Search objects…"));
        let q = self.outliner_search.to_ascii_lowercase();
        let l = &doc.layout;
        let sel = self.selection();
        let mut clicked: Option<(ItemRef, [f32; 2], bool, bool)> = None;
        let mut row = |ui: &mut egui::Ui, r: ItemRef, label: String, pos: [f32; 2]| {
            if !q.is_empty() && !label.to_ascii_lowercase().contains(&q) {
                return;
            }
            let resp = ui.selectable_label(sel.contains(&r), &label);
            if resp.clicked() {
                clicked = Some((r, pos, ui.input(|i| i.modifiers.ctrl), false));
            }
            if resp.double_clicked() {
                clicked = Some((r, pos, false, true));
            }
        };
        let side = |t: u8| match t {
            1 => "Blue",
            2 => "Red",
            _ => "Neutral",
        };
        egui::ScrollArea::vertical().id_salt("outliner").show(ui, |ui| {
            let groups: [(&str, Box<dyn Fn(&lol_map_studio::layout::Objective) -> bool>); 3] = [
                ("Structures", Box::new(|o| !o.kind.is_camp() && !o.kind.is_mode_object())),
                ("Jungle camps", Box::new(|o| o.kind.is_camp())),
                ("Game-mode objects", Box::new(|o| o.kind.is_mode_object())),
            ];
            for (title, f) in groups.iter() {
                let items: Vec<&lol_map_studio::layout::Objective> = l.objectives.iter().filter(|o| f(o)).collect();
                if items.is_empty() {
                    continue;
                }
                egui::CollapsingHeader::new(format!("{title} ({})", items.len())).default_open(!q.is_empty()).show(ui, |ui| {
                    for o in items {
                        let name = o.name.clone().unwrap_or_else(|| o.kind.label().to_string());
                        row(ui, ItemRef::Objective(o.id), format!("{name}  ·  {}", side(o.team)), o.pos);
                    }
                });
            }
            if !l.props.is_empty() {
                let mut cats: std::collections::BTreeMap<String, Vec<&lol_map_studio::layout::PropInstance>> = Default::default();
                for p in &l.props {
                    let c = lib.and_then(|lb| lb.get(&p.prop)).map_or("Other".to_string(), |d| d.category.clone());
                    cats.entry(c).or_default().push(p);
                }
                egui::CollapsingHeader::new(format!("Scenery ({})", l.props.len())).default_open(!q.is_empty()).show(ui, |ui| {
                    for (c, ps) in cats {
                        egui::CollapsingHeader::new(format!("{c} ({})", ps.len())).id_salt(format!("oc_{c}")).default_open(!q.is_empty()).show(ui, |ui| {
                            for p in ps.into_iter().take(500) {
                                let name = lib.and_then(|lb| lb.get(&p.prop)).map_or(p.prop.clone(), |d| d.name.clone());
                                row(ui, ItemRef::Prop(p.id), format!("{name}  #{}", p.id), p.pos);
                            }
                        });
                    }
                });
            }
            if !l.effects.is_empty() {
                egui::CollapsingHeader::new(format!("Effects ({})", l.effects.len())).default_open(!q.is_empty()).show(ui, |ui| {
                    for e in l.effects.iter().take(1000) {
                        row(ui, ItemRef::Effect(e.id), format!("{}  #{}", e.name.trim_end_matches(".troy"), e.id), e.pos);
                    }
                });
            }
            if !l.lights.is_empty() {
                egui::CollapsingHeader::new(format!("Lights ({})", l.lights.len())).default_open(!q.is_empty()).show(ui, |ui| {
                    for lt in l.lights.iter().take(1000) {
                        row(ui, ItemRef::Light(lt.id), format!("Light {:.0}u  #{}", lt.outer, lt.id), lt.pos);
                    }
                });
            }
            if !l.lanes.is_empty() {
                egui::CollapsingHeader::new(format!("Lanes ({})", l.lanes.len())).show(ui, |ui| {
                    for lp in &l.lanes {
                        ui.label(format!("{} lane: {} points", lp.lane.label(), lp.points.len()));
                    }
                });
            }
        });
        if let Some((r, pos, add, focus)) = clicked {
            if add {
                let mut s = self.selection();
                self.clear_selection();
                if let Some(k) = s.iter().position(|x| *x == r) {
                    s.remove(k);
                } else {
                    s.push(r);
                }
                self.multi = s;
            } else {
                self.clear_selection();
                match r {
                    ItemRef::Objective(id) => self.selected = Some(id),
                    ItemRef::Prop(id) => self.selected_prop = Some(id),
                    ItemRef::Effect(id) | ItemRef::Light(id) => self.selected_fx = Some(id),
                }
                if self.tool != LTool::Effects {
                    self.tool = if matches!(r, ItemRef::Effect(_) | ItemRef::Light(_)) { LTool::Effects } else { LTool::Select };
                }
            }
            if focus {
                self.focus_on = Some(pos);
                self.center = pos;
            }
        }
    }

    /// The map's look and behaviour settings (fog, clouds, gamma, shadows, grass, camera, gold,
    /// respawn, experience). Unset values show what the map (or the rules map) has.
    fn map_settings_ui(&mut self, ui: &mut egui::Ui, doc: &mut LayoutDoc, rules_source: &str) {
        use lol_map_studio::map_settings::{Kind, specs};
        let src = doc.layout.base.as_ref().map(|b| b.path.clone()).unwrap_or_else(|| rules_source.to_string());
        if self.settings_src.as_ref().is_none_or(|s| s.0 != src) {
            let mut vals = HashMap::new();
            if let Ok(p) = lol_map_studio::layout_base::base_project(&src) {
                for sp in specs() {
                    if let Some(v) = lol_map_studio::map_settings::read(&p, &sp) {
                        vals.insert(sp.id(), v);
                    }
                }
            }
            self.settings_src = Some((src.clone(), vals));
        }
        let defaults = self.settings_src.as_ref().map(|x| x.1.clone()).unwrap_or_default();
        ui.label(RichText::new("Settings the game reads from the map's own files. Blue values are yours; the rest come from the map. Settings marked (server) only change a real match.").weak());
        ui.checkbox(&mut self.show_server_settings, "Show gameplay settings (server)");
        let mut l = doc.layout.settings.clone();
        let mut group = "";
        for sp in specs() {
            if sp.server && !self.show_server_settings {
                continue;
            }
            if sp.group != group {
                group = sp.group;
                ui.add_space(4.0);
                ui.label(RichText::new(group).strong().size(15.0));
            }
            let id = sp.id();
            let set = l.contains_key(&id);
            let cur = l.get(&id).cloned().or_else(|| defaults.get(&id).cloned()).unwrap_or_else(|| sp.default.clone());
            let mut out: Option<String> = None;
            ui.horizontal(|ui| {
                let label = RichText::new(format!("{}{}", sp.label, if sp.server { "  (server)" } else { "" }));
                ui.label(if set { label.color(Color32::from_rgb(120, 180, 255)) } else { label }).on_hover_text(format!("{}\n{} · {}", sp.help, sp.file, sp.key));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if set && ui.small_button("Reset").on_hover_text("Back to the map's value").clicked() {
                        out = Some(String::new());
                    }
                    match sp.kind {
                        Kind::Float(a, b) => {
                            let mut v: f32 = cur.split_whitespace().next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
                            let speed = ((b - a) / 500.0).max(0.0001) as f64;
                            if ui.add(egui::DragValue::new(&mut v).range(a..=b).speed(speed).max_decimals(4)).changed() {
                                out = Some(format!("{v}"));
                            }
                        }
                        Kind::Int(a, b) => {
                            let mut v: i32 = cur.trim().parse::<f32>().map(|x| x as i32).unwrap_or(0);
                            if ui.add(egui::DragValue::new(&mut v).range(a..=b)).changed() {
                                out = Some(format!("{v}"));
                            }
                        }
                        Kind::Switch => {
                            let mut v = cur.trim().parse::<f32>().map(|x| x != 0.0).unwrap_or(cur.trim().eq_ignore_ascii_case("true"));
                            if ui.checkbox(&mut v, "").changed() {
                                out = Some(if v { "1".into() } else { "0".into() });
                            }
                        }
                        Kind::Color => {
                            let c: Vec<f32> = cur.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                            let mut rgb = [c.first().copied().unwrap_or(128.0) as u8, c.get(1).copied().unwrap_or(128.0) as u8, c.get(2).copied().unwrap_or(128.0) as u8];
                            if ui.color_edit_button_srgb(&mut rgb).changed() {
                                out = Some(format!("{} {} {}", rgb[0], rgb[1], rgb[2]));
                            }
                        }
                        Kind::Text => {
                            let mut v = cur.clone();
                            if ui.add(egui::TextEdit::singleline(&mut v).desired_width(150.0)).changed() {
                                out = Some(v);
                            }
                        }
                        Kind::Choice(opts) => {
                            egui::ComboBox::from_id_salt(&id).selected_text(cur.clone()).show_ui(ui, |ui| {
                                for o in opts {
                                    if ui.selectable_label(cur == *o, *o).clicked() {
                                        out = Some(o.to_string());
                                    }
                                }
                            });
                        }
                    }
                });
            });
            match out {
                Some(v) if v.is_empty() => {
                    l.remove(&id);
                }
                Some(v) => {
                    l.insert(id, v);
                }
                None => {}
            }
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if !l.is_empty() && ui.button("Reset all to the map's values").clicked() {
                l.clear();
            }
            ui.label(RichText::new(format!("{} changed", l.len())).weak());
        });
        if l != doc.layout.settings {
            doc.begin();
            doc.layout.settings = l;
            doc.end();
        }
    }

    /// Actions on the selected objects (Roblox Studio's align, Unreal's foliage variety, prefabs).
    fn selection_ui(&mut self, ui: &mut egui::Ui, doc: &mut LayoutDoc) {
        use lol_map_studio::worldtools as wt;
        let sel = self.selection();
        let sel: Vec<_> = sel.into_iter().filter(|r| doc.layout.item_pos(*r).is_some()).collect();
        if sel.is_empty() {
            return;
        }
        ui.heading(format!("Selection ({})", sel.len()));
        let mut change: Option<Box<dyn FnOnce(&mut lol_map_studio::layout::Layout)>> = None;
        if sel.len() > 1 {
            ui.horizontal(|ui| {
                ui.label("Align X");
                for (m, n) in [(0u8, "min"), (1, "centre"), (2, "max")] {
                    if ui.small_button(n).clicked() {
                        let s = sel.clone();
                        change = Some(Box::new(move |l| wt::align(l, &s, 0, m)));
                    }
                }
                ui.label("Z");
                for (m, n) in [(0u8, "min"), (1, "centre"), (2, "max")] {
                    if ui.small_button(n).clicked() {
                        let s = sel.clone();
                        change = Some(Box::new(move |l| wt::align(l, &s, 1, m)));
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Spread evenly");
                for (a, n) in [(0usize, "along X"), (1, "along Z")] {
                    if ui.small_button(n).clicked() {
                        let s = sel.clone();
                        change = Some(Box::new(move |l| wt::distribute(l, &s, a)));
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            ui.label("Random size");
            ui.add(egui::DragValue::new(&mut self.rand_scale[0]).speed(0.01).range(0.05..=10.0));
            ui.add(egui::DragValue::new(&mut self.rand_scale[1]).speed(0.01).range(0.05..=10.0));
            if ui.small_button("Randomize").on_hover_text("Random turn and size for the selected props").clicked() {
                let (s, r) = (sel.clone(), self.rand_scale);
                let seed = doc.rev as u32;
                change = Some(Box::new(move |l| wt::randomize(l, &s, true, r, seed)));
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Drop to ground").on_hover_text("No lift, no tilt").clicked() {
                let s = sel.clone();
                change = Some(Box::new(move |l| wt::drop_to_ground(l, &s)));
            }
            if ui.button("Mirror to other side").on_hover_text("Copy to the other team's side (map symmetry)").clicked() {
                let s = sel.clone();
                change = Some(Box::new(move |l| {
                    wt::mirror_items(l, &s);
                }));
            }
            if let Some(k) = self.prop_key.clone()
                && sel.iter().any(|r| matches!(r, lol_map_studio::layout::ItemRef::Prop(_)))
                && ui.button("Replace model").on_hover_text("Use the prop chosen in the Scenery tool").clicked()
            {
                let s = sel.clone();
                change = Some(Box::new(move |l| {
                    wt::replace_props(l, &s, &k);
                }));
            }
        });
        ui.horizontal(|ui| {
            let ids: Vec<u32> = sel.iter().map(|r| match r {
                lol_map_studio::layout::ItemRef::Prop(i) | lol_map_studio::layout::ItemRef::Objective(i) | lol_map_studio::layout::ItemRef::Effect(i) | lol_map_studio::layout::ItemRef::Light(i) => *i,
            }).collect();
            let locked = ids.iter().all(|i| doc.layout.locked.contains(i));
            if ui.button(if locked { "Unlock" } else { "Lock" }).on_hover_text("Locked objects cannot be picked in the viewport (outliner still works)").clicked() {
                change = Some(Box::new(move |l| {
                    if locked {
                        l.locked.retain(|i| !ids.contains(i));
                    } else {
                        for i in ids {
                            if !l.locked.contains(&i) {
                                l.locked.push(i);
                            }
                        }
                    }
                }));
            }
            if ui.button("Delete").clicked() {
                let s = sel.clone();
                change = Some(Box::new(move |l| {
                    for r in &s {
                        l.remove_item(*r);
                    }
                }));
                self.clear_selection();
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.prefab_name).desired_width(140.0));
            if ui.button("Save as prefab").on_hover_text("Reuse this group on any map (Scenery tool > Prefabs)").clicked() {
                let cb = doc.layout.copy_items(&sel);
                self.status = Some(match wt::save_prefab(&self.prefab_name, &cb) {
                    Ok(_) => format!("Prefab \"{}\" saved ({} objects)", self.prefab_name, cb.len()),
                    Err(e) => format!("Could not save the prefab: {e:#}"),
                });
            }
        });
        if let Some(f) = change {
            doc.begin();
            f(&mut doc.layout);
            doc.end();
        }
        ui.separator();
    }

    /// Everything selected: the single selections plus the multi-selection.
    pub fn selection(&self) -> Vec<lol_map_studio::layout::ItemRef> {
        use lol_map_studio::layout::ItemRef;
        let mut v = self.multi.clone();
        for r in [self.selected.map(ItemRef::Objective), self.selected_prop.map(ItemRef::Prop), self.selected_fx.map(ItemRef::Effect), self.selected_fx.map(ItemRef::Light)].into_iter().flatten() {
            if !v.contains(&r) {
                v.push(r);
            }
        }
        v
    }

    pub fn clear_selection(&mut self) {
        self.multi.clear();
        self.selected = None;
        self.selected_prop = None;
        self.selected_fx = None;
    }

    pub fn reset_view(&mut self) {
        self.fitted = false;
        self.canvas = None;
    }

    fn to_screen(&self, rect: Rect, x: f32, z: f32) -> Pos2 {
        let c = rect.center();
        Pos2::new(c.x + (x - self.center[0]) * self.scale, c.y - (z - self.center[1]) * self.scale)
    }

    fn to_world(&self, rect: Rect, p: Pos2) -> [f32; 2] {
        let c = rect.center();
        [self.center[0] + (p.x - c.x) / self.scale, self.center[1] - (p.y - c.y) / self.scale]
    }

    pub fn ortho(&self, rect: Rect) -> super::gl_scene::Mat4 {
        let (w, h) = (rect.width().max(1.0), rect.height().max(1.0));
        let (sx, sz) = (2.0 * self.scale / w, 2.0 * self.scale / h);
        let mut m = [0.0f32; 16];
        m[0] = sx;
        m[12] = -self.center[0] * sx;
        m[9] = sz;
        m[13] = -self.center[1] * sz;
        m[6] = -1.0 / 40000.0;
        m[15] = 1.0;
        m
    }

    /// The canvas for the Play-mode minimap (default layer colours).
    pub fn canvas_image_pub(l: &Layout) -> egui::ColorImage {
        let cols: Vec<[f32; 3]> = (0..MAX_LAYERS).map(layer_color).collect();
        Self::canvas_image(l, &cols)
    }

    fn canvas_image(l: &Layout, cols: &[[f32; 3]]) -> egui::ColorImage {
        let (nx, ny) = (l.nx, l.ny);
        let mut px = vec![0u8; nx * ny * 4];
        for j in 0..ny {
            for i in 0..nx {
                let k = j * nx + i;
                let s = l.surface_at(i, j);
                let mut c = s.color().map(|v| v as f32);
                if matches!(s, Surface::Ground) {
                    let mut g = [0.0f32; 3];
                    for (n, w) in l.paint[k].iter().enumerate() {
                        if *w == 0 {
                            continue;
                        }
                        let t = *w as f32 / 255.0;
                        let lc = cols.get(n).copied().unwrap_or([128.0; 3]).map(|v| v * 1.35);
                        for a in 0..3 {
                            g[a] += lc[a] * t;
                        }
                    }
                    c = g;
                }
                // Hill shading from sculpted heights.
                let h = l.height[k];
                let hx = if i + 1 < nx { l.height[k + 1] } else { h };
                let shade = (1.0 + (h - hx) * 0.01 + h * 0.0012).clamp(0.6, 1.4);
                let o = ((ny - 1 - j) * nx + i) * 4;
                for a in 0..3 {
                    px[o + a] = (c[a] * shade).clamp(0.0, 255.0) as u8;
                }
                px[o + 3] = 255;
            }
        }
        egui::ColorImage::from_rgba_unmultiplied([nx, ny], &px)
    }

    fn pick_objective(&self, l: &Layout, rect: Rect, p: Pos2) -> Option<u32> {
        l.objectives
            .iter()
            .map(|o| (o.id, self.to_screen(rect, o.pos[0], o.pos[1]).distance(p), (o.kind.radius() * self.scale).max(8.0)))
            .filter(|(_, d, r)| d <= r)
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|x| x.0)
    }

    pub fn canvas_ui(&mut self, ui: &mut egui::Ui, doc: &mut LayoutDoc, gl: &super::gl_scene::Shared, lib: Option<&PropLibrary>) {
        self.hotkeys(ui);
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        let size = doc.layout.size();
        if !self.fitted {
            self.center = [size[0] / 2.0, size[1] / 2.0];
            self.scale = (rect.width() / size[0]).min(rect.height() / size[1]) * 0.94;
            self.fitted = true;
        }
        // Pan / zoom.
        let hover = resp.hover_pos();
        if let Some(hp) = hover {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let f = zoom * (scroll * 0.0025).exp();
            if (f - 1.0).abs() > 1e-4 {
                let before = self.to_world(rect, hp);
                self.scale = (self.scale * f).clamp(0.005, 6.0);
                let after = self.to_world(rect, hp);
                self.center[0] += before[0] - after[0];
                self.center[1] += before[1] - after[1];
            }
        }
        if resp.dragged_by(egui::PointerButton::Middle) || resp.dragged_by(egui::PointerButton::Secondary) {
            let d = resp.drag_delta();
            self.center[0] -= d.x / self.scale;
            self.center[1] += d.y / self.scale;
        }
        let map_rect = Rect::from_two_pos(self.to_screen(rect, 0.0, 0.0), self.to_screen(rect, size[0], size[1]));
        painter.rect_filled(rect, 0.0, Color32::from_rgb(14, 15, 18));
        if self.game_look {
            let mvp = self.ortho(rect);
            let g = gl.clone();
            let eye = [self.center[0], 100000.0, self.center[1]];
            let cb = egui_glow::CallbackFn::new(move |info, p| {
                let mut s = g.lock().unwrap();
                unsafe { s.paint(p.gl(), &info, &mvp, eye, [0.055, 0.06, 0.07, 1.0]) };
            });
            painter.add(egui::PaintCallback { rect, callback: std::sync::Arc::new(cb) });
            // Opened maps: cells painted differently from the original (wall, brush, walkable).
            if let Some(base) = &doc.layout.base {
                if self.base_was.as_ref().is_none_or(|b| b.0 != base.path)
                    && let Ok(bp) = lol_map_studio::layout_base::base_project(&base.path)
                    && let Some(g) = bp.navgrid()
                {
                    self.base_was = Some((base.path.clone(), std::sync::Arc::new(lol_map_studio::layout_import::grid_surface(g, doc.layout.nx, doc.layout.ny))));
                }
                if let Some((_, was)) = &self.base_was
                    && self.diff_tex.as_ref().is_none_or(|d| d.1 != doc.rev)
                {
                    let (nx, ny) = (doc.layout.nx, doc.layout.ny);
                    let mut px = vec![0u8; nx * ny * 4];
                    for j in 0..ny {
                        for i in 0..nx {
                            let k = j * nx + i;
                            let (now, before) = (doc.layout.surface[k], was.get(k).copied().unwrap_or(doc.layout.surface[k]));
                            if now == before {
                                continue;
                            }
                            let c = if now == Surface::Wall as u8 || now == Surface::Void as u8 || now == Surface::DeepWater as u8 { [200, 120, 80, 200] } else if now == Surface::Brush as u8 { [60, 220, 90, 170] } else { [250, 240, 170, 150] };
                            let o = ((ny - 1 - j) * nx + i) * 4;
                            px[o..o + 4].copy_from_slice(&c);
                        }
                    }
                    let img = egui::ColorImage::from_rgba_unmultiplied([nx, ny], &px);
                    match &mut self.diff_tex {
                        Some((h, r)) => {
                            h.set(img, TextureOptions::NEAREST);
                            *r = doc.rev;
                        }
                        None => self.diff_tex = Some((ui.ctx().load_texture("layout-diff", img, TextureOptions::NEAREST), doc.rev)),
                    }
                }
                if let Some((t, _)) = &self.diff_tex {
                    painter.image(t.id(), map_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
            }
        } else {
            if self.canvas.as_ref().is_none_or(|c| c.1 != doc.rev) {
                let cols: Vec<[f32; 3]> = (0..MAX_LAYERS)
                    .map(|i| doc.layout.theme.ground.get(i).map(|g| texture_color(&mut self.colors, &g.texture).map(|v| v as f32)).unwrap_or(layer_color(i)))
                    .collect();
                let img = Self::canvas_image(&doc.layout, &cols);
                match &mut self.canvas {
                    Some((h, r)) => {
                        h.set(img, TextureOptions::NEAREST);
                        *r = doc.rev;
                    }
                    None => self.canvas = Some((ui.ctx().load_texture("layout", img, TextureOptions::NEAREST), doc.rev)),
                }
            }
            if let Some((t, _)) = &self.canvas {
                painter.image(t.id(), map_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
        }
        painter.rect_stroke(map_rect, 0.0, Stroke::new(1.0, Color32::from_gray(110)), StrokeKind::Outside);
        if self.show_grid {
            let step = 1000.0;
            let mut x = 0.0;
            while x <= size[0] {
                painter.line_segment([self.to_screen(rect, x, 0.0), self.to_screen(rect, x, size[1])], Stroke::new(0.5, Color32::from_white_alpha(30)));
                x += step;
            }
            let mut z = 0.0;
            while z <= size[1] {
                painter.line_segment([self.to_screen(rect, 0.0, z), self.to_screen(rect, size[0], z)], Stroke::new(0.5, Color32::from_white_alpha(30)));
                z += step;
            }
        }
        // Symmetry guide.
        let l = &doc.layout;
        let guide = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 50));
        match l.symmetry {
            Symmetry::Diagonal => {
                painter.line_segment([self.to_screen(rect, 0.0, size[1]), self.to_screen(rect, size[0], 0.0)], guide);
            }
            Symmetry::MirrorX => {
                painter.line_segment([self.to_screen(rect, size[0] / 2.0, 0.0), self.to_screen(rect, size[0] / 2.0, size[1])], guide);
            }
            Symmetry::MirrorZ => {
                painter.line_segment([self.to_screen(rect, 0.0, size[1] / 2.0), self.to_screen(rect, size[0], size[1] / 2.0)], guide);
            }
            Symmetry::Point => {
                painter.circle_stroke(self.to_screen(rect, size[0] / 2.0, size[1] / 2.0), 6.0, guide);
            }
            Symmetry::None => {}
        }
        // Props.
        if self.show_props {
            for pr in &l.props {
                let p = self.to_screen(rect, pr.pos[0], pr.pos[1]);
                if !rect.expand(30.0).contains(p) {
                    continue;
                }
                let (rad, col) = match lib.and_then(|lb| lb.get(&pr.prop)) {
                    Some(d) => (d.radius * pr.scale, category_color(&d.category)),
                    None => (60.0, Color32::from_rgb(220, 90, 220)),
                };
                let r = (rad * self.scale).max(2.5);
                painter.circle_filled(p, r, col.gamma_multiply(0.45));
                painter.circle_stroke(p, r, Stroke::new(1.0, col));
                if self.selected_prop == Some(pr.id) {
                    painter.circle_stroke(p, r + 4.0, Stroke::new(2.0, Color32::WHITE));
                    // The prop's front (+Z of its model), as the editor turns it.
                    let (sn, cs) = pr.yaw.to_radians().sin_cos();
                    let dir = (self.to_screen(rect, pr.pos[0] - sn * 100.0, pr.pos[1] + cs * 100.0) - p).normalized();
                    painter.arrow(p, dir * (r + 12.0), Stroke::new(2.0, Color32::WHITE));
                }
            }
        }
        // Effects (orange stars) and lights (their radius in their colour).
        if self.show_props {
            for lt in l.lights.iter().filter(|x| self.tool == LTool::Effects || self.selected_fx == Some(x.id)) {
                let p = self.to_screen(rect, lt.pos[0], lt.pos[1]);
                let c = Color32::from_rgb(lt.color[0], lt.color[1], lt.color[2]);
                painter.circle_stroke(p, (lt.outer * self.scale).max(4.0), Stroke::new(1.0, c.gamma_multiply(0.6)));
                painter.circle_filled(p, 4.0, c);
                if self.selected_fx == Some(lt.id) {
                    painter.circle_stroke(p, 8.0, Stroke::new(2.0, Color32::WHITE));
                }
            }
            for e in l.effects.iter().filter(|x| self.tool == LTool::Effects || self.selected_fx == Some(x.id)) {
                let p = self.to_screen(rect, e.pos[0], e.pos[1]);
                let c = Color32::from_rgb(255, 160, 60);
                for k in 0..4 {
                    let a = k as f32 * std::f32::consts::FRAC_PI_4;
                    let d = Vec2::new(a.cos(), a.sin()) * 6.0;
                    painter.line_segment([p - d, p + d], Stroke::new(1.5, c));
                }
                if self.selected_fx == Some(e.id) {
                    painter.circle_stroke(p, 9.0, Stroke::new(2.0, Color32::WHITE));
                }
            }
        }
        // Lanes.
        for lp in l.lanes.iter().filter(|_| self.show_lanes) {
            let pts: Vec<Pos2> = lp.points.iter().map(|p| self.to_screen(rect, p[0], p[1])).collect();
            painter.add(egui::Shape::line(pts.clone(), Stroke::new(3.0, Color32::from_rgba_unmultiplied(255, 230, 120, 170))));
            if let Some(first) = pts.first() {
                painter.text(*first + Vec2::new(6.0, -6.0), Align2::LEFT_BOTTOM, format!("{} lane", lp.lane.label()), FontId::proportional(12.0), Color32::from_rgb(255, 230, 120));
            }
        }
        // Objectives.
        for o in l.objectives.iter().filter(|_| self.show_objectives) {
            let p = self.to_screen(rect, o.pos[0], o.pos[1]);
            let r = (o.kind.radius() * self.scale).max(4.0);
            let col = team_color(o.team);
            match o.kind {
                ObjKind::Nexus => {
                    painter.circle_filled(p, r, col.gamma_multiply(0.55));
                    painter.circle_stroke(p, r, Stroke::new(2.0, col));
                    painter.text(p, Align2::CENTER_CENTER, "N", FontId::proportional((r * 0.9).clamp(9.0, 22.0)), Color32::WHITE);
                }
                ObjKind::Inhibitor => {
                    painter.circle_filled(p, r, col.gamma_multiply(0.45));
                    painter.circle_stroke(p, r, Stroke::new(2.0, col));
                }
                ObjKind::Turret => {
                    painter.rect_filled(Rect::from_center_size(p, Vec2::splat(r * 1.6)), 2.0, col);
                    painter.text(p, Align2::CENTER_CENTER, format!("{}", o.tier.code()), FontId::proportional(r.clamp(8.0, 14.0)), Color32::BLACK);
                }
                ObjKind::Fountain | ObjKind::SpawnPoint => {
                    painter.circle_stroke(p, r, Stroke::new(3.0, col));
                    painter.circle_filled(p, r * 0.3, col);
                    painter.text(p + Vec2::new(0.0, r + 2.0), Align2::CENTER_TOP, "spawn", FontId::proportional(10.0), col);
                }
                ObjKind::FountainTurret => {
                    painter.rect_filled(Rect::from_center_size(p, Vec2::splat(r * 1.6)), 2.0, col);
                    painter.text(p, Align2::CENTER_CENTER, "F", FontId::proportional(r.clamp(8.0, 14.0)), Color32::BLACK);
                }
                ObjKind::Shop => {
                    painter.rect_filled(Rect::from_center_size(p, Vec2::splat(r * 1.4)), 6.0, Color32::from_rgb(230, 190, 60));
                    painter.rect_stroke(Rect::from_center_size(p, Vec2::splat(r * 1.4)), 6.0, Stroke::new(2.0, col), StrokeKind::Outside);
                    painter.text(p, Align2::CENTER_CENTER, "$", FontId::proportional(r.clamp(9.0, 18.0)), Color32::BLACK);
                }
                ObjKind::MinionSpawn => {
                    let pts = vec![p + Vec2::new(0.0, -7.0), p + Vec2::new(7.0, 0.0), p + Vec2::new(0.0, 7.0), p + Vec2::new(-7.0, 0.0)];
                    painter.add(egui::Shape::convex_polygon(pts, col, Stroke::new(1.0, Color32::BLACK)));
                }
                k if k.is_camp() => {
                    // Camp area, one dot per monster, and a short name.
                    let cc = camp_color(k);
                    painter.circle_filled(p, r, cc.gamma_multiply(0.25));
                    painter.circle_stroke(p, r, Stroke::new(if k.is_epic() { 2.5 } else { 1.5 }, cc));
                    for m in lol_map_studio::layout_build::camp_positions(o) {
                        painter.circle_filled(self.to_screen(rect, m[0], m[1]), (40.0 * self.scale).max(2.5), cc);
                    }
                    if self.scale > 0.025 {
                        painter.text(p + Vec2::new(0.0, r + 2.0), Align2::CENTER_TOP, camp_short(k), FontId::proportional(10.0), cc);
                    }
                }
                _ => {}
            }
            if self.selected == Some(o.id) {
                painter.circle_stroke(p, r + 5.0, Stroke::new(2.0, Color32::WHITE));
            }
        }

        // Tools.
        let world = hover.map(|h| self.to_world(rect, h));
        let primary_down = resp.dragged_by(egui::PointerButton::Primary) || resp.clicked_by(egui::PointerButton::Primary);
        let pressed = resp.drag_started_by(egui::PointerButton::Primary) || resp.clicked_by(egui::PointerButton::Primary);
        let released = resp.drag_stopped() || resp.clicked();
        let press_world = ui.input(|i| i.pointer.press_origin()).map(|p| self.to_world(rect, p));
        let brushy = matches!(self.tool, LTool::Brush | LTool::Texture | LTool::Sculpt);
        if brushy && let Some(h) = hover {
            let r = self.radius * self.scale;
            if self.square && self.tool == LTool::Brush {
                painter.rect_stroke(Rect::from_center_size(h, Vec2::splat(r * 2.0)), 0.0, Stroke::new(1.5, Color32::YELLOW), StrokeKind::Middle);
            } else {
                painter.circle_stroke(h, r, Stroke::new(1.5, Color32::YELLOW));
            }
            if let Some(w) = world {
                for m in doc.layout.mirrored(w).into_iter().skip(1) {
                    painter.circle_stroke(self.to_screen(rect, m[0], m[1]), r, Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 0, 90)));
                }
            }
        }
        match self.tool {
            LTool::Measure => {
                if let (Some(a), Some(b)) = (press_world, world)
                    && primary_down
                {
                    let (pa, pb) = (self.to_screen(rect, a[0], a[1]), self.to_screen(rect, b[0], b[1]));
                    painter.line_segment([pa, pb], Stroke::new(2.0, Color32::WHITE));
                    let d = (b[0] - a[0]).hypot(b[1] - a[1]);
                    painter.text(pb + Vec2::new(10.0, -10.0), Align2::LEFT_BOTTOM, format!("{d:.0} units  ≈ {:.1} s walking", d / 345.0), FontId::proportional(14.0), Color32::WHITE);
                    painter.circle_stroke(pa, d * self.scale, Stroke::new(1.0, Color32::from_white_alpha(60)));
                }
            }
            LTool::Sculpt if self.sculpt == SculptMode::Ramp => {
                if let (Some(a), Some(b)) = (press_world, world) {
                    if primary_down {
                        painter.line_segment([self.to_screen(rect, a[0], a[1]), self.to_screen(rect, b[0], b[1])], Stroke::new((self.radius * 2.0 * self.scale).max(2.0), Color32::from_rgba_unmultiplied(255, 230, 90, 90)));
                    }
                    if released && (b[0] - a[0]).hypot(b[1] - a[1]) > 50.0 {
                        doc.begin();
                        lol_map_studio::worldtools::ramp(&mut doc.layout, a, b, self.radius * 2.0);
                        doc.end();
                    }
                }
            }
            LTool::Brush | LTool::Texture | LTool::Sculpt => {
                if primary_down && let Some(pp) = resp.interact_pointer_pos() {
                    let w = self.to_world(rect, pp);
                    doc.begin();
                    let from = self.last_paint.unwrap_or(w);
                    let dist = (w[0] - from[0]).hypot(w[1] - from[1]);
                    let steps = (dist / (self.radius * 0.35).max(CELL)).ceil().max(1.0) as usize;
                    if self.tool == LTool::Sculpt && self.sculpt == SculptMode::Flatten && self.flatten_to.is_none() {
                        self.flatten_to = doc.layout.cell_of(w[0], w[1]).map(|(i, j)| doc.layout.height[j * doc.layout.nx + i]);
                    }
                    for k in 1..=steps {
                        let t = k as f32 / steps as f32;
                        let (x, z) = (from[0] + (w[0] - from[0]) * t, from[1] + (w[1] - from[1]) * t);
                        match self.tool {
                            LTool::Brush => doc.layout.paint_surface(x, z, self.radius, self.square, self.surface),
                            LTool::Texture => doc.layout.paint_texture(x, z, self.radius, self.layer, self.strength * 0.4),
                            _ => {
                                if k == steps {
                                    doc.layout.sculpt(x, z, self.radius, self.strength * 18.0, self.sculpt, self.flatten_to.unwrap_or(0.0));
                                }
                            }
                        }
                    }
                    self.last_paint = Some(w);
                }
                if released {
                    self.last_paint = None;
                    self.flatten_to = None;
                    doc.end();
                }
            }
            LTool::Rect | LTool::Ellipse => {
                if let (Some(a), Some(b)) = (press_world, world)
                    && resp.dragged_by(egui::PointerButton::Primary)
                {
                    let r = Rect::from_two_pos(self.to_screen(rect, a[0], a[1]), self.to_screen(rect, b[0], b[1]));
                    if self.tool == LTool::Rect {
                        painter.rect_stroke(r, 0.0, Stroke::new(2.0, Color32::YELLOW), StrokeKind::Middle);
                    } else {
                        let pts: Vec<Pos2> = (0..48).map(|k| {
                            let t = k as f32 / 48.0 * std::f32::consts::TAU;
                            r.center() + Vec2::new(t.cos() * r.width() / 2.0, t.sin() * r.height() / 2.0)
                        }).collect();
                        painter.add(egui::Shape::closed_line(pts, Stroke::new(2.0, Color32::YELLOW)));
                    }
                }
                if resp.drag_stopped()
                    && let (Some(a), Some(b)) = (press_world, world)
                {
                    let poly: Vec<[f32; 2]> = if self.tool == LTool::Rect {
                        vec![[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]]
                    } else {
                        let (cx, cz, rx, rz) = ((a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (b[0] - a[0]).abs() / 2.0, (b[1] - a[1]).abs() / 2.0);
                        (0..64).map(|k| {
                            let t = k as f32 / 64.0 * std::f32::consts::TAU;
                            [cx + t.cos() * rx, cz + t.sin() * rz]
                        }).collect()
                    };
                    doc.begin();
                    doc.layout.fill_polygon(&poly, self.surface);
                    doc.end();
                }
            }
            LTool::Polygon => {
                if resp.clicked_by(egui::PointerButton::Primary) && let Some(w) = world {
                    self.poly.push(w);
                }
                let finish = resp.double_clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                if finish && self.poly.len() >= 3 {
                    doc.begin();
                    let poly = std::mem::take(&mut self.poly);
                    doc.layout.fill_polygon(&poly, self.surface);
                    doc.end();
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.poly.clear();
                }
                let mut pts: Vec<Pos2> = self.poly.iter().map(|p| self.to_screen(rect, p[0], p[1])).collect();
                if let Some(h) = hover && !pts.is_empty() {
                    pts.push(h);
                }
                if pts.len() >= 2 {
                    painter.add(egui::Shape::closed_line(pts.clone(), Stroke::new(2.0, Color32::YELLOW)));
                }
                for p in &pts {
                    painter.circle_filled(*p, 3.0, Color32::YELLOW);
                }
            }
            LTool::Line => {
                if let (Some(a), Some(b)) = (press_world, world)
                    && resp.dragged_by(egui::PointerButton::Primary)
                {
                    painter.line_segment([self.to_screen(rect, a[0], a[1]), self.to_screen(rect, b[0], b[1])], Stroke::new((self.line_width * self.scale).max(2.0), Color32::from_rgba_unmultiplied(255, 255, 0, 110)));
                    for m in doc.layout.mirrored(a).into_iter().skip(1).zip(doc.layout.mirrored(b).into_iter().skip(1)) {
                        painter.line_segment([self.to_screen(rect, m.0[0], m.0[1]), self.to_screen(rect, m.1[0], m.1[1])], Stroke::new((self.line_width * self.scale).max(2.0), Color32::from_rgba_unmultiplied(255, 255, 0, 50)));
                    }
                }
                if resp.drag_stopped()
                    && let (Some(a), Some(b)) = (press_world, world)
                {
                    doc.begin();
                    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
                    let steps = (len / (CELL * 0.7)).ceil().max(1.0) as usize;
                    for k in 0..=steps {
                        let t = k as f32 / steps as f32;
                        doc.layout.paint_surface(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, self.line_width / 2.0, false, self.surface);
                    }
                    doc.end();
                }
            }
            LTool::Effects => {
                if resp.clicked_by(egui::PointerButton::Primary)
                    && let (Some(w), Some(pp)) = (world, resp.interact_pointer_pos())
                {
                    // Clicking an existing effect or light selects it; elsewhere places a new one.
                    let near = |q: [f32; 2]| self.to_screen(rect, q[0], q[1]).distance(pp) < 12.0;
                    let hit = doc.layout.effects.iter().find(|e| near(e.pos)).map(|e| e.id).or_else(|| doc.layout.lights.iter().find(|l| near(l.pos)).map(|l| l.id));
                    if let Some(id) = hit {
                        self.selected_fx = Some(id);
                    } else {
                        doc.begin();
                        let id = if self.fx_light {
                            Some(doc.layout.add_light(w, self.fx_lift.max(60.0), self.light_color, self.light_inner, self.light_outer, self.light_intensity))
                        } else {
                            self.fx_name.clone().map(|n| doc.layout.add_effect(&n, w, self.fx_lift, 0.0))
                        };
                        doc.end();
                        self.selected_fx = id;
                    }
                }
            }
            LTool::Scenery => {
                if let Some(h) = hover {
                    let r = match self.scenery {
                        SceneryMode::Place => self.prop_key.as_ref().and_then(|k| lib.and_then(|lb| lb.get(k))).map_or(40.0, |d| d.radius * self.prop_scale[1]),
                        _ => self.radius,
                    };
                    painter.circle_stroke(h, (r * self.scale).max(3.0), Stroke::new(1.5, if self.scenery == SceneryMode::Erase { Color32::RED } else { Color32::from_rgb(120, 255, 140) }));
                }
                let candidates: Vec<String> = match (&self.prop_key, lib) {
                    (Some(k), _) if self.scenery == SceneryMode::Place => vec![k.clone()],
                    (_, Some(lb)) => {
                        let mut v: Vec<String> = lb.props.iter().filter(|p| PropLibrary::allowed(&p.key, &doc.layout.asset_maps) && p.category == self.prop_cat && (self.prop_search.is_empty() || p.name.to_ascii_lowercase().contains(&self.prop_search.to_ascii_lowercase()))).map(|p| p.key.clone()).collect();
                        if let Some(k) = &self.prop_key {
                            v = vec![k.clone()];
                        }
                        v
                    }
                    _ => Vec::new(),
                };
                if let Some(w) = world {
                    match self.scenery {
                        SceneryMode::Place => {
                            // The next prop's turn and size, shown before it is placed.
                            let next = self.seed.wrapping_add(1);
                            let mut yaw = if self.random_turn { 360.0 * lol_map_studio::layout_build::hash01(next, 7, 9) } else { 0.0 };
                            let sc = self.prop_scale[0] + (self.prop_scale[1] - self.prop_scale[0]) * lol_map_studio::layout_build::hash01(next, 8, 9);
                            // Press, drag and release: the drag direction becomes the prop's front.
                            if resp.drag_started_by(egui::PointerButton::Primary) {
                                self.drag_anchor = press_world.or(Some(w));
                            }
                            let dragging = resp.dragged_by(egui::PointerButton::Primary) || resp.drag_stopped_by(egui::PointerButton::Primary);
                            let anchor = if dragging { self.drag_anchor.unwrap_or(w) } else { w };
                            if dragging && self.to_screen(rect, w[0], w[1]).distance(self.to_screen(rect, anchor[0], anchor[1])) > 8.0 {
                                yaw = -(w[0] - anchor[0]).atan2(w[1] - anchor[1]).to_degrees();
                                painter.line_segment([self.to_screen(rect, anchor[0], anchor[1]), self.to_screen(rect, w[0], w[1])], Stroke::new(1.5, Color32::WHITE));
                            }
                            if let Some(def) = candidates.first().and_then(|k| lib.and_then(|lb| lb.get(k))) {
                                let mut spots = vec![(anchor, yaw, 1.0f32)];
                                if doc.layout.symmetry != Symmetry::None {
                                    spots.push((doc.layout.mirror_point(anchor), yaw + 180.0, 0.4));
                                }
                                for (q, y, a) in spots {
                                    let c = self.to_screen(rect, q[0], q[1]);
                                    let r = (def.radius * sc * self.scale).max(4.0);
                                    let col = category_color(&def.category).gamma_multiply(a);
                                    painter.circle_filled(c, r, col.gamma_multiply(0.3));
                                    painter.circle_stroke(c, r, Stroke::new(1.5, col));
                                    let (sn, cs) = y.to_radians().sin_cos();
                                    // The prop's front (+Z of its model) on screen.
                                    let dir = (self.to_screen(rect, q[0] - sn * 100.0, q[1] + cs * 100.0) - c).normalized();
                                    painter.arrow(c, dir * (r + 14.0), Stroke::new(2.0, Color32::WHITE.gamma_multiply(a)));
                                }
                            }
                            let place = resp.clicked_by(egui::PointerButton::Primary) || resp.drag_stopped_by(egui::PointerButton::Primary);
                            if place {
                                self.drag_anchor = None;
                                let w = anchor;
                                if let Some(k) = candidates.first() {
                                    self.seed = next;
                                    doc.begin();
                                    let id = doc.layout.add_prop(k, w, yaw, sc);
                                    if doc.layout.symmetry != Symmetry::None {
                                        let m = doc.layout.mirror_point(w);
                                        doc.layout.add_prop(k, m, (yaw + 180.0) % 360.0, sc);
                                    }
                                    doc.end();
                                    self.selected_prop = Some(id);
                                } else {
                                    self.status = Some("Choose a prop in the scenery list first.".into());
                                }
                            }
                        }
                        SceneryMode::Scatter => {
                            if primary_down && !candidates.is_empty() {
                                self.seed = self.seed.wrapping_add(17);
                                doc.begin();
                                doc.layout.scatter_props(&candidates, w[0], w[1], self.radius, self.scatter_spacing, self.prop_scale, self.seed);
                            }
                            if released {
                                doc.end();
                            }
                        }
                        SceneryMode::Erase => {
                            if primary_down {
                                doc.begin();
                                doc.layout.erase_props(w[0], w[1], self.radius);
                            }
                            if released {
                                doc.end();
                            }
                        }
                        SceneryMode::Path => {
                            if let (Some(a), Some(k)) = (press_world, candidates.first()) {
                                let def = lib.and_then(|lb| lb.get(k));
                                let step = def.map_or(200.0, |d| d.radius * 2.0 * self.prop_scale[1]) * self.path_gap;
                                if resp.drag_started_by(egui::PointerButton::Primary) {
                                    self.path_line = vec![a];
                                }
                                if self.path_free && self.path_line.last().is_some_and(|l| (l[0] - w[0]).hypot(l[1] - w[1]) > 12.0 / self.scale.max(1e-3)) {
                                    self.path_line.push(w);
                                }
                                let spots = if self.path_free {
                                    let mut line = self.path_line.clone();
                                    line.push(w);
                                    lol_map_studio::layout::line_spots(&line, step, self.path_turn)
                                } else {
                                    lol_map_studio::layout::path_spots(a, w, step, self.path_turn)
                                };
                                if resp.dragged_by(egui::PointerButton::Primary) {
                                    if self.path_free {
                                        let pts: Vec<Pos2> = self.path_line.iter().map(|q| self.to_screen(rect, q[0], q[1])).collect();
                                        painter.add(egui::Shape::line(pts, Stroke::new(1.5, Color32::WHITE)));
                                    }
                                    for (q, _) in &spots {
                                        painter.circle_stroke(self.to_screen(rect, q[0], q[1]), (step / 2.0 * self.scale).max(3.0), Stroke::new(1.0, Color32::from_rgb(120, 255, 140)));
                                    }
                                }
                                if resp.drag_stopped_by(egui::PointerButton::Primary) {
                                    doc.begin();
                                    for (q, y) in spots {
                                        doc.layout.add_prop(k, q, y, self.prop_scale[1]);
                                    }
                                    doc.end();
                                    self.path_line.clear();
                                }
                            }
                        }
                    }
                }
            }
            LTool::Fill => {
                if resp.clicked_by(egui::PointerButton::Primary) && let Some(w) = world {
                    doc.begin();
                    doc.layout.flood_fill(w[0], w[1], self.surface);
                    doc.end();
                }
            }
            LTool::Objective => {
                if resp.clicked_by(egui::PointerButton::Primary) && let Some(w) = world {
                    doc.begin();
                    let id = if self.fountain { doc.layout.add_fountain(self.team, w) } else { doc.layout.add_objective(self.obj_kind, self.team, self.lane, self.tier, w) };
                    doc.end();
                    self.selected = Some(id);
                }
            }
            LTool::Lane => {
                if resp.clicked_by(egui::PointerButton::Primary) && let Some(w) = world {
                    self.lane_pts.push(w);
                }
                let finish = resp.double_clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                if finish && self.lane_pts.len() >= 2 {
                    doc.begin();
                    let pts = std::mem::take(&mut self.lane_pts);
                    doc.layout.set_lane(self.lane, pts);
                    doc.end();
                    self.status = Some(format!("{} lane set. 'Place lane structures' adds its turrets, inhibitor and minion spawn.", self.lane.label()));
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.lane_pts.clear();
                }
                let mut pts: Vec<Pos2> = self.lane_pts.iter().map(|p| self.to_screen(rect, p[0], p[1])).collect();
                if let Some(h) = hover && !pts.is_empty() {
                    pts.push(h);
                }
                if pts.len() >= 2 {
                    painter.add(egui::Shape::line(pts.clone(), Stroke::new(3.0, Color32::from_rgb(255, 230, 120))));
                }
            }
            LTool::Select => {
                if pressed && let Some(pp) = ui.input(|i| i.pointer.press_origin()) {
                    self.selected = if self.show_objectives { self.pick_objective(&doc.layout, rect, pp) } else { None };
                    self.moving = self.selected;
                    self.selected_prop = None;
                    if self.selected.is_none() && self.show_props {
                        let w0 = self.to_world(rect, pp);
                        self.selected_prop = doc
                            .layout
                            .props
                            .iter()
                            .map(|p| (p.id, (p.pos[0] - w0[0]).hypot(p.pos[1] - w0[1]), lib.and_then(|lb| lb.get(&p.prop)).map_or(60.0, |d| d.radius * p.scale).max(12.0 / self.scale)))
                            .filter(|(_, d, r)| d <= r)
                            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                            .map(|x| x.0);
                        self.moving_prop = self.selected_prop;
                    }
                }
                if resp.dragged_by(egui::PointerButton::Primary)
                    && let Some(w) = world
                {
                    if let Some(id) = self.moving {
                        doc.begin();
                        doc.layout.move_objective(id, w);
                    } else if let Some(id) = self.moving_prop {
                        doc.begin();
                        if let Some(p) = doc.layout.props.iter_mut().find(|p| p.id == id) {
                            p.pos = w;
                        }
                    }
                }
                if released {
                    self.moving = None;
                    self.moving_prop = None;
                    doc.end();
                }
            }
        }
        if self.tool == LTool::Effects && ui.input(|i| i.key_pressed(egui::Key::Delete)) && !ui.ctx().egui_wants_keyboard_input()
            && let Some(id) = self.selected_fx.take()
        {
            doc.begin();
            doc.layout.effects.retain(|e| e.id != id);
            doc.layout.lights.retain(|x| x.id != id);
            doc.end();
        }
        if self.tool == LTool::Select && ui.input(|i| i.key_pressed(egui::Key::Delete)) && !ui.ctx().egui_wants_keyboard_input() {
            if let Some(id) = self.selected.take() {
                doc.begin();
                doc.layout.remove_objective(id);
                doc.end();
            } else if let Some(id) = self.selected_prop.take() {
                doc.begin();
                doc.layout.props.retain(|p| p.id != id);
                doc.end();
            }
        }
        if let Some(w) = world {
            let mut s = format!("x {:.0}  z {:.0}", w[0], w[1]);
            if let Some((i, j)) = doc.layout.cell_of(w[0], w[1]) {
                let k = j * doc.layout.nx + i;
                s += &format!("   {}   height {:+.0}", doc.layout.surface_at(i, j).label(), doc.layout.height[k]);
            }
            painter.text(rect.left_bottom() + Vec2::new(8.0, -8.0), Align2::LEFT_BOTTOM, s, FontId::monospace(12.0), Color32::from_gray(220));
        }
    }

    /// Left panel: tools and their options.
    pub fn hotkeys(&mut self, ui: &egui::Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let pick = ui.input(|i| {
            if i.modifiers.any() {
                return None;
            }
            use egui::Key::*;
            [(V, LTool::Select), (B, LTool::Brush), (L, LTool::Line), (R, LTool::Rect), (E, LTool::Ellipse), (P, LTool::Polygon), (F, LTool::Fill), (T, LTool::Texture), (H, LTool::Sculpt), (O, LTool::Objective), (N, LTool::Lane), (G, LTool::Scenery), (X, LTool::Effects), (M, LTool::Measure)]
                .into_iter()
                .find(|(k, _)| i.key_pressed(*k))
                .map(|x| x.1)
        });
        if let Some(t) = pick {
            self.tool = t;
            self.poly.clear();
            self.lane_pts.clear();
        }
        // [ and ] resize the brush.
        ui.input(|i| {
            if i.key_pressed(egui::Key::OpenBracket) {
                self.radius = (self.radius / 1.25).max(CELL);
            }
            if i.key_pressed(egui::Key::CloseBracket) {
                self.radius = (self.radius * 1.25).min(3000.0);
            }
        });
    }

    fn thumb(&mut self, ctx: &egui::Context, key: &str, make: impl FnOnce() -> Option<(usize, usize, Vec<u8>)>) -> Option<egui::TextureId> {
        if let Some(t) = self.thumbs.get(key) {
            return Some(t.id());
        }
        let (w, h, px) = make()?;
        let t = ctx.load_texture(key, egui::ColorImage::from_rgba_unmultiplied([w, h], &px), TextureOptions::LINEAR);
        let id = t.id();
        self.thumbs.insert(key.to_string(), t);
        Some(id)
    }

    fn texture_thumb(&mut self, ctx: &egui::Context, r: &str) -> Option<egui::TextureId> {
        let key = format!("tex:{r}");
        self.thumb(ctx, &key, || {
            let (_, b) = lol_map_studio::layout_build::resolve_texture(r).ok()?;
            let img = lol_map_studio::formats::dds::decode(&b, 64).ok()?;
            Some((img.width, img.height, img.pixels))
        })
    }

    pub fn tools_ui(&mut self, ui: &mut egui::Ui, doc: &mut LayoutDoc, lib: Option<&PropLibrary>) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.left_tab, 0, RichText::new("Tool").strong());
            ui.selectable_value(&mut self.left_tab, 1, RichText::new("Outliner").strong());
        });
        ui.separator();
        if self.left_tab == 1 {
            self.outliner_ui(ui, doc, lib);
            return;
        }
        ui.heading("Tools");
        egui::Grid::new("tools").num_columns(2).spacing([4.0, 4.0]).show(ui, |ui| {
            for (n, (t, label, tip)) in LTool::ALL.iter().enumerate() {
                if ui.add_sized([112.0, 26.0], egui::Button::selectable(self.tool == *t, *label)).on_hover_text(*tip).clicked() {
                    self.tool = *t;
                    self.poly.clear();
                    self.lane_pts.clear();
                }
                if n % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        ui.separator();
        match self.tool {
            LTool::Brush | LTool::Rect | LTool::Ellipse | LTool::Polygon | LTool::Fill | LTool::Line => {
                ui.label(RichText::new("Surface").strong());
                for s in Surface::ALL {
                    let c = s.color();
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(16.0, 16.0), Sense::hover());
                        ui.painter().rect_filled(r, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
                        ui.selectable_value(&mut self.surface, s, s.label());
                    });
                }
                if self.tool == LTool::Brush {
                    ui.add(egui::Slider::new(&mut self.radius, CELL..=2500.0).logarithmic(true).text("size  [ ]"));
                    ui.checkbox(&mut self.square, "Square brush");
                }
                if self.tool == LTool::Line {
                    ui.add(egui::Slider::new(&mut self.line_width, CELL..=1500.0).logarithmic(true).text("width"));
                }
            }
            LTool::Texture => {
                ui.label(RichText::new("Ground layers").strong());
                ui.label(RichText::new("Paint a layer onto the ground. Each area of the map mixes its four strongest layers.").weak());
                let ctx = ui.ctx().clone();
                let layers = doc.layout.theme.ground.clone();
                let mut remove = None;
                for (i, g) in layers.iter().enumerate() {
                    ui.horizontal(|ui| {
                        match self.texture_thumb(&ctx, &g.texture) {
                            Some(t) => {
                                ui.image((t, Vec2::splat(28.0)));
                            }
                            None => {
                                ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                            }
                        }
                        ui.selectable_value(&mut self.layer, i, &g.name);
                        if i > 0 && ui.small_button("✕").on_hover_text("Remove this layer (its paint goes back to the first layer)").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    doc.begin();
                    doc.layout.remove_layer(i);
                    doc.end();
                    self.layer = self.layer.min(doc.layout.theme.ground.len() - 1);
                    self.canvas = None;
                }
                ui.horizontal(|ui| {
                    if doc.layout.theme.ground.len() < MAX_LAYERS && ui.button("Add layer from a file…").on_hover_text("Your own .dds, .png or .jpg texture").clicked()
                        && let Some(p) = rfd::FileDialog::new().add_filter("Textures", &["dds", "png", "jpg", "jpeg"]).pick_file()
                    {
                        doc.begin();
                        let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Layer".into());
                        doc.layout.theme.ground.push(GroundLayer { name, texture: p.to_string_lossy().to_string() });
                        doc.end();
                        self.layer = doc.layout.theme.ground.len() - 1;
                    }
                });
                if doc.layout.theme.ground.len() < MAX_LAYERS {
                    ui.label(RichText::new("More layers can be added from the texture library in Look (right panel).").weak());
                }
                ui.add(egui::Slider::new(&mut self.radius, CELL..=2500.0).logarithmic(true).text("size  [ ]"));
                ui.add(egui::Slider::new(&mut self.strength, 0.05..=1.0).text("strength"));
            }
            LTool::Sculpt => {
                for (m, n) in [(SculptMode::Raise, "Raise"), (SculptMode::Lower, "Lower"), (SculptMode::Flatten, "Flatten"), (SculptMode::Smooth, "Smooth"), (SculptMode::Noise, "Noise (bumps)"), (SculptMode::Ramp, "Ramp (drag from low to high)")] {
                    ui.radio_value(&mut self.sculpt, m, n);
                }
                ui.add(egui::Slider::new(&mut self.radius, CELL..=2500.0).logarithmic(true).text(if self.sculpt == SculptMode::Ramp { "half width" } else { "size" }));
                ui.add(egui::Slider::new(&mut self.strength, 0.05..=1.0).text("strength"));
                ui.separator();
                ui.label(RichText::new("Generate terrain").strong());
                egui::Grid::new("gen").num_columns(2).show(ui, |ui| {
                    ui.label("Seed");
                    ui.add(egui::DragValue::new(&mut self.genopt.seed));
                    ui.end_row();
                    ui.label("Jungle walls");
                    ui.add(egui::Slider::new(&mut self.genopt.walls, 0.0..=0.8));
                    ui.end_row();
                    ui.label("Wall size");
                    ui.add(egui::Slider::new(&mut self.genopt.feature, 300.0..=4000.0).logarithmic(true));
                    ui.end_row();
                    ui.label("Brush");
                    ui.add(egui::Slider::new(&mut self.genopt.brush, 0.0..=0.5));
                    ui.end_row();
                    ui.label("Hills");
                    ui.add(egui::Slider::new(&mut self.genopt.hills, 0.0..=300.0));
                    ui.end_row();
                });
                ui.checkbox(&mut self.genopt.river, "River across the map");
                ui.checkbox(&mut self.genopt.keep_lanes, "Keep lanes and objectives clear");
                ui.horizontal(|ui| {
                    if ui.button("Generate").on_hover_text("Replaces walls, brush, rivers and heights (undo with Ctrl+Z)").clicked() {
                        doc.begin();
                        lol_map_studio::worldtools::generate(&mut doc.layout, &self.genopt);
                        doc.end();
                    }
                    if ui.button("New seed").clicked() {
                        self.genopt.seed = self.genopt.seed.wrapping_mul(1103515245).wrapping_add(12345) % 100000;
                    }
                });
                ui.separator();
                ui.label(RichText::new("Heightmap").strong());
                ui.horizontal(|ui| {
                    ui.label("Black / white =");
                    ui.add(egui::DragValue::new(&mut self.hm_range[0]).speed(5.0));
                    ui.add(egui::DragValue::new(&mut self.hm_range[1]).speed(5.0));
                });
                ui.add(egui::Slider::new(&mut self.hm_walls, 0.0..=1.01).text("walls above"));
                ui.horizontal(|ui| {
                    if ui.button("Import image…").on_hover_text("PNG/JPG/TGA/BMP: bright = high").clicked()
                        && let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "tga", "bmp"]).pick_file()
                    {
                        doc.begin();
                        match lol_map_studio::worldtools::import_heightmap(&mut doc.layout, &p, self.hm_range[0], self.hm_range[1], self.hm_walls) {
                            Ok(()) => self.status = Some(format!("Heightmap {} imported", p.display())),
                            Err(e) => self.status = Some(format!("{e:#}")),
                        }
                        doc.end();
                    }
                    if ui.button("Export…").clicked()
                        && let Some(p) = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name("heightmap.png").save_file()
                    {
                        let (img, lo, hi) = lol_map_studio::worldtools::export_heightmap(&doc.layout);
                        self.status = Some(match img.save(&p) {
                            Ok(()) => format!("Heightmap saved ({lo:.0} to {hi:.0} units)"),
                            Err(e) => format!("Could not save: {e}"),
                        });
                    }
                });
            }
            LTool::Measure => {
                ui.label(RichText::new("Drag on the ground to measure. Game ranges for reference:").weak());
                for (n, r) in [("Turret attack range", "775 (+ turret size)"), ("Champion vision", "1200"), ("Turret vision", "1095"), ("Fountain healing", "1100"), ("Flash", "400"), ("Experience range", "1600")] {
                    ui.label(format!("{n}: {r}"));
                }
                ui.checkbox(&mut self.show_ranges, "Show turret and fountain ranges on the map");
            }
            LTool::Objective => {
                ui.label(RichText::new("Place").strong());
                if ui.radio(self.fountain, "Fountain (spawn point + fountain turret + shop)").clicked() {
                    self.fountain = true;
                }
                for k in ObjKind::ALL {
                    if ui.radio(!self.fountain && self.obj_kind == k, k.label()).clicked() {
                        self.obj_kind = k;
                        self.fountain = false;
                    }
                }
                ui.label(RichText::new("Jungle camps").strong());
                for k in ObjKind::CAMPS {
                    if ui.radio(!self.fountain && self.obj_kind == k, RichText::new(k.label()).color(camp_color(k))).clicked() {
                        self.obj_kind = k;
                        self.fountain = false;
                    }
                }
                if ui.button("Add Summoner's Rift jungle").on_hover_text("All 14 camps at Map1's positions, scaled to this map").clicked() {
                    doc.begin();
                    doc.layout.classic_jungle();
                    doc.end();
                }
                ui.label(RichText::new("Game modes (Dominion / ARAM)").strong());
                for k in ObjKind::MODES {
                    if ui.radio(!self.fountain && self.obj_kind == k, k.label()).clicked() {
                        self.obj_kind = k;
                        self.fountain = false;
                    }
                }
                if self.obj_kind.is_mode_object() {
                    ui.label(RichText::new("The game server spawns these at their markers (Info_Point, Info_SpeedShrine, Info_HealthPack…); health relics are written as ARAM-style camps.").weak());
                }
                if self.obj_kind.is_epic() {
                    ui.label(RichText::new("Neutral: placed once, not mirrored.").weak());
                } else if self.obj_kind.is_camp() {
                    ui.label(RichText::new("Team = whose side the buffs belong to.").weak());
                }
                ui.horizontal(|ui| {
                    ui.label("Team");
                    ui.selectable_value(&mut self.team, 1, RichText::new("Blue").color(team_color(1)));
                    ui.selectable_value(&mut self.team, 2, RichText::new("Red").color(team_color(2)));
                });
                if self.obj_kind.has_lane() {
                    ui.horizontal(|ui| {
                        ui.label("Lane");
                        for l in Lane::ALL {
                            ui.selectable_value(&mut self.lane, l, l.label());
                        }
                    });
                }
                if self.obj_kind == ObjKind::Turret {
                    egui::ComboBox::from_label("Tier").selected_text(self.tier.label()).show_ui(ui, |ui| {
                        for t in TurretTier::ALL {
                            ui.selectable_value(&mut self.tier, t, t.label());
                        }
                    });
                }
                if doc.layout.symmetry != Symmetry::None {
                    ui.label(RichText::new("Symmetry places the other team's copy too.").weak());
                }
            }
            LTool::Lane => {
                ui.horizontal(|ui| {
                    ui.label("Lane");
                    for l in Lane::ALL {
                        ui.selectable_value(&mut self.lane, l, l.label());
                    }
                });
                ui.label(RichText::new("Click points from the blue base to the red base; double-click or Enter to finish.").weak());
                if ui.button("Place lane structures").on_hover_text("Adds a minion spawn, inhibitor and three turrets per lane on the blue side, mirrored for red").clicked() {
                    doc.begin();
                    doc.layout.auto_structures();
                    doc.end();
                }
            }
            LTool::Select => {
                ui.label(RichText::new("Click an objective or a prop to select it; drag to move. Del removes it.").weak());
            }
            LTool::Effects => {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.fx_light, false, "Particle effect");
                    ui.selectable_value(&mut self.fx_light, true, "Light");
                });
                ui.add(egui::Slider::new(&mut self.fx_lift, 0.0..=800.0).text("height above ground"));
                if self.fx_light {
                    ui.horizontal(|ui| {
                        ui.label("Colour");
                        ui.color_edit_button_srgb(&mut self.light_color);
                    });
                    ui.add(egui::Slider::new(&mut self.light_outer, 50.0..=3000.0).logarithmic(true).text("radius"));
                    ui.add(egui::Slider::new(&mut self.light_inner, 0.0..=1000.0).text("inner radius"));
                    ui.add(egui::Slider::new(&mut self.light_intensity, 0.0..=4.0).text("intensity"));
                    ui.label(RichText::new("Written to Light_Env.dat (and Light.dat) like Summoner's Rift's 374 lights.").weak());
                } else {
                    ui.add(egui::TextEdit::singleline(&mut self.fx_search).hint_text("Search 5800 effects… (fire, torch, glow, water)"));
                    if self.effect_names.is_empty() {
                        ui.label(RichText::new("Needs the game's DATA/Particles folder.").weak());
                    }
                    let q = self.fx_search.to_ascii_lowercase();
                    let list: Vec<&String> = self.effect_names.iter().filter(|n| q.is_empty() || n.to_ascii_lowercase().contains(&q)).take(400).collect();
                    egui::ScrollArea::vertical().max_height(320.0).id_salt("fxlist").show(ui, |ui| {
                        for n in list {
                            if ui.selectable_label(self.fx_name.as_deref() == Some(n.as_str()), n.trim_end_matches(".troy")).clicked() {
                                self.fx_name = Some(n.clone());
                            }
                        }
                    });
                    ui.label(RichText::new("Common: Fire_Cauldron, FireTorch_Med, WaterRipples, Fountain…").weak());
                }
                ui.label(RichText::new("Click to place; click a marker to select it. In Edit (3D) the effect plays live.").weak());
            }
            LTool::Scenery => {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.scenery, SceneryMode::Place, "Place");
                    ui.selectable_value(&mut self.scenery, SceneryMode::Scatter, "Scatter");
                    ui.selectable_value(&mut self.scenery, SceneryMode::Erase, "Erase");
                    ui.selectable_value(&mut self.scenery, SceneryMode::Path, "Path").on_hover_text("Draw a line: a row of the chosen prop along it, exactly spaced and turned along the line (tree lines, fences, torches, walls)");
                });
                if self.scenery == SceneryMode::Path {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.path_free, true, "Freehand").on_hover_text("The row follows the line you draw, exactly spaced and turned along it");
                        ui.selectable_value(&mut self.path_free, false, "Straight");
                    });
                    ui.add(egui::Slider::new(&mut self.path_gap, 0.3..=4.0).text("spacing (× width)"));
                    ui.horizontal(|ui| {
                        ui.label("Turn");
                        for t in [0.0, 90.0, 180.0, 270.0] {
                            ui.selectable_value(&mut self.path_turn, t, format!("{t:.0}°"));
                        }
                    });
                }
                if self.scenery != SceneryMode::Place {
                    ui.add(egui::Slider::new(&mut self.radius, CELL..=2500.0).logarithmic(true).text("brush  [ ]"));
                }
                if self.scenery == SceneryMode::Scatter {
                    ui.add(egui::Slider::new(&mut self.scatter_spacing, 60.0..=1200.0).logarithmic(true).text("spacing"));
                    ui.label(RichText::new("Scatters the chosen prop, or the whole category when none is chosen.").weak());
                }
                if self.scenery != SceneryMode::Erase {
                    ui.horizontal(|ui| {
                        ui.label("Size");
                        ui.add(egui::DragValue::new(&mut self.prop_scale[0]).speed(0.01).range(0.1..=5.0));
                        ui.label("to");
                        ui.add(egui::DragValue::new(&mut self.prop_scale[1]).speed(0.01).range(0.1..=5.0));
                    });
                    ui.checkbox(&mut self.random_turn, "Random turn");
                }
                ui.separator();
                match lib {
                    None => {
                        ui.spinner();
                        ui.label("Loading the scenery library…");
                    }
                    Some(lb) => {
                        // Which maps' art this map uses (old maps should not get modern scenery).
                        let am = &doc.layout.asset_maps;
                        let label = if am.is_empty() { "All maps".to_string() } else { am.join(", ") };
                        let mut want = am.clone();
                        ui.horizontal(|ui| {
                            ui.label("Scenery from");
                            egui::ComboBox::from_id_salt("assetmaps").selected_text(label).show_ui(ui, |ui| {
                                if ui.selectable_label(want.is_empty(), "All maps").clicked() {
                                    want.clear();
                                }
                                for src in lb.sources() {
                                    let mut on = want.iter().any(|m| m.eq_ignore_ascii_case(&src));
                                    let hint = if src.eq_ignore_ascii_case("Map1") { " (classic Summoner's Rift)" } else if src.eq_ignore_ascii_case("map11") { " (modern Summoner's Rift)" } else { "" };
                                    if ui.checkbox(&mut on, format!("{src}{hint}")).changed() {
                                        want.retain(|m| !m.eq_ignore_ascii_case(&src));
                                        if on {
                                            want.push(src.clone());
                                        }
                                    }
                                }
                            }).response.on_hover_text("The maps whose props this map uses: the scenery list, scatter, AI decoration and the default wall trees and rocks. Your own models are always available.");
                        });
                        if want != doc.layout.asset_maps {
                            doc.begin();
                            doc.layout.asset_maps = want;
                            doc.end();
                            if self.prop_key.as_ref().is_some_and(|k| !PropLibrary::allowed(k, &doc.layout.asset_maps)) {
                                self.prop_key = None;
                            }
                        }
                        egui::ComboBox::from_id_salt("propcat").selected_text(self.prop_cat.clone()).show_ui(ui, |ui| {
                            for c in lb.categories() {
                                if !lb.props.iter().any(|p| p.category == c && PropLibrary::allowed(&p.key, &doc.layout.asset_maps)) {
                                    continue;
                                }
                                if ui.selectable_label(self.prop_cat == c, &c).clicked() {
                                    self.prop_cat = c;
                                    self.prop_key = None;
                                }
                            }
                        });
                        ui.add(egui::TextEdit::singleline(&mut self.prop_search).hint_text("Search props…"));
                        if ui.selectable_label(self.prop_key.is_none(), "Any in this category").clicked() {
                            self.prop_key = None;
                        }
                        let ctx = ui.ctx().clone();
                        let search = self.prop_search.to_ascii_lowercase();
                        let items: Vec<&lol_map_studio::props::PropDef> = lb.props.iter().filter(|p| PropLibrary::allowed(&p.key, &doc.layout.asset_maps) && p.category == self.prop_cat && (search.is_empty() || p.name.to_ascii_lowercase().contains(&search))).collect();
                        egui::ScrollArea::vertical().max_height(360.0).id_salt("props").show(ui, |ui| {
                            egui::Grid::new("propgrid").num_columns(3).spacing([4.0, 4.0]).show(ui, |ui| {
                                for (n, d) in items.iter().enumerate() {
                                    let tid = self.thumb(&ctx, &d.key, || (!d.thumb.is_empty()).then(|| (48, 48, d.thumb.clone())));
                                    let sel = self.prop_key.as_deref() == Some(d.key.as_str());
                                    let resp = match tid {
                                        Some(t) => ui.add(egui::Button::image((t, Vec2::splat(56.0))).selected(sel)),
                                        None => ui.add(egui::Button::new(&d.name).selected(sel)),
                                    };
                                    if resp.on_hover_text(format!("{}\n{:.0} wide, {:.0} tall{}", d.name, d.radius * 2.0, d.height, if d.copies > 1 { format!(", used {} times in its map", d.copies) } else { String::new() })).clicked() {
                                        self.prop_key = Some(d.key.clone());
                                        if self.scenery == SceneryMode::Erase {
                                            self.scenery = SceneryMode::Place;
                                        }
                                    }
                                    if n % 3 == 2 {
                                        ui.end_row();
                                    }
                                }
                            });
                        });
                        ui.label(RichText::new(format!("{} props", items.len())).weak());
                        ui.separator();
                        ui.label(RichText::new("Prefabs (saved groups)").strong());
                        let prefabs = lol_map_studio::worldtools::list_prefabs();
                        if prefabs.is_empty() {
                            ui.label(RichText::new("Select objects and use \"Save as prefab\" in the Properties panel.").weak());
                        }
                        for (name, cb) in prefabs {
                            ui.horizontal(|ui| {
                                if ui.button(format!("{name}  ({})", cb.len())).on_hover_text("Click, then click on the map to place it").clicked() {
                                    self.placing = Some(cb.clone());
                                }
                                if ui.small_button("Delete").clicked() {
                                    lol_map_studio::worldtools::delete_prefab(&name);
                                }
                            });
                        }
                    }
                }
            }
        }
        ui.separator();
        ui.label(RichText::new("Symmetry").strong());
        let mut sym = doc.layout.symmetry;
        egui::ComboBox::from_id_salt("sym").selected_text(sym.label()).width(200.0).show_ui(ui, |ui| {
            for s in Symmetry::ALL {
                ui.selectable_value(&mut sym, s, s.label());
            }
        });
        if sym != doc.layout.symmetry {
            doc.begin();
            doc.layout.symmetry = sym;
            doc.end();
        }
        ui.separator();
        ui.checkbox(&mut self.game_look, "Game look (render the built map)").on_hover_text("Draws the 2D canvas with the live preview of the built map");
        ui.checkbox(&mut self.show_grid, "Grid (1000 units)");
        ui.horizontal(|ui| {
            ui.label("Show");
            ui.checkbox(&mut self.show_objectives, "objectives");
            ui.checkbox(&mut self.show_lanes, "lanes");
            ui.checkbox(&mut self.show_props, "props");
        });
        if ui.button("Fit map").clicked() {
            self.fitted = false;
        }
    }

    /// Right panel: the selected objective and the map's settings.
    pub fn settings_ui(&mut self, ui: &mut egui::Ui, doc: &mut LayoutDoc, library: &[(String, String)], rules_source: &mut String, lib: Option<&PropLibrary>) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.right_tab, 0, RichText::new("Properties").strong());
            ui.selectable_value(&mut self.right_tab, 1, RichText::new("Look & behaviour").strong());
        });
        ui.separator();
        if self.right_tab == 1 {
            self.map_settings_ui(ui, doc, rules_source);
            return;
        }
        self.selection_ui(ui, doc);
        if let Some(id) = self.selected_fx {
            let mut l = doc.layout.clone();
            let mut remove = false;
            if let Some(e) = l.effects.iter_mut().find(|e| e.id == id) {
                ui.heading(e.name.trim_end_matches(".troy").to_string());
                egui::Grid::new("fxprops").num_columns(2).show(ui, |ui| {
                    ui.label("Position");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut e.pos[0]).speed(5.0));
                        ui.add(egui::DragValue::new(&mut e.pos[1]).speed(5.0));
                    });
                    ui.end_row();
                    ui.label("Height");
                    ui.add(egui::DragValue::new(&mut e.lift).speed(1.0));
                    ui.end_row();
                    ui.label("Turn (°)");
                    ui.add(egui::DragValue::new(&mut e.yaw).speed(1.0));
                    ui.end_row();
                });
                remove = ui.button("Delete effect").clicked();
            } else if let Some(lt) = l.lights.iter_mut().find(|x| x.id == id) {
                ui.heading("Light");
                egui::Grid::new("lightprops").num_columns(2).show(ui, |ui| {
                    ui.label("Position");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut lt.pos[0]).speed(5.0));
                        ui.add(egui::DragValue::new(&mut lt.pos[1]).speed(5.0));
                    });
                    ui.end_row();
                    ui.label("Height");
                    ui.add(egui::DragValue::new(&mut lt.lift).speed(1.0));
                    ui.end_row();
                    ui.label("Colour");
                    ui.color_edit_button_srgb(&mut lt.color);
                    ui.end_row();
                    ui.label("Radius");
                    ui.add(egui::DragValue::new(&mut lt.outer).speed(5.0).range(1.0..=5000.0));
                    ui.end_row();
                    ui.label("Inner radius");
                    ui.add(egui::DragValue::new(&mut lt.inner).speed(1.0).range(0.0..=5000.0));
                    ui.end_row();
                    ui.label("Intensity");
                    ui.add(egui::DragValue::new(&mut lt.intensity).speed(0.01).range(0.0..=10.0));
                    ui.end_row();
                });
                remove = ui.button("Delete light").clicked();
            } else {
                self.selected_fx = None;
            }
            if remove {
                l.effects.retain(|e| e.id != id);
                l.lights.retain(|x| x.id != id);
                self.selected_fx = None;
            }
            if l != doc.layout {
                doc.begin();
                doc.layout = l;
                doc.end();
            }
            ui.separator();
        }
        if let Some(id) = self.selected_prop
            && let Some(pr) = doc.layout.props.iter().find(|p| p.id == id).cloned()
        {
            let name = lib.and_then(|lb| lb.get(&pr.prop)).map_or(pr.prop.clone(), |d| d.name.clone());
            ui.heading(name);
            let mut n = pr.clone();
            egui::Grid::new("prop").num_columns(2).show(ui, |ui| {
                ui.label("Position");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut n.pos[0]).speed(5.0));
                    ui.add(egui::DragValue::new(&mut n.pos[1]).speed(5.0));
                });
                ui.end_row();
                ui.label("Turn (°)");
                ui.add(egui::DragValue::new(&mut n.yaw).speed(1.0));
                ui.end_row();
                ui.label("Size");
                ui.add(egui::DragValue::new(&mut n.scale).speed(0.01).range(0.02..=50.0));
                ui.end_row();
                ui.label("Raise / lower");
                ui.add(egui::DragValue::new(&mut n.lift).speed(1.0));
                ui.end_row();
                ui.label("Tilt X / Z (°)");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut n.tilt[0]).speed(0.5).range(-180.0..=180.0));
                    ui.add(egui::DragValue::new(&mut n.tilt[1]).speed(0.5).range(-180.0..=180.0));
                });
                ui.end_row();
                ui.label("Stretch X / Y / Z");
                ui.horizontal(|ui| {
                    for a in 0..3 {
                        ui.add(egui::DragValue::new(&mut n.stretch[a]).speed(0.01).range(0.05..=20.0));
                    }
                });
                ui.end_row();
                ui.label("Collision");
                ui.checkbox(&mut n.walk_through, "Walk through").on_hover_text("Decoration only: champions and minions walk over / through it (no hitbox on the navigation grid)");
                ui.end_row();
            });
            if n != pr {
                doc.begin();
                if let Some(x) = doc.layout.props.iter_mut().find(|p| p.id == id) {
                    *x = n;
                }
                doc.end();
            }
            if ui.button("Edit in Model Studio").on_hover_text("Edit this model; saving it updates every copy on the map (for your assets)").clicked() {
                self.open_in_studio = Some(pr.prop.clone());
            }
            if ui.button("Delete prop").clicked() {
                doc.begin();
                doc.layout.props.retain(|p| p.id != id);
                doc.end();
                self.selected_prop = None;
            }
            ui.separator();
        }
        if let Some(id) = self.selected
            && let Some(o) = doc.layout.objective(id).cloned()
        {
            ui.heading(o.kind.label());
            let mut n = o.clone();
            egui::Grid::new("obj").num_columns(2).show(ui, |ui| {
                ui.label("Team");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut n.team, 1, RichText::new("Blue").color(team_color(1)));
                    ui.selectable_value(&mut n.team, 2, RichText::new("Red").color(team_color(2)));
                });
                ui.end_row();
                if n.kind.has_lane() {
                    ui.label("Lane");
                    ui.horizontal(|ui| {
                        for l in Lane::ALL {
                            ui.selectable_value(&mut n.lane, l, l.label());
                        }
                    });
                    ui.end_row();
                }
                if n.kind == ObjKind::Turret {
                    ui.label("Tier");
                    egui::ComboBox::from_id_salt("tier").selected_text(n.tier.label()).show_ui(ui, |ui| {
                        for t in TurretTier::ALL {
                            ui.selectable_value(&mut n.tier, t, t.label());
                        }
                    });
                    ui.end_row();
                }
                ui.label("Position");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut n.pos[0]).speed(5.0));
                    ui.add(egui::DragValue::new(&mut n.pos[1]).speed(5.0));
                });
                ui.end_row();
                ui.label("Facing (°)");
                ui.add(egui::DragValue::new(&mut n.yaw).speed(1.0));
                ui.end_row();
            });
            if n != o {
                doc.begin();
                if n.pos != o.pos {
                    doc.layout.move_objective(id, n.pos);
                }
                if let Some(x) = doc.layout.objectives.iter_mut().find(|x| x.id == id) {
                    x.team = n.team;
                    x.lane = n.lane;
                    x.tier = n.tier;
                    x.yaw = n.yaw;
                }
                doc.end();
            }
            if ui.button("Delete").clicked() {
                doc.begin();
                doc.layout.remove_objective(id);
                doc.end();
                self.selected = None;
            }
            ui.separator();
        }

        ui.heading("Map");
        let mut l = doc.layout.clone();
        egui::Grid::new("map").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut l.name);
            ui.end_row();
            ui.label("Map ID");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut l.map_id).range(1..=99));
                ui.label(RichText::new(format!("-> LEVELS/Map{}", l.map_id)).weak());
            });
            ui.end_row();
            ui.label("Size");
            ui.label(format!("{:.0} × {:.0} units", l.size()[0], l.size()[1]));
            ui.end_row();
            ui.label("Rules");
            ui.vertical(|ui| {
                ui.radio_value(&mut l.rules, Rules::Classic, "Classic 5v5 (minion waves, items, XP)");
                ui.radio_value(&mut l.rules, Rules::Sandbox, "Sandbox (structures only, experimental)");
            });
            ui.end_row();
            if l.rules == Rules::Classic {
                ui.label("Rules from");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(rules_source).desired_width(180.0)).on_hover_text("A map folder whose generic gameplay files (items, XP, minion logic) are used. Its terrain and objects are not used.");
                    if ui.small_button("…").clicked()
                        && let Some(p) = rfd::FileDialog::new().pick_folder()
                    {
                        *rules_source = p.to_string_lossy().to_string();
                    }
                });
                ui.end_row();
            }
        });
        ui.separator();
        // Texture choices from the maps this map draws its art from (plus generated ones).
        let allowed_tex: Vec<(String, String)> = library.iter().filter(|(_, r)| {
            r.starts_with("gen:") || std::path::Path::new(r).ancestors().nth(3).and_then(|d| d.file_name()).is_none_or(|d| PropLibrary::allowed(&format!("{}/x", d.to_string_lossy()), &l.asset_maps))
        }).cloned().collect();
        let library: &[(String, String)] = &allowed_tex;
        if let Some(b) = l.base.clone() {
            ui.heading("Opened map");
            ui.label(RichText::new(&b.path).monospace());
            ui.label(RichText::new("The map's terrain, scenery and textures are kept exactly. Build and export rewrites only what you change: structures, lanes, camps, effects, lights, walkability; new props and painted walls are added to its scene.").weak());
            ui.label(RichText::new("Painted cells show in colour (Game look): brown = new wall, green = new brush, yellow = newly walkable. Walls painted on open ground get a rock wall; existing art cannot be removed.").weak());
            if b.keep_camps {
                ui.label(RichText::new("This map's jungle script is kept as it is (its camps could not be read), so camp changes are not exported.").color(Color32::from_rgb(255, 200, 120)));
            }
            let changed = (l.objectives != b.objectives) as usize + (l.lanes != b.lanes) as usize + (l.effects != b.effects) as usize + (l.lights != b.lights) as usize + (!l.props.is_empty()) as usize;
            ui.label(format!("{} of 5 parts changed (structures and camps, lanes, effects, lights, new props)", changed));
            ui.horizontal(|ui| {
                ui.label("Cliff texture for new walls");
                egui::ComboBox::from_id_salt("basecliff").selected_text(l.theme.cliff.rsplit(['/', '\\']).next().unwrap_or("").to_string()).show_ui(ui, |ui| {
                    for (name, r) in library {
                        if ui.selectable_label(&l.theme.cliff == r, name).clicked() {
                            l.theme.cliff = r.clone();
                        }
                    }
                });
            });
        } else {
        ui.heading("Look");
        let tex_combo = |ui: &mut egui::Ui, id: &str, v: &mut String| {
            let shown = library.iter().find(|(_, r)| r == v).map(|x| x.0.clone()).unwrap_or_else(|| v.rsplit(['/', '\\']).next().unwrap_or(v).to_string());
            egui::ComboBox::from_id_salt(id.to_string()).selected_text(shown).width(190.0).height(400.0).show_ui(ui, |ui| {
                for (name, r) in library {
                    ui.selectable_value(v, r.clone(), name);
                }
                if ui.button("Browse for an image…").clicked()
                    && let Some(p) = rfd::FileDialog::new().add_filter("Images", &["dds", "png", "jpg", "jpeg"]).pick_file()
                {
                    *v = p.to_string_lossy().to_string();
                }
            });
        };
        egui::Grid::new("theme").num_columns(2).show(ui, |ui| {
            for i in 0..l.theme.ground.len() {
                ui.text_edit_singleline(&mut l.theme.ground[i].name);
                tex_combo(ui, &format!("layer{i}"), &mut l.theme.ground[i].texture);
                ui.end_row();
            }
            if l.theme.ground.len() < MAX_LAYERS {
                ui.label("");
                let mut add = String::new();
                egui::ComboBox::from_id_salt("addlayer").selected_text("+ Add ground layer").width(190.0).height(400.0).show_ui(ui, |ui| {
                    for (name, r) in library {
                        if ui.selectable_label(false, name).clicked() {
                            add = r.clone();
                        }
                    }
                    if ui.button("Browse for an image…").clicked()
                        && let Some(p) = rfd::FileDialog::new().add_filter("Images", &["dds", "png", "jpg", "jpeg"]).pick_file()
                    {
                        add = p.to_string_lossy().to_string();
                    }
                });
                if !add.is_empty() {
                    let name = library.iter().find(|(_, r)| *r == add).map(|x| x.0.trim_end_matches(".dds").to_string()).unwrap_or_else(|| std::path::Path::new(&add).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                    l.theme.ground.push(GroundLayer { name, texture: add });
                }
                ui.end_row();
            }
            ui.label("Cliffs");
            tex_combo(ui, "cliff", &mut l.theme.cliff);
            ui.end_row();
            ui.label("Brush");
            tex_combo(ui, "brush", &mut l.theme.brush);
            ui.end_row();
            ui.label("Texture repeat");
            ui.add(egui::DragValue::new(&mut l.theme.tiling).range(64.0..=4096.0).suffix(" units"));
            ui.end_row();
            ui.label("Ground height");
            ui.add(egui::DragValue::new(&mut l.theme.base_height).speed(1.0));
            ui.end_row();
            ui.label("Wall height");
            ui.add(egui::DragValue::new(&mut l.theme.wall_height).range(50.0..=1200.0));
            ui.end_row();
            ui.label("River depth");
            ui.add(egui::DragValue::new(&mut l.theme.river_depth).range(0.0..=600.0));
            ui.end_row();
            ui.label("Sun colour");
            let mut c = l.theme.sun_color.map(|v| v / 255.0);
            if ui.color_edit_button_rgb(&mut c).changed() {
                l.theme.sun_color = c.map(|v| v * 255.0);
            }
            ui.end_row();
            ui.label("Ambient colour");
            let mut a = l.theme.ambient.map(|v| v / 255.0);
            if ui.color_edit_button_rgb(&mut a).changed() {
                l.theme.ambient = a.map(|v| v * 255.0);
            }
            ui.end_row();
            ui.label("Sun direction");
            ui.horizontal(|ui| {
                for k in 0..3 {
                    ui.add(egui::DragValue::new(&mut l.theme.sun_dir[k]).speed(0.01).range(-1.0..=1.0));
                }
            });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.button("Summoner's Rift look").on_hover_text("Ground, cliff and grass textures from the Map1 example (if present)").clicked() {
                let find = |n: &str| library.iter().find(|(name, _)| name.eq_ignore_ascii_case(n)).map(|x| x.1.clone());
                let picks = [("grass_TX_CM.dds", 0), ("order_dirt.dds", 1), ("gravel_stone_TX_CM.dds", 2), ("td_sand1.dds", 3)];
                for (n, i) in picks {
                    if let (Some(r), Some(g)) = (find(n), l.theme.ground.get_mut(i)) {
                        g.texture = r;
                    }
                }
                if let Some(r) = find("rocks.dds") {
                    l.theme.cliff = r;
                }
                if let Some(r) = find("WallOfGrass.dds") {
                    l.theme.brush = r;
                }
            }
            if ui.button("Generated look").clicked() {
                let d = lol_map_studio::layout::Theme::default();
                for (i, g) in d.ground.into_iter().enumerate() {
                    if let Some(x) = l.theme.ground.get_mut(i) {
                        x.texture = g.texture;
                    }
                }
                l.theme.cliff = d.cliff;
                l.theme.brush = d.brush;
            }
        });
        ui.separator();
        ui.heading("Jungle walls");
        ui.horizontal(|ui| {
            ui.label("Style");
            ui.selectable_value(&mut l.theme.forest.league_walls, true, "League (3D rocks + trees)").on_hover_text("Walls stay at ground level and are built from Summoner's Rift's rock and tree models, as in the real maps");
            ui.selectable_value(&mut l.theme.forest.league_walls, false, "Raised terrain");
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut l.theme.forest.auto_dress, "Auto-dress walls").on_hover_text("Each painted wall is traced into a smooth outline, rocks are placed along it at exact steps, turned along the edge, and trees fill the inside. Repaint and it follows.");
            if let Some(lb) = lib
                && l.base.is_none()
                && l.theme.forest.auto_dress
                && ui.button("Bake into props").on_hover_text("Turns the auto dressing into ordinary props you can move, turn and delete one by one; auto-dress is then switched off").clicked()
            {
                for d in lol_map_studio::layout_build::wall_dressing(&l, lb, None) {
                    let id = l.add_prop(&d.key, d.pos, d.yaw, d.scale);
                    if let Some(p) = l.props.iter_mut().find(|p| p.id == id) {
                        p.lift = d.lift;
                        p.stretch = d.stretch;
                    }
                }
                l.theme.forest.auto_dress = false;
            }
        });
        if l.theme.forest.league_walls && l.theme.forest.auto_dress {
            ui.horizontal(|ui| {
                ui.label("Preset");
                if ui.button("Classic: trees / mound / trees").on_hover_text("As on classic Summoner's Rift: pine trees ring every wall, a mossy mound in the middle, trees on top").clicked() {
                    l.theme.forest.edge_tree_share = 1.0;
                    l.theme.forest.mound = 90.0;
                    l.theme.forest.edge_spacing = 150.0;
                    l.theme.forest.enabled = true;
                    l.theme.forest.spacing = 340.0;
                    l.theme.forest.inset = 170.0;
                }
                if ui.button("Rocky edges").on_hover_text("Stone faces along the edges, trees inside").clicked() {
                    l.theme.forest.edge_tree_share = 0.2;
                    l.theme.forest.mound = 30.0;
                    l.theme.forest.edge_spacing = 150.0;
                    l.theme.forest.enabled = true;
                    l.theme.forest.spacing = 200.0;
                    l.theme.forest.inset = 40.0;
                }
            });
            egui::Grid::new("edgerocks").num_columns(2).show(ui, |ui| {
                ui.label("Mound height");
                ui.add(egui::DragValue::new(&mut l.theme.forest.mound).range(0.0..=400.0).suffix(" units")).on_hover_text("The raised middle of each wall");
                ui.end_row();
                ui.label("Mound texture");
                let mut mt = if l.theme.forest.mound_texture.is_empty() { lol_map_studio::layout_build::mound_texture(&l.theme) } else { l.theme.forest.mound_texture.clone() };
                let before = mt.clone();
                tex_combo(ui, "moundtex", &mut mt);
                if mt != before {
                    l.theme.forest.mound_texture = mt;
                }
                ui.end_row();
                ui.label("Edge spacing");
                ui.add(egui::DragValue::new(&mut l.theme.forest.edge_spacing).range(40.0..=600.0).suffix(" units")).on_hover_text("Exact step between rocks along the wall outline");
                ui.end_row();
                ui.label("Rock size");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut l.theme.forest.edge_size[0]).range(40.0..=1200.0).suffix(" u"));
                    ui.add(egui::DragValue::new(&mut l.theme.forest.edge_size[1]).range(40.0..=1200.0).suffix(" u"));
                });
                ui.end_row();
                ui.label("Inside the edge");
                ui.add(egui::Slider::new(&mut l.theme.forest.edge_inset, -0.5..=1.0)).on_hover_text("How far inside the wall the rocks sit, as a share of their size (0 = centred on the outline)");
                ui.end_row();
                ui.label("Edge: rocks ↔ trees");
                ui.add(egui::Slider::new(&mut l.theme.forest.edge_tree_share, 0.0..=1.0)).on_hover_text("0 = only rocks along the edges, 1 = only trees (classic Summoner's Rift)");
                ui.end_row();
                ui.label("Follow the outline");
                ui.checkbox(&mut l.theme.forest.edge_align, "").on_hover_text("Turn rocks along the wall edge (off: random turns)");
                ui.end_row();
            });
            ui.label(if l.theme.forest.edge_props.is_empty() { RichText::new("Edge rocks: the big rocks of the map's scenery").weak() } else { RichText::new(format!("Edge rocks: {} chosen", l.theme.forest.edge_props.len())).weak() });
            ui.horizontal(|ui| {
                if let Some(k) = &self.prop_key
                    && ui.button("Add the chosen prop as edge rock").clicked()
                    && !l.theme.forest.edge_props.contains(k)
                {
                    l.theme.forest.edge_props.push(k.clone());
                }
                if !l.theme.forest.edge_props.is_empty() && ui.button("Default rocks").clicked() {
                    l.theme.forest.edge_props.clear();
                }
            });
        }
        ui.checkbox(&mut l.theme.forest.enabled, "Fill walls with trees");
        if l.theme.forest.enabled {
            egui::Grid::new("forest").num_columns(2).show(ui, |ui| {
                ui.label("Spacing");
                ui.add(egui::DragValue::new(&mut l.theme.forest.spacing).range(80.0..=1000.0).suffix(" units"));
                ui.end_row();
                ui.label("Size");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut l.theme.forest.scale[0]).speed(0.01).range(0.2..=4.0));
                    ui.add(egui::DragValue::new(&mut l.theme.forest.scale[1]).speed(0.01).range(0.2..=4.0));
                });
                ui.end_row();
                ui.label("Keep from edge");
                ui.add(egui::DragValue::new(&mut l.theme.forest.inset).range(0.0..=600.0).suffix(" units"));
                ui.end_row();
            });
            ui.label(if l.theme.forest.props.is_empty() { RichText::new("Trees: the library's large trees (choose your own below)").weak() } else { RichText::new(format!("Trees: {} chosen", l.theme.forest.props.len())).weak() });
            ui.horizontal(|ui| {
                if let Some(k) = &self.prop_key
                    && ui.button("Add the chosen prop").on_hover_text("Choose a prop with the Scenery tool").clicked()
                    && !l.theme.forest.props.contains(k)
                {
                    l.theme.forest.props.push(k.clone());
                }
                if !l.theme.forest.props.is_empty() && ui.button("Use default trees").clicked() {
                    l.theme.forest.props.clear();
                }
            });
            let mut drop = None;
            for (i, k) in l.theme.forest.props.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(lib.and_then(|lb| lb.get(k)).map_or(k.clone(), |d| d.name.clone()));
                    if ui.small_button("✕").clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop {
                l.theme.forest.props.remove(i);
            }
        }
        }
        ui.separator();
        ui.heading("My props");
        for c in &l.custom_props {
            ui.label(format!("{}  ({})", c.name, std::path::Path::new(&c.obj).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()));
        }
        match &mut self.import {
            None => {
                if ui.button("Import a model (OBJ + texture)…").clicked() {
                    self.import = Some(CustomProp { name: "My prop".into(), obj: String::new(), texture: String::new(), scale: 1.0 });
                }
            }
            Some(c) => {
                egui::Grid::new("imp").num_columns(2).show(ui, |ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut c.name);
                    ui.end_row();
                    ui.label("Model");
                    ui.horizontal(|ui| {
                        ui.label(std::path::Path::new(&c.obj).file_name().map_or("(none)".into(), |s| s.to_string_lossy().to_string()));
                        if ui.small_button("…").clicked()
                            && let Some(p) = rfd::FileDialog::new().add_filter("Wavefront OBJ", &["obj"]).pick_file()
                        {
                            c.obj = p.to_string_lossy().to_string();
                        }
                    });
                    ui.end_row();
                    ui.label("Texture");
                    ui.horizontal(|ui| {
                        ui.label(std::path::Path::new(&c.texture).file_name().map_or("(none)".into(), |s| s.to_string_lossy().to_string()));
                        if ui.small_button("…").clicked()
                            && let Some(p) = rfd::FileDialog::new().add_filter("Images", &["dds", "png", "jpg", "jpeg"]).pick_file()
                        {
                            c.texture = p.to_string_lossy().to_string();
                        }
                    });
                    ui.end_row();
                    ui.label("Scale");
                    ui.add(egui::DragValue::new(&mut c.scale).speed(0.01).range(0.001..=1000.0));
                    ui.end_row();
                });
                let ready = !c.obj.is_empty() && !c.texture.is_empty() && !c.name.is_empty();
                let mut done = false;
                ui.horizontal(|ui| {
                    if ui.add_enabled(ready, egui::Button::new("Add")).clicked() {
                        match lol_map_studio::props::load_custom(c) {
                            Ok(_) => {
                                l.custom_props.retain(|x| x.name != c.name);
                                l.custom_props.push(c.clone());
                                self.prop_cat = "My props".into();
                                self.prop_key = Some(format!("custom:{}", c.name));
                                self.status = Some(format!("Added {}: find it under My props in the Scenery tool.", c.name));
                                done = true;
                            }
                            Err(e) => self.status = Some(format!("Could not use the model: {e:#}")),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
                if done {
                    self.import = None;
                }
            }
        }
        if l != doc.layout {
            doc.begin();
            doc.layout = l;
            doc.end();
        }
        ui.separator();
        let counts = |k: ObjKind| doc.layout.objectives.iter().filter(|o| o.kind == k).count();
        ui.label(format!("{} nexus, {} inhibitors, {} turrets, {} spawn points, {} fountain turrets, {} shops, {} minion spawns, {} lanes, {} props", counts(ObjKind::Nexus), counts(ObjKind::Inhibitor), counts(ObjKind::Turret), counts(ObjKind::SpawnPoint), counts(ObjKind::FountainTurret), counts(ObjKind::Shop), counts(ObjKind::MinionSpawn), doc.layout.lanes.len(), doc.layout.props.len()));
    }
}
