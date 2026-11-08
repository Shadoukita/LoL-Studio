//! The map editor's tool bar (a vertical strip of tool icons, like Blender's and Photoshop's) and
//! the options bar above the viewport with the active tool's settings.

use super::layout_view::{LTool, LayoutDoc, LayoutView, SceneryMode};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Shape, Stroke, Vec2};
use lol_map_studio::layout::{CELL, MAX_LAYERS, ObjKind, SculptMode, Surface};
use lol_map_studio::props::PropLibrary;

/// Tools in strip order, with their key and what they do.
const TOOLS: [(LTool, &str, &str, &str); 14] = [
    (LTool::Select, "V", "Select / Move", "Select, move and turn objectives, props, effects. Shift+drag turns."),
    (LTool::Brush, "B", "Brush", "Paint walls, ground, brush, river, deep water"),
    (LTool::Line, "L", "Line", "Drag a straight wall or path"),
    (LTool::Rect, "R", "Rectangle", "Drag a rectangle of the surface"),
    (LTool::Ellipse, "E", "Ellipse", "Drag an ellipse of the surface"),
    (LTool::Polygon, "P", "Polygon", "Click corners, Enter to fill"),
    (LTool::Fill, "F", "Fill", "Flood-fill a connected area"),
    (LTool::Texture, "T", "Ground texture", "Paint ground texture layers"),
    (LTool::Sculpt, "H", "Sculpt", "Raise, lower, flatten, smooth the ground"),
    (LTool::Objective, "O", "Objectives", "Turrets, nexus, inhibitors, spawns, shops, camps, game-mode objects"),
    (LTool::Lane, "N", "Lanes", "Draw minion lanes"),
    (LTool::Scenery, "G", "Scenery", "Trees, rocks, walls, decals and your own assets"),
    (LTool::Effects, "X", "Effects & lights", "Particle effects and point lights"),
    (LTool::Measure, "M", "Measure", "Distances and game ranges"),
];

fn icon(p: &egui::Painter, r: Rect, tool: LTool, c: Color32) {
    let s = Stroke::new(1.8, c);
    let ctr = r.center();
    let u = r.width() / 24.0;
    let at = |x: f32, y: f32| Pos2::new(r.left() + x * u, r.top() + y * u);
    match tool {
        LTool::Select => {
            p.add(Shape::convex_polygon(vec![at(7.0, 4.0), at(7.0, 19.0), at(11.0, 15.0), at(14.0, 21.0), at(16.5, 20.0), at(13.5, 14.0), at(18.5, 14.0)], c, Stroke::NONE));
        }
        LTool::Brush => {
            p.line_segment([at(17.0, 4.0), at(10.0, 13.0)], Stroke::new(2.4, c));
            p.add(Shape::convex_polygon(vec![at(9.0, 12.0), at(12.0, 15.0), at(8.0, 20.0), at(4.0, 20.0), at(5.0, 16.0)], c, Stroke::NONE));
        }
        LTool::Line => {
            p.line_segment([at(5.0, 19.0), at(19.0, 5.0)], Stroke::new(2.4, c));
            p.circle_filled(at(5.0, 19.0), 2.0 * u, c);
            p.circle_filled(at(19.0, 5.0), 2.0 * u, c);
        }
        LTool::Rect => {
            p.rect_stroke(Rect::from_min_max(at(5.0, 7.0), at(19.0, 17.0)), 1.0, s, egui::StrokeKind::Middle);
        }
        LTool::Ellipse => {
            let pts: Vec<Pos2> = (0..32).map(|i| {
                let a = i as f32 / 32.0 * std::f32::consts::TAU;
                ctr + Vec2::new(a.cos() * 7.5 * u, a.sin() * 5.0 * u)
            }).collect();
            p.add(Shape::closed_line(pts, s));
        }
        LTool::Polygon => {
            p.add(Shape::closed_line(vec![at(12.0, 4.0), at(20.0, 10.0), at(17.0, 19.0), at(7.0, 19.0), at(4.0, 10.0)], s));
            for q in [at(12.0, 4.0), at(20.0, 10.0), at(17.0, 19.0), at(7.0, 19.0), at(4.0, 10.0)] {
                p.circle_filled(q, 1.4 * u, c);
            }
        }
        LTool::Fill => {
            p.add(Shape::convex_polygon(vec![at(4.0, 11.0), at(11.0, 4.0), at(18.0, 11.0), at(11.0, 18.0)], Color32::TRANSPARENT, s));
            p.add(Shape::convex_polygon(vec![at(4.0, 11.0), at(18.0, 11.0), at(11.0, 18.0)], c, Stroke::NONE));
            p.circle_filled(at(19.5, 16.5), 2.0 * u, c);
        }
        LTool::Texture => {
            for (i, j) in [(0, 0), (1, 1), (2, 0), (0, 2), (2, 2), (1, 3), (3, 1), (3, 3)] {
                if (i + j) % 2 == 0 && i < 3 && j < 3 {
                    p.rect_filled(Rect::from_min_size(at(5.0 + i as f32 * 4.7, 5.0 + j as f32 * 4.7), Vec2::splat(4.7 * u)), 0.0, c);
                }
            }
            p.rect_stroke(Rect::from_min_max(at(5.0, 5.0), at(19.0, 19.0)), 0.0, s, egui::StrokeKind::Middle);
        }
        LTool::Sculpt => {
            p.add(Shape::convex_polygon(vec![at(3.0, 19.0), at(10.0, 7.0), at(14.0, 13.0), at(16.0, 10.0), at(21.0, 19.0)], c, Stroke::NONE));
        }
        LTool::Objective => {
            p.rect_filled(Rect::from_min_max(at(9.0, 8.0), at(15.0, 19.0)), 0.0, c);
            p.add(Shape::convex_polygon(vec![at(7.0, 8.0), at(12.0, 3.0), at(17.0, 8.0)], c, Stroke::NONE));
            p.rect_filled(Rect::from_min_max(at(6.0, 19.0), at(18.0, 21.0)), 0.0, c);
        }
        LTool::Lane => {
            p.add(Shape::line(vec![at(3.0, 19.0), at(8.0, 12.0), at(12.0, 15.0), at(16.0, 8.0), at(21.0, 5.0)], Stroke::new(2.2, c)));
        }
        LTool::Scenery => {
            p.add(Shape::convex_polygon(vec![at(12.0, 3.0), at(19.0, 16.0), at(5.0, 16.0)], c, Stroke::NONE));
            p.rect_filled(Rect::from_min_max(at(10.8, 16.0), at(13.2, 21.0)), 0.0, c);
        }
        LTool::Measure => {
            p.line_segment([at(3.0, 17.0), at(17.0, 3.0)], Stroke::new(5.0, c));
            for k in 1..6 {
                let t = k as f32 / 6.0;
                let q = at(3.0 + 14.0 * t, 17.0 - 14.0 * t);
                p.line_segment([q, q + Vec2::new(2.5 * u, 2.5 * u)], Stroke::new(1.2, Color32::BLACK));
            }
        }
        LTool::Effects => {
            for k in 0..4 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let d = Vec2::new(a.cos(), a.sin()) * 8.0 * u;
                p.line_segment([ctr - d, ctr + d], Stroke::new(if k % 2 == 0 { 2.2 } else { 1.2 }, c));
            }
            p.circle_filled(ctr, 2.5 * u, c);
        }
    }
}

fn icon_button(ui: &mut egui::Ui, selected: bool, draw: impl FnOnce(&egui::Painter, Rect, Color32)) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
    let v = ui.visuals();
    let bg = if selected { v.selection.bg_fill } else if resp.hovered() { v.widgets.hovered.bg_fill } else { Color32::TRANSPARENT };
    ui.painter().rect_filled(r, 5.0, bg);
    let c = if selected { Color32::WHITE } else if resp.hovered() { v.widgets.hovered.fg_stroke.color } else { v.widgets.inactive.fg_stroke.color };
    draw(ui.painter(), r.shrink(5.0), c);
    resp
}

/// The vertical tool strip. Returns true when something was changed (undo / redo).
pub fn strip(ui: &mut egui::Ui, lv: &mut LayoutView, doc: &mut LayoutDoc) {
    ui.add_space(4.0);
    ui.vertical_centered(|ui| {
        for (i, (tool, key, name, help)) in TOOLS.iter().enumerate() {
            if i == 7 || i == 9 || i == 11 {
                ui.separator();
            }
            let r = icon_button(ui, lv.tool == *tool, |p, r, c| icon(p, r, *tool, c));
            if r.on_hover_text(RichText::new(format!("{name}   ({key})\n{help}"))).clicked() {
                lv.tool = *tool;
            }
        }
        ui.separator();
        let pv = lv.show_pathing;
        if icon_button(ui, pv, |p, r, c| {
            for k in 0..3 {
                for m in 0..3 {
                    let col = if (k + m) % 2 == 0 { Color32::from_rgb(220, 80, 70) } else { Color32::from_rgb(80, 200, 100) };
                    p.rect_filled(Rect::from_min_size(r.min + Vec2::new(k as f32, m as f32) * r.width() / 3.0, Vec2::splat(r.width() / 3.0 - 1.0)), 1.0, if pv { col } else { col.gamma_multiply(0.6) });
                }
            }
            let _ = c;
        }).on_hover_text("Pathing view: red = units cannot walk, green = brush").clicked() {
            lv.show_pathing = !lv.show_pathing;
        }
        ui.separator();
        let undo = icon_button(ui, false, |p, r, c| {
            p.add(Shape::line((0..=12).map(|i| {
                let a = std::f32::consts::PI * (0.15 + i as f32 / 12.0 * 1.2);
                r.center() + Vec2::new(-a.cos(), -a.sin()) * r.width() * 0.32 + Vec2::new(0.0, r.height() * 0.1)
            }).collect(), Stroke::new(1.8, c)));
            p.add(Shape::convex_polygon(vec![r.left_center() + Vec2::new(2.0, -3.0), r.left_center() + Vec2::new(9.0, 2.0), r.left_center() + Vec2::new(1.0, 6.0)], c, Stroke::NONE));
        });
        if undo.on_hover_text("Undo  (Ctrl+Z)").clicked() {
            doc.undo();
        }
        let redo = icon_button(ui, false, |p, r, c| {
            p.add(Shape::line((0..=12).map(|i| {
                let a = std::f32::consts::PI * (0.15 + i as f32 / 12.0 * 1.2);
                r.center() + Vec2::new(a.cos(), -a.sin()) * r.width() * 0.32 + Vec2::new(0.0, r.height() * 0.1)
            }).collect(), Stroke::new(1.8, c)));
            p.add(Shape::convex_polygon(vec![r.right_center() + Vec2::new(-2.0, -3.0), r.right_center() + Vec2::new(-1.0, 6.0), r.right_center() + Vec2::new(-9.0, 2.0)], c, Stroke::NONE));
        });
        if redo.on_hover_text("Redo  (Ctrl+Y)").clicked() {
            doc.redo();
        }
    });
}

fn size_slider(ui: &mut egui::Ui, v: &mut f32, label: &str) {
    ui.label(label);
    ui.add(egui::DragValue::new(v).range(CELL..=3000.0).speed(5.0).suffix(" u"));
}

/// The options bar: the active tool's settings in one row.
pub fn options(ui: &mut egui::Ui, lv: &mut LayoutView, doc: &mut LayoutDoc, lib: Option<&PropLibrary>) {
    ui.horizontal(|ui| {
        let name = TOOLS.iter().find(|t| t.0 == lv.tool).map_or("", |t| t.2);
        ui.label(RichText::new(name).strong());
        ui.separator();
        let surface_combo = |ui: &mut egui::Ui, s: &mut Surface| {
            egui::ComboBox::from_id_salt("opt_surface").selected_text(s.label()).show_ui(ui, |ui| {
                for x in Surface::ALL {
                    ui.selectable_value(s, x, x.label());
                }
            });
        };
        match lv.tool {
            LTool::Brush => {
                surface_combo(ui, &mut lv.surface);
                size_slider(ui, &mut lv.radius, "Size");
                ui.checkbox(&mut lv.square, "Square");
            }
            LTool::Line => {
                surface_combo(ui, &mut lv.surface);
                ui.label("Width");
                ui.add(egui::DragValue::new(&mut lv.line_width).range(CELL..=1500.0).speed(5.0).suffix(" u"));
            }
            LTool::Rect | LTool::Ellipse | LTool::Polygon | LTool::Fill => surface_combo(ui, &mut lv.surface),
            LTool::Texture => {
                let names: Vec<String> = doc.layout.theme.ground.iter().map(|g| g.name.clone()).collect();
                egui::ComboBox::from_id_salt("opt_layer").selected_text(names.get(lv.layer).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate().take(MAX_LAYERS) {
                        ui.selectable_value(&mut lv.layer, i, n);
                    }
                });
                size_slider(ui, &mut lv.radius, "Size");
                ui.label("Strength");
                ui.add(egui::Slider::new(&mut lv.strength, 0.05..=1.0).show_value(true));
            }
            LTool::Sculpt => {
                for (m, n) in [(SculptMode::Raise, "Raise"), (SculptMode::Lower, "Lower"), (SculptMode::Flatten, "Flatten"), (SculptMode::Smooth, "Smooth"), (SculptMode::Noise, "Noise"), (SculptMode::Ramp, "Ramp")] {
                    ui.selectable_value(&mut lv.sculpt, m, n);
                }
                size_slider(ui, &mut lv.radius, "Size");
                ui.label("Strength");
                ui.add(egui::Slider::new(&mut lv.strength, 0.05..=1.0));
            }
            LTool::Objective => {
                let label = if lv.fountain { "Fountain (spawn + turret + shop)" } else { lv.obj_kind.label() };
                egui::ComboBox::from_id_salt("opt_obj").selected_text(label).width(220.0).show_ui(ui, |ui| {
                    if ui.selectable_label(lv.fountain, "Fountain (spawn + turret + shop)").clicked() {
                        lv.fountain = true;
                    }
                    for k in ObjKind::ALL.iter().chain(ObjKind::CAMPS.iter()).chain(ObjKind::MODES.iter()) {
                        if ui.selectable_label(!lv.fountain && lv.obj_kind == *k, k.label()).clicked() {
                            lv.obj_kind = *k;
                            lv.fountain = false;
                        }
                    }
                });
                ui.selectable_value(&mut lv.team, 1, RichText::new("Blue").color(super::layout_view::team_color(1)));
                ui.selectable_value(&mut lv.team, 2, RichText::new("Red").color(super::layout_view::team_color(2)));
            }
            LTool::Scenery => {
                ui.selectable_value(&mut lv.scenery, SceneryMode::Place, "Place");
                ui.selectable_value(&mut lv.scenery, SceneryMode::Scatter, "Scatter");
                ui.selectable_value(&mut lv.scenery, SceneryMode::Erase, "Erase");
                ui.selectable_value(&mut lv.scenery, SceneryMode::Path, "Path");
                if let Some(lb) = lib {
                    let cur = lv.prop_key.as_ref().and_then(|k| lb.get(k)).map_or("Any in category".to_string(), |d| d.name.clone());
                    egui::ComboBox::from_id_salt("opt_cat").selected_text(lv.prop_cat.clone()).show_ui(ui, |ui| {
                        for c in lb.categories() {
                            if !lb.props.iter().any(|p| p.category == c && PropLibrary::allowed(&p.key, &doc.layout.asset_maps)) {
                                continue;
                            }
                            if ui.selectable_label(lv.prop_cat == c, &c).clicked() {
                                lv.prop_cat = c;
                                lv.prop_key = None;
                            }
                        }
                    });
                    ui.label(RichText::new(cur).weak());
                }
                ui.label("Size");
                ui.add(egui::DragValue::new(&mut lv.prop_scale[0]).speed(0.01).range(0.05..=10.0));
                ui.add(egui::DragValue::new(&mut lv.prop_scale[1]).speed(0.01).range(0.05..=10.0));
                ui.checkbox(&mut lv.random_turn, "Random turn");
                ui.checkbox(&mut lv.align_slope, "Follow slope").on_hover_text("Tilt placed props to the ground's slope");
                if matches!(lv.scenery, SceneryMode::Scatter | SceneryMode::Erase) {
                    size_slider(ui, &mut lv.radius, "Brush");
                }
                if lv.scenery == SceneryMode::Path {
                    ui.selectable_value(&mut lv.path_free, true, "Freehand");
                    ui.selectable_value(&mut lv.path_free, false, "Straight");
                    ui.label("Spacing");
                    ui.add(egui::DragValue::new(&mut lv.path_gap).range(0.3..=4.0).speed(0.02));
                    ui.label("Turn");
                    ui.add(egui::DragValue::new(&mut lv.path_turn).speed(5.0).suffix("°"));
                }
            }
            LTool::Effects => {
                ui.selectable_value(&mut lv.fx_light, false, "Particle effect");
                ui.selectable_value(&mut lv.fx_light, true, "Light");
                if lv.fx_light {
                    ui.color_edit_button_srgb(&mut lv.light_color);
                    ui.label("Radius");
                    ui.add(egui::DragValue::new(&mut lv.light_outer).range(50.0..=3000.0).speed(5.0));
                } else {
                    ui.label(RichText::new(lv.fx_name.clone().unwrap_or_default()).weak());
                }
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut lv.fx_lift).range(0.0..=800.0).speed(2.0));
            }
            LTool::Select => {
                let n = lv.multi.len() + lv.selected.is_some() as usize + lv.selected_prop.is_some() as usize;
                ui.label(format!("{n} selected"));
                ui.label(RichText::new("Shift+drag on empty ground: box select  ·  Ctrl+C / Ctrl+V copy & paste  ·  Ctrl+D duplicate  ·  Del delete").weak());
            }
            LTool::Measure => {
                ui.checkbox(&mut lv.show_ranges, "Show ranges");
                ui.label(RichText::new("Drag on the ground to measure").weak());
            }
            LTool::Lane => {
                for l in [lol_map_studio::layout::Lane::Top, lol_map_studio::layout::Lane::Mid, lol_map_studio::layout::Lane::Bot] {
                    ui.selectable_value(&mut lv.lane, l, l.label());
                }
            }
        }
    });
}
