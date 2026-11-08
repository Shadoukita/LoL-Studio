//! Top-down map view: scene image, navigation overlay, objects; selection, dragging, painting.

use lol_map_studio::edit::{Editor, NavPaint, Sel};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};
use lol_map_studio::formats::{dds, mob::ObjectType, navgrid::flags};
use lol_map_studio::project::Doc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tool {
    Select,
    Paint,
    Place,
    Duplicate,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Background {
    Scene,
    Minimap,
    None,
}

pub struct Layers {
    pub background: Background,
    pub nav: bool,
    pub nav_alpha: f32,
    pub structures: bool,
    pub nav_points: bool,
    pub props: bool,
    pub brush_props: bool,
    pub labels: bool,
    pub particles: bool,
    /// Show object markers in the 3D view (structures are drawn as models).
    pub markers_3d: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Self { background: Background::Scene, nav: false, nav_alpha: 0.45, structures: true, nav_points: true, props: false, brush_props: false, labels: true, particles: false, markers_3d: false }
    }
}

pub struct PlaceSpec {
    pub kind: ObjectType,
    pub name: String,
}

pub struct View2D {
    pub center: [f32; 2],
    /// Screen pixels per world unit.
    pub scale: f32,
    pub fitted: bool,
    pub layers: Layers,
    pub tool: Tool,
    pub paint: NavPaint,
    pub radius: f32,
    pub strength: f32,
    pub place: PlaceSpec,
    minimap_tex: Option<(TextureHandle, u64)>,
    nav_tex: Option<(TextureHandle, u64)>,
    drag: Option<DragState>,
    pub hover_world: Option<[f32; 2]>,
    pub status: Option<String>,
    pub focus_request: Option<[f32; 2]>,
    last_paint: Option<[f32; 2]>,
}

struct DragState {
    sel: Sel,
    start_world: [f32; 2],
    moved: bool,
}

pub struct Marker {
    pub sel: Sel,
    pub name: String,
    pub pos: [f32; 3],
    pub color: Color32,
    pub radius: f32,
    pub shape: u8,
}

impl Default for View2D {
    fn default() -> Self {
        Self {
            center: [7000.0, 7000.0],
            scale: 0.05,
            fitted: false,
            layers: Layers::default(),
            tool: Tool::Select,
            paint: NavPaint::Wall,
            radius: 150.0,
            strength: 15.0,
            place: PlaceSpec { kind: ObjectType::Turret, name: "Turret_T1_C_08".into() },
            minimap_tex: None,
            nav_tex: None,
            drag: None,
            hover_world: None,
            status: None,
            focus_request: None,
            last_paint: None,
        }
    }
}

pub fn team_color(name: &str, base: Color32) -> Color32 {
    let n = name.to_ascii_lowercase();
    if n.contains("_t1") || n.contains("order") || n.ends_with("t1") {
        Color32::from_rgb(70, 150, 255)
    } else if n.contains("_t2") || n.contains("chaos") || n.ends_with("t2") {
        Color32::from_rgb(255, 80, 70)
    } else {
        base
    }
}

pub fn markers(ed: &Editor, layers: &Layers) -> Vec<Marker> {
    let mut out = Vec::new();
    if let Some(m) = ed.project.mob() {
        for o in &m.objects {
            let brush = o.name.starts_with("LevelProp_brush");
            let (show, color, r, shape) = match o.kind {
                ObjectType::Turret => (layers.structures, Color32::GRAY, 7.0, 1),
                ObjectType::Nexus => (layers.structures, Color32::GRAY, 11.0, 2),
                ObjectType::Inhibitor => (layers.structures, Color32::GRAY, 8.0, 2),
                ObjectType::SpawnPoint => (layers.structures, Color32::from_rgb(255, 230, 90), 9.0, 3),
                ObjectType::Barracks => (layers.structures, Color32::from_rgb(200, 160, 255), 6.0, 3),
                ObjectType::Shop => (layers.structures, Color32::from_rgb(255, 200, 60), 7.0, 1),
                ObjectType::NavPoint => (layers.nav_points, Color32::from_rgb(120, 230, 160), 4.0, 0),
                ObjectType::LevelSize => (false, Color32::WHITE, 0.0, 0),
                ObjectType::LevelProp if brush => (layers.brush_props, Color32::from_rgb(60, 170, 60), 3.0, 0),
                ObjectType::LevelProp => (layers.props, Color32::from_rgb(230, 140, 220), 4.0, 0),
                _ => (layers.props, Color32::LIGHT_GRAY, 4.0, 0),
            };
            if show {
                out.push(Marker { sel: Sel::Mob(o.name.clone()), name: o.name.clone(), pos: o.position, color: team_color(&o.name, color), radius: r, shape });
            }
        }
    }
    if layers.structures {
        for e in ed.project.dsc_entries() {
            let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0).to_string();
            if stem.eq_ignore_ascii_case("AIPath") {
                continue;
            }
            if let Some(c) = ed.dsc_central(&stem) {
                let r = if stem.starts_with("HQ") { 11.0 } else if stem.starts_with("Barracks") { 8.0 } else if stem.starts_with("Lake") { 5.0 } else { 7.0 };
                out.push(Marker { sel: Sel::Dsc(stem.clone()), name: stem.clone(), pos: c, color: team_color(&stem, Color32::from_rgb(200, 200, 200)), radius: r, shape: 1 });
            }
        }
    }
    out
}

impl View2D {
    pub fn world_rect(ed: &Editor) -> [f32; 4] {
        if let Some(g) = ed.project.navgrid() {
            return [g.min[0], g.min[2], g.max[0], g.max[2]];
        }
        if let Some(ls) = ed.project.mob().and_then(|m| m.objects.iter().find(|o| o.kind == ObjectType::LevelSize)) {
            return [ls.bbox_min[0], ls.bbox_min[2], ls.bbox_max[0], ls.bbox_max[2]];
        }
        [0.0, 0.0, 15000.0, 15000.0]
    }

    pub fn fit(&mut self, ed: &Editor, size: Vec2) {
        let r = Self::world_rect(ed);
        self.center = [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0];
        self.scale = (size.x / (r[2] - r[0])).min(size.y / (r[3] - r[1])) * 0.95;
        self.fitted = true;
    }

    fn to_screen(&self, rect: Rect, x: f32, z: f32) -> Pos2 {
        let c = rect.center();
        Pos2::new(c.x + (x - self.center[0]) * self.scale, c.y - (z - self.center[1]) * self.scale)
    }

    fn to_world(&self, rect: Rect, p: Pos2) -> [f32; 2] {
        let c = rect.center();
        [self.center[0] + (p.x - c.x) / self.scale, self.center[1] - (p.y - c.y) / self.scale]
    }

    fn world_rect_screen(&self, rect: Rect, r: [f32; 4]) -> Rect {
        Rect::from_two_pos(self.to_screen(rect, r[0], r[1]), self.to_screen(rect, r[2], r[3]))
    }

    fn update_textures(&mut self, ctx: &egui::Context, ed: &Editor) {
        if self.layers.background == Background::Minimap && self.minimap_tex.as_ref().is_none_or(|t| t.1 != ed.scene_rev) {
            let img = ed.project.file("INFO/2DLevelMiniMap.dds").and_then(|f| dds::decode(&f.bytes().ok()?, 2048).ok());
            if let Some(img) = img {
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.pixels);
                self.minimap_tex = Some((ctx.load_texture("minimap", ci, TextureOptions::LINEAR), ed.scene_rev));
            }
        }
        if self.layers.nav && self.nav_tex.as_ref().is_none_or(|t| t.1 != ed.nav_rev)
            && let Some(g) = ed.project.navgrid()
        {
            let (lo, hi) = (0..g.cell_count()).fold((f32::MAX, f32::MIN), |(a, b), k| (a.min(g.height(k)), b.max(g.height(k))));
            let mut px = vec![0u8; g.nx * g.ny * 4];
            for j in 0..g.ny {
                for i in 0..g.nx {
                    let k = j * g.nx + i;
                    let f = g.flags(k);
                    let t = if hi > lo { (g.height(k) - lo) / (hi - lo) } else { 0.5 };
                    let c: [u8; 4] = if f & flags::WALL != 0 {
                        [200, 40, 40, 255]
                    } else if f & flags::BRUSH != 0 {
                        [30, 200, 60, 255]
                    } else {
                        let v = (60.0 + 160.0 * t) as u8;
                        [v / 3, v / 2, v, 150]
                    };
                    // Image rows go top-down = high Z first.
                    let o = ((g.ny - 1 - j) * g.nx + i) * 4;
                    px[o..o + 4].copy_from_slice(&c);
                }
            }
            let ci = egui::ColorImage::from_rgba_unmultiplied([g.nx, g.ny], &px);
            self.nav_tex = Some((ctx.load_texture("nav", ci, TextureOptions::NEAREST), ed.nav_rev));
        }
    }

    /// Orthographic top-down projection matching this view (x right, z up, higher ground nearer).
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

    pub fn ui(&mut self, ui: &mut egui::Ui, ed: &mut Editor, gl: &super::gl_scene::Shared) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        if !self.fitted {
            self.fit(ed, rect.size());
        }
        if let Some(f) = self.focus_request.take() {
            self.center = f;
            self.scale = self.scale.max(0.12);
        }
        self.update_textures(ui.ctx(), ed);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(14, 15, 18));

        // Pan and zoom.
        let hover = resp.hover_pos();
        if let Some(hp) = hover {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = zoom * (scroll * 0.0025).exp();
            if (factor - 1.0).abs() > 1e-4 {
                let before = self.to_world(rect, hp);
                self.scale = (self.scale * factor).clamp(0.005, 8.0);
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

        // Background.
        let wr = Self::world_rect(ed);
        match self.layers.background {
            Background::Scene => {
                let mvp = self.ortho(rect);
                let g = gl.clone();
                let eye = [self.center[0], 100000.0, self.center[1]];
                let cb = egui_glow::CallbackFn::new(move |info, p| {
                    let mut s = g.lock().unwrap();
                    unsafe { s.paint(p.gl(), &info, &mvp, eye, [0.055, 0.06, 0.07, 1.0]) };
                });
                painter.add(egui::PaintCallback { rect, callback: std::sync::Arc::new(cb) });
            }
            Background::Minimap => {
                if let Some((t, _)) = &self.minimap_tex {
                    let r = ed.project.mob().and_then(|m| m.objects.iter().find(|o| o.kind == ObjectType::LevelSize)).map_or(wr, |ls| [ls.bbox_min[0], ls.bbox_min[2], ls.bbox_max[0], ls.bbox_max[2]]);
                    painter.image(t.id(), self.world_rect_screen(rect, r), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
            }
            Background::None => {}
        }
        if self.layers.nav
            && let Some((t, _)) = &self.nav_tex
        {
            painter.image(t.id(), self.world_rect_screen(rect, wr), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::from_white_alpha((self.layers.nav_alpha * 255.0) as u8));
        }
        painter.rect_stroke(self.world_rect_screen(rect, wr), 0.0, Stroke::new(1.0, Color32::from_gray(90)), StrokeKind::Outside);

        // Particles.
        if self.layers.particles
            && let Some(t) = ed.project.text("Particles.dat")
        {
            for line in t.lines() {
                let v: Vec<f32> = line.split_whitespace().skip(1).take(3).filter_map(|s| s.parse().ok()).collect();
                if v.len() == 3 {
                    let p = self.to_screen(rect, v[0], v[2]);
                    if rect.contains(p) {
                        painter.circle_filled(p, 2.5, Color32::from_rgb(255, 160, 40));
                    }
                }
            }
        }

        // Objects.
        let marks = markers(ed, &self.layers);
        let drag_offset = match (&self.drag, resp.interact_pointer_pos()) {
            (Some(d), Some(pp)) if d.moved => {
                let w = self.to_world(rect, pp);
                Some((d.sel.clone(), [w[0] - d.start_world[0], w[1] - d.start_world[1]]))
            }
            _ => None,
        };
        let mut hovered: Option<(Sel, f32)> = None;
        // Drags are hit-tested where the button went down (egui reports the drag a few pixels later).
        let press = ui.input(|i| i.pointer.press_origin());
        let mut pressed: Option<(Sel, f32)> = None;
        for m in &marks {
            let (mut x, mut z) = (m.pos[0], m.pos[2]);
            if let Some((s, d)) = &drag_offset
                && *s == m.sel
            {
                x += d[0];
                z += d[1];
            }
            let p = self.to_screen(rect, x, z);
            if !rect.expand(20.0).contains(p) {
                continue;
            }
            let r = m.radius * (self.scale / 0.05).clamp(0.6, 1.8);
            let selected = ed.sel == m.sel;
            match m.shape {
                1 => {
                    painter.rect_filled(Rect::from_center_size(p, Vec2::splat(r * 1.6)), 2.0, m.color);
                }
                2 => {
                    painter.circle_filled(p, r, m.color);
                    painter.circle_stroke(p, r * 0.55, Stroke::new(1.5, Color32::BLACK));
                }
                3 => {
                    let pts = vec![p + Vec2::new(0.0, -r), p + Vec2::new(r, 0.0), p + Vec2::new(0.0, r), p + Vec2::new(-r, 0.0)];
                    painter.add(egui::Shape::convex_polygon(pts, m.color, Stroke::new(1.0, Color32::BLACK)));
                }
                _ => {
                    painter.circle_filled(p, r, m.color);
                }
            }
            if selected {
                painter.circle_stroke(p, r + 5.0, Stroke::new(2.0, Color32::WHITE));
            }
            if self.layers.labels && (self.scale > 0.07 || selected) && m.shape != 0 {
                painter.text(p + Vec2::new(r + 3.0, 0.0), Align2::LEFT_CENTER, &m.name, FontId::proportional(11.0), Color32::from_gray(230));
            }
            if let Some(hp) = hover {
                let d = hp.distance(p);
                if d <= r + 4.0 && hovered.as_ref().is_none_or(|h| d < h.1) {
                    hovered = Some((m.sel.clone(), d));
                }
            }
            if let Some(pp) = press {
                let d = pp.distance(p);
                if d <= r + 4.0 && pressed.as_ref().is_none_or(|h| d < h.1) {
                    pressed = Some((m.sel.clone(), d));
                }
            }
        }

        // Tool interaction.
        self.hover_world = hover.map(|h| self.to_world(rect, h));
        match self.tool {
            Tool::Select => {
                if resp.drag_started_by(egui::PointerButton::Primary)
                    && let Some(pp) = press
                {
                    if let Some((s, _)) = &pressed {
                        ed.sel = s.clone();
                        self.drag = Some(DragState { sel: s.clone(), start_world: self.to_world(rect, pp), moved: false });
                    }
                }
                if resp.dragged_by(egui::PointerButton::Primary)
                    && let Some(d) = &mut self.drag
                    && resp.drag_delta().length() > 0.0
                {
                    d.moved = true;
                }
                if resp.drag_stopped() {
                    if let (Some(d), Some(pp)) = (self.drag.take(), resp.interact_pointer_pos())
                        && d.moved
                    {
                        let w = self.to_world(rect, pp);
                        let (dx, dz) = (w[0] - d.start_world[0], w[1] - d.start_world[1]);
                        let pos = match &d.sel {
                            Sel::Mob(n) => ed.project.mob().and_then(|m| m.find(n).map(|i| m.objects[i].position)),
                            Sel::Dsc(s) => ed.dsc_central(s),
                            _ => None,
                        };
                        if let Some(p0) = pos {
                            let dy = match (ed.ground_height(p0[0], p0[2]), ed.ground_height(p0[0] + dx, p0[2] + dz)) {
                                (Some(a), Some(b)) => b - a,
                                _ => 0.0,
                            };
                            if let Err(e) = ed.move_object(&d.sel, [dx, dy, dz]) {
                                self.status = Some(format!("Could not move: {e:#}"));
                            }
                            ed.end_stroke();
                        }
                    }
                    self.drag = None;
                }
                if resp.clicked_by(egui::PointerButton::Primary) {
                    ed.sel = hovered.clone().map(|h| h.0).unwrap_or(Sel::None);
                }
            }
            Tool::Paint => {
                if let Some(hp) = hover {
                    painter.circle_stroke(hp, self.radius * self.scale, Stroke::new(1.5, Color32::from_rgb(255, 255, 120)));
                }
                if (resp.dragged_by(egui::PointerButton::Primary) || resp.clicked_by(egui::PointerButton::Primary))
                    && let Some(pp) = resp.interact_pointer_pos()
                {
                    let w = self.to_world(rect, pp);
                    // Fill the gap since the last dab so fast strokes stay continuous.
                    let from = self.last_paint.unwrap_or(w);
                    let dist = (w[0] - from[0]).hypot(w[1] - from[1]);
                    let steps = (dist / (self.radius * 0.4).max(10.0)).ceil().max(1.0) as usize;
                    let height_tool = matches!(self.paint, NavPaint::Raise | NavPaint::Lower | NavPaint::Smooth);
                    for k in 1..=steps {
                        if height_tool && k < steps {
                            continue;
                        }
                        let t = k as f32 / steps as f32;
                        ed.paint_nav(self.paint, from[0] + (w[0] - from[0]) * t, from[1] + (w[1] - from[1]) * t, self.radius, self.strength);
                    }
                    self.last_paint = Some(w);
                    self.layers.nav = true;
                }
                if resp.drag_stopped() || resp.clicked() {
                    ed.end_stroke();
                    ed.flatten_height = None;
                    self.last_paint = None;
                }
            }
            Tool::Place | Tool::Duplicate => {
                if let Some(hp) = hover {
                    painter.circle_stroke(hp, 8.0, Stroke::new(2.0, Color32::from_rgb(120, 255, 255)));
                }
                if resp.clicked_by(egui::PointerButton::Primary)
                    && let Some(pp) = resp.interact_pointer_pos()
                {
                    let w = self.to_world(rect, pp);
                    let r = if self.tool == Tool::Place { ed.place_object(self.place.kind, &self.place.name.clone(), w) } else { ed.duplicate_selected(w) };
                    match r {
                        Ok(s) => {
                            self.status = Some(match &s {
                                Sel::Mob(n) | Sel::Dsc(n) => format!("Added {n}"),
                                _ => String::new(),
                            });
                            ed.sel = s;
                            if self.tool == Tool::Place
                                && let Some(m) = ed.project.mob()
                            {
                                self.place.name = m.unique_name(&lol_map_studio::edit::next_name(&self.place.name));
                            }
                        }
                        Err(e) => self.status = Some(format!("{e:#}")),
                    }
                }
            }
        }

        // Cursor readout.
        if let Some(w) = self.hover_world {
            let mut s = format!("x {:.0}  z {:.0}", w[0], w[1]);
            if let Some(g) = ed.project.navgrid()
                && let Some((i, j)) = g.cell_of(w[0], w[1])
            {
                let k = j * g.nx + i;
                let f = g.flags(k);
                s += &format!("  cell {i},{j}  height {:.1}  {}{}{}", g.height(k), if f & flags::WALL != 0 { "wall" } else { "walkable" }, if f & flags::BRUSH != 0 { ", brush" } else { "" }, if f & !(flags::WALL | flags::BRUSH) != 0 { format!(", flags {f:#x}") } else { String::new() });
            }
            if let Some((Sel::Mob(n) | Sel::Dsc(n), _)) = &hovered {
                s += &format!("  |  {n}");
            }
            painter.text(rect.left_bottom() + Vec2::new(8.0, -8.0), Align2::LEFT_BOTTOM, s, FontId::monospace(12.0), Color32::from_gray(220));
        }
        let _ = Doc::Raw;
    }
}
