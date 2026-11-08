//! Right-hand panel: properties of the selected object, and editors for each kind of file.

use lol_map_studio::edit::{Editor, Sel};
use eframe::egui::{self, Color32, RichText, TextureHandle, TextureOptions};
use lol_map_studio::formats::{
    dds,
    inibin::{Value, ValueKind, hash_key},
    luaobj,
    mob::ObjectType,
    navgrid::flags,
    nvr::{material_flags, material_type},
};
use lol_map_studio::keynames::KeyNames;
use lol_map_studio::project::{Doc, FileKind, MapFile};

#[derive(Default)]
pub struct InspectorState {
    text: Option<(String, String)>,
    cfg_text: Option<(String, String)>,
    rename: Option<(String, String)>,
    ini_row: Option<usize>,
    ini_value: String,
    ini_new_section: String,
    ini_new_key: String,
    ini_new_kind: Option<ValueKind>,
    ini_new_value: String,
    nvr_material: usize,
    nvr_mesh: usize,
    nvr_move: [f32; 3],
    wgeo_mesh: usize,
    preview: Option<(String, u64, TextureHandle, String)>,
    lua: Option<(String, Vec<String>)>,
    pub message: Option<String>,
    pub request_obj_import: bool,
    pub request_replace_texture: Option<String>,
}

fn vec3_edit(ui: &mut egui::Ui, v: &mut [f32; 3], speed: f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for (i, l) in ["x", "y", "z"].iter().enumerate() {
            ui.label(*l);
            changed |= ui.add(egui::DragValue::new(&mut v[i]).speed(speed).max_decimals(3)).changed();
        }
    });
    changed
}

pub fn ui(ui: &mut egui::Ui, ed: &mut Editor, st: &mut InspectorState, keys: &mut KeyNames) {
    if let Some(m) = &st.message {
        ui.colored_label(Color32::from_rgb(255, 200, 120), m);
        ui.separator();
    }
    match ed.sel.clone() {
        Sel::None => {
            ui.label(RichText::new("Nothing selected").italics());
            ui.label("Click an object on the map, or a file in the list on the left.");
        }
        Sel::Mob(name) => mob_ui(ui, ed, st, &name),
        Sel::Dsc(stem) => dsc_ui(ui, ed, st, &stem),
        Sel::File(path) => file_ui(ui, ed, st, keys, &path),
    }
}

fn cfg_ui(ui: &mut egui::Ui, ed: &mut Editor, st: &mut InspectorState, name: &str) {
    let Some(sec) = ed.cfg_section(name) else {
        if ed.cfg_path().is_some() {
            ui.label(RichText::new(format!("No [{name}] section in Scene/CFG/ObjectCFG.cfg.")).weak());
        }
        return;
    };
    egui::CollapsingHeader::new("Object settings (ObjectCFG.cfg)").default_open(false).show(ui, |ui| {
        if st.cfg_text.as_ref().is_none_or(|(n, _)| n != name) {
            st.cfg_text = Some((name.to_string(), sec.clone()));
        }
        let buf = &mut st.cfg_text.as_mut().unwrap().1;
        ui.add(egui::TextEdit::multiline(buf).code_editor().desired_rows(8).desired_width(f32::INFINITY));
        ui.horizontal(|ui| {
            if ui.button("Apply").clicked() {
                let b = buf.clone();
                ed.set_cfg_section(name, &b);
            }
            if ui.button("Revert").clicked() {
                *buf = sec.clone();
            }
        });
    });
}

fn mob_ui(ui: &mut egui::Ui, ed: &mut Editor, st: &mut InspectorState, name: &str) {
    let Some(obj) = ed.project.mob().and_then(|m| m.find(name).map(|i| m.objects[i].clone())) else {
        ed.sel = Sel::None;
        return;
    };
    ui.heading(&obj.name);
    ui.label(RichText::new(format!("Map object ({}) in Scene/MapObjects.mob", obj.kind.label())).weak());
    ui.separator();
    let mut o = obj.clone();
    let mut changed = false;
    egui::Grid::new("mob").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        let buf = match &mut st.rename {
            Some((n, b)) if n == name => b,
            _ => {
                st.rename = Some((name.to_string(), name.to_string()));
                &mut st.rename.as_mut().unwrap().1
            }
        };
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(buf).desired_width(170.0));
            if buf != name && ui.button("Rename").clicked() {
                let nb = buf.clone();
                if let Err(e) = ed.rename_mob(name, &nb) {
                    st.message = Some(format!("{e:#}"));
                }
            }
        });
        ui.end_row();
        ui.label("Type");
        egui::ComboBox::from_id_salt("mobtype").selected_text(o.kind.label()).show_ui(ui, |ui| {
            for k in ObjectType::ALL {
                changed |= ui.selectable_value(&mut o.kind, k, k.label()).changed();
            }
        });
        ui.end_row();
        ui.label("Position");
        changed |= vec3_edit(ui, &mut o.position, 2.0);
        ui.end_row();
        ui.label("Rotation (°)");
        changed |= vec3_edit(ui, &mut o.pitch_yaw_roll, 0.5);
        ui.end_row();
        ui.label("Scale");
        changed |= vec3_edit(ui, &mut o.scale, 0.01);
        ui.end_row();
        ui.label("Box min");
        changed |= vec3_edit(ui, &mut o.bbox_min, 2.0);
        ui.end_row();
        ui.label("Box max");
        changed |= vec3_edit(ui, &mut o.bbox_max, 2.0);
        ui.end_row();
        ui.label("Skin");
        changed |= ui.add(egui::DragValue::new(&mut o.skin_id)).changed();
        ui.end_row();
        ui.label("Placement");
        let mut ig = o.ignore_collision_on_placement != 0;
        if ui.checkbox(&mut ig, "Ignore collision on placement").changed() {
            o.ignore_collision_on_placement = ig as u8;
            changed = true;
        }
        ui.end_row();
    });
    if changed {
        ed.set_mob_object(name, o.clone(), "Edit object");
    } else if !ui.ctx().egui_is_using_pointer() {
        ed.end_stroke();
    }
    ui.horizontal(|ui| {
        if ui.button("Snap to ground").clicked()
            && let Some(h) = ed.ground_height(o.position[0], o.position[2])
        {
            let mut n = o.clone();
            n.position[1] = h;
            ed.set_mob_object(name, n, "Snap to ground");
            ed.end_stroke();
        }
        if ui.button("Delete").clicked() {
            let _ = ed.delete_selected();
        }
    });
    cfg_ui(ui, ed, st, name);
}

fn dsc_ui(ui: &mut egui::Ui, ed: &mut Editor, st: &mut InspectorState, stem: &str) {
    let Some(c) = ed.dsc_central(stem) else {
        ed.sel = Sel::None;
        return;
    };
    ui.heading(stem);
    ui.label(RichText::new("Scene object: a mesh listed in Scene/room.dsc (the game loads its .SCB, else its .sco)").weak());
    ui.separator();
    let files: Vec<String> = ed.project.files.iter().filter(|f| f.path.to_ascii_lowercase().starts_with("scene/") && f.stem().eq_ignore_ascii_case(stem) && matches!(f.kind, FileKind::Scb | FileKind::Sco)).map(|f| f.path.clone()).collect();
    let mut p = c;
    egui::Grid::new("dsc").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Central point");
        if vec3_edit(ui, &mut p, 2.0) {
            let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
            if let Err(e) = ed.move_object(&Sel::Dsc(stem.into()), d) {
                st.message = Some(format!("{e:#}"));
            }
        } else if !ui.ctx().egui_is_using_pointer() {
            ed.end_stroke();
        }
        ui.end_row();
        ui.label("Files");
        ui.vertical(|ui| {
            for f in &files {
                if ui.link(f).clicked() {
                    ed.sel = Sel::File(f.clone());
                }
            }
        });
        ui.end_row();
        if let Some(e) = ed.project.dsc_entries().into_iter().find(|e| e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0).eq_ignore_ascii_case(stem)) {
            ui.label("Quality");
            ui.label(format!("{} (loaded when world quality ≥ this; -100 = always)", e.quality));
            ui.end_row();
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Snap to ground").clicked()
            && let Some(h) = ed.ground_height(c[0], c[2])
        {
            let _ = ed.move_object(&Sel::Dsc(stem.into()), [0.0, h - c[1], 0.0]);
            ed.end_stroke();
        }
        if ui.button("Delete").clicked() {
            let _ = ed.delete_selected();
        }
    });
    cfg_ui(ui, ed, st, stem);
}

fn file_ui(ui: &mut egui::Ui, ed: &mut Editor, st: &mut InspectorState, keys: &mut KeyNames, path: &str) {
    let Some(f) = ed.project.file(path) else {
        ed.sel = Sel::None;
        return;
    };
    let kind = f.kind;
    ui.heading(f.file_name());
    ui.label(RichText::new(format!("{} — {} — {} bytes{}", f.path, kind.label(), f.original.len(), if f.dirty { " — modified" } else { "" })).weak());
    if let Some(e) = &f.parse_error {
        ui.colored_label(Color32::from_rgb(255, 150, 90), format!("Not readable as {}: {e}. It is kept as it is.", kind.label()));
    }
    ui.separator();
    match kind {
        FileKind::Text => text_ui(ui, ed, st, path),
        FileKind::Inibin => inibin_ui(ui, ed, st, keys, path),
        FileKind::Nvr => nvr_ui(ui, ed, st, path),
        FileKind::Wgeo => wgeo_ui(ui, ed, st, path),
        FileKind::NavGrid => navgrid_ui(ui, ed),
        FileKind::Mob => {
            if let Some(m) = ed.project.mob() {
                let mut counts = std::collections::BTreeMap::new();
                for o in &m.objects {
                    *counts.entry(o.kind.label()).or_insert(0) += 1;
                }
                ui.label(format!("{} objects", m.objects.len()));
                for (k, n) in counts {
                    ui.label(format!("  {n:5}  {k}"));
                }
                ui.label(RichText::new("Select objects on the map to edit them.").weak());
            }
        }
        FileKind::Scb | FileKind::Sco => {
            match &ed.project.file(path).unwrap().doc {
                Doc::Scb(s) => {
                    ui.label(format!("Mesh {}: {} vertices, {} faces", s.name, s.vertices.len(), s.faces.len()));
                    ui.label(format!("Central point {:.1} {:.1} {:.1}", s.central[0], s.central[1], s.central[2]));
                    let mut mats: Vec<&str> = s.faces.iter().map(|f| f.material.as_str()).collect();
                    mats.sort();
                    mats.dedup();
                    ui.label(format!("Materials: {}", mats.join(", ")));
                }
                Doc::Sco(s) => {
                    ui.label(format!("Mesh {}: {} vertices, {} faces", s.name, s.vertices.len(), s.face_count));
                    ui.label(format!("Central point {:.1} {:.1} {:.1}", s.central[0], s.central[1], s.central[2]));
                }
                _ => {}
            }
            let stem = ed.project.file(path).unwrap().stem().to_string();
            if ed.project.dsc_entries().iter().any(|e| e.file.to_ascii_lowercase().starts_with(&format!("{}.", stem.to_ascii_lowercase())))
                && ui.button("Select the scene object").clicked()
            {
                ed.sel = Sel::Dsc(stem);
            }
        }
        FileKind::Dds => dds_ui(ui, ed, st, path),
        FileKind::Lua => {
            if st.lua.as_ref().is_none_or(|(p, _)| p != path) {
                let strings = luaobj::inspect(&ed.project.file(path).unwrap().original).map(|i| i.strings).unwrap_or_default();
                st.lua = Some((path.to_string(), strings));
            }
            let s = &st.lua.as_ref().unwrap().1;
            ui.label("Compiled Lua 5.1 script; copied unchanged. String constants it uses:");
            egui::ScrollArea::vertical().max_height(500.0).show(ui, |ui| {
                for x in s {
                    ui.monospace(x);
                }
            });
        }
        FileKind::AiMesh => {
            if let Doc::AiMesh(a) = &ed.project.file(path).unwrap().doc {
                ui.label(format!("{} triangles (version {})", a.triangles.len(), a.version));
                ui.label(RichText::new("The client reads AIPath.aimesh_ngrid; this legacy mesh is kept for completeness.").weak());
            }
        }
        FileKind::Binary => {
            let b = &ed.project.file(path).unwrap().original;
            let hex: String = b.iter().take(256).enumerate().map(|(i, x)| format!("{x:02x}{}", if i % 16 == 15 { "\n" } else { " " })).collect();
            ui.monospace(hex);
        }
    }
}

fn text_ui(ui: &mut egui::Ui, ed: &mut Editor, st: &mut InspectorState, path: &str) {
    let cur = ed.project.text(path).unwrap_or("").to_string();
    if st.text.as_ref().is_none_or(|(p, _)| p != path) {
        st.text = Some((path.to_string(), cur.clone()));
    }
    let buf = &mut st.text.as_mut().unwrap().1;
    let edited = *buf != cur;
    ui.horizontal(|ui| {
        if ui.add_enabled(edited, egui::Button::new("Apply changes")).clicked() {
            ed.record("Edit text", &[path.to_string()]);
            ed.project.set_text(path, buf.clone());
            ed.touch_all();
        }
        if ui.add_enabled(edited, egui::Button::new("Revert")).clicked() {
            *buf = cur.clone();
        }
        ui.label(RichText::new("Line endings are kept as typed (the game's files use CRLF).").weak());
    });
    egui::ScrollArea::both().show(ui, |ui| {
        ui.add(egui::TextEdit::multiline(buf).code_editor().desired_width(f32::INFINITY).desired_rows(30));
    });
}

fn inibin_ui(ui: &mut egui::Ui, ed: &mut Editor, st: &mut InspectorState, keys: &mut KeyNames, path: &str) {
    // Learn names from a matching .ini next to it.
    let stem = ed.project.file(path).unwrap().stem().to_string();
    let dir = path.rsplit_once('/').map_or("", |x| x.0);
    for ext in ["ini", "cfg"] {
        let p = if dir.is_empty() { format!("{stem}.{ext}") } else { format!("{dir}/{stem}.{ext}") };
        if let Some(t) = ed.project.text(&p) {
            keys.learn_ini(t);
        }
    }
    let Doc::Inibin(ini) = &ed.project.file(path).unwrap().doc else { return };
    let entries = ini.entries.clone();
    ui.label(format!("{} values. Keys are hashes of \"Section*Key\"; names are shown where known.", entries.len()));
    let mut apply: Option<(usize, Value)> = None;
    let mut delete: Option<usize> = None;
    egui::ScrollArea::vertical().max_height(360.0).id_salt("inirows").show(ui, |ui| {
        egui::Grid::new("ini").striped(true).num_columns(3).show(ui, |ui| {
            ui.strong("Key");
            ui.strong("Type");
            ui.strong("Value");
            ui.end_row();
            for (i, e) in entries.iter().enumerate() {
                let name = keys.name(e.key).map(|s| s.to_string()).unwrap_or_else(|| format!("{:#010x}", e.key));
                if ui.selectable_label(st.ini_row == Some(i), name).clicked() {
                    st.ini_row = Some(i);
                    st.ini_value = e.value.to_text();
                }
                ui.label(e.value.kind().label());
                ui.label(e.value.to_text());
                ui.end_row();
            }
        });
    });
    if let Some(i) = st.ini_row
        && let Some(e) = entries.get(i)
    {
        ui.separator();
        ui.label(format!("Edit {:#010x} ({})", e.key, e.value.kind().label()));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut st.ini_value).desired_width(260.0));
            if ui.button("Set").clicked() {
                match Value::parse_text(e.value.kind(), &st.ini_value) {
                    Ok(v) => apply = Some((i, v)),
                    Err(err) => st.message = Some(format!("{err:#}")),
                }
            }
            if ui.button("Remove").clicked() {
                delete = Some(i);
            }
        });
    }
    ui.separator();
    ui.collapsing("Add a value", |ui| {
        egui::Grid::new("iniadd").num_columns(2).show(ui, |ui| {
            ui.label("Section");
            ui.text_edit_singleline(&mut st.ini_new_section);
            ui.end_row();
            ui.label("Key");
            ui.text_edit_singleline(&mut st.ini_new_key);
            ui.end_row();
            ui.label("Type");
            let k = st.ini_new_kind.get_or_insert(ValueKind::F32);
            egui::ComboBox::from_id_salt("inikind").selected_text(k.label()).show_ui(ui, |ui| {
                for v in ValueKind::ALL {
                    ui.selectable_value(k, v, v.label());
                }
            });
            ui.end_row();
            ui.label("Value");
            ui.text_edit_singleline(&mut st.ini_new_value);
            ui.end_row();
        });
        if ui.button("Add").clicked() {
            let key = if let Some(h) = st.ini_new_key.strip_prefix("0x") { u32::from_str_radix(h, 16).ok() } else { Some(hash_key(&st.ini_new_section, &st.ini_new_key)) };
            match (key, Value::parse_text(st.ini_new_kind.unwrap_or(ValueKind::F32), &st.ini_new_value)) {
                (Some(key), Ok(v)) => {
                    keys.add(&st.ini_new_section, &st.ini_new_key);
                    ed.record("Add setting", &[path.to_string()]);
                    if let Some(MapFile { doc: Doc::Inibin(ini), dirty, .. }) = ed.project.file_mut(path) {
                        ini.entries.retain(|e| e.key != key);
                        ini.set(key, v);
                        *dirty = true;
                    }
                    ed.touch_all();
                }
                (None, _) => st.message = Some("the key is not a valid hex hash".into()),
                (_, Err(e)) => st.message = Some(format!("{e:#}")),
            }
        }
    });
    if let Some((i, v)) = apply {
        ed.record("Edit setting", &[path.to_string()]);
        if let Some(MapFile { doc: Doc::Inibin(ini), dirty, .. }) = ed.project.file_mut(path) {
            ini.entries[i].value = v;
            *dirty = true;
        }
        ed.touch_all();
    }
    if let Some(i) = delete {
        ed.record("Remove setting", &[path.to_string()]);
        if let Some(MapFile { doc: Doc::Inibin(ini), dirty, .. }) = ed.project.file_mut(path) {
            ini.entries.remove(i);
            *dirty = true;
        }
        st.ini_row = None;
        ed.touch_all();
    }
}

fn nvr_ui(ui: &mut egui::Ui, ed: &mut Editor, st: &mut InspectorState, path: &str) {
    let Doc::Nvr(n) = &ed.project.file(path).unwrap().doc else { return };
    let tris: u32 = n.meshes.iter().map(|m| m.prims[0].index_count / 3).sum();
    ui.label(format!("NVR {}.{}: {} materials, {} meshes, {} triangles, {} vertex buffers, {} nodes", n.major, n.minor, n.materials.len(), n.meshes.len(), tris, n.vertex_buffers.len(), n.nodes.len()));
    if ui.button("Import OBJ model as new meshes…").clicked() {
        st.request_obj_import = true;
    }
    ui.separator();
    ui.strong("Materials");
    let mats: Vec<(String, i32, String)> = n.materials.iter().map(|m| (m.name.clone(), m.kind, m.channels[0].texture.clone())).collect();
    egui::ScrollArea::vertical().max_height(200.0).id_salt("nvrmats").show(ui, |ui| {
        for (i, (name, kind, tex)) in mats.iter().enumerate() {
            if ui.selectable_label(st.nvr_material == i, format!("{i:3}  {name}  [{}]  {tex}", material_type::label(*kind))).clicked() {
                st.nvr_material = i;
            }
        }
    });
    if st.nvr_material < mats.len() {
        let Doc::Nvr(n) = &ed.project.file(path).unwrap().doc else { return };
        let mut m = n.materials[st.nvr_material].clone();
        let orig = m.clone();
        egui::Grid::new("nvrmat").num_columns(2).show(ui, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut m.name);
            ui.end_row();
            ui.label("Type");
            egui::ComboBox::from_id_salt("nvrmattype").selected_text(material_type::label(m.kind)).show_ui(ui, |ui| {
                for t in 0..5 {
                    ui.selectable_value(&mut m.kind, t, material_type::label(t));
                }
            });
            ui.end_row();
            ui.label("Flags");
            ui.vertical(|ui| {
                for (bit, label) in material_flags::ALL {
                    let mut on = m.flags & bit != 0;
                    if ui.checkbox(&mut on, label).changed() {
                        m.flags = if on { m.flags | bit } else { m.flags & !bit };
                    }
                }
            });
            ui.end_row();
            for c in 0..2 {
                ui.label(format!("Texture {c}"));
                ui.text_edit_singleline(&mut m.channels[c].texture);
                ui.end_row();
            }
        });
        if m.vertex_size() != orig.vertex_size() {
            ui.colored_label(Color32::from_rgb(255, 150, 90), "This type/flag combination changes the vertex layout of existing meshes; keep the original type and the 'Dual vertex color' flag.");
        } else if m != orig {
            ed.record_once("Edit material", &[path.to_string()]);
            if let Some(MapFile { doc: Doc::Nvr(n), dirty, .. }) = ed.project.file_mut(path) {
                n.materials[st.nvr_material] = m;
                *dirty = true;
            }
            ed.scene_rev += 1;
        } else if !ui.ctx().egui_is_using_pointer() {
            ed.end_stroke();
        }
    }
    ui.separator();
    let Doc::Nvr(n) = &ed.project.file(path).unwrap().doc else { return };
    let count = n.meshes.len();
    ui.strong(format!("Meshes ({count})"));
    let rows: Vec<String> = n.meshes.iter().enumerate().map(|(i, m)| format!("{i:4}  {}  q{}  ({:.0}, {:.0})", n.materials[m.material as usize].name, m.quality, m.sphere[0], m.sphere[2])).collect();
    egui::ScrollArea::vertical().max_height(200.0).id_salt("nvrmesh").show_rows(ui, 18.0, count, |ui, range| {
        for i in range {
            if ui.selectable_label(st.nvr_mesh == i, &rows[i]).clicked() {
                st.nvr_mesh = i;
            }
        }
    });
    if st.nvr_mesh < count {
        let m = n.meshes[st.nvr_mesh].clone();
        ui.label(format!("Mesh {}: {} vertices, {} triangles, centre {:.0} {:.0} {:.0}", st.nvr_mesh, m.prims[0].vertex_count, m.prims[0].index_count / 3, m.sphere[0], m.sphere[1], m.sphere[2]));
        ui.horizontal(|ui| {
            let hidden = m.quality >= 100;
            if ui.button(if hidden { "Show mesh" } else { "Hide mesh" }).clicked() {
                ed.record("Hide mesh", &[path.to_string()]);
                if let Some(MapFile { doc: Doc::Nvr(n), dirty, .. }) = ed.project.file_mut(path) {
                    n.meshes[st.nvr_mesh].quality = if hidden { -100 } else { 1000 };
                    *dirty = true;
                }
                ed.scene_rev += 1;
            }
            ui.label(RichText::new("Hidden meshes keep their data but are never drawn.").weak());
        });
        ui.horizontal(|ui| {
            ui.label("Move by");
            vec3_edit(ui, &mut st.nvr_move, 5.0);
            if ui.button("Move").clicked() {
                ed.record("Move mesh", &[path.to_string()]);
                if let Some(MapFile { doc: Doc::Nvr(n), dirty, .. }) = ed.project.file_mut(path) {
                    n.translate_mesh(st.nvr_mesh, st.nvr_move);
                    *dirty = true;
                }
                ed.scene_rev += 1;
            }
        });
    }
}

fn wgeo_ui(ui: &mut egui::Ui, ed: &mut Editor, st: &mut InspectorState, path: &str) {
    let Doc::Wgeo(w) = &ed.project.file(path).unwrap().doc else { return };
    ui.label(format!("WGEO v{}: {} meshes, {} triangles; ground buckets {}x{} ({} triangles)", w.version, w.meshes.len(), w.triangle_count(), w.buckets.header.buckets_per_side, w.buckets.header.buckets_per_side, w.buckets.indices.len() / 3));
    if ui.button("Import OBJ model as new meshes…").clicked() {
        st.request_obj_import = true;
    }
    let rows: Vec<String> = w.meshes.iter().enumerate().map(|(i, m)| format!("{i:4}  {}  {}  {} tris", m.texture, m.material, m.indices.len() / 3)).collect();
    let count = rows.len();
    egui::ScrollArea::vertical().max_height(300.0).id_salt("wgeomesh").show_rows(ui, 18.0, count, |ui, range| {
        for i in range {
            if ui.selectable_label(st.wgeo_mesh == i, &rows[i]).clicked() {
                st.wgeo_mesh = i;
            }
        }
    });
    if st.wgeo_mesh < count {
        let mut m = w.meshes[st.wgeo_mesh].clone();
        let (t0, m0) = (m.texture.clone(), m.material.clone());
        egui::Grid::new("wgeom").num_columns(2).show(ui, |ui| {
            ui.label("Texture");
            ui.text_edit_singleline(&mut m.texture);
            ui.end_row();
            ui.label("Material");
            ui.text_edit_singleline(&mut m.material);
            ui.end_row();
        });
        if m.texture != t0 || m.material != m0 {
            ed.record_once("Edit WGEO mesh", &[path.to_string()]);
            if let Some(MapFile { doc: Doc::Wgeo(w), dirty, .. }) = ed.project.file_mut(path) {
                w.meshes[st.wgeo_mesh].texture = m.texture;
                w.meshes[st.wgeo_mesh].material = m.material;
                *dirty = true;
            }
            ed.scene_rev += 1;
        } else if !ui.ctx().egui_is_using_pointer() {
            ed.end_stroke();
        }
    }
}

fn navgrid_ui(ui: &mut egui::Ui, ed: &mut Editor) {
    let Some(g) = ed.project.navgrid() else { return };
    let (mut wall, mut brush, mut other) = (0, 0, 0);
    for k in 0..g.cell_count() {
        let f = g.flags(k);
        if f & flags::WALL != 0 {
            wall += 1;
        }
        if f & flags::BRUSH != 0 {
            brush += 1;
        }
        if f & !(flags::WALL | flags::BRUSH) != 0 {
            other += 1;
        }
    }
    ui.label(format!("Version {}, {} x {} cells of {} units", g.version, g.nx, g.ny, g.cell_size));
    ui.label(format!("From ({:.0}, {:.0}) to ({:.0}, {:.0})", g.min[0], g.min[2], g.max[0], g.max[2]));
    ui.label(format!("{} walkable, {wall} wall, {brush} brush, {other} with other flags", g.cell_count() - wall));
    ui.label(format!("Sampled heights {} x {}", g.hx, g.hy));
    if ed.nav_walk_changed {
        ui.colored_label(Color32::from_rgb(255, 200, 120), "Walkable cells changed: pathfinding hints will be recomputed on export.");
    }
    if ui.button("Recompute pathfinding hints now").clicked() {
        ed.record("Recompute hints", &["AIPath.aimesh_ngrid".to_string()]);
        if let Some(g) = ed.project.navgrid_mut() {
            g.rebuild_hints();
        }
        ed.nav_walk_changed = false;
    }
    ui.label(RichText::new("Use the Paint tool on the map to change walls, brush and heights.").weak());
}

fn dds_ui(ui: &mut egui::Ui, ed: &mut Editor, st: &mut InspectorState, path: &str) {
    let f = ed.project.file(path).unwrap();
    let bytes = f.bytes().unwrap_or_default();
    let rev = ed.scene_rev;
    if st.preview.as_ref().is_none_or(|(p, r, _, _)| p != path || *r != rev) {
        match dds::decode(&bytes, 1024) {
            Ok(img) => {
                let info = dds::info(&bytes).map(|i| format!("{} x {}, {}, {} mip levels", i.width, i.height, i.format, i.mipmaps)).unwrap_or_default();
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.pixels);
                let h = ui.ctx().load_texture(path, ci, TextureOptions::LINEAR);
                st.preview = Some((path.to_string(), rev, h, info));
            }
            Err(e) => {
                ui.label(format!("Cannot preview: {e:#}"));
                st.preview = None;
            }
        }
    }
    if let Some((_, _, h, info)) = &st.preview {
        ui.label(info);
        let size = h.size_vec2();
        let w = ui.available_width().min(size.x);
        ui.image((h.id(), egui::vec2(w, w * size.y / size.x)));
    }
    if ui.button("Replace with an image (PNG/JPG/DDS)…").clicked() {
        st.request_replace_texture = Some(path.to_string());
    }
}
