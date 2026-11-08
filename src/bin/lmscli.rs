//! Command-line companion: round-trip checks, summaries and export.
use anyhow::{Result, bail};
use lol_map_studio::formats::{inibin::Inibin, nvr::Nvr};
use lol_map_studio::project::{Doc, MapProject};
use std::path::PathBuf;

fn fresh_encode(f: &lol_map_studio::project::MapFile) -> Result<Vec<u8>> {
    Ok(match &f.doc {
        Doc::Inibin(i) => {
            let mut i: Inibin = i.clone();
            i.forget_original();
            i.write()?
        }
        Doc::Nvr(n) => {
            let mut n: Nvr = (**n).clone();
            n.forget_raw();
            n.write()?
        }
        Doc::Raw => return Ok(f.original.to_vec()),
        _ => {
            let mut g = lol_map_studio::project::MapFile::new(&f.path, f.original.to_vec());
            g.dirty = true;
            g.bytes()?
        }
    })
}

fn roundtrip(dir: PathBuf) -> Result<bool> {
    let t = std::time::Instant::now();
    let p = MapProject::open(&dir)?;
    println!("{}: {} files loaded in {:.2}s, scene format {:?}", p.name, p.files.len(), t.elapsed().as_secs_f32(), p.scene_format());
    let mut ok = true;
    let mut counts = std::collections::BTreeMap::new();
    for f in &p.files {
        *counts.entry(f.kind.label()).or_insert(0) += 1;
        if let Some(e) = &f.parse_error {
            println!("  PARSE FAIL {}: {e}", f.path);
            ok = false;
            continue;
        }
        let out = fresh_encode(f)?;
        if out != *f.original {
            let first = out.iter().zip(f.original.iter()).position(|(a, b)| a != b).unwrap_or(out.len().min(f.original.len()));
            println!("  MISMATCH {}: {} vs {} bytes, first difference at {first}", f.path, out.len(), f.original.len());
            ok = false;
        }
    }
    for (k, n) in counts {
        println!("  {n:4} {k}");
    }
    println!("  {}", if ok { "every parsed file re-encodes to identical bytes" } else { "differences found" });
    Ok(ok)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("roundtrip") => {
            let mut all = true;
            for d in &args[1..] {
                all &= roundtrip(PathBuf::from(d))?;
            }
            if !all {
                std::process::exit(1);
            }
        }
        Some("buckets") => bucket_check(&args[1])?,
        Some("nvr") => nvr_info(&args[1])?,
        Some("validate") => validate_cmd(&args[1])?,
        Some("champ") => champ_test(&args[1], &args[2])?,
        Some("hudfiles") => hud_files(&args[1])?,
        Some("importtest") => import_test(&args[1], &args[2])?,
        Some("wgeoinfo") => wgeo_info(&args[1])?,
        Some("settingstest") => settings_test(&args[1])?,
        Some("props") => props_list(&args[1], args.get(2).map_or("", |s| s.as_str()))?,
        Some("nvrnear") => {
            // Scene meshes near a point: material, texture, size, height, triangles.
            let p = MapProject::open(&PathBuf::from(&args[1]))?;
            let (x, z, r): (f32, f32, f32) = (args[2].parse()?, args[3].parse()?, args[4].parse()?);
            let Some(lol_map_studio::project::Doc::Nvr(n)) = p.files.iter().find(|f| f.path.to_ascii_lowercase().ends_with("room.nvr")).map(|f| &f.doc) else { anyhow::bail!("no room.nvr") };
            for (i, m) in n.meshes.iter().enumerate() {
                let b = m.bbox;
                let (cx, cz) = ((b[0] + b[3]) / 2.0, (b[2] + b[5]) / 2.0);
                let dx = (x - x.clamp(b[0], b[3])).abs();
                let dz = (z - z.clamp(b[2], b[5])).abs();
                if dx.hypot(dz) > r || (b[3] - b[0]).max(b[5] - b[2]) > 6000.0 { continue; }
                let mat = &n.materials[m.material as usize];
                let tris = n.mesh_geometry(i).indices.len() / 3;
                println!("{i:5} {:40} tex {:36} kind {} centre ({cx:.0},{cz:.0}) size {:.0}x{:.0} y {:.0}..{:.0} tris {tris}", mat.name, mat.channels.first().map(|c| c.texture.as_str()).unwrap_or(""), mat.kind, b[3] - b[0], b[5] - b[2], b[1], b[4]);
            }
        }
        Some("laneprops") => {
            let l = lol_map_studio::layout::Layout::load(&std::fs::read_to_string(&args[1])?)?;
            let lib = lol_map_studio::props::library_from(&[lv("Map1"), lv("Season1")]);
            let seg = |p: [f32; 2], a: [f32; 2], b: [f32; 2]| { let ab = [b[0] - a[0], b[1] - a[1]]; let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-6)).clamp(0.0, 1.0); (a[0] + ab[0] * t - p[0]).hypot(a[1] + ab[1] * t - p[1]) };
            let mut counts: std::collections::BTreeMap<String, (usize, f32, f32, f32)> = Default::default();
            for p in &l.props {
                let Some(d) = lib.get(&p.prop) else { continue };
                let dl = l.lanes.iter().flat_map(|lp| lp.points.windows(2).map(|w| seg(p.pos, w[0], w[1])).collect::<Vec<_>>()).fold(f32::MAX, f32::min);
                if dl < 360.0 + d.radius * p.scale * 0.5 && d.height * p.scale > 25.0 {
                    let e = counts.entry(p.prop.clone()).or_insert((0, 0.0, d.height * p.scale, d.radius * p.scale));
                    e.0 += 1;
                    e.1 = dl;
                }
            }
            for (k, (n, dl, h, r)) in counts { println!("{n:4} x {k} (h {h:.0}, r {r:.0}, e.g. {dl:.0} from lane centre)"); }
        }
        Some("basedist") => {
            let p = MapProject::open(&PathBuf::from(&args[1]))?;
            let l = lol_map_studio::layout_import::import(&p, &PathBuf::from(&args[1]))?;
            use lol_map_studio::layout::ObjKind as K;
            for t in [1u8, 2] {
                let f = |k: K| l.objectives.iter().filter(|o| o.kind == k && o.team == t).map(|o| o.pos).collect::<Vec<_>>();
                let (s, n, sh, ft) = (f(K::SpawnPoint), f(K::Nexus), f(K::Shop), f(K::FountainTurret));
                if let (Some(s), Some(n)) = (s.first(), n.first()) {
                    let d = |a: &[f32; 2], b: &[f32; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
                    println!("team {t}: spawn {s:?} nexus {n:?} spawn-nexus {:.0}, shop {:?}, fountain turret {:?}", d(s, n), sh.first().map(|x| d(s, x)), ft.first().map(|x| d(s, x)));
                    for o in l.objectives.iter().filter(|o| o.team == t && matches!(o.kind, K::Inhibitor | K::Turret)) { print!("{:?}/{:?} {:.0}; ", o.kind, o.tier, d(n, &o.pos)); }
                    println!();
                }
            }
        }
        Some("propinfo") => {
            let lib = lol_map_studio::props::library_from(&[lv("Map1"), lv("Season1")]);
            for k in lol_map_studio::ai_agent::expand_keys(&lib, &args[1..]) {
                let k = &k;
                match lib.get(k) {
                    Some(d) => println!("{k}: [{}] r{:.0} h{:.0} copies {} mats {:?} dir {}", d.category, d.radius, d.height, d.copies, d.parts.iter().map(|p| (p.material.name.clone(), p.material.channels.first().map(|c| c.texture.clone()).unwrap_or_default())).collect::<Vec<_>>(), d.texture_dir.display()),
                    None => println!("{k}: MISSING"),
                }
            }
        }
        Some("agentscript") => {
            use lol_map_studio::ai_agent as ag;
            let file = args.get(1).ok_or_else(|| anyhow::anyhow!("usage: lmscli agentscript <recipe.json> [out folder] [project.lmsmap]"))?;
            let script: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file)?)?;
            let lib = lol_map_studio::props::library_from(&[lv("Map1"), lv("Season1")]);
            let mut ws = ag::Workshop::new(Some(std::sync::Arc::new(lib)), Some(lv("Map1/Scene/Textures")), Vec::new());
            let t = std::time::Instant::now();
            for line in ag::run_script(&script, &mut ws, |_, _| {})? {
                println!("  {line}");
            }
            println!("{} tool calls in {:.1}s\n{}\n{}", ws.calls, t.elapsed().as_secs_f32(), ag::overview(&ws.l), ag::check(&ws.l));
            if let Some(proj) = args.get(3) {
                std::fs::write(proj, ws.l.save()?)?;
                println!("project saved to {proj}");
            }
            if let Some(out) = args.get(2) {
                let opt = lol_map_studio::layout_build::BuildOptions { rules_source: Some(lv("Map1")), preview: false, props: ws.lib.clone() };
                let p = lol_map_studio::layout_build::build(&ws.l, &opt)?;
                p.export(&PathBuf::from(out), true)?;
                let again = MapProject::open(&PathBuf::from(out))?;
                println!("built {} files, reopened {}", p.files.len(), again.files.len());
                // The exported navigation grid (with every model's footprint): spawns free, bases connected.
                if let Some(g) = again.navgrid() {
                    let walk = g.walkable();
                    let cell = |p: [f32; 2]| g.cell_of(p[0], p[1]);
                    let find = |k: lol_map_studio::layout::ObjKind, t: u8| ws.l.objectives.iter().find(|o| o.kind == k && o.team == t).map(|o| o.pos);
                    let (bs, rs, bn, rn) = (find(lol_map_studio::layout::ObjKind::SpawnPoint, 1), find(lol_map_studio::layout::ObjKind::SpawnPoint, 2), find(lol_map_studio::layout::ObjKind::Nexus, 1), find(lol_map_studio::layout::ObjKind::Nexus, 2));
                    if let (Some(bs), Some(rs), Some(bn), Some(rn)) = (bs, rs, bn, rn) {
                        let free = |p: [f32; 2]| cell(p).is_some_and(|(i, j)| walk[j * g.nx + i]);
                        println!("navgrid: blue spawn walkable {}, red spawn walkable {}", free(bs), free(rs));
                        let ring = |c: [f32; 2]| { let (mut w, mut n) = (0, 0); for dz in (-500..=500).step_by(50) { for dx in (-500..=500).step_by(50) { if dx * dx + dz * dz <= 500 * 500 { n += 1; if free([c[0] + dx as f32, c[1] + dz as f32]) { w += 1; } } } } w as f32 * 100.0 / n as f32 };
                        println!("navgrid: walkable within 500 of the spawn: blue {:.0}%, red {:.0}%", ring(bs), ring(rs));
                        if let Some(pw) = lol_map_studio::play::World::from_project(&again) { let line: Vec<String> = (-8..=8).map(|k| format!("{:.0}", pw.height(bs[0] + k as f32 * 100.0, bs[1]))).collect(); println!("stand height across the blue fountain (x -800..+800 every 100): {}", line.join(" ")); }
                        if let Some(start) = cell(bs) {
                            let d = lol_map_studio::formats::navgrid::walking_distances(&walk, g.nx, g.ny, start);
                            let at = |p: [f32; 2]| cell(p).map(|(i, j)| d[j * g.nx + i]).unwrap_or(f32::INFINITY);
                            println!("navgrid: blue spawn -> red nexus {:.0}, -> red spawn {:.0}, -> own nexus {:.0} (cells)", at(rn), at(rs), at(bn));
                            let blocked: Vec<String> = ws.l.objectives.iter().filter(|o| !o.kind.is_camp() && !at(o.pos).is_finite() && !free(o.pos)).map(|o| format!("{:?} {}", o.kind, o.id)).collect();
                            println!("navgrid: objects on blocked cells: {blocked:?}");
                        }
                    }
                }
            }
        }
        Some("aitest") => {
            let prompt = args.get(1).cloned().unwrap_or_default();
            let mut plan = lol_map_studio::ai_map::plan_from_keywords(&prompt);
            println!("online: {:?}", plan.online.iter().map(|o| &o.query).collect::<Vec<_>>());
            let mut lib = lol_map_studio::props::library_from(&[lv("Map1")]);
            for line in lol_map_studio::ai_map::fetch_online(&mut plan, Some(&lib), Some(&lv("Map1/Scene/Textures"))) { println!("  {line}"); }
            lib.set_assets(lol_map_studio::asset::library_props());
            let l = lol_map_studio::ai_map::build_layout(&plan, Some(&lib), 30);
            let online = l.props.iter().filter(|p| p.prop.starts_with("asset:")).count();
            println!("map: {} props, {} of them online models", l.props.len(), online);
        }
        Some("onlinetest") => {
            use lol_map_studio::online_assets as oa;
            let q = args.get(1).cloned().unwrap_or_else(|| "wooden barrel".into());
            let cat = oa::catalogue()?;
            let hits = oa::search(&cat, &q);
            println!("{} models; {q:?}: {:?}", cat.len(), hits.iter().take(5).map(|m| (&m.id, m.polycount)).collect::<Vec<_>>());
            let m = hits.first().ok_or_else(|| anyhow::anyhow!("no match"))?;
            let c = oa::category_for(m, "");
            let lib = lol_map_studio::props::library_from(&[lv("Map1")]);
            let t = std::time::Instant::now();
            let key = oa::add_classic(m, &c, Some(&lib), Some(&lv("Map1/Scene/Textures")))?;
            let name = key.trim_start_matches("asset:");
            let (a, _) = lol_map_studio::asset::load(&lol_map_studio::asset::asset_dir(name))?;
            let (lo, hi) = a.bounds();
            println!("{key} [{c}] in {:.1}s: {:?} verts/tris, size {:.0} x {:.0} x {:.0}, blocks {}", t.elapsed().as_secs_f32(), a.stats(), hi[0]-lo[0], hi[1]-lo[1], hi[2]-lo[2], a.blocks);
            println!("folder {}", lol_map_studio::asset::asset_dir(name).display());
        }
        Some("dresstest") => {
            let mut l = lol_map_studio::layout::Layout::classic_start("T", 30, 12000.0);
            let dirs: Vec<PathBuf> = std::fs::read_dir(lol_map_studio::paths::levels_dir().unwrap_or_default())?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            let lib = lol_map_studio::props::library_from(&dirs);
            println!("sources {:?}", lib.sources());
            for m in [vec![], vec!["Map1".to_string()], vec!["map11".to_string()]] { println!("{m:?}: trees {:?} rocks {:?}", lib.default_forest_in(&m), lib.default_wall_rocks_in(&m)); }
            let d = lol_map_studio::layout_build::wall_dressing(&l, &lib, None);
            println!("classic map dressing: {} models, not from Map1: {}", d.len(), d.iter().filter(|p| !p.key.starts_with("Map1/")).count());
            let g = |l: &lol_map_studio::layout::Layout, x: f32, z: f32| l.cell_of(x, z).map(|(i, j)| l.surface[j * l.nx + i]).unwrap_or(9);
            let mut c0 = (0.0f32, 0.0f32);
            'f: for zz in (8..40).map(|k| k as f32 * 250.0) { for xx in (8..40).map(|k| k as f32 * 250.0) { if (-4..=4).all(|a| (-4..=4).all(|b| g(&l, xx + a as f32 * 100.0, zz + b as f32 * 100.0) == 1)) { c0 = (xx, zz); break 'f; } } }
            let cc = c0; println!("open ground at {cc:?}");
            let count = |l: &lol_map_studio::layout::Layout| { let d = lol_map_studio::layout_build::wall_dressing(l, &lib, None); let near = d.iter().filter(|p| (p.pos[0] - cc.0).abs() < 600.0 && (p.pos[1] - cc.1).abs() < 600.0).count(); (d.len(), near) };
            println!("before {:?}", count(&l));
            for k in 0..20 { l.paint_surface(cc.0 - 300.0 + k as f32 * 30.0, cc.1 - 300.0 + k as f32 * 30.0, 125.0, false, lol_map_studio::layout::Surface::Wall); }
            println!("after {:?} surface {}", count(&l), g(&l, cc.0, cc.1));
        }
        Some("layouttest") => layout_test(&args[1], args.get(2).map(|s| s.as_str()))?,
        Some("edittest") => edit_test(&args[1], &args[2])?,
        Some("new") => new_cmd(&args[1], &args[2], args.get(3).map_or(Ok(12000.0), |s| s.parse())?, args.iter().any(|a| a == "--blank"))?,
        Some("nvrmat") => nvr_mat(&args[1], args[2].parse()?)?,
        Some("copy") if args.len() == 3 => {
            let p = MapProject::open(&PathBuf::from(&args[1]))?;
            let r = p.export(&PathBuf::from(&args[2]), false)?;
            println!("wrote {} files ({} bytes)", r.files, r.bytes);
        }
        _ => bail!("usage: lmscli roundtrip <map folder>... | copy <map folder> <new folder>"),
    }
    Ok(())
}

#[allow(dead_code)]
pub fn bucket_check(dir: &str) -> Result<()> {
    use lol_map_studio::formats::wgeo::{Wgeo, build_buckets};
    let p = MapProject::open(&PathBuf::from(dir))?;
    let Doc::Wgeo(w) = &p.file("Scene/room.wgeo").unwrap().doc else { bail!("no wgeo") };
    let w: &Wgeo = w;
    let h = w.buckets.header;
    println!("original: {:?} verts {} idx {}", h, w.buckets.vertices.len(), w.buckets.indices.len());
    // Recover the triangle list the original was built from: bucket triangles in order.
    let mut tris = Vec::new();
    for b in &w.buckets.buckets {
        let n = (b.inside_triangles as usize + b.outside_triangles as usize) * 3;
        for k in 0..n {
            let i = w.buckets.indices[b.index_start as usize + k] as usize;
            tris.push(w.buckets.vertices[b.vertex_start as usize + i]);
        }
    }
    let rebuilt = build_buckets(&tris, h.buckets_per_side);
    println!("rebuilt:  {:?} verts {} idx {}", rebuilt.header, rebuilt.vertices.len(), rebuilt.indices.len());
    println!("identical: {}", rebuilt == w.buckets);
    println!("differing buckets: {}", (0..rebuilt.buckets.len()).filter(|&i| rebuilt.buckets[i] != w.buckets.buckets[i]).count());
    if let Some(i) = (0..rebuilt.buckets.len()).find(|&i| rebuilt.buckets[i] != w.buckets.buckets[i]) { println!("bucket {i}: {:?} vs {:?}", rebuilt.buckets[i], w.buckets.buckets[i]); }
    println!("verts same {} idx same {}", rebuilt.vertices == w.buckets.vertices, rebuilt.indices == w.buckets.indices);
    let total: usize = w.meshes.iter().map(|m| m.indices.len() / 3).sum();
    println!("render triangles {total}, header total {}, bucket triangles {}", w.total_triangles, tris.len() / 3);
    Ok(())
}

#[allow(dead_code)]
pub fn nvr_info(dir: &str) -> Result<()> {
    let p = MapProject::open(&PathBuf::from(dir))?;
    let Doc::Nvr(n) = &p.file("Scene/room.nvr").unwrap().doc else { bail!("no nvr") };
    println!("materials {} vbufs {} ibufs {} meshes {} nodes {}", n.materials.len(), n.vertex_buffers.len(), n.index_buffers.len(), n.meshes.len(), n.nodes.len());
    let root = n.nodes.last().unwrap();
    println!("root {:?}", root);
    for (i, m) in n.materials.iter().enumerate().take(100) {
        let uses: Vec<usize> = n.meshes.iter().enumerate().filter(|(_, x)| x.material as usize == i).map(|(k, _)| k).collect();
        let col = uses.first().map(|&k| { let g = n.mesh_geometry(k); (g.colors.iter().take(3).map(|c| format!("{c:08x}")).collect::<Vec<_>>().join(","), g.positions.len(), g.indices.len()) });
        println!("{i:3} {:28} type {} flags {:#x} stride {} tex {:40} meshes {:4} {:?} c0 {:?} c1tex {:?}", m.name, m.kind, m.flags, m.vertex_size(), m.channels[0].texture, uses.len(), col, m.channels[0].color, m.channels[1].texture);
    }
    let q: std::collections::BTreeMap<i32, usize> = n.meshes.iter().fold(Default::default(), |mut a, m| { *a.entry(m.quality).or_default() += 1; a });
    println!("qualities {q:?}");
    Ok(())
}

#[allow(dead_code)]
pub fn nvr_mat(dir: &str, idx: usize) -> Result<()> {
    let p = MapProject::open(&PathBuf::from(dir))?;
    let Doc::Nvr(n) = &p.file("Scene/room.nvr").unwrap().doc else { bail!("no nvr") };
    let m = &n.materials[idx];
    println!("{} type {} flags {:#x}", m.name, m.kind, m.flags);
    for (i, c) in m.channels.iter().enumerate() {
        println!("  ch{i} color {:?} tex {:?} transform {:?}", c.color, c.texture, c.transform);
    }
    Ok(())
}

#[allow(dead_code)]
pub fn validate_cmd(dir: &str) -> Result<()> {
    let p = MapProject::open(&PathBuf::from(dir))?;
    for i in lol_map_studio::validate::validate(&p) {
        println!("{:?}: {}{}", i.severity, i.file.map(|f| format!("[{f}] ")).unwrap_or_default(), i.message);
    }
    Ok(())
}

#[allow(dead_code)]
pub fn new_cmd(template: &str, out: &str, size: f32, blank: bool) -> Result<()> {
    use lol_map_studio::create::{NewMapOptions, StartLayout, new_map};
    let t = std::time::Instant::now();
    let tp = MapProject::open(&PathBuf::from(template))?;
    let out = PathBuf::from(out);
    let mut o = NewMapOptions { folder_name: out.file_name().unwrap().to_string_lossy().to_string(), size: [size, size], ..Default::default() };
    if blank {
        o.layout = StartLayout::Blank;
        o.hills = 80.0;
    }
    let p = new_map(&tp, &o)?;
    println!("generated {} files in {:.1}s", p.files.len(), t.elapsed().as_secs_f32());
    for i in lol_map_studio::validate::validate(&p) {
        println!("  {:?}: {}{}", i.severity, i.file.map(|f| format!("[{f}] ")).unwrap_or_default(), i.message);
    }
    let r = p.export(&out, true)?;
    println!("wrote {} files, {} bytes; reopening...", r.files, r.bytes);
    let again = MapProject::open(&out)?;
    let bad: Vec<_> = again.files.iter().filter(|f| f.parse_error.is_some()).map(|f| format!("{}: {}", f.path, f.parse_error.as_ref().unwrap())).collect();
    println!("reopened {} files, {} unreadable {:?}, scene format {:?}", again.files.len(), bad.len(), bad, again.scene_format());
    Ok(())
}

/// Edits a map through the editor operations the GUI uses, exports it, reopens it and checks
/// that every untouched file is byte-identical and every edit landed.
#[allow(dead_code)]
pub fn edit_test(src: &str, out: &str) -> Result<()> {
    use lol_map_studio::edit::{Editor, NavPaint, Sel};
    use lol_map_studio::formats::{inibin::{Value, hash_key}, mob::ObjectType, navgrid::flags};
    let original = MapProject::open(&PathBuf::from(src))?;
    let mut ed = Editor::new(original.clone());
    let baked = ed.project.scene_format() == Some(lol_map_studio::project::SceneFormat::Baked);
    let mut expect_changed: Vec<String> = Vec::new();

    // 1. Move a turret (room.dsc mesh on Map1, map object on map11).
    let turret_sel = if ed.project.dsc_entries().iter().any(|e| e.file.starts_with("Turret_T1_C_05")) { Sel::Dsc("Turret_T1_C_05".into()) } else { Sel::Mob("Turret_T1_C_05".into()) };
    let before = match &turret_sel { Sel::Dsc(s) => ed.dsc_central(s).unwrap(), Sel::Mob(n) => { let m = ed.project.mob().unwrap(); m.objects[m.find(n).unwrap()].position } _ => unreachable!() };
    ed.move_object(&turret_sel, [300.0, 0.0, 200.0])?;
    ed.end_stroke();
    // 2. Move the blue spawn.
    ed.move_object(&Sel::Mob("__Spawn_T1".into()), [50.0, 0.0, 50.0])?;
    ed.end_stroke();
    // 3. Duplicate the turret (with its ObjectCFG section).
    ed.sel = turret_sel.clone();
    let dup = ed.duplicate_selected([before[0] + 900.0, before[2] - 600.0])?;
    println!("duplicated as {dup:?}");
    // 4. Place a new nav point.
    let placed = ed.place_object(ObjectType::NavPoint, "__NAV_C099", [7000.0, 7000.0])?;
    // 5. Paint a wall circle and a brush circle.
    ed.paint_nav(NavPaint::Wall, 6000.0, 9000.0, 300.0, 1.0);
    ed.end_stroke();
    ed.paint_nav(NavPaint::Brush, 9000.0, 6000.0, 250.0, 1.0);
    ed.end_stroke();
    // 6. Change a setting in terrain.inibin.
    let fog_key = hash_key("Environment", "TestValue");
    if let Some(lol_map_studio::project::MapFile { doc: Doc::Inibin(ini), dirty, .. }) = ed.project.file_mut("terrain.inibin") {
        ini.set(fog_key, Value::F32(42.5));
        *dirty = true;
    }
    // 7. Import a model (a 200-unit cube) into the scene.
    let cube = "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nf 1/1 2/2 3/3 4/4\nf 5/1 8/4 7/3 6/2\nf 1/1 5/2 6/3 2/4\nf 2/1 6/2 7/3 3/4\nf 3/1 7/2 8/3 4/4\nf 5/1 1/2 4/3 8/4\n";
    let mut g = lol_map_studio::scene::read_obj(cube)?;
    lol_map_studio::scene::place_geometry(&mut g, 200.0, 30.0, [7200.0, 60.0, 7200.0]);
    if baked {
        let f = ed.project.file_mut("Scene/room.wgeo").unwrap();
        let Doc::Wgeo(w) = &mut f.doc else { bail!("no wgeo") };
        let mut ground = w.bucket_triangles();
        for &i in &g.indices { ground.push(g.positions[i as usize]); }
        w.meshes.push(lol_map_studio::formats::wgeo::WgeoMesh::new("grnd_terrain_a.dds", "custom_cube", g.positions.clone(), g.uvs.clone(), g.indices.clone()));
        let side = w.buckets.header.buckets_per_side;
        w.buckets = lol_map_studio::formats::wgeo::build_buckets(&ground, side);
        w.total_triangles = w.triangle_count();
        f.dirty = true;
    } else {
        let f = ed.project.file_mut("Scene/room.nvr").unwrap();
        let Doc::Nvr(n) = &mut f.doc else { bail!("no nvr") };
        let m = n.add_material(lol_map_studio::formats::nvr::Material::new("custom_cube", "rocks.dds", 0, 0x10));
        n.append_geometry(m, &g, -100)?;
        f.dirty = true;
    }

    let steps = ed.prepare_export()?;
    println!("prepare: {steps:?}");
    for f in &ed.project.files { if f.dirty { expect_changed.push(f.path.to_ascii_lowercase()); } }
    for i in lol_map_studio::validate::validate(&ed.project).iter().filter(|i| i.severity == lol_map_studio::validate::Severity::Error) {
        println!("  validation error: {}", i.message);
    }
    let r = ed.project.export(&PathBuf::from(out), true)?;
    println!("exported {} files, {} re-encoded: {:?}", r.files, r.rewritten.len(), r.rewritten);

    // Reopen and check.
    let back = MapProject::open(&PathBuf::from(out))?;
    let mut problems = 0;
    for f in &back.files {
        if let Some(e) = &f.parse_error { println!("  UNREADABLE {}: {e}", f.path); problems += 1; }
        if let Some(o) = original.file(&f.path) {
            let same = *o.original == *f.original;
            let changed = expect_changed.contains(&f.path.to_ascii_lowercase());
            if !same && !changed { println!("  UNEXPECTED CHANGE {}", f.path); problems += 1; }
        }
    }
    for f in &original.files { if back.file(&f.path).is_none() { println!("  MISSING {}", f.path); problems += 1; } }
    let after = match &turret_sel { Sel::Dsc(s) => back.files.iter().find_map(|f| match &f.doc { Doc::Scb(x) if f.stem() == s => Some(x.central), _ => None }).unwrap(), Sel::Mob(n) => { let m = back.mob().unwrap(); m.objects[m.find(n).unwrap()].position } _ => unreachable!() };
    println!("turret moved {:?} -> {:?}", before, after);
    if (after[0] - before[0] - 300.0).abs() > 0.01 || (after[2] - before[2] - 200.0).abs() > 0.01 { println!("  TURRET NOT MOVED"); problems += 1; }
    let dup_name = match &dup { Sel::Dsc(s) | Sel::Mob(s) => s.clone(), _ => String::new() };
    let dup_ok = match &dup { Sel::Dsc(s) => back.dsc_entries().iter().any(|e| e.file.eq_ignore_ascii_case(&format!("{s}.sco"))) && back.find(&format!("Scene/{s}.SCB")).is_some(), Sel::Mob(s) => back.mob().unwrap().find(s).is_some(), _ => false };
    let cfg_ok = back.text("Scene/CFG/ObjectCFG.cfg").is_some_and(|t| t.contains(&format!("[{dup_name}]")));
    println!("duplicate {dup_name}: present {dup_ok}, settings copied {cfg_ok}");
    if !dup_ok || !cfg_ok { problems += 1; }
    let nav_ok = matches!(&placed, Sel::Mob(n) if back.mob().unwrap().find(n).is_some());
    let g = back.navgrid().unwrap();
    let (wi, wj) = g.cell_of(6000.0, 9000.0).unwrap();
    let (bi, bj) = g.cell_of(9000.0, 6000.0).unwrap();
    let wall = g.flags(wj * g.nx + wi) & flags::WALL != 0;
    let brush = g.flags(bj * g.nx + bi) & flags::BRUSH != 0;
    let ini = match &back.file("terrain.inibin").unwrap().doc { Doc::Inibin(i) => i.get(fog_key).cloned(), _ => None };
    let scene_ok = match back.scene_format() { Some(lol_map_studio::project::SceneFormat::Baked) => matches!(&back.file("Scene/room.wgeo").unwrap().doc, Doc::Wgeo(w) if w.meshes.iter().any(|m| m.material == "custom_cube")), _ => matches!(&back.file("Scene/room.nvr").unwrap().doc, Doc::Nvr(n) if n.materials.iter().any(|m| m.name == "custom_cube")) };
    println!("nav point placed {nav_ok}, wall painted {wall}, brush painted {brush}, setting {ini:?}, model imported {scene_ok}, renderer {:?}", back.renderer_setting());
    if !(nav_ok && wall && brush && ini == Some(Value::F32(42.5)) && scene_ok) { problems += 1; }
    println!("{}", if problems == 0 { "EDIT TEST PASSED" } else { "EDIT TEST FAILED" });
    if problems > 0 { std::process::exit(1); }
    Ok(())
}

/// Builds the classic starter layout into a full map folder and checks it.
#[allow(dead_code)]
pub fn layout_test(out: &str, rules: Option<&str>) -> Result<()> {
    use lol_map_studio::layout::Layout;
    use lol_map_studio::layout_build::{BuildOptions, build};
    let t = std::time::Instant::now();
    let mut l = Layout::classic_start("Test Rift", 30, 12000.0);
    if rules.is_none() {
        l.rules = lol_map_studio::layout::Rules::Sandbox;
    }
    let text = l.save()?;
    let back = Layout::load(&text)?;
    println!("layout {}x{} cells, {} objectives, {} lanes, project file {} KB, reload equal: {}", l.nx, l.ny, l.objectives.len(), l.lanes.len(), text.len() / 1024, back == l);
    let t2 = std::time::Instant::now();
    let lib = lol_map_studio::props::library_from(&[lv("Map1")]);
    let mut cats = std::collections::BTreeMap::new();
    for pd in &lib.props { *cats.entry(pd.category.clone()).or_insert(0) += 1; }
    println!("prop library: {} props in {:.1}s {:?}; forest {:?}; wall rocks {:?}", lib.props.len(), t2.elapsed().as_secs_f32(), cats, lib.default_forest(), lib.default_wall_rocks());
    for pd in lib.props.iter().filter(|p| p.category == "Trees").take(6) { println!("   {} r{:.0} h{:.0} x{} parts {}", pd.key, pd.radius, pd.height, pd.copies, pd.parts.len()); }
    if let Some(r) = lib.props.iter().find(|p| p.category == "Rocks") { let k = r.key.clone(); l.add_prop(&k, [6000.0, 3000.0], 30.0, 1.0); }
    l.add_effect("Fire_Cauldron.troy", [5000.0, 4000.0], 0.0, 0.0);
    l.add_light([5000.0, 4000.0], 150.0, [255, 140, 40], 20.0, 650.0, 1.0);
    l.add_objective(lol_map_studio::layout::ObjKind::HealthRelic, 1, lol_map_studio::layout::Lane::Mid, lol_map_studio::layout::TurretTier::Outer, [5600.0, 5600.0]);
    l.add_objective(lol_map_studio::layout::ObjKind::CapturePoint, 1, lol_map_studio::layout::Lane::Mid, lol_map_studio::layout::TurretTier::Outer, [4000.0, 7000.0]);
    let p = build(&l, &BuildOptions { rules_source: rules.map(PathBuf::from), preview: false, props: Some(std::sync::Arc::new(lib)) })?;
    println!("built {} files in {:.1}s", p.files.len(), t.elapsed().as_secs_f32());
    for i in lol_map_studio::validate::validate(&p) {
        println!("  {:?}: {}{}", i.severity, i.file.map(|f| format!("[{f}] ")).unwrap_or_default(), i.message);
    }
    p.export(&PathBuf::from(out), true)?;
    let again = MapProject::open(&PathBuf::from(out))?;
    let bad: Vec<_> = again.files.iter().filter(|f| f.parse_error.is_some()).map(|f| format!("{}: {}", f.path, f.parse_error.as_ref().unwrap())).collect();
    println!("reopened {} files, unreadable {:?}, scene {:?}", again.files.len(), bad, again.scene_format());
    for f in &again.files { if !f.path.contains("Textures/") { println!("    {:8} {}", f.original.len(), f.path); } }
    Ok(())
}

#[allow(dead_code)]
pub fn champ_test(data: &str, name: &str) -> Result<()> {
    {
        let c = lol_map_studio::champion::Champion::load(std::path::Path::new(data), name)?;
        println!("spells {:?}; icons {:?}; passive {}; circle {}; stats {:?}", c.spells, c.spell_icons.iter().map(|i| i.as_ref().map(|i| i.width)).collect::<Vec<_>>(), c.passive_icon.is_some(), c.circle.is_some(), c.stats);
        let h = lol_map_studio::hud::Hud::load(std::path::Path::new(data))?;
        let on = h.elements.iter().filter(|e| e.enabled).count();
        println!("hud: {} elements ({} enabled); atlas {:?}", h.elements.len(), on, h.texture_path("HUDAtlas"));
        for n in ["HUDCenterFrame", "MinimapContent", "HPStat", "Spell0"] {
            if let Some(e) = h.get(n) {
                println!("  {n}: {:?} {:?} 1920x1080 -> {:?}", e.kind, e.rect, e.screen_rect(1920.0, 1080.0));
            }
        }
    }
    let c = lol_map_studio::champion::Champion::load(std::path::Path::new(data), name)?;
    println!("{}: {} verts, {} tris, {} joints, {} influences, scale {}, speed {}, radius {}, texture {:?}, portrait {}", c.name, c.mesh.positions.len(), c.mesh.indices.len() / 3, c.skeleton.joints.len(), c.skeleton.influences.len(), c.scale, c.move_speed, c.radius, c.texture.as_ref().map(|t| (t.width, t.height)), c.portrait.is_some());
    let idle = c.find_anim(&["idle1"]);
    let run = c.find_anim(&["run"]);
    println!("idle {:?} run {:?} spells {:?} recall {:?} dance {:?}", idle, run, c.find_anim(&["spell1"]), c.find_anim(&["recall"]), c.find_anim(&["dance"]));
    let bb = |p: &[[f32; 3]]| { let (b, _) = lol_map_studio::formats::nvr::bounds(p); format!("x {:.0}..{:.0} y {:.0}..{:.0} z {:.0}..{:.0}", b[0], b[3], b[1], b[4], b[2], b[5]) };
    println!("mesh bind {}", bb(&c.mesh.positions));
    for (label, a) in [("no anim", None), ("idle", idle.as_deref()), ("run", run.as_deref())] {
        let m = c.pose(a, 0.3, true);
        let (p, _) = c.skin(&m);
        println!("posed {label:8} {}", bb(&p));
    }
    if let Some(r) = &run { let a = &c.anims[r]; println!("run: {} frames, {:.3}s/frame, {} tracks, matched {}", a.frames, a.frame_time, a.tracks.len(), c.skeleton.joints.iter().filter(|j| a.tracks.contains_key(&j.hash)).count()); }
    Ok(())
}

pub fn hud_files(data: &str) -> Result<()> {
    let h = lol_map_studio::hud::Hud::load(std::path::Path::new(data))?;
    let mut files: Vec<String> = Vec::new();
    for e in &h.elements {
        if !files.contains(&e.file) {
            files.push(e.file.clone());
        }
    }
    for f in files {
        let es: Vec<_> = h.elements.iter().filter(|e| e.file == f && e.enabled && (e.kind != lol_map_studio::hud::Kind::Icon || (e.texture != "None" && !e.texture.is_empty()))).collect();
        if es.is_empty() {
            continue;
        }
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for e in &es {
            let r = e.screen_rect(1616.0, 969.0);
            b = [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])];
        }
        println!("{f}: {} drawn, {:?} groups {:?}", es.len(), b, es.iter().map(|e| e.group.as_str()).collect::<std::collections::BTreeSet<_>>());
    }
    Ok(())
}

pub fn props_list(map: &str, cat: &str) -> Result<()> {
    let defs = lol_map_studio::props::extract(std::path::Path::new(map), "Map1")?;
    for d in defs.iter().filter(|d| cat.is_empty() || d.category == cat || d.name.to_ascii_lowercase().contains(&cat.to_ascii_lowercase())) {
        println!("{:<16} {:<34} r{:>6.0} h{:>5.0} x{} mats {:?}", d.category, d.name, d.radius, d.height, d.copies, d.parts.iter().map(|p| p.material.name.as_str()).collect::<Vec<_>>());
    }
    Ok(())
}

/// Imports an opened map as an editable layout, checks that an unchanged export is identical,
/// then edits it (structures, effects, props, camps, walls) and exports it to `out`.
pub fn import_test(map: &str, out: &str) -> Result<()> {
    use lol_map_studio::layout::{Lane, Layout, ObjKind, Surface, TurretTier};
    use lol_map_studio::layout_build::{BuildOptions, build};
    let t = std::time::Instant::now();
    let path = std::fs::canonicalize(map)?;
    let orig = MapProject::open(&path)?;
    let l = lol_map_studio::layout_import::import(&orig, &path)?;
    let mut kinds = std::collections::BTreeMap::new();
    for o in &l.objectives {
        *kinds.entry(format!("{:?}", o.kind)).or_insert(0) += 1;
    }
    let walls = l.surface.iter().filter(|&&s| s == Surface::Wall as u8).count();
    println!("imported {} in {:.1}s: {}x{} cells ({} wall), {:?}, {} lanes {:?}, {} effects, {} lights", orig.name, t.elapsed().as_secs_f32(), l.nx, l.ny, walls, kinds, l.lanes.len(), l.lanes.iter().map(|x| x.points.len()).collect::<Vec<_>>(), l.effects.len(), l.lights.len());
    for o in l.objectives.iter().filter(|o| o.kind == ObjKind::Turret).take(3) {
        println!("  {:?} {:?} team {} {:?} {:?} skin {:?} at {:?}", o.name, o.kind, o.team, o.lane, o.tier, o.skin, o.pos);
    }
    let text = l.save()?;
    println!("project file {} KB, reload equal: {}", text.len() / 1024, Layout::load(&text)? == l);
    let opt = BuildOptions { rules_source: None, preview: false, props: None };
    let p = build(&l, &opt)?;
    let mut diff = Vec::new();
    for f in &orig.files {
        match p.file(&f.path) {
            Some(g) if g.bytes()? == f.bytes()? => {}
            _ => diff.push(f.path.clone()),
        }
    }
    println!("unchanged export: {} files, {} differ {:?}", p.files.len(), diff.len(), diff.iter().take(5).collect::<Vec<_>>());

    // Edits.
    let mut l2 = l.clone();
    if let Some(o) = l2.objectives.iter_mut().find(|o| o.kind == ObjKind::Turret) {
        o.pos[0] += 300.0;
    }
    l2.add_objective(ObjKind::Turret, 1, Lane::Mid, TurretTier::Outer, [6000.0, 6000.0]);
    l2.add_objective(ObjKind::Wolves, 1, Lane::Mid, TurretTier::Outer, [5000.0, 7000.0]);
    l2.add_effect("Fire_Cauldron.troy", [7000.0, 7000.0], 0.0, 0.0);
    l2.add_light([7000.0, 7000.0], 150.0, [255, 120, 40], 20.0, 600.0, 1.0);
    l2.paint_surface(7400.0, 6600.0, 250.0, false, Surface::Wall);
    let lib = lol_map_studio::props::library_from(&[lv("Map1")]);
    if let Some(r) = lib.props.iter().find(|p| p.category == "Rocks") {
        let k = r.key.clone();
        l2.add_prop(&k, [6500.0, 6500.0], 30.0, 1.0);
    }
    let p2 = build(&l2, &BuildOptions { rules_source: None, preview: false, props: Some(std::sync::Arc::new(lib)) })?;
    let changed: Vec<&str> = p2.files.iter().filter(|f| orig.file(&f.path).is_none_or(|o| o.bytes().ok() != f.bytes().ok())).map(|f| f.path.as_str()).collect();
    println!("edited export: {} files; changed or new: {:?}", p2.files.len(), changed);
    let removed: Vec<&str> = orig.files.iter().filter(|f| p2.file(&f.path).is_none()).map(|f| f.path.as_str()).collect();
    println!("removed: {:?}", removed);
    let dest = PathBuf::from(out);
    let r = p2.export(&dest, true)?;
    let back = MapProject::open(&dest)?;
    let unreadable: Vec<&str> = back.files.iter().filter(|f| f.parse_error.is_some()).map(|f| f.path.as_str()).collect();
    println!("exported {} files ({} KB), reopened {} files, unreadable {:?}, in {:.1}s", r.files, r.bytes / 1024, back.files.len(), unreadable, t.elapsed().as_secs_f32());
    Ok(())
}

pub fn wgeo_info(map: &str) -> Result<()> {
    let p = MapProject::open(&PathBuf::from(map))?;
    for f in &p.files {
        if let lol_map_studio::project::Doc::Wgeo(w) = &f.doc {
            println!("{}: {} meshes; first: {:?} / {:?}", f.path, w.meshes.len(), w.meshes.iter().take(3).map(|m| (&m.texture, &m.material)).collect::<Vec<_>>(), w.buckets.header);
        }
    }
    Ok(())
}

/// Sets look and behaviour values on an opened map and on a new map, builds both and reads the
/// values back from the built files.
pub fn settings_test(map: &str) -> Result<()> {
    use lol_map_studio::layout_build::{BuildOptions, build};
    use lol_map_studio::map_settings::{read, specs};
    let path = std::fs::canonicalize(map)?;
    let path = PathBuf::from(path.to_string_lossy().trim_start_matches(r"\?\").to_string());
    let orig = MapProject::open(&path)?;
    let mut l = lol_map_studio::layout_import::import(&orig, &path)?;
    let mut fresh = lol_map_studio::layout::Layout::classic_start("S", 30, 12000.0);
    let want = [("terrain.inibin|Fog*enable", "1"), ("terrain.inibin|Fog*opacity", "0.7"), ("Gamma.inibin|Gamma*Contrast", "1.3"), ("Grass.ini|SeeThroughMinAlpha", "0.3"), ("Constants.var|ai_StartingGold", "1400"), ("clouds.inibin|Clouds*Tile", "8000"), ("DeathTimes.inibin|TimeDeadPerLevel*Level05", "3")];
    for (k, v) in want {
        l.settings.insert(k.into(), v.into());
        fresh.settings.insert(k.into(), v.into());
    }
    let all = specs();
    for (name, layout, rules) in [("opened map", &l, None), ("new map", &fresh, Some(path.clone()))] {
        let p = build(layout, &BuildOptions { rules_source: rules, preview: false, props: None })?;
        let mut ok = 0;
        for (k, v) in want {
            let sp = all.iter().find(|s| s.id() == k).unwrap();
            let got = read(&p, sp);
            let good = got.as_deref().and_then(|g| g.split_whitespace().next()).and_then(|g| g.parse::<f32>().ok()).zip(v.parse::<f32>().ok()).is_some_and(|(a, b)| (a - b).abs() < 1e-3);
            if good {
                ok += 1;
            } else {
                println!("  {name}: {k} = {got:?}, wanted {v}");
            }
        }
        println!("{name}: {ok}/{} settings read back", want.len());
    }
    let built = build(&l, &BuildOptions { rules_source: None, preview: false, props: None })?;
    let changed: Vec<&str> = built.files.iter().filter(|f| orig.file(&f.path).is_none_or(|o| o.bytes().ok() != f.bytes().ok())).map(|f| f.path.as_str()).collect();
    println!("files changed on the opened map: {changed:?}");
    Ok(())
}

/// A map folder under game/LEVELS (or the older Example Maps folder).
fn lv(name: &str) -> PathBuf {
    lol_map_studio::paths::map_dir(name)
}
