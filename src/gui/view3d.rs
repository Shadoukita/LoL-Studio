//! 3D scene view: orbit / game camera over the textured scene, objects as markers.

use super::gl_scene::{self, Mat4, Shared};
use super::view2d::{Layers, markers};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Sense, Stroke, Vec2};
use lol_map_studio::edit::{Editor, Sel};
use std::sync::Arc;

pub struct View3D {
    pub target: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov: f32,
    fitted: bool,
}

impl Default for View3D {
    fn default() -> Self {
        Self { target: [7000.0, 0.0, 7000.0], yaw: -std::f32::consts::FRAC_PI_2, pitch: 0.97, distance: 14000.0, fov: 0.9, fitted: false }
    }
}

fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fovy / 2.0).tan();
    let mut m = [0.0; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = (far + near) / (near - far);
    m[11] = -1.0;
    m[14] = 2.0 * far * near / (near - far);
    m
}

fn look_at(eye: [f32; 3], at: [f32; 3]) -> Mat4 {
    let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let norm = |v: [f32; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let f = norm(sub(at, eye));
    let s = norm(cross(f, [0.0, 1.0, 0.0]));
    let u = cross(s, f);
    [s[0], u[0], -f[0], 0.0, s[1], u[1], -f[1], 0.0, s[2], u[2], -f[2], 0.0, -dot(s, eye), -dot(u, eye), dot(f, eye), 1.0]
}

/// The game is left-handed (Direct3D); drawing mirrored on X shows the map as players see it.
const MIRROR: Mat4 = [-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

impl View3D {
    fn camera(&self) -> ([f32; 3], [f32; 3]) {
        let t = [-self.target[0], self.target[1], self.target[2]];
        let (cy, sy) = (self.yaw.cos(), self.yaw.sin());
        let (cp, sp) = (self.pitch.cos(), self.pitch.sin());
        (t, [t[0] + self.distance * cp * cy, t[1] + self.distance * sp, t[2] + self.distance * cp * sy])
    }

    pub fn eye_world(&self) -> [f32; 3] {
        let (_, e) = self.camera();
        [-e[0], e[1], e[2]]
    }

    pub fn mvp(&self, aspect: f32) -> Mat4 {
        let near = (self.distance * 0.01).max(5.0);
        let (t, eye) = self.camera();
        gl_scene::mul(&perspective(self.fov, aspect, near, self.distance * 6.0 + 30000.0), &gl_scene::mul(&look_at(eye, t), &MIRROR))
    }

    pub fn fit(&mut self, b: [f32; 6]) {
        if b[0] < b[3] {
            self.target = [(b[0] + b[3]) / 2.0, (b[1] + b[4]) / 2.0, (b[2] + b[5]) / 2.0];
            self.distance = (b[3] - b[0]).max(b[5] - b[2]) * 0.85;
            self.fitted = true;
        }
    }

    pub fn fit_once(&mut self, b: [f32; 6]) {
        if !self.fitted {
            self.fit(b);
        }
    }

    /// The in-game camera: looking north, about 56° down, close in.
    pub fn game_camera(&mut self) {
        self.yaw = -std::f32::consts::FRAC_PI_2;
        self.pitch = 56f32.to_radians();
        self.distance = 1900.0;
        self.fov = 50f32.to_radians();
    }

    /// Inverse of the view-projection for a viewport aspect (for billboards and picking).
    pub fn inverse(&self, aspect: f32) -> Option<Mat4> {
        super::play_view::invert_pub(&self.mvp(aspect))
    }

    pub fn focus(&mut self, p: [f32; 3]) {
        self.target = p;
        self.distance = self.distance.min(3000.0);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ed: &mut Editor, layers: &Layers, gl: &Shared) {
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let (cy, sy) = (self.yaw.cos(), self.yaw.sin());
        // Ground-plane axes of the view in world coordinates (X is mirrored on screen).
        let right = [sy, -cy];
        let fwd = [cy, sy];
        if resp.dragged_by(egui::PointerButton::Primary) {
            let d = resp.drag_delta();
            self.yaw -= d.x * 0.006;
            self.pitch = (self.pitch + d.y * 0.006).clamp(0.08, 1.55);
        }
        let pan = |dx: f32, dy: f32, k: f32, t: &mut [f32; 3]| {
            t[0] += (right[0] * dx + fwd[0] * dy) * k;
            t[2] += (right[1] * dx + fwd[1] * dy) * k;
        };
        if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) {
            let d = resp.drag_delta();
            pan(-d.x, -d.y, self.distance * 0.0015, &mut self.target);
        }
        if resp.hovered() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let f = zoom * (scroll * 0.002).exp();
            self.distance = (self.distance / f.max(0.01)).clamp(150.0, 60000.0);
            let mut m = [0.0f32; 2];
            ui.input(|i| {
                if i.key_down(egui::Key::ArrowUp) || i.key_down(egui::Key::W) {
                    m[1] -= 1.0;
                }
                if i.key_down(egui::Key::ArrowDown) || i.key_down(egui::Key::S) {
                    m[1] += 1.0;
                }
                if i.key_down(egui::Key::ArrowLeft) || i.key_down(egui::Key::A) {
                    m[0] -= 1.0;
                }
                if i.key_down(egui::Key::ArrowRight) || i.key_down(egui::Key::D) {
                    m[0] += 1.0;
                }
            });
            if m != [0.0, 0.0] {
                pan(m[0], m[1], self.distance * 0.02, &mut self.target);
                ui.ctx().request_repaint();
            }
        }
        let aspect = rect.width() / rect.height().max(1.0);
        let mvp = self.mvp(aspect);
        let eye = self.eye_world();
        let g = gl.clone();
        let cb = egui_glow::CallbackFn::new(move |info, painter| {
            let mut s = g.lock().unwrap();
            unsafe { s.paint(painter.gl(), &info, &mvp, eye, [0.06, 0.07, 0.09, 1.0]) };
        });
        ui.painter().add(egui::PaintCallback { rect, callback: Arc::new(cb) });

        let painter = ui.painter_at(rect);
        {
            let s = gl.lock().unwrap();
            if !s.has_geometry() {
                painter.text(rect.center(), Align2::CENTER_CENTER, "No scene geometry yet", FontId::proportional(16.0), Color32::GRAY);
            }
            if let Some(e) = &s.error {
                painter.text(rect.center() + Vec2::new(0.0, 24.0), Align2::CENTER_CENTER, e, FontId::proportional(13.0), Color32::RED);
            }
        }
        let project = |p: [f32; 3]| -> Option<Pos2> {
            let c = gl_scene::transform(&mvp, [p[0], p[1] + 60.0, p[2]]);
            if c[3] <= 1.0 {
                return None;
            }
            Some(Pos2::new(rect.left() + (c[0] / c[3] + 1.0) / 2.0 * rect.width(), rect.top() + (1.0 - c[1] / c[3]) / 2.0 * rect.height()))
        };
        let mut best: Option<(Sel, f32)> = None;
        let hover = resp.hover_pos();
        for m in markers(ed, layers) {
            let Some(p) = project(m.pos) else { continue };
            if !rect.contains(p) {
                continue;
            }
            let r = (m.radius * 0.6).max(3.0);
            let selected = ed.sel == m.sel;
            if selected || layers.markers_3d {
                painter.circle_filled(p, r, m.color);
                painter.circle_stroke(p, r, Stroke::new(1.0, Color32::BLACK));
            }
            if selected {
                painter.circle_stroke(p, r + 4.0, Stroke::new(2.0, Color32::WHITE));
                painter.text(p + Vec2::new(r + 3.0, 0.0), Align2::LEFT_CENTER, &m.name, FontId::proportional(12.0), Color32::WHITE);
            }
            if let Some(h) = hover {
                let d = h.distance(p);
                if d < r + 6.0 && best.as_ref().is_none_or(|b| d < b.1) {
                    best = Some((m.sel.clone(), d));
                }
            }
        }
        if resp.clicked() {
            ed.sel = best.map(|b| b.0).unwrap_or(Sel::None);
        }
        painter.text(rect.left_bottom() + Vec2::new(8.0, -8.0), Align2::LEFT_BOTTOM, "Left drag: orbit   Right drag: pan   Wheel: zoom   WASD/arrows: move", FontId::proportional(12.0), Color32::from_gray(200));
    }
}
