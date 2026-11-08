//! Edit (3D): every map tool applied directly in the 3D view, on the terrain under the cursor.
//!
//! Camera (Roblox Studio style): right-drag orbits, middle-drag pans, wheel zooms, WASD moves,
//! F focuses the selection (or the cursor), Home shows the whole map. The left button belongs to
//! the active tool. Placement shows a see-through ghost of the real model; press, drag and release
//! to turn it (Shift snaps to the angle step); R / Shift+R turn by the step.

use super::gl_scene::{self, DynMesh, Mat4, Shared};
use super::layout_view::{LTool, LayoutDoc, LayoutView, SceneryMode};
use super::models::{Models, Place, objective_models};
use super::view3d::View3D;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use lol_map_studio::gamedata::GameData;
use lol_map_studio::layout::{CELL, ObjKind, Objective, Symmetry};
use lol_map_studio::layout_build::{Terrain, hash01, terrain};
use lol_map_studio::props::PropLibrary;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq)]
enum Moving {
    /// The whole selection, from the last ground point.
    Group([f32; 2]),
    /// Gizmo arrow along X (0) or Z (1), from the last ground point.
    GizmoAxis(usize, [f32; 2]),
    /// Gizmo up arrow, from the last screen height.
    GizmoLift(f32),
    /// Gizmo ring around a centre, from the last angle (radians).
    GizmoTurn([f32; 2], f32),
    Objective(u32, [f32; 2]),
    Prop(u32, [f32; 2]),
    RotateObjective(u32),
    RotateProp(u32),
}

pub struct Edit3D {
    terrain: Option<(u64, Arc<Terrain>, std::time::Instant)>,
    pub models: Models,
    last_paint: Option<[f32; 2]>,
    press: Option<[f32; 2]>,
    flatten_to: Option<f32>,
    poly: Vec<[f32; 2]>,
    moving: Option<Moving>,
    /// The next prop's turn and size (rolled ahead so the ghost shows exactly what is placed).
    next_yaw: f32,
    next_scale: f32,
    /// Turn for the next objective.
    obj_yaw: f32,
    seed: u32,
    pub snap_grid: f32,
    pub angle_step: f32,
    pub snap_angle: bool,
    start: std::time::Instant,
    /// True while a stroke is being painted (the app rebuilds the preview continuously then).
    pub painting: bool,
    /// Opened maps: walls and brush as imported (to show what was painted differently).
    base_was: Option<(String, Arc<Vec<u8>>)>,
    /// Screen point where a box selection started.
    box_start: Option<Pos2>,
    /// Saved camera views (target, yaw, pitch, distance).
    pub bookmarks: [Option<([f32; 3], f32, f32, f32)>; 9],
    /// The pathing view's meshes for a layout revision.
    pathing: Option<(u64, Vec<(Vec<lol_map_studio::render::RVertex>, Vec<u32>, [f32; 4])>)>,
}

/// Quads over blocked cells (red) and brush (green), merged into horizontal runs.
fn pathing_mesh(l: &lol_map_studio::layout::Layout, ground: &dyn Fn(f32, f32) -> f32) -> Vec<(Vec<lol_map_studio::render::RVertex>, Vec<u32>, [f32; 4])> {
    use lol_map_studio::layout::Surface;
    let mut out: Vec<(Vec<lol_map_studio::render::RVertex>, Vec<u32>, [f32; 4])> = vec![(Vec::new(), Vec::new(), [1.0, 0.25, 0.2, 0.45]), (Vec::new(), Vec::new(), [0.3, 1.0, 0.4, 0.4])];
    let kind = |s: u8| if s == Surface::Wall as u8 || s == Surface::Void as u8 || s == Surface::DeepWater as u8 { Some(0) } else if s == Surface::Brush as u8 { Some(1) } else { None };
    for j in 0..l.ny {
        let mut i = 0;
        while i < l.nx {
            let Some(k) = kind(l.surface[j * l.nx + i]) else {
                i += 1;
                continue;
            };
            let start = i;
            while i < l.nx && kind(l.surface[j * l.nx + i]) == Some(k) && i - start < 16 {
                i += 1;
            }
            let (x0, x1, z0, z1) = (start as f32 * CELL, i as f32 * CELL, j as f32 * CELL, (j + 1) as f32 * CELL);
            let (vs, is, _) = &mut out[k];
            let b = vs.len() as u32;
            for (x, z) in [(x0, z0), (x1, z0), (x1, z1), (x0, z1)] {
                vs.push(lol_map_studio::render::RVertex { pos: [x, ground(x, z) + 12.0, z], normal: [0.0, 1.0, 0.0], uv0: [0.0; 2], uv1: [0.0; 2], color: [0.1, 0.1, 0.1, 1.0] });
            }
            is.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
        }
    }
    out
}

impl Default for Edit3D {
    fn default() -> Self {
        Self {
            terrain: None,
            models: Models::default(),
            last_paint: None,
            press: None,
            flatten_to: None,
            poly: Vec::new(),
            moving: None,
            next_yaw: 0.0,
            next_scale: 1.0,
            obj_yaw: 0.0,
            seed: 7,
            snap_grid: 0.0,
            angle_step: 15.0,
            snap_angle: false,
            start: std::time::Instant::now(),
            painting: false,
            base_was: None,
            box_start: None,
            bookmarks: [None; 9],
            pathing: None,
        }
    }
}

fn invert(m: &Mat4) -> Option<Mat4> {
    let mut a = [[0.0f64; 8]; 4];
    for r in 0..4 {
        for c in 0..4 {
            a[r][c] = m[c * 4 + r] as f64;
        }
        a[r][4 + r] = 1.0;
    }
    for c in 0..4 {
        let p = (c..4).max_by(|&x, &y| a[x][c].abs().partial_cmp(&a[y][c].abs()).unwrap())?;
        if a[p][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, p);
        let d = a[c][c];
        for k in 0..8 {
            a[c][k] /= d;
        }
        for r in 0..4 {
            if r != c {
                let f = a[r][c];
                for k in 0..8 {
                    a[r][k] -= f * a[c][k];
                }
            }
        }
    }
    let mut out = [0.0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            out[c * 4 + r] = a[r][4 + c] as f32;
        }
    }
    Some(out)
}

fn ang(d: [f32; 2]) -> f32 {
    d[0].atan2(d[1]).to_degrees()
}

impl Edit3D {
    /// The imported walls and brush of an opened map (cached).
    pub fn base_surface(&mut self, l: &lol_map_studio::layout::Layout) -> Option<Arc<Vec<u8>>> {
        let base = l.base.as_ref()?;
        if self.base_was.as_ref().is_none_or(|b| b.0 != base.path) {
            let p = lol_map_studio::layout_base::base_project(&base.path).ok()?;
            let g = p.navgrid()?;
            self.base_was = Some((base.path.clone(), Arc::new(lol_map_studio::layout_import::grid_surface(g, l.nx, l.ny))));
        }
        self.base_was.as_ref().map(|b| b.1.clone())
    }

    /// Opened maps: their scene is drawn without structures, so structures, new props and painted
    /// walls / brush are drawn here (live, without rebuilding the map).
    pub fn base_extras(&mut self, glk: &mut gl_scene::GlScene, data: Option<&GameData>, l: &lol_map_studio::layout::Layout, lib: Option<&PropLibrary>, ground: &dyn Fn(f32, f32) -> f32) -> Vec<DynMesh> {
        let mut out = Vec::new();
        if l.base.is_none() {
            return out;
        }
        if let Some(d) = data {
            for o in l.objectives.iter().filter(|o| !o.kind.is_camp()) {
                for (skin, at, yw, monster) in objective_models(o) {
                    let pl = Place { x: at[0], y: ground(at[0], at[1]), z: at[1], yaw: yw, scale: 1.0 };
                    out.extend(self.models.unit(glk, d, &skin, monster, pl, [1.0; 4]));
                }
            }
        }
        if let Some(lib) = lib {
            for pr in &l.props {
                if let Some(def) = lib.get(&pr.prop) {
                    let pl = Place { x: pr.pos[0], y: ground(pr.pos[0], pr.pos[1]) + pr.lift + if def.is_decal() { 3.0 } else { 0.0 }, z: pr.pos[1], yaw: pr.yaw, scale: pr.scale };
                    out.extend(self.models.prop_ex(glk, def, pl, pr.tilt, pr.stretch, [1.0; 4]));
                }
            }
        }
        // Cells painted differently from the original map.
        if let Some(was) = self.base_surface(l) {
            use lol_map_studio::layout::Surface;
            let mut meshes: [(Vec<lol_map_studio::render::RVertex>, Vec<u32>); 3] = Default::default();
            for (k, (&now, &before)) in l.surface.iter().zip(was.iter()).enumerate() {
                if now == before {
                    continue;
                }
                let (which, lift) = if now == Surface::Wall as u8 || now == Surface::Void as u8 || now == Surface::DeepWater as u8 { (0, 70.0) } else if now == Surface::Brush as u8 { (1, 12.0) } else { (2, 8.0) };
                let (i, j) = ((k % l.nx) as f32, (k / l.nx) as f32);
                let (vs, is) = &mut meshes[which];
                let b = vs.len() as u32;
                for (dx, dz) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                    let (x, z) = ((i + dx) * CELL, (j + dz) * CELL);
                    vs.push(lol_map_studio::render::RVertex { pos: [x, ground(x, z) + lift, z], normal: [0.0, 1.0, 0.0], uv0: [0.0; 2], uv1: [0.0; 2], color: [0.1, 0.1, 0.1, 1.0] });
                }
                is.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
            }
            let tints = [[0.75, 0.45, 0.3, 0.6], [0.3, 0.9, 0.35, 0.5], [1.0, 1.0, 0.7, 0.4]];
            for ((vertices, indices), tint) in meshes.into_iter().zip(tints) {
                if !indices.is_empty() {
                    out.push(DynMesh { texture: None, vertices, indices, tint, cutout: false, particle: false, additive: false });
                }
            }
        }
        out
    }

    pub fn terrain_for(&mut self, doc: &LayoutDoc) -> Arc<Terrain> {
        // While painting, the terrain is re-derived at most ten times a second.
        let stale = match &self.terrain {
            None => true,
            Some((rev, _, at)) => *rev != doc.rev && (!self.painting || at.elapsed() > std::time::Duration::from_millis(100)),
        };
        if stale {
            self.terrain = Some((doc.rev, Arc::new(terrain(&doc.layout)), std::time::Instant::now()));
        }
        self.terrain.as_ref().unwrap().1.clone()
    }

    /// The terrain point under a screen position.
    fn pick(inv: &Mat4, rect: Rect, p: Pos2, t: &Terrain, size: [f32; 2]) -> Option<[f32; 3]> {
        let ndc = [(p.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.top()) / rect.height() * 2.0];
        let un = |z: f32| {
            let v = gl_scene::transform(inv, [ndc[0], ndc[1], z]);
            [v[0] / v[3], v[1] / v[3], v[2] / v[3]]
        };
        let (a, b) = (un(-1.0), un(1.0));
        let dir = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        let d = dir.map(|v| v / len.max(1e-6));
        let h = |x: f32, z: f32| if x < 0.0 || z < 0.0 || x > size[0] || z > size[1] { -500.0 } else { t.height_at(x, z) };
        let step = (len / 4000.0).clamp(8.0, 60.0);
        let mut prev = a;
        let mut s = 0.0;
        while s < len {
            s += step;
            let q = [a[0] + d[0] * s, a[1] + d[1] * s, a[2] + d[2] * s];
            if q[1] <= h(q[0], q[2]) {
                let (mut lo, mut hi) = (prev, q);
                for _ in 0..14 {
                    let m = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
                    if m[1] <= h(m[0], m[2]) {
                        hi = m;
                    } else {
                        lo = m;
                    }
                }
                return Some(hi);
            }
            prev = q;
        }
        None
    }

    fn snap(&self, w: [f32; 2]) -> [f32; 2] {
        if self.snap_grid > 0.0 { w.map(|v| (v / self.snap_grid).round() * self.snap_grid) } else { w }
    }

    fn snap_yaw(&self, y: f32, force: bool) -> f32 {
        if (self.snap_angle || force) && self.angle_step > 0.0 { (y / self.angle_step).round() * self.angle_step } else { y }
    }

    fn roll(&mut self, lv: &LayoutView) {
        self.seed = self.seed.wrapping_add(1);
        self.next_yaw = if lv.random_turn { self.snap_yaw(360.0 * hash01(self.seed, 7, 9), false) } else { 0.0 };
        self.next_scale = lv.prop_scale[0] + (lv.prop_scale[1] - lv.prop_scale[0]) * hash01(self.seed, 8, 9);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ui(&mut self, ui: &mut egui::Ui, doc: &mut LayoutDoc, lv: &mut LayoutView, cam: &mut View3D, gl: &Shared, lib: Option<&PropLibrary>, data: Option<&GameData>, monsters: &[lol_map_studio::jungle::Monster], fx: &mut super::fx::Fx) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let t = self.terrain_for(doc);
        let size = doc.layout.size();
        if cam.distance > 50000.0 || !cam.target[0].is_finite() {
            cam.fit([0.0, 0.0, 0.0, size[0], 300.0, size[1]]);
        }

        // ---- camera
        let typing = ui.ctx().egui_wants_keyboard_input();
        if !typing && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Home)) {
            cam.fit([0.0, 0.0, 0.0, size[0], 300.0, size[1]]);
        }
        let focus_key = !typing && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F));
        if !typing {
            lv.hotkeys(ui);
        }
        let (cy, sy) = (cam.yaw.cos(), cam.yaw.sin());
        let right = [sy, -cy];
        let fwd = [cy, sy];
        let pan = |dx: f32, dy: f32, k: f32, tg: &mut [f32; 3]| {
            tg[0] += (right[0] * dx + fwd[0] * dy) * k;
            tg[2] += (right[1] * dx + fwd[1] * dy) * k;
        };
        if resp.dragged_by(egui::PointerButton::Secondary) {
            let d = resp.drag_delta();
            cam.yaw -= d.x * 0.006;
            cam.pitch = (cam.pitch + d.y * 0.006).clamp(0.08, 1.55);
        }
        if resp.dragged_by(egui::PointerButton::Middle) {
            let d = resp.drag_delta();
            pan(-d.x, -d.y, cam.distance * 0.0015, &mut cam.target);
        }
        let mods = ui.input(|i| i.modifiers);
        if resp.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            // Ctrl+wheel resizes the brush or the next prop instead of zooming.
            if (mods.ctrl || mods.alt) && scroll != 0.0 && lv.tool == LTool::Select && lv.selected_prop.is_some() {
                // Selected prop: Ctrl+wheel resizes it, Alt+wheel raises or lowers it.
                let id = lv.selected_prop.unwrap();
                doc.begin();
                if let Some(pr) = doc.layout.props.iter_mut().find(|p| p.id == id) {
                    if mods.ctrl {
                        pr.scale = (pr.scale * (scroll * 0.002).exp()).clamp(0.02, 50.0);
                    } else {
                        pr.lift += scroll * 0.5;
                    }
                }
                doc.end();
            } else if mods.ctrl && scroll != 0.0 {
                let f = (scroll * 0.002).exp();
                if lv.tool == LTool::Scenery && lv.scenery == SceneryMode::Place {
                    self.next_scale = (self.next_scale * f).clamp(0.05, 10.0);
                } else {
                    lv.radius = (lv.radius * f).clamp(CELL, 3000.0);
                }
            } else {
                let f = zoom * (scroll * 0.002).exp();
                cam.distance = (cam.distance / f.max(0.01)).clamp(150.0, 60000.0);
            }
            if !typing && !mods.ctrl {
                let mut m = [0.0f32; 2];
                ui.input(|i| {
                    if i.key_down(egui::Key::W) || i.key_down(egui::Key::ArrowUp) {
                        m[1] -= 1.0;
                    }
                    if i.key_down(egui::Key::S) || i.key_down(egui::Key::ArrowDown) {
                        m[1] += 1.0;
                    }
                    if i.key_down(egui::Key::A) || i.key_down(egui::Key::ArrowLeft) {
                        m[0] -= 1.0;
                    }
                    if i.key_down(egui::Key::D) || i.key_down(egui::Key::ArrowRight) {
                        m[0] += 1.0;
                    }
                });
                if m != [0.0, 0.0] {
                    let k = if mods.shift { 0.05 } else { 0.02 };
                    pan(m[0], m[1], cam.distance * k, &mut cam.target);
                    ui.ctx().request_repaint();
                }
            }
        }
        let aspect = rect.width() / rect.height().max(1.0);
        let mvp = cam.mvp(aspect);
        let Some(inv) = invert(&mvp) else { return };
        let hover = resp.hover_pos().or(ui.input(|i| i.pointer.interact_pos()).filter(|p| rect.contains(*p)));
        let hit = hover.and_then(|p| Self::pick(&inv, rect, p, &t, size));
        let world = hit.map(|h| self.snap([h[0], h[2]]));
        // Camera bookmarks: Ctrl+1..9 saves the view, 1..9 returns to it.
        if !typing {
            let keys = [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5, egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9];
            for (n, k) in keys.iter().enumerate() {
                if ui.input(|i| i.key_pressed(*k)) {
                    if mods.ctrl {
                        self.bookmarks[n] = Some((cam.target, cam.yaw, cam.pitch, cam.distance));
                    } else if let Some((t, y, p, d)) = self.bookmarks[n] {
                        cam.target = t;
                        cam.yaw = y;
                        cam.pitch = p;
                        cam.distance = d;
                    }
                }
            }
        }
        if let Some(p) = lv.focus_on.take() {
            cam.focus([p[0], t.height_at(p[0].clamp(0.0, size[0]), p[1].clamp(0.0, size[1])), p[1]]);
        }
        if focus_key {
            let sel = lv.selected.and_then(|id| doc.layout.objective(id).map(|o| o.pos)).or_else(|| lv.selected_prop.and_then(|id| doc.layout.props.iter().find(|p| p.id == id).map(|p| p.pos))).or(world);
            if let Some(p) = sel {
                cam.focus([p[0], t.height_at(p[0], p[1]), p[1]]);
            }
        }

        // ---- input state for tools
        let pressed = resp.hovered() && ui.input(|i| i.pointer.primary_pressed());
        let down = ui.input(|i| i.pointer.primary_down()) && (resp.is_pointer_button_down_on() || self.press.is_some());
        let released = ui.input(|i| i.pointer.primary_released());
        if pressed {
            self.press = world;
        }
        let press = self.press;

        // ---- the scene (drawn first; overlays below are painted on top of it)
        let eye = cam.eye_world();
        let g = gl.clone();
        let cb = egui_glow::CallbackFn::new(move |info, p| {
            let mut s = g.lock().unwrap();
            unsafe { s.paint(p.gl(), &info, &mvp, eye, [0.06, 0.07, 0.09, 1.0]) };
        });
        ui.painter().add(egui::PaintCallback { rect, callback: Arc::new(cb) });
        let painter = ui.painter_at(rect);
        let project = |p: [f32; 3]| -> Option<Pos2> {
            let c = gl_scene::transform(&mvp, p);
            (c[3] > 1.0).then(|| Pos2::new(rect.left() + (c[0] / c[3] + 1.0) / 2.0 * rect.width(), rect.top() + (1.0 - c[1] / c[3]) / 2.0 * rect.height()))
        };
        let ground = |x: f32, z: f32| t.height_at(x.clamp(0.0, size[0]), z.clamp(0.0, size[1]));
        let ring = |c: [f32; 2], r: f32, col: Color32, w: f32| {
            let n = ((r / 40.0) as usize).clamp(24, 96);
            let pts: Vec<Pos2> = (0..=n)
                .filter_map(|i| {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    let (x, z) = (c[0] + a.cos() * r, c[1] + a.sin() * r);
                    project([x, ground(x, z) + 6.0, z])
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(w, col)));
        };
        let path = |pts: &[[f32; 2]], closed: bool, col: Color32, w: f32| {
            let mut all: Vec<[f32; 2]> = Vec::new();
            let n = pts.len();
            let segs = if closed { n } else { n.saturating_sub(1) };
            for k in 0..segs {
                let (a, b) = (pts[k], pts[(k + 1) % n]);
                let len = (b[0] - a[0]).hypot(b[1] - a[1]);
                let steps = ((len / 60.0) as usize).clamp(1, 200);
                for s in 0..steps {
                    let f = s as f32 / steps as f32;
                    all.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]);
                }
            }
            if let Some(l) = if closed { pts.first() } else { pts.last() } {
                all.push(*l);
            }
            let sp: Vec<Pos2> = all.iter().filter_map(|p| project([p[0], ground(p[0], p[1]) + 8.0, p[1]])).collect();
            painter.add(egui::Shape::line(sp, Stroke::new(w, col)));
        };
        let mut extras: Vec<DynMesh> = Vec::new();
        // Particle effects to run this frame: (key, effect, position).
        let mut effect_items: Vec<(String, String, [f32; 3])> = Vec::new();
        // Effects near the view (opened maps list hundreds); sound emitters have nothing to draw.
        let reach = cam.distance * 1.5 + 2500.0;
        let mut near: Vec<(f32, &lol_map_studio::layout::EffectPlace)> = doc.layout.effects.iter().filter(|e| !e.name.to_ascii_lowercase().starts_with("audio")).map(|e| ((e.pos[0] - cam.target[0]).hypot(e.pos[1] - cam.target[2]), e)).filter(|x| x.0 < reach).collect();
        near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for (_, e) in near.into_iter().take(64) {
            effect_items.push((format!("e{}", e.id), e.name.clone(), [e.pos[0], ground(e.pos[0], e.pos[1]) + e.lift, e.pos[1]]));
        }
        // Relics and shrines are invisible units with particle auras in the game.
        for o in &doc.layout.objectives {
            let name = match o.kind {
                ObjKind::HealthRelic => "HA_AP_HealthRelic",
                ObjKind::SpeedShrine => "OdinSpeedShrine",
                ObjKind::CenterRelic => "OdinCenterRelic",
                _ => continue,
            };
            if let Some(fx_name) = super::fx::unit_effect(name) {
                effect_items.push((format!("o{}", o.id), fx_name.to_string(), [o.pos[0], ground(o.pos[0], o.pos[1]) + 20.0, o.pos[1]]));
            }
        }
        let time = self.start.elapsed().as_secs_f32();
        let mut glk = gl.lock().unwrap();

        // Animated jungle monsters, and (opened maps) structures, new props and painted cells.
        if let Some(d) = data {
            extras.extend(self.models.monsters(&mut glk, d, monsters, &|x, z| ground(x, z), time));
        }
        if doc.layout.base.is_some() {
            let more = self.base_extras(&mut glk, data, &doc.layout, lib, &|x, z| ground(x, z));
            extras.extend(more);
        }

        // ---- tools
        let surface_col = Color32::from_rgb(255, 230, 90);
        // Prefab / paste placement: the objects follow the cursor; click places, Esc cancels.
        let mut placing_now = false;
        if let Some(cb) = lv.placing.clone() {
            placing_now = true;
            if let Some(w) = world {
                let mut pts: Vec<[f32; 2]> = cb.props.iter().map(|p| p.pos).collect();
                pts.extend(cb.objectives.iter().map(|o| o.pos));
                pts.extend(cb.effects.iter().map(|e| e.pos));
                pts.extend(cb.lights.iter().map(|l| l.pos));
                for q in &pts {
                    ring([w[0] + q[0], w[1] + q[1]], 50.0, Color32::from_rgb(120, 255, 140), 1.5);
                }
                if let Some(lb) = lib {
                    for p in cb.props.iter().take(60) {
                        if let Some(def) = lb.get(&p.prop) {
                            let (x, z) = (w[0] + p.pos[0], w[1] + p.pos[1]);
                            let pl = Place { x, y: ground(x, z) + p.lift, z, yaw: p.yaw, scale: p.scale };
                            extras.extend(self.models.prop_ex(&mut glk, def, pl, p.tilt, p.stretch, [1.0, 1.0, 1.0, 0.5]));
                        }
                    }
                }
                if resp.clicked() {
                    doc.begin();
                    let new = doc.layout.paste(&cb, w);
                    doc.end();
                    lv.clear_selection();
                    lv.multi = new;
                    if !mods.shift {
                        lv.placing = None;
                    }
                }
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                lv.placing = None;
            }
        }
        let mirrored = |p: [f32; 2]| doc.layout.mirrored(p);
        let hint: String;
        match lv.tool {
            _ if placing_now => {
                hint = "Click: place the group (Shift+click: place again)   Esc: cancel".into();
            }
            LTool::Sculpt if lv.sculpt == lol_map_studio::layout::SculptMode::Ramp => {
                if let (Some(a), Some(b)) = (press, world)
                    && down
                {
                    path(&[a, b], false, Color32::from_rgb(255, 230, 90), 2.0);
                    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
                    let l = dx.hypot(dz).max(1.0);
                    let n = [-dz / l * lv.radius, dx / l * lv.radius];
                    path(&[[a[0] + n[0], a[1] + n[1]], [b[0] + n[0], b[1] + n[1]]], false, Color32::from_rgba_unmultiplied(255, 230, 90, 120), 1.0);
                    path(&[[a[0] - n[0], a[1] - n[1]], [b[0] - n[0], b[1] - n[1]]], false, Color32::from_rgba_unmultiplied(255, 230, 90, 120), 1.0);
                }
                if released
                    && let (Some(a), Some(b)) = (press, world)
                    && (b[0] - a[0]).hypot(b[1] - a[1]) > 50.0
                {
                    doc.begin();
                    lol_map_studio::worldtools::ramp(&mut doc.layout, a, b, lv.radius * 2.0);
                    doc.end();
                }
                hint = "Drag from the low end to the high end: a ramp between their heights".into();
            }
            LTool::Measure => {
                if let (Some(a), Some(b)) = (press, world)
                    && down
                {
                    path(&[a, b], false, Color32::WHITE, 2.0);
                    let d = (b[0] - a[0]).hypot(b[1] - a[1]);
                    let dh = ground(b[0], b[1]) - ground(a[0], a[1]);
                    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                    if let Some(s) = project([mid[0], ground(mid[0], mid[1]) + 40.0, mid[1]]) {
                        painter.text(s, Align2::CENTER_BOTTOM, format!("{d:.0} units  (height {dh:+.0})\n≈ {:.1} s at 345 move speed", d / 345.0), FontId::proportional(14.0), Color32::WHITE);
                    }
                    ring(a, d, Color32::from_white_alpha(70), 1.0);
                }
                hint = "Drag: measure a distance (and the time a champion takes to walk it)".into();
            }
            LTool::Brush | LTool::Texture | LTool::Sculpt => {
                if let Some(w) = world {
                    for (k, m) in mirrored(w).into_iter().enumerate() {
                        ring(m, lv.radius, if k == 0 { surface_col } else { surface_col.gamma_multiply(0.4) }, if k == 0 { 2.0 } else { 1.0 });
                    }
                }
                if down && let Some(w) = world {
                    doc.begin();
                    self.painting = true;
                    let from = self.last_paint.unwrap_or(w);
                    let dist = (w[0] - from[0]).hypot(w[1] - from[1]);
                    let steps = (dist / (lv.radius * 0.35).max(CELL)).ceil().max(1.0) as usize;
                    if lv.tool == LTool::Sculpt && lv.sculpt == lol_map_studio::layout::SculptMode::Flatten && self.flatten_to.is_none() {
                        self.flatten_to = doc.layout.cell_of(w[0], w[1]).map(|(i, j)| doc.layout.height[j * doc.layout.nx + i]);
                    }
                    for k in 1..=steps {
                        let f = k as f32 / steps as f32;
                        let (x, z) = (from[0] + (w[0] - from[0]) * f, from[1] + (w[1] - from[1]) * f);
                        match lv.tool {
                            LTool::Brush => doc.layout.paint_surface(x, z, lv.radius, lv.square, lv.surface),
                            LTool::Texture => doc.layout.paint_texture(x, z, lv.radius, lv.layer, lv.strength * 0.4),
                            _ => {
                                if k == steps {
                                    doc.layout.sculpt(x, z, lv.radius, lv.strength * 18.0, lv.sculpt, self.flatten_to.unwrap_or(0.0));
                                }
                            }
                        }
                    }
                    self.last_paint = Some(w);
                }
                hint = "Left drag: paint   Ctrl+wheel: brush size".into();
            }
            LTool::Line | LTool::Rect | LTool::Ellipse => {
                if let (Some(a), Some(b)) = (press, world)
                    && down
                {
                    let shape = shape_points(lv.tool, a, b);
                    for (k, m) in mirrored([0.0, 0.0]).iter().enumerate() {
                        let _ = m;
                        let pts: Vec<[f32; 2]> = if k == 0 { shape.clone() } else { shape.iter().map(|p| doc.layout.mirrored(*p).get(k).copied().unwrap_or(*p)).collect() };
                        let col = if k == 0 { surface_col } else { surface_col.gamma_multiply(0.4) };
                        path(&pts, lv.tool != LTool::Line, col, if lv.tool == LTool::Line { 2.0 } else { 2.0 });
                    }
                    if lv.tool == LTool::Line {
                        for p in [a, b] {
                            ring(p, lv.line_width / 2.0, surface_col.gamma_multiply(0.6), 1.0);
                        }
                    }
                }
                if released
                    && let (Some(a), Some(b)) = (press, world)
                    && (b[0] - a[0]).hypot(b[1] - a[1]) > 20.0
                {
                    doc.begin();
                    if lv.tool == LTool::Line {
                        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
                        let steps = (len / (CELL * 0.7)).ceil().max(1.0) as usize;
                        for k in 0..=steps {
                            let f = k as f32 / steps as f32;
                            doc.layout.paint_surface(a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, lv.line_width / 2.0, false, lv.surface);
                        }
                    } else {
                        doc.layout.fill_polygon(&shape_points(lv.tool, a, b), lv.surface);
                    }
                    doc.end();
                }
                hint = "Left drag on the ground: draw the shape".into();
            }
            LTool::Polygon | LTool::Lane => {
                let finish = ui.input(|i| i.key_pressed(egui::Key::Enter)) || (resp.double_clicked() && self.poly.len() >= 3);
                if resp.clicked() && let Some(w) = world {
                    self.poly.push(w);
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.poly.clear();
                }
                let mut pts = self.poly.clone();
                if let Some(w) = world {
                    pts.push(w);
                }
                if pts.len() >= 2 {
                    path(&pts, lv.tool == LTool::Polygon, if lv.tool == LTool::Lane { Color32::from_rgb(255, 230, 120) } else { surface_col }, 2.0);
                }
                if finish {
                    let p = std::mem::take(&mut self.poly);
                    doc.begin();
                    if lv.tool == LTool::Polygon && p.len() >= 3 {
                        doc.layout.fill_polygon(&p, lv.surface);
                    } else if lv.tool == LTool::Lane && p.len() >= 2 {
                        doc.layout.set_lane(lv.lane, p);
                    }
                    doc.end();
                }
                hint = format!("Click: add point ({})   Enter or double-click: finish   Esc: cancel", self.poly.len());
            }
            LTool::Fill => {
                if resp.clicked() && let Some(w) = world {
                    doc.begin();
                    doc.layout.flood_fill(w[0], w[1], lv.surface);
                    doc.end();
                }
                hint = "Click: fill the connected area".into();
            }
            LTool::Objective => {
                if !typing && ui.input(|i| i.key_pressed(egui::Key::R)) {
                    self.obj_yaw += if mods.shift { -self.angle_step } else { self.angle_step };
                }
                let at = if down { press } else { world };
                let mut yaw = self.obj_yaw;
                if let (Some(a), Some(b)) = (press, world)
                    && down
                    && (b[0] - a[0]).hypot(b[1] - a[1]) > 30.0
                {
                    yaw = self.snap_yaw(ang([b[0] - a[0], b[1] - a[1]]), mods.shift);
                    path(&[a, b], false, Color32::WHITE, 1.5);
                }
                if let (Some(p), Some(d)) = (at, data) {
                    let kinds: Vec<(ObjKind, [f32; 2])> = if lv.fountain { vec![(ObjKind::SpawnPoint, p)] } else { vec![(lv.obj_kind, p)] };
                    for (kind, p) in kinds {
                        let mut targets = vec![(p, yaw, lv.team)];
                        if doc.layout.symmetry != Symmetry::None && !kind.is_epic() {
                            targets.push((doc.layout.mirror_point(p), yaw + 180.0, 3 - lv.team));
                        }
                        for (k, (q, y, team)) in targets.into_iter().enumerate() {
                            let o = Objective { id: 0, kind, team, lane: lv.lane, tier: lv.tier, pos: q, yaw: y, mirror: None, name: None, skin: None };
                            let tint = if k == 0 { [1.0, 1.0, 1.0, 0.55] } else { [1.0, 1.0, 1.0, 0.25] };
                            for (skin, at, yw, monster) in objective_models(&o) {
                                let pl = Place { x: at[0], y: ground(at[0], at[1]), z: at[1], yaw: yw, scale: 1.0 };
                                if let Some(m) = self.models.unit(&mut glk, d, &skin, monster, pl, tint) {
                                    extras.push(m);
                                }
                            }
                            ring(q, kind.radius(), if k == 0 { Color32::WHITE } else { Color32::from_white_alpha(80) }, 1.5);
                        }
                    }
                }
                if released
                    && let Some(a) = press
                {
                    doc.begin();
                    let id = if lv.fountain { doc.layout.add_fountain(lv.team, a) } else { doc.layout.add_objective(lv.obj_kind, lv.team, lv.lane, lv.tier, a) };
                    let partner = doc.layout.objective(id).and_then(|o| o.mirror);
                    if let Some(o) = doc.layout.objectives.iter_mut().find(|o| o.id == id) {
                        o.yaw = yaw;
                    }
                    if let Some(m) = partner
                        && let Some(o) = doc.layout.objectives.iter_mut().find(|o| o.id == m)
                    {
                        o.yaw = yaw + 180.0;
                    }
                    doc.end();
                    lv.selected = Some(id);
                }
                hint = "Click: place   Drag: place and turn   R / Shift+R: turn".into();
            }
            LTool::Scenery => {
                let def = lv.prop_key.as_ref().and_then(|k| lib.and_then(|l| l.get(k)));
                match lv.scenery {
                    SceneryMode::Place => {
                        if !typing && ui.input(|i| i.key_pressed(egui::Key::R)) {
                            self.next_yaw += if mods.shift { -self.angle_step } else { self.angle_step };
                        }
                        let at = if down { press } else { world };
                        let mut yaw = self.next_yaw;
                        if let (Some(a), Some(b)) = (press, world)
                            && down
                            && (b[0] - a[0]).hypot(b[1] - a[1]) > 30.0
                        {
                            // Props turn so the drag direction becomes their front.
                            yaw = self.snap_yaw(-ang([b[0] - a[0], b[1] - a[1]]), mods.shift);
                            path(&[a, b], false, Color32::WHITE, 1.5);
                        }
                        if let (Some(p), Some(def)) = (at, def) {
                            let mut targets = vec![(p, yaw, 0.55f32)];
                            if doc.layout.symmetry != Symmetry::None {
                                targets.push((doc.layout.mirror_point(p), yaw + 180.0, 0.25));
                            }
                            for (q, y, alpha) in targets {
                                let pl = Place { x: q[0], y: ground(q[0], q[1]) + if def.is_decal() { 3.0 } else { 0.0 }, z: q[1], yaw: y, scale: self.next_scale };
                                extras.extend(self.models.prop(&mut glk, def, pl, [1.0, 1.0, 1.0, alpha]));
                                ring(q, (def.radius * self.next_scale).max(20.0), Color32::from_white_alpha((alpha * 255.0) as u8), 1.0);
                            }
                        }
                        if released
                            && let (Some(a), Some(k)) = (press, lv.prop_key.clone())
                        {
                            doc.begin();
                            let id = doc.layout.add_prop(&k, a, yaw, self.next_scale);
                            let mut ids = vec![(id, a, yaw)];
                            if doc.layout.symmetry != Symmetry::None {
                                let m = doc.layout.mirror_point(a);
                                let id2 = doc.layout.add_prop(&k, m, yaw + 180.0, self.next_scale);
                                ids.push((id2, m, yaw + 180.0));
                            }
                            if lv.align_slope {
                                for (pid, at, y) in ids {
                                    let e = 40.0;
                                    let g = [(ground(at[0] + e, at[1]) - ground(at[0] - e, at[1])) / (2.0 * e), (ground(at[0], at[1] + e) - ground(at[0], at[1] - e)) / (2.0 * e)];
                                    let tilt = lol_map_studio::layout::slope_tilt(g, y);
                                    if let Some(p) = doc.layout.props.iter_mut().find(|p| p.id == pid) {
                                        p.tilt = tilt;
                                    }
                                }
                            }
                            doc.end();
                            lv.selected_prop = Some(id);
                            self.roll(lv);
                        }
                        hint = if def.is_some() { "Click: place   Drag: place and turn   R: turn   Ctrl+wheel: size".into() } else { "Choose a prop in the scenery list (left)".into() };
                    }
                    SceneryMode::Path => {
                        if let (Some(a), Some(b), Some(def), Some(k)) = (press, world, def, lv.prop_key.clone()) {
                            let step = def.radius * 2.0 * lv.prop_scale[1] * lv.path_gap;
                            if pressed {
                                lv.path_line = vec![a];
                            }
                            if lv.path_free && lv.path_line.last().is_some_and(|l| (l[0] - b[0]).hypot(l[1] - b[1]) > 20.0) {
                                lv.path_line.push(b);
                            }
                            let spots = if lv.path_free {
                                let mut line = lv.path_line.clone();
                                line.push(b);
                                lol_map_studio::layout::line_spots(&line, step, lv.path_turn)
                            } else {
                                lol_map_studio::layout::path_spots(a, b, step, lv.path_turn)
                            };
                            if released {
                                lv.path_line.clear();
                            }
                            if down {
                                if lv.path_free {
                                    let line = lv.path_line.clone();
                                    path(&line, false, Color32::WHITE, 1.5);
                                } else {
                                    path(&[a, b], false, Color32::WHITE, 1.5);
                                }
                                for (q, y) in spots.iter().take(80) {
                                    let pl = Place { x: q[0], y: ground(q[0], q[1]), z: q[1], yaw: *y, scale: lv.prop_scale[1] };
                                    extras.extend(self.models.prop(&mut glk, def, pl, [1.0, 1.0, 1.0, 0.5]));
                                }
                            }
                            if released && spots.len() > 1 {
                                doc.begin();
                                for (q, y) in &spots {
                                    doc.layout.add_prop(&k, *q, *y, lv.prop_scale[1]);
                                }
                                doc.end();
                            }
                        }
                        hint = if lv.path_free { "Draw: a row of the chosen prop following your stroke, exactly spaced and turned along it" } else { "Drag: a straight row of the chosen prop (spacing and turn in the options bar)" }.into();
                    }
                    SceneryMode::Scatter | SceneryMode::Erase => {
                        if let Some(w) = world {
                            ring(w, lv.radius, if lv.scenery == SceneryMode::Erase { Color32::RED } else { Color32::from_rgb(120, 255, 140) }, 2.0);
                        }
                        if down && let Some(w) = world {
                            doc.begin();
                            self.painting = true;
                            if lv.scenery == SceneryMode::Erase {
                                doc.layout.erase_props(w[0], w[1], lv.radius);
                            } else if let Some(lb) = lib {
                                let cands: Vec<String> = match &lv.prop_key {
                                    Some(k) => vec![k.clone()],
                                    None => lb.props.iter().filter(|p| p.category == lv.prop_cat && PropLibrary::allowed(&p.key, &doc.layout.asset_maps)).map(|p| p.key.clone()).collect(),
                                };
                                if !cands.is_empty() {
                                    self.seed = self.seed.wrapping_add(17);
                                    doc.layout.scatter_props(&cands, w[0], w[1], lv.radius, lv.scatter_spacing, lv.prop_scale, self.seed);
                                }
                            }
                        }
                        hint = "Left drag: paint props".into();
                    }
                }
            }
            LTool::Effects => {
                if let Some(w) = world {
                    if lv.fx_light {
                        ring(w, lv.light_outer, Color32::from_rgb(lv.light_color[0], lv.light_color[1], lv.light_color[2]), 1.5);
                    } else if let Some(n) = &lv.fx_name {
                        effect_items.push(("ghost".into(), n.clone(), [w[0], ground(w[0], w[1]) + lv.fx_lift, w[1]]));
                        ring(w, 60.0, Color32::from_rgb(255, 170, 70), 1.5);
                    }
                }
                if resp.clicked() && let Some(w) = world {
                    let near = hover.and_then(|pp| {
                        let d = |q: [f32; 2], lift: f32| project([q[0], ground(q[0], q[1]) + lift, q[1]]).map_or(f32::MAX, |s| s.distance(pp));
                        doc.layout.effects.iter().map(|e| (e.id, d(e.pos, e.lift))).chain(doc.layout.lights.iter().map(|l| (l.id, d(l.pos, l.lift)))).filter(|x| x.1 < 16.0).min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).map(|x| x.0)
                    });
                    if let Some(id) = near {
                        lv.selected_fx = Some(id);
                    } else {
                        doc.begin();
                        let id = if lv.fx_light {
                            Some(doc.layout.add_light(w, lv.fx_lift.max(60.0), lv.light_color, lv.light_inner, lv.light_outer, lv.light_intensity))
                        } else {
                            lv.fx_name.clone().map(|n| doc.layout.add_effect(&n, w, lv.fx_lift, 0.0))
                        };
                        doc.end();
                        lv.selected_fx = id;
                    }
                }
                if !typing && ui.input(|i| i.key_pressed(egui::Key::Delete)) && let Some(id) = lv.selected_fx.take() {
                    doc.begin();
                    doc.layout.effects.retain(|e| e.id != id);
                    doc.layout.lights.retain(|x| x.id != id);
                    doc.end();
                }
                hint = "Click: place (the effect plays live)   Click a marker: select   Del: delete".into();
            }
            LTool::Select => {
                use lol_map_studio::layout::ItemRef;
                // What is under the cursor: objectives, effects and lights by their screen position,
                // props by their footprint.
                let hit_at = |pp: Pos2| -> Option<ItemRef> {
                    let near = |q: [f32; 2], lift: f32| project([q[0], ground(q[0], q[1]) + lift, q[1]]).map_or(f32::MAX, |s| s.distance(pp));
                    let mut best: Option<(ItemRef, f32)> = None;
                    let mut consider = |r: ItemRef, d: f32| {
                        if d < 26.0 && best.is_none_or(|b| d < b.1) {
                            best = Some((r, d));
                        }
                    };
                    for o in &doc.layout.objectives {
                        consider(ItemRef::Objective(o.id), near(o.pos, 60.0));
                    }
                    if lv.show_props {
                        for e in &doc.layout.effects {
                            consider(ItemRef::Effect(e.id), near(e.pos, e.lift));
                        }
                        for l in &doc.layout.lights {
                            consider(ItemRef::Light(l.id), near(l.pos, l.lift));
                        }
                    }
                    best.map(|b| b.0)
                };
                let prop_at = |w: [f32; 2]| {
                    doc.layout
                        .props
                        .iter()
                        .map(|p| (p.id, (p.pos[0] - w[0]).hypot(p.pos[1] - w[1]), lib.and_then(|l| l.get(&p.prop)).map_or(60.0, |d| d.radius * p.scale).max(40.0)))
                        .filter(|(_, d, r)| d <= r)
                        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                        .map(|x| ItemRef::Prop(x.0))
                };
                let locked = doc.layout.locked.clone();
                let id_of = |r: ItemRef| match r {
                    ItemRef::Prop(i) | ItemRef::Objective(i) | ItemRef::Effect(i) | ItemRef::Light(i) => i,
                };
                let hit = hover.and_then(hit_at).or_else(|| world.and_then(prop_at)).filter(|r| !locked.contains(&id_of(*r)));
                let set = lv.selection();
                // The gizmo at the selection's centre: X / Z arrows move, the up arrow lifts,
                // the ring turns (a group turns around its centre).
                let mut grabbed = false;
                let pts: Vec<[f32; 2]> = set.iter().filter_map(|r| doc.layout.item_pos(*r)).collect();
                if !pts.is_empty() && self.moving.is_none() || matches!(self.moving, Some(Moving::GizmoAxis(..) | Moving::GizmoLift(_) | Moving::GizmoTurn(..))) {
                    let c = if pts.is_empty() { [0.0, 0.0] } else { [pts.iter().map(|q| q[0]).sum::<f32>() / pts.len() as f32, pts.iter().map(|q| q[1]).sum::<f32>() / pts.len() as f32] };
                    let gy = ground(c[0], c[1]) + 40.0;
                    let len = (cam.distance * 0.07).clamp(80.0, 700.0);
                    let o = project([c[0], gy, c[1]]);
                    let ends = [project([c[0] + len, gy, c[1]]), project([c[0], gy, c[1] + len]), project([c[0], gy + len, c[1]])];
                    let cols = [Color32::from_rgb(235, 80, 70), Color32::from_rgb(80, 150, 255), Color32::from_rgb(90, 220, 100)];
                    let ring_pts: Vec<Pos2> = (0..=48).filter_map(|k| {
                        let a = k as f32 / 48.0 * std::f32::consts::TAU;
                        project([c[0] + a.cos() * len * 0.75, gy, c[1] + a.sin() * len * 0.75])
                    }).collect();
                    if !pts.is_empty() && let Some(o) = o {
                        painter.add(egui::Shape::line(ring_pts.clone(), Stroke::new(2.0, Color32::from_rgb(255, 210, 70))));
                        for (e, col) in ends.iter().zip(cols) {
                            if let Some(e) = e {
                                painter.line_segment([o, *e], Stroke::new(3.0, col));
                                painter.circle_filled(*e, 6.0, col);
                            }
                        }
                        painter.circle_filled(o, 4.0, Color32::WHITE);
                    }
                    let seg_d = |p: Pos2, a: Pos2, b: Pos2| {
                        let ab = b - a;
                        let t = ((p - a).dot(ab) / ab.length_sq().max(1.0)).clamp(0.0, 1.0);
                        (a + ab * t).distance(p)
                    };
                    if pressed && !pts.is_empty() && !mods.ctrl && let (Some(pp), Some(o)) = (hover, o) {
                        let mut best: Option<(f32, u8)> = None;
                        for (k, e) in ends.iter().enumerate() {
                            if let Some(e) = e {
                                let d = seg_d(pp, o, *e);
                                if d < 9.0 && best.is_none_or(|b| d < b.0) {
                                    best = Some((d, k as u8));
                                }
                            }
                        }
                        let ring_d = ring_pts.windows(2).map(|w| seg_d(pp, w[0], w[1])).fold(f32::MAX, f32::min);
                        if ring_d < 7.0 && best.is_none_or(|b| ring_d < b.0) {
                            best = Some((ring_d, 3));
                        }
                        if let Some((_, k)) = best {
                            let w = world.unwrap_or(c);
                            self.moving = Some(match k {
                                0 => Moving::GizmoAxis(0, w),
                                1 => Moving::GizmoAxis(1, w),
                                2 => Moving::GizmoLift(pp.y),
                                _ => Moving::GizmoTurn(c, (w[1] - c[1]).atan2(w[0] - c[0])),
                            });
                            grabbed = true;
                        }
                    }
                    if resp.dragged_by(egui::PointerButton::Primary) {
                        match self.moving {
                            Some(Moving::GizmoAxis(ax, last)) => {
                                if let Some(w) = world {
                                    let mut d = [0.0, 0.0];
                                    d[ax] = w[ax] - last[ax];
                                    if self.snap_grid > 0.0 {
                                        d[ax] = (d[ax] / self.snap_grid).trunc() * self.snap_grid;
                                    }
                                    if d[ax] != 0.0 {
                                        doc.begin();
                                        for r in &set {
                                            doc.layout.shift_item(*r, d);
                                        }
                                        let mut nl = last;
                                        nl[ax] += d[ax];
                                        self.moving = Some(Moving::GizmoAxis(ax, nl));
                                    }
                                }
                            }
                            Some(Moving::GizmoLift(last)) => {
                                if let Some(pp) = hover {
                                    let dy = (last - pp.y) * cam.distance * 0.0012;
                                    if dy != 0.0 {
                                        doc.begin();
                                        for r in &set {
                                            doc.layout.lift_item(*r, dy);
                                        }
                                        self.moving = Some(Moving::GizmoLift(pp.y));
                                    }
                                }
                            }
                            Some(Moving::GizmoTurn(c, last)) => {
                                if let Some(w) = world {
                                    let now = (w[1] - c[1]).atan2(w[0] - c[0]);
                                    let mut d = now - last;
                                    if d > std::f32::consts::PI {
                                        d -= std::f32::consts::TAU;
                                    } else if d < -std::f32::consts::PI {
                                        d += std::f32::consts::TAU;
                                    }
                                    let step = if self.snap_angle || mods.shift { self.angle_step.max(1.0).to_radians() } else { 0.0 };
                                    let d = if step > 0.0 { (d / step).trunc() * step } else { d };
                                    if d != 0.0 {
                                        doc.begin();
                                        let (sn, cs) = d.sin_cos();
                                        for r in &set {
                                            if set.len() > 1 && let Some(p) = doc.layout.item_pos(*r) {
                                                let (dx, dz) = (p[0] - c[0], p[1] - c[1]);
                                                let np = [c[0] + dx * cs - dz * sn, c[1] + dx * sn + dz * cs];
                                                doc.layout.shift_item(*r, [np[0] - p[0], np[1] - p[1]]);
                                            }
                                            doc.layout.turn_item(*r, -d.to_degrees());
                                        }
                                        self.moving = Some(Moving::GizmoTurn(c, last + d));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                if pressed && hover.is_some() && !grabbed {
                    let w = world.unwrap_or([0.0, 0.0]);
                    match hit {
                        // Ctrl+click adds or removes from the selection.
                        Some(r) if mods.ctrl => {
                            lv.multi = set.clone();
                            lv.selected = None;
                            lv.selected_prop = None;
                            lv.selected_fx = None;
                            if let Some(k) = lv.multi.iter().position(|x| *x == r) {
                                lv.multi.remove(k);
                            } else {
                                lv.multi.push(r);
                            }
                        }
                        // Pressing on part of a group moves the group.
                        Some(r) if set.len() > 1 && set.contains(&r) && !mods.shift => {
                            self.moving = Some(Moving::Group(w));
                        }
                        Some(_) => {}
                        None => {
                            lv.clear_selection();
                            if mods.shift {
                                self.box_start = hover;
                            }
                        }
                    }
                }
                if pressed && hit.is_some() && self.moving.is_none() && self.box_start.is_none() && !mods.ctrl && let Some(pp) = hover {
                    lv.multi.clear();
                    let obj = doc
                        .layout
                        .objectives
                        .iter()
                        .filter_map(|o| project([o.pos[0], ground(o.pos[0], o.pos[1]) + 60.0, o.pos[1]]).map(|s| (o.id, s.distance(pp), o.pos)))
                        .filter(|x| x.1 < 26.0)
                        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                    let prop = world.and_then(|w| {
                        doc.layout
                            .props
                            .iter()
                            .map(|p| (p.id, (p.pos[0] - w[0]).hypot(p.pos[1] - w[1]), lib.and_then(|l| l.get(&p.prop)).map_or(60.0, |d| d.radius * p.scale).max(40.0), p.pos))
                            .filter(|(_, d, r, _)| d <= r)
                            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                    });
                    lv.selected = None;
                    lv.selected_prop = None;
                    lv.selected_fx = None;
                    let w = world.unwrap_or([0.0, 0.0]);
                    if let Some((id, _, pos)) = obj {
                        lv.selected = Some(id);
                        self.moving = Some(if mods.shift { Moving::RotateObjective(id) } else { Moving::Objective(id, [pos[0] - w[0], pos[1] - w[1]]) });
                    } else if let Some((id, _, _, pos)) = prop {
                        lv.selected_prop = Some(id);
                        self.moving = Some(if mods.shift { Moving::RotateProp(id) } else { Moving::Prop(id, [pos[0] - w[0], pos[1] - w[1]]) });
                    } else if let Some(ItemRef::Effect(id) | ItemRef::Light(id)) = hit {
                        lv.selected_fx = Some(id);
                        self.moving = Some(Moving::Group(w));
                    }
                }
                // Box selection.
                if let (Some(a), Some(b)) = (self.box_start, hover) {
                    painter.rect_stroke(Rect::from_two_pos(a, b), 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Middle);
                    painter.rect_filled(Rect::from_two_pos(a, b), 0.0, Color32::from_white_alpha(14));
                    if released {
                        let r = Rect::from_two_pos(a, b);
                        let inside = |q: [f32; 2], lift: f32| project([q[0], ground(q[0], q[1]) + lift, q[1]]).is_some_and(|s| r.contains(s));
                        let mut v: Vec<ItemRef> = Vec::new();
                        v.extend(doc.layout.objectives.iter().filter(|o| inside(o.pos, 40.0)).map(|o| ItemRef::Objective(o.id)));
                        if lv.show_props {
                            v.extend(doc.layout.props.iter().filter(|p| inside(p.pos, 0.0)).map(|p| ItemRef::Prop(p.id)));
                            v.extend(doc.layout.effects.iter().filter(|e| inside(e.pos, e.lift)).map(|e| ItemRef::Effect(e.id)));
                            v.extend(doc.layout.lights.iter().filter(|l| inside(l.pos, l.lift)).map(|l| ItemRef::Light(l.id)));
                        }
                        v.retain(|r| !locked.contains(&id_of(*r)));
                        lv.clear_selection();
                        lv.multi = v;
                        self.box_start = None;
                    }
                }
                if resp.dragged_by(egui::PointerButton::Primary)
                    && let (Some(Moving::Group(prev)), Some(w)) = (self.moving, world)
                {
                    let w = self.snap(w);
                    let d = [w[0] - prev[0], w[1] - prev[1]];
                    if d != [0.0, 0.0] {
                        doc.begin();
                        for r in lv.selection() {
                            doc.layout.shift_item(r, d);
                        }
                        self.moving = Some(Moving::Group(w));
                    }
                }
                if resp.dragged_by(egui::PointerButton::Primary)
                    && let (Some(m), Some(w)) = (self.moving, world)
                    && !matches!(m, Moving::Group(_))
                {
                    doc.begin();
                    match m {
                        Moving::Group(_) | Moving::GizmoAxis(..) | Moving::GizmoLift(_) | Moving::GizmoTurn(..) => {}
                        Moving::Objective(id, off) => doc.layout.move_objective(id, self.snap([w[0] + off[0], w[1] + off[1]])),
                        Moving::Prop(id, off) => {
                            if let Some(p) = doc.layout.props.iter_mut().find(|p| p.id == id) {
                                p.pos = self.snap([w[0] + off[0], w[1] + off[1]]);
                            }
                        }
                        Moving::RotateObjective(id) => {
                            let at = doc.layout.objective(id).map(|o| o.pos);
                            if let Some(a) = at {
                                let y = self.snap_yaw(ang([w[0] - a[0], w[1] - a[1]]), false);
                                if let Some(o) = doc.layout.objectives.iter_mut().find(|o| o.id == id) {
                                    o.yaw = y;
                                }
                            }
                        }
                        Moving::RotateProp(id) => {
                            let y0 = self.snap_angle;
                            if let Some(p) = doc.layout.props.iter_mut().find(|p| p.id == id) {
                                let y = -ang([w[0] - p.pos[0], w[1] - p.pos[1]]);
                                p.yaw = if y0 && self.angle_step > 0.0 { (y / self.angle_step).round() * self.angle_step } else { y };
                            }
                        }
                    }
                }
                // Clipboard: Ctrl+C, Ctrl+X, Ctrl+V (at the cursor); Delete and Ctrl+D for groups.
                if !typing {
                    let (c, x, v, d, del) = ui.input(|i| (i.modifiers.ctrl && i.key_pressed(egui::Key::C), i.modifiers.ctrl && i.key_pressed(egui::Key::X), i.modifiers.ctrl && i.key_pressed(egui::Key::V), i.modifiers.ctrl && i.key_pressed(egui::Key::D), i.key_pressed(egui::Key::Delete)));
                    let sel = lv.selection();
                    if (c || x) && !sel.is_empty() {
                        lv.clipboard = doc.layout.copy_items(&sel);
                        if x {
                            doc.begin();
                            for r in &sel {
                                doc.layout.remove_item(*r);
                            }
                            doc.end();
                            lv.clear_selection();
                        }
                    }
                    if v && !lv.clipboard.is_empty() && let Some(w) = world {
                        doc.begin();
                        let new = doc.layout.paste(&lv.clipboard.clone(), w);
                        doc.end();
                        lv.clear_selection();
                        lv.multi = new;
                    }
                    if sel.len() > 1 && (d || del) {
                        doc.begin();
                        if d {
                            let cb = doc.layout.copy_items(&sel);
                            let pts: Vec<[f32; 2]> = sel.iter().filter_map(|r| doc.layout.item_pos(*r)).collect();
                            let c = [pts.iter().map(|p| p[0]).sum::<f32>() / pts.len() as f32 + 150.0, pts.iter().map(|p| p[1]).sum::<f32>() / pts.len() as f32 + 150.0];
                            let new = doc.layout.paste(&cb, c);
                            lv.clear_selection();
                            lv.multi = new;
                        } else {
                            for r in &sel {
                                doc.layout.remove_item(*r);
                            }
                            lv.clear_selection();
                        }
                        doc.end();
                    }
                }
                // Keyboard: R turns, Ctrl+D duplicates, Delete removes.
                if !typing && lv.multi.is_empty() {
                    let (r, dup, del) = ui.input(|i| (i.key_pressed(egui::Key::R), i.modifiers.ctrl && i.key_pressed(egui::Key::D), i.key_pressed(egui::Key::Delete)));
                    let step = if mods.shift { -self.angle_step } else { self.angle_step };
                    if r && !mods.ctrl {
                        doc.begin();
                        if let Some(id) = lv.selected_prop
                            && let Some(p) = doc.layout.props.iter_mut().find(|p| p.id == id)
                        {
                            p.yaw += step;
                        } else if let Some(id) = lv.selected
                            && let Some(o) = doc.layout.objectives.iter_mut().find(|o| o.id == id)
                        {
                            o.yaw += step;
                        }
                        doc.end();
                    }
                    if dup
                        && let Some(id) = lv.selected_prop
                        && let Some(p) = doc.layout.props.iter().find(|p| p.id == id).cloned()
                    {
                        doc.begin();
                        let n = doc.layout.add_prop(&p.prop, [p.pos[0] + 120.0, p.pos[1] + 120.0], p.yaw, p.scale);
                        if let Some(q) = doc.layout.props.iter_mut().find(|q| q.id == n) {
                            q.lift = p.lift;
                            q.tilt = p.tilt;
                            q.stretch = p.stretch;
                        }
                        doc.end();
                        lv.selected_prop = Some(n);
                    }
                    if del {
                        doc.begin();
                        if let Some(id) = lv.selected_prop.take() {
                            doc.layout.props.retain(|p| p.id != id);
                        } else if let Some(id) = lv.selected.take() {
                            doc.layout.remove_objective(id);
                        }
                        doc.end();
                    }
                }
                hint = "Click: select   Ctrl+click: add   Shift+drag on ground: box select   Drag: move   Shift+drag: turn   R: turn   Ctrl+C / X / V: copy, cut, paste   Ctrl+D: duplicate   Del: delete".into();
                // The multi-selection, highlighted.
                for r in &lv.multi {
                    if let Some(p) = doc.layout.item_pos(*r) {
                        ring(p, 70.0, Color32::from_rgb(255, 220, 80), 2.0);
                    }
                }
            }
        }
        if released {
            if self.painting || self.moving.is_some() || self.last_paint.is_some() {
                doc.end();
            }
            self.painting = false;
            self.last_paint = None;
            self.flatten_to = None;
            self.moving = None;
            self.press = None;
        }

        // ---- selection highlight (drawn at the live position, ahead of the rebuilt scene)
        if let Some(id) = lv.selected_prop
            && let Some(p) = doc.layout.props.iter().find(|p| p.id == id)
            && let Some(def) = lib.and_then(|l| l.get(&p.prop))
        {
            let pl = Place { x: p.pos[0], y: ground(p.pos[0], p.pos[1]) + p.lift + if def.is_decal() { 3.0 } else { 0.0 }, z: p.pos[1], yaw: p.yaw, scale: p.scale };
            extras.extend(self.models.prop_ex(&mut glk, def, pl, p.tilt, p.stretch, [1.6, 1.5, 0.7, 0.5]));
            let r = (def.radius * p.scale).max(30.0);
            ring(p.pos, r, Color32::from_rgb(255, 220, 80), 2.0);
            let a = (-p.yaw).to_radians();
            let tip = [p.pos[0] + a.sin() * (r + 60.0), p.pos[1] + a.cos() * (r + 60.0)];
            path(&[p.pos, tip], false, Color32::from_rgb(255, 220, 80), 2.0);
        }
        if let Some(id) = lv.selected
            && let Some(o) = doc.layout.objective(id).cloned()
        {
            if let Some(d) = data {
                for (skin, at, yw, monster) in objective_models(&o) {
                    let pl = Place { x: at[0], y: ground(at[0], at[1]), z: at[1], yaw: yw, scale: 1.0 };
                    if let Some(m) = self.models.unit(&mut glk, d, &skin, monster, pl, [1.6, 1.5, 0.7, 0.45]) {
                        extras.push(m);
                    }
                }
            }
            ring(o.pos, o.kind.radius(), Color32::from_rgb(255, 220, 80), 2.0);
            let a = o.yaw.to_radians();
            let tip = [o.pos[0] + a.sin() * (o.kind.radius() + 80.0), o.pos[1] + a.cos() * (o.kind.radius() + 80.0)];
            path(&[o.pos, tip], false, Color32::from_rgb(255, 220, 80), 2.0);
        }
        // Live particle effects, and light gizmos.
        if let Some(d) = data {
            extras.extend(fx.frame(&mut glk, &d.root, &effect_items, &inv));
        }
        // Light markers only with the Effects tool (opened maps have hundreds of lights).
        for lt in doc.layout.lights.iter().filter(|l| lv.tool == LTool::Effects || lv.selected_fx == Some(l.id)) {
            let c = Color32::from_rgb(lt.color[0], lt.color[1], lt.color[2]);
            let g0 = ground(lt.pos[0], lt.pos[1]);
            if let (Some(a), Some(b)) = (project([lt.pos[0], g0, lt.pos[1]]), project([lt.pos[0], g0 + lt.lift, lt.pos[1]])) {
                painter.line_segment([a, b], Stroke::new(1.0, c.gamma_multiply(0.7)));
                painter.circle_filled(b, 6.0, c);
                painter.circle_stroke(b, 6.0, Stroke::new(1.0, Color32::BLACK));
                if lv.selected_fx == Some(lt.id) {
                    painter.circle_stroke(b, 10.0, Stroke::new(2.0, Color32::WHITE));
                }
            }
            ring(lt.pos, lt.outer, c.gamma_multiply(0.5), 1.0);
        }
        for e in &doc.layout.effects {
            if lv.selected_fx == Some(e.id)
                && let Some(b) = project([e.pos[0], ground(e.pos[0], e.pos[1]) + e.lift, e.pos[1]])
            {
                painter.circle_stroke(b, 12.0, Stroke::new(2.0, Color32::WHITE));
            }
        }
        // Game ranges around turrets and fountains (Measure tool or the toggle).
        if lv.show_ranges || lv.tool == LTool::Measure {
            use lol_map_studio::layout::ObjKind;
            for o in &doc.layout.objectives {
                let (r, c) = match o.kind {
                    ObjKind::Turret => (775.0 + 88.0, Color32::from_rgba_unmultiplied(255, 90, 70, 160)),
                    ObjKind::FountainTurret => (1250.0, Color32::from_rgba_unmultiplied(255, 60, 60, 160)),
                    ObjKind::SpawnPoint => (1100.0, Color32::from_rgba_unmultiplied(90, 200, 255, 140)),
                    _ => continue,
                };
                ring(o.pos, r, c, 1.5);
            }
        }
        // Pathing view (StarCraft's H): red where units cannot walk, green brush.
        if lv.show_pathing {
            if self.pathing.as_ref().is_none_or(|p| p.0 != doc.rev) {
                self.pathing = Some((doc.rev, pathing_mesh(&doc.layout, &|x, z| ground(x, z))));
            }
            if let Some((_, meshes)) = &self.pathing {
                for (v, i, tint) in meshes {
                    extras.push(DynMesh { texture: None, vertices: v.clone(), indices: i.clone(), tint: *tint, cutout: false, particle: false, additive: false });
                }
            }
        }
        glk.extras = extras;
        drop(glk);

        // ---- markers: objectives and lanes, like the 2D canvas
        if lv.show_lanes {
            for lp in &doc.layout.lanes {
                path(&lp.points, false, Color32::from_rgba_unmultiplied(255, 230, 120, 140), 2.0);
            }
        }
        if lv.show_objectives {
            for o in &doc.layout.objectives {
                if let Some(s) = project([o.pos[0], ground(o.pos[0], o.pos[1]) + 40.0, o.pos[1]]) {
                    if !rect.contains(s) {
                        continue;
                    }
                    let col = super::layout_view::team_color(o.team);
                    painter.circle_filled(s, 4.0, col);
                    painter.circle_stroke(s, 4.0, Stroke::new(1.0, Color32::BLACK));
                    if cam.distance < 5000.0 {
                        painter.text(s + Vec2::new(0.0, -8.0), Align2::CENTER_BOTTOM, o.kind.label(), FontId::proportional(11.0), col);
                    }
                }
            }
        }


        // ---- status line and snapping controls
        let info = match hit {
            Some(h) => format!("x {:.0}  z {:.0}  height {:.0}", h[0], h[2], h[1]),
            None => String::new(),
        };
        painter.text(rect.left_bottom() + Vec2::new(10.0, -10.0), Align2::LEFT_BOTTOM, format!("{hint}\nRight drag: orbit   Middle drag: pan   Wheel: zoom   WASD: move   F: focus   Home: whole map\n{info}"), FontId::proportional(12.0), Color32::from_gray(220));
        let bar = Rect::from_min_size(rect.left_top() + Vec2::new(10.0, 10.0), Vec2::new(520.0, 26.0));
        ui.put(bar, |ui: &mut egui::Ui| {
            egui::Frame::popup(ui.style())
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Snap");
                        egui::ComboBox::from_id_salt("snapgrid").width(70.0).selected_text(if self.snap_grid > 0.0 { format!("{:.0}", self.snap_grid) } else { "off".into() }).show_ui(ui, |ui| {
                            for g in [0.0, 25.0, 50.0, 100.0, 250.0, 500.0] {
                                ui.selectable_value(&mut self.snap_grid, g, if g > 0.0 { format!("{g:.0} units") } else { "off".into() });
                            }
                        });
                        ui.checkbox(&mut self.snap_angle, "Angle");
                        ui.add(egui::DragValue::new(&mut self.angle_step).range(1.0..=90.0).suffix("°"));
                        ui.separator();
                        ui.label(format!("Tool: {}", LTool::ALL.iter().find(|x| x.0 == lv.tool).map_or("", |x| x.1)));
                    });
                })
                .response
        });
        if self.painting || down || cam.distance > 0.0 && !monsters.is_empty() {
            ui.ctx().request_repaint();
        }
    }
}

/// Ground outline of a dragged shape tool.
fn shape_points(tool: LTool, a: [f32; 2], b: [f32; 2]) -> Vec<[f32; 2]> {
    match tool {
        LTool::Line => vec![a, b],
        LTool::Rect => vec![a, [b[0], a[1]], b, [a[0], b[1]]],
        _ => {
            let c = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let r = [(b[0] - a[0]).abs() / 2.0, (b[1] - a[1]).abs() / 2.0];
            (0..48).map(|i| {
                let t = i as f32 / 48.0 * std::f32::consts::TAU;
                [c[0] + r[0] * t.cos(), c[1] + r[1] * t.sin()]
            }).collect()
        }
    }
}
