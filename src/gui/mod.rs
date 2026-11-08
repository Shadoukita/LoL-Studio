//! The editor window.


mod edit3d;
mod fx;
mod gl_scene;
mod inspector;
mod layout_view;
mod model_studio;
mod models;
mod play_view;
mod toolbar;
mod view2d;
mod view3d;

use lol_map_studio::edit::{Editor, NavPaint, Sel};
use eframe::egui::{self, Color32, RichText};
use lol_map_studio::create::{self, NewMapOptions, StartLayout, TerrainStyle};
use lol_map_studio::formats::{dds, mob::ObjectType, nvr::Material, wgeo::WgeoMesh};
use lol_map_studio::keynames::KeyNames;
use lol_map_studio::project::{Doc, FileKind, MapFile, MapProject, SceneFormat};
use lol_map_studio::scene;
use lol_map_studio::validate::{self, Issue, Severity};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use view2d::{Background, Tool, View2D};
use view3d::View3D;

enum Job {
    Preview(u64, std::sync::Arc<lol_map_studio::render::RenderScene>, Option<std::sync::Arc<MapProject>>),
    Props(std::sync::Arc<lol_map_studio::props::PropLibrary>),
    /// An opened map read as an editable layout.
    Imported(Result<lol_map_studio::layout::Layout, String>),
    Built(Result<String, String>),
    Opened(Result<MapProject, String>, Option<(TerrainStyle, SceneFormat)>),
    Scene(u64, std::sync::Arc<lol_map_studio::render::RenderScene>),
    Exported(Result<String, String>),
    /// A map plan from a prompt.
    AiPlan(Result<lol_map_studio::ai_map::MapPlan, String>),
    /// The plan with its online models fetched (and what happened).
    AiAssets(lol_map_studio::ai_map::MapPlan, Vec<String>),
    /// The AI builder's map after a tool call, and what it did.
    AgentStep(Box<lol_map_studio::layout::Layout>, String),
    AgentDone(Result<String, String>),
}

/// The AI building a map with the editor tools (or a recipe running).
struct AgentRun {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    log: Vec<String>,
    done: bool,
    /// The map is open in the editor.
    opened: bool,
}

#[derive(PartialEq, Clone, Copy)]
enum CentralTab {
    Map2D,
    Map3D,
    /// Custom maps: the tools applied in 3D.
    Edit3D,
}

#[derive(PartialEq, Clone, Copy)]
enum LeftTab {
    Objects,
    Files,
}

#[derive(Default)]
struct Settings {
    levels_dir: String,
    last_open: String,
    template: String,
    rules: String,
    /// Anthropic API key for AI map generation (kept in this local settings file).
    api_key: String,
    ai_model: String,
}

impl Settings {
    fn path() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("LoL Map Studio").join("settings.txt"))
    }
    fn load() -> Self {
        let mut s = Self::default();
        if let Some(text) = Self::path().and_then(|p| std::fs::read_to_string(p).ok()) {
            for line in text.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    match k {
                        "levels_dir" => s.levels_dir = v.into(),
                        "last_open" => s.last_open = v.into(),
                        "template" => s.template = v.into(),
                        "rules" => s.rules = v.into(),
                        "api_key" => s.api_key = v.into(),
                        "ai_model" => s.ai_model = v.into(),
                        _ => {}
                    }
                }
            }
        }
        s
    }
    fn save(&self) {
        if let Some(p) = Self::path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(p, format!("levels_dir={}\nlast_open={}\ntemplate={}\nrules={}\napi_key={}\nai_model={}\n", self.levels_dir, self.last_open, self.template, self.rules, self.api_key, self.ai_model));
        }
    }
}

/// "Create a map with AI": the prompt, the planner, and the plan it returned.
struct AiDialog {
    prompt: String,
    use_claude: bool,
    busy: bool,
    plan: Option<lol_map_studio::ai_map::MapPlan>,
    error: Option<String>,
    show_key: bool,
    /// Fetch the plan's extra models online.
    online: bool,
    fetching: bool,
    /// Build step by step with the editor tools (else a one-shot plan).
    step_by_step: bool,
    /// A reference picture (concept art) for Claude to follow.
    image: Option<PathBuf>,
}

const AI_EXAMPLES: [&str; 6] = [
    "A frozen Freljord map with two lanes, lots of brush, a frozen river and torches along the lanes",
    "A small one-lane ARAM bridge over a lava chasm with health relics",
    "Summoner's Rift style 5v5 at night, dense maze-like jungle, glowing mushrooms",
    "A desert Shurima map with three lanes, no river, ancient ruins and statues everywhere",
    "A haunted Shadow Isles map with fog, dead trees and a big open center for teamfights",
    "Dominion style capture point map with five points and speed shrines, autumn colours",
];

struct NewMapDialog {
    template: String,
    opt: NewMapOptions,
    water: bool,
    water_level: f32,
}

struct ExportDialog {
    levels_dir: String,
    folder: String,
    replace: bool,
}

struct TerrainDialog {
    style: TerrainStyle,
    water: bool,
    water_level: f32,
    confirm: bool,
}

struct CustomDialog {
    name: String,
    map_id: u32,
    size: f32,
    classic: bool,
    rift_look: bool,
}

/// Textures to choose from: the generated ones, and every texture of the example maps.
fn texture_library() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = ["grass", "dirt", "path", "sand", "rock", "brush"].iter().map(|k| (format!("Generated {k}"), format!("gen:{k}"))).collect();
    let mut seen = std::collections::HashSet::new();
    if let Some(ex) = example_maps_dir() {
        let top: Vec<PathBuf> = std::fs::read_dir(&ex).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        // Also the maps inside collection folders (Season1/Map8).
        let mut dirs: Vec<PathBuf> = top.iter().flat_map(|d| if d.join("Scene").is_dir() { vec![d.clone()] } else { std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.join("Scene").is_dir()).collect() }).collect();
        dirs.sort();
        for d in dirs {
            for sub in ["Scene/Textures", "Scene/textures"] {
                let Ok(rd) = std::fs::read_dir(d.join(sub)) else { continue };
                let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dds"))).collect();
                files.sort();
                for f in files {
                    let n = f.file_name().unwrap().to_string_lossy().to_string();
                    if n.starts_with("2x_") || n.starts_with("4x_") || !seen.insert(n.to_ascii_lowercase()) {
                        continue;
                    }
                    v.push((n, f.to_string_lossy().to_string()));
                }
            }
        }
    }
    v
}

struct ObjDialog {
    path: String,
    scale: f32,
    yaw: f32,
    at: [f32; 3],
    material: usize,
    new_material: bool,
    new_name: String,
    texture: String,
    ground: bool,
}

enum Confirm {
    Discard(Box<dyn FnOnce(&mut App)>),
}

pub struct App {
    ed: Option<Editor>,
    view2d: View2D,
    view3d: View3D,
    edit3d: edit3d::Edit3D,
    studio: model_studio::ModelStudio,
    studio_open: bool,
    fx: fx::Fx,
    /// Jungle monsters of the map on screen (animated in the 3D views).
    monsters: Vec<lol_map_studio::jungle::Monster>,
    monsters_of: usize,
    /// Opened map whose original scene is shown (rendered once).
    base_scene: Option<String>,
    map_effects: Vec<(String, [f32; 3])>,
    particles_of: usize,
    insp: inspector::InspectorState,
    keys: KeyNames,
    tab: CentralTab,
    left: LeftTab,
    filter: String,
    tx: Sender<Job>,
    rx: Receiver<Job>,
    busy: Option<String>,
    scene_job_rev: u64,
    scene: Option<std::sync::Arc<lol_map_studio::render::RenderScene>>,
    gl: gl_scene::Shared,
    scene_shown: u64,
    data: Option<lol_map_studio::gamedata::GameData>,
    log: Vec<(Color32, String)>,
    issues: Option<Vec<Issue>>,
    settings: Settings,
    new_dialog: Option<NewMapDialog>,
    export_dialog: Option<ExportDialog>,
    terrain_dialog: Option<TerrainDialog>,
    obj_dialog: Option<ObjDialog>,
    confirm: Option<Confirm>,
    allow_close: bool,
    layout: Option<layout_view::LayoutDoc>,
    lview: layout_view::LayoutView,
    preview_rev: u64,
    preview_busy: bool,
    last_layout_change: (u64, std::time::Instant),
    library: Vec<(String, String)>,
    custom_dialog: Option<CustomDialog>,
    ai: Option<AiDialog>,
    agent: Option<AgentRun>,
    look_src: Option<(String, std::collections::HashMap<String, String>)>,
    play: Option<play_view::PlayState>,
    champion: Option<std::sync::Arc<lol_map_studio::champion::Champion>>,
    pub props_lib: Option<std::sync::Arc<lol_map_studio::props::PropLibrary>>,
    preview_project: Option<std::sync::Arc<MapProject>>,
    build_dialog: Option<ExportDialog>,
    /// Files marked saved when the running export started (marked changed again if it fails).
    exporting: Vec<String>,
}

/// The folder with the map folders (game/LEVELS, see lol_map_studio::paths).
fn example_maps_dir() -> Option<PathBuf> {
    lol_map_studio::paths::levels_dir()
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        apply_style(&cc.egui_ctx);
        let (tx, rx) = channel();
        let mut settings = Settings::load();
        if settings.template.is_empty()
            && let Some(d) = example_maps_dir()
        {
            settings.template = d.join("Map1").to_string_lossy().to_string();
        }
        let mut app = Self {
            ed: None,
            view2d: View2D::default(),
            view3d: View3D::default(),
            edit3d: edit3d::Edit3D::default(),
            studio: model_studio::ModelStudio::new(),
            studio_open: false,
            fx: fx::Fx::default(),
            monsters: Vec::new(),
            monsters_of: 0,
            base_scene: None,
            map_effects: Vec::new(),
            particles_of: 0,
            insp: Default::default(),
            keys: KeyNames::new(),
            tab: CentralTab::Map2D,
            left: LeftTab::Objects,
            filter: String::new(),
            tx,
            rx,
            busy: None,
            scene_job_rev: 0,
            scene: None,
            gl: gl_scene::new_shared(),
            scene_shown: 0,
            data: lol_map_studio::gamedata::GameData::locate(None, None),
            log: Vec::new(),
            issues: None,
            settings,
            new_dialog: None,
            export_dialog: None,
            terrain_dialog: None,
            obj_dialog: None,
            confirm: None,
            allow_close: false,
            exporting: Vec::new(),
            layout: None,
            lview: Default::default(),
            preview_rev: 0,
            preview_busy: false,
            last_layout_change: (0, std::time::Instant::now()),
            library: texture_library(),
            custom_dialog: None,
            ai: None,
            agent: None,
            look_src: None,
            build_dialog: None,
            play: None,
            champion: None,
            props_lib: None,
            preview_project: None,
        };
        // The scenery library (props of the example maps) loads in the background.
        if let Some(ex) = example_maps_dir() {
            let tx = app.tx.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let dirs: Vec<PathBuf> = std::fs::read_dir(&ex).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
                let mut lib = lol_map_studio::props::library_from(&dirs);
                // The user's own assets (Model Studio) join the scenery library.
                lib.set_assets(lol_map_studio::asset::library_props());
                let _ = tx.send(Job::Props(std::sync::Arc::new(lib)));
                ctx.request_repaint();
            });
        }
        if app.settings.rules.is_empty()
            && let Some(d) = example_maps_dir()
        {
            app.settings.rules = d.join("Map1").to_string_lossy().to_string();
        }
        // A map folder given on the command line is opened at start.
        if let Some(dir) = std::env::args().nth(1).map(PathBuf::from).filter(|p| p.is_dir()) {
            app.open_folder(dir);
        }
        app
    }

    fn info(&mut self, s: impl Into<String>) {
        self.log.push((Color32::from_gray(210), s.into()));
    }
    fn error(&mut self, s: impl Into<String>) {
        self.log.push((Color32::from_rgb(255, 120, 100), s.into()));
    }

    fn dirty(&self) -> bool {
        self.layout.as_ref().is_some_and(|d| d.dirty) || self.ed.as_ref().is_some_and(|e| e.project.dirty_count() > 0 || e.terrain_stale)
    }

    /// Runs `f` now, or after the user agrees to discard unsaved changes.
    fn guard(&mut self, f: impl FnOnce(&mut App) + 'static) {
        if self.dirty() {
            self.confirm = Some(Confirm::Discard(Box::new(f)));
        } else {
            f(self);
        }
    }

    /// Re-reads the asset library into the scenery library.
    fn reload_assets(&mut self) {
        // Re-saved assets keep their texture file names: drop the cached GL copies.
        self.gl.lock().unwrap().forget_textures("prop:");
        if let Some(lib) = &self.props_lib {
            let mut l = (**lib).clone();
            l.set_assets(lol_map_studio::asset::library_props());
            self.props_lib = Some(std::sync::Arc::new(l));
            self.preview_rev = 0;
        }
    }

    /// Opens a map folder in the editor (all tools), keeping its art: see layout_import.
    fn edit_opened_map(&mut self, dir: PathBuf) {
        self.busy = Some(format!("Reading {} for editing…", dir.display()));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = (|| {
                let path = std::fs::canonicalize(&dir).unwrap_or(dir.clone());
                // Windows canonical paths start with \\?\; the plain form reads better and works the same.
                let path = PathBuf::from(path.to_string_lossy().trim_start_matches(r"\\?\").to_string());
                let p = lol_map_studio::layout_base::base_project(&path.to_string_lossy())?;
                lol_map_studio::layout_import::import(&p, &path)
            })()
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Job::Imported(r));
        });
    }

    fn open_folder(&mut self, dir: PathBuf) {
        self.busy = Some(format!("Opening {}…", dir.display()));
        self.settings.last_open = dir.to_string_lossy().to_string();
        self.settings.save();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = MapProject::open(&dir).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Job::Opened(r, None));
        });
    }

    fn set_project(&mut self, p: MapProject, generated: Option<(TerrainStyle, SceneFormat)>) {
        for f in &p.files {
            if f.kind == FileKind::Text && (f.path.ends_with(".ini") || f.path.ends_with(".cfg")) {
                if let Doc::Text(t) = &f.doc {
                    self.keys.learn_ini(t);
                }
            }
        }
        let unreadable = p.files.iter().filter(|f| f.parse_error.is_some()).count();
        let scene = match p.scene_format() {
            Some(SceneFormat::Legacy) => "legacy scene (room.nvr)",
            Some(SceneFormat::Baked) => "baked scene (room.wgeo)",
            None => "no scene geometry",
        };
        self.info(format!("Opened {}: {} files, {scene}{}", p.name, p.files.len(), if unreadable > 0 { format!(", {unreadable} kept as raw data") } else { String::new() }));
        let generated = generated.or_else(|| detect_generated(&p));
        if generated.is_some() {
            self.info("This map's terrain was generated from its navigation grid; painting the grid reshapes it.");
        }
        if self.data.is_none() {
            self.data = lol_map_studio::gamedata::GameData::locate(p.source.as_deref(), None);
        }
        self.layout = None;
        let mut ed = Editor::new(p);
        ed.generated = generated;
        self.ed = Some(ed);
        self.view2d = View2D::default();
        self.view3d = View3D::default();
        self.scene = None;
        self.scene_job_rev = 0;
        self.scene_shown = 0;
        self.issues = None;
        self.insp = Default::default();
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        while let Ok(job) = self.rx.try_recv() {
            match job {
                Job::Opened(r, generated) => {
                    self.busy = None;
                    match r {
                        Ok(p) => self.set_project(p, generated),
                        Err(e) => self.error(format!("Could not open the map: {e}")),
                    }
                }
                Job::Scene(rev, scene) => {
                    self.view3d.fit_once(scene.bounds);
                    self.gl.lock().unwrap().set(scene.clone());
                    self.scene = Some(scene);
                    self.scene_shown = rev;
                    self.scene_job_rev = 0;
                }
                Job::AgentStep(l, note) => {
                    if let Some(a) = &mut self.agent {
                        a.log.push(note);
                        if !a.opened {
                            a.opened = true;
                            self.start_layout(*l, None);
                            self.tab = CentralTab::Edit3D;
                        } else if let Some(doc) = &mut self.layout {
                            doc.begin();
                            doc.layout = *l;
                            doc.end();
                        }
                    }
                }
                Job::AgentDone(r) => {
                    self.reload_assets();
                    if let Some(a) = &mut self.agent {
                        a.done = true;
                        match r {
                            Ok(s) => a.log.push(format!("Done: {s}")),
                            Err(e) => a.log.push(format!("Stopped: {e}")),
                        }
                    }
                }
                Job::AiAssets(plan, log) => {
                    for line in &log {
                        self.info(line.clone());
                    }
                    self.reload_assets();
                    let l = lol_map_studio::ai_map::build_layout(&plan, self.props_lib.as_deref(), 30);
                    self.ai = None;
                    self.busy = None;
                    self.start_layout(l, None);
                    self.tab = CentralTab::Edit3D;
                    self.info("Your AI map is ready: everything on it can be edited");
                }
                Job::AiPlan(r) => {
                    if let Some(d) = &mut self.ai {
                        d.busy = false;
                        match r {
                            Ok(p) => {
                                d.plan = Some(p);
                                d.error = None;
                            }
                            Err(e) => d.error = Some(e),
                        }
                    }
                }
                Job::Imported(r) => {
                    self.busy = None;
                    match r {
                        Ok(l) => {
                            let info = format!("Editing {}: {} structures and camps, {} lanes, {} effects, {} lights. The map's own art is kept; Build and export writes only what you change.", l.name, l.objectives.len(), l.lanes.len(), l.effects.len(), l.lights.len());
                            self.start_layout(l, None);
                            self.lview.game_look = true;
                            self.tab = CentralTab::Edit3D;
                            self.info(info);
                        }
                        Err(e) => self.error(format!("Could not edit the map: {e}")),
                    }
                }
                Job::Props(lib) => {
                    self.info(format!("Scenery library: {} props", lib.props.len()));
                    // Assets saved while the library was loading are added now.
                    let mut l = (*lib).clone();
                    l.set_assets(lol_map_studio::asset::library_props());
                    self.props_lib = Some(std::sync::Arc::new(l));
                    self.preview_rev = 0;
                }
                Job::Preview(rev, scene, project) => {
                    self.preview_busy = false;
                    if let Some(p) = &project {
                        self.monsters = lol_map_studio::jungle::monsters(p);
                        self.preview_project = project;
                    }
                    if self.layout.is_some() {
                        self.view3d.fit_once(scene.bounds);
                        self.gl.lock().unwrap().set(scene.clone());
                        self.scene = Some(scene);
                    }
                    let _ = rev;
                }
                Job::Built(r) => {
                    self.busy = None;
                    match r {
                        Ok(s) => self.info(s),
                        Err(e) => self.error(format!("Build failed: {e}")),
                    }
                }
                Job::Exported(r) => {
                    self.busy = None;
                    match r {
                        Ok(s) => {
                            self.exporting.clear();
                            self.info(s)
                        }
                        Err(e) => {
                            if let Some(ed) = &mut self.ed {
                                for p in std::mem::take(&mut self.exporting) {
                                    if let Some(f) = ed.project.file_mut(&p) {
                                        f.dirty = true;
                                    }
                                }
                            }
                            self.error(format!("Export failed: {e}"))
                        }
                    }
                }
            }
            ctx.request_repaint();
        }
        // Live preview of a custom map: rebuilt shortly after the last edit.
        if let Some(doc) = &self.layout
            && let Some(base) = &doc.layout.base
        {
            if self.base_scene.as_deref() != Some(base.path.as_str()) && !self.preview_busy {
                self.base_scene = Some(base.path.clone());
                self.preview_busy = true;
                let path = base.path.clone();
                let tx = self.tx.clone();
                let ctx2 = ctx.clone();
                let rev = doc.rev;
                std::thread::spawn(move || {
                    // Without game DATA: structures are drawn live by the views.
                    let rs = match lol_map_studio::layout_base::base_project(&path) {
                        Ok(p) => lol_map_studio::render::build(&p, 1024, None),
                        Err(_) => lol_map_studio::render::RenderScene { batches: Vec::new(), textures: Vec::new(), sun: Default::default(), bounds: [0.0; 6], triangles: 0 },
                    };
                    let _ = tx.send(Job::Preview(rev, std::sync::Arc::new(rs), None));
                    ctx2.request_repaint();
                });
            }
        } else if let Some(doc) = &self.layout {
            if self.last_layout_change.0 != doc.rev {
                self.last_layout_change = (doc.rev, std::time::Instant::now());
            }
            // Editing in 3D shows changes as soon as a rebuild is free; the 2D canvas waits for a pause.
            let wait = if self.tab == CentralTab::Edit3D { 40 } else { 350 };
            let settled = self.last_layout_change.1.elapsed() > std::time::Duration::from_millis(wait);
            if !self.preview_busy && self.preview_rev != doc.rev && settled {
                self.preview_busy = true;
                self.preview_rev = doc.rev;
                let rev = doc.rev;
                let layout = doc.layout.clone();
                let data = self.data.clone();
                let tx = self.tx.clone();
                let ctx2 = ctx.clone();
                let props = self.props_lib.clone();
                std::thread::spawn(move || {
                    let opt = lol_map_studio::layout_build::BuildOptions { rules_source: None, preview: true, props };
                    match lol_map_studio::layout_build::build(&layout, &opt) {
                        Ok(p) => {
                            let rs = lol_map_studio::render::build(&p, 512, data.as_ref());
                            let sub = p.subset(|f| matches!(f.kind, FileKind::NavGrid | FileKind::Mob | FileKind::Scb | FileKind::Sco) || f.path.ends_with("room.dsc") || f.path.starts_with("Scripts/NeutralMinionSpawn"));
                            let _ = tx.send(Job::Preview(rev, std::sync::Arc::new(rs), Some(std::sync::Arc::new(sub))));
                        }
                        Err(_) => {
                            let _ = tx.send(Job::Preview(rev, std::sync::Arc::new(lol_map_studio::render::RenderScene { batches: Vec::new(), textures: Vec::new(), sun: Default::default(), bounds: [0.0; 6], triangles: 0 }), None));
                        }
                    }
                    ctx2.request_repaint();
                });
            } else if self.preview_rev != doc.rev {
                ctx.request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        // The drawable scene for the 2D/3D views, rebuilt in the background after changes.
        if let Some(ed) = &self.ed
            && self.scene_job_rev == 0
            && self.scene_shown != ed.scene_rev.max(ed.objects_rev * 1_000_000)
        {
            let rev = ed.scene_rev.max(ed.objects_rev * 1_000_000);
            self.scene_job_rev = rev;
            let sub = ed.project.subset(|f| !matches!(f.kind, FileKind::Lua) || f.path.to_ascii_lowercase().contains("createlevelprops"));
            let data = self.data.clone();
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let rs = lol_map_studio::render::build(&sub, 1024, data.as_ref());
                let _ = tx.send(Job::Scene(rev, std::sync::Arc::new(rs)));
                ctx.request_repaint();
            });
        }
    }

    fn start_export(&mut self, dest: PathBuf, replace: bool) {
        let Some(ed) = &mut self.ed else { return };
        let steps = match ed.prepare_export() {
            Ok(s) => s,
            Err(e) => {
                self.error(format!("Export failed: {e:#}"));
                return;
            }
        };
        let issues = validate::validate(&ed.project);
        let errors: Vec<String> = issues.iter().filter(|i| i.severity == Severity::Error).map(|i| i.message.clone()).collect();
        let snapshot = ed.project.clone();
        // What is written is what is open now: mark everything saved.
        self.exporting = ed.project.files.iter().filter(|f| f.dirty).map(|f| f.path.clone()).collect();
        for f in &mut ed.project.files {
            if f.dirty
                && let Ok(b) = f.bytes()
            {
                f.original = std::sync::Arc::new(b);
                f.dirty = false;
            }
        }
        ed.terrain_stale = false;
        for s in steps {
            self.info(format!("Before export: {s}"));
        }
        for e in &errors {
            self.error(format!("Warning: {e}"));
        }
        self.busy = Some(format!("Writing {}…", dest.display()));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = snapshot.export(&dest, replace).map(|r| format!("Exported {} files ({:.1} MB, {} re-encoded) to {}", r.files, r.bytes as f64 / 1e6, r.rewritten.len(), dest.display())).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Job::Exported(r));
        });
    }

    fn menu(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New custom map…").clicked() {
                    self.custom_dialog = Some(CustomDialog { name: "My Map".into(), map_id: 30, size: 12000.0, classic: true, rift_look: true });
                    ui.close();
                }
                if ui.button("Open custom map project…").clicked() {
                    ui.close();
                    self.open_project_dialog();
                }
                if self.layout.is_some() {
                    if ui.add(egui::Button::new("Save project").shortcut_text("Ctrl+S")).clicked() {
                        ui.close();
                        self.save_project(false);
                    }
                    if ui.button("Save project as…").clicked() {
                        ui.close();
                        self.save_project(true);
                    }
                    if ui.add(egui::Button::new("Build and export map…").shortcut_text("Ctrl+E")).clicked() {
                        ui.close();
                        self.open_build_dialog();
                    }
                    if ui.button("Close project").clicked() {
                        ui.close();
                        self.layout = None;
                        self.scene = None;
                        self.preview_rev = 0;
                    }
                }
                ui.separator();
                if ui.button("New map from a template…").clicked() {
                    let tpl = self.settings.template.clone();
                    self.new_dialog = Some(NewMapDialog { template: tpl, opt: NewMapOptions::default(), water: false, water_level: -60.0 });
                    ui.close();
                }
                if ui.button("Edit a map folder with the map tools…").on_hover_text("Paint, place and play on an existing map (Map1, map11, your exports); its art is kept").clicked() {
                    ui.close();
                    if let Some(dir) = rfd::FileDialog::new().set_title("Edit a map folder (LEVELS/MapN)").pick_folder() {
                        self.edit_opened_map(dir);
                    }
                }
                if ui.button("Open map folder…").clicked() {
                    ui.close();
                    let mut d = rfd::FileDialog::new().set_title("Open a map folder (LEVELS/MapN)");
                    if !self.settings.last_open.is_empty() {
                        d = d.set_directory(Path::new(&self.settings.last_open).parent().unwrap_or(Path::new(".")));
                    }
                    if let Some(dir) = d.pick_folder() {
                        self.guard(move |a| a.open_folder(dir));
                    }
                }
                if let Some(ex) = example_maps_dir() {
                    ui.menu_button("Open example", |ui| {
                        if let Ok(rd) = std::fs::read_dir(&ex) {
                            let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
                            dirs.sort();
                            for d in dirs {
                                if ui.button(d.file_name().unwrap().to_string_lossy()).clicked() {
                                    ui.close();
                                    self.guard(move |a| a.open_folder(d));
                                }
                            }
                        }
                    });
                }
                ui.separator();
                let has = self.ed.is_some();
                if ui.add_enabled(has, egui::Button::new("Export map…").shortcut_text("Ctrl+E")).clicked() {
                    self.open_export_dialog();
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Import file into the map…")).clicked() {
                    ui.close();
                    self.import_file();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Edit", |ui| {
                let (u, r) = self.ed.as_ref().map_or((None, None), |e| (e.undo_label().map(|s| s.to_string()), e.redo_label().map(|s| s.to_string())));
                if ui.add_enabled(u.is_some(), egui::Button::new(format!("Undo {}", u.unwrap_or_default())).shortcut_text("Ctrl+Z")).clicked() {
                    self.do_undo(false);
                    ui.close();
                }
                if ui.add_enabled(r.is_some(), egui::Button::new(format!("Redo {}", r.unwrap_or_default())).shortcut_text("Ctrl+Y")).clicked() {
                    self.do_undo(true);
                    ui.close();
                }
                ui.separator();
                let sel = self.ed.as_ref().is_some_and(|e| e.sel != Sel::None);
                if ui.add_enabled(sel, egui::Button::new("Duplicate (then click to place)").shortcut_text("Ctrl+D")).clicked() {
                    self.view2d.tool = Tool::Duplicate;
                    ui.close();
                }
                if ui.add_enabled(sel, egui::Button::new("Delete").shortcut_text("Del")).clicked() {
                    self.delete_sel();
                    ui.close();
                }
            });
            ui.menu_button("Map", |ui| {
                if ui.button("Create a map with AI…").clicked() {
                    ui.close();
                    self.open_ai();
                }
                let has = self.ed.is_some();
                if ui.add_enabled(has, egui::Button::new("Check map")).clicked() {
                    self.issues = self.ed.as_ref().map(|e| validate::validate(&e.project));
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Rebuild terrain from navigation grid…")).clicked() {
                    self.open_terrain_dialog();
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Import OBJ model into the scene…")).clicked() {
                    self.open_obj_dialog();
                    ui.close();
                }
                if ui.add_enabled(has, egui::Button::new("Recompute pathfinding hints")).clicked() {
                    if let Some(ed) = &mut self.ed {
                        ed.record("Recompute hints", &["AIPath.aimesh_ngrid".to_string()]);
                        if let Some(g) = ed.project.navgrid_mut() {
                            g.rebuild_hints();
                        }
                        ed.nav_walk_changed = false;
                    }
                    ui.close();
                }
            });
            ui.menu_button("View", |ui| {
                ui.radio_value(&mut self.tab, CentralTab::Map2D, "2D map");
                ui.radio_value(&mut self.tab, CentralTab::Map3D, "3D scene");
                ui.separator();
                let l = &mut self.view2d.layers;
                ui.label("Background");
                ui.radio_value(&mut l.background, Background::Scene, "Scene geometry");
                ui.radio_value(&mut l.background, Background::Minimap, "Minimap image");
                ui.radio_value(&mut l.background, Background::None, "None");
                ui.separator();
                ui.checkbox(&mut l.nav, "Navigation grid");
                ui.add(egui::Slider::new(&mut l.nav_alpha, 0.1..=1.0).text("grid opacity"));
                ui.checkbox(&mut l.structures, "Structures and spawns");
                ui.checkbox(&mut l.nav_points, "Lane nav points");
                ui.checkbox(&mut l.props, "Level props");
                ui.checkbox(&mut l.brush_props, "Brush props");
                ui.checkbox(&mut l.particles, "Particles");
                ui.checkbox(&mut l.labels, "Labels");
                ui.checkbox(&mut l.markers_3d, "Object markers in 3D");
                ui.separator();
                if ui.button("Fit map").clicked() {
                    self.view2d.fitted = false;
                    ui.close();
                }
            });
            ui.menu_button("Help", |ui| {
                ui.label("LoL Map Studio edits LEVELS/MapN folders for the 4.17/4.20 client.");
                ui.label("Files you do not change are written back byte for byte.");
                ui.label("Edited files are re-encoded in the game's own formats.");
            });
        });
    }

    fn do_undo(&mut self, redo: bool) {
        if let Some(ed) = &mut self.ed {
            let r = if redo { ed.redo() } else { ed.undo() };
            if let Some(l) = r {
                self.info(format!("{} {l}", if redo { "Redid" } else { "Undid" }));
            }
        }
    }

    fn delete_sel(&mut self) {
        if let Some(ed) = &mut self.ed {
            match ed.delete_selected() {
                Ok(n) => self.info(format!("Deleted {n}")),
                Err(e) => self.error(format!("{e:#}")),
            }
        }
    }

    fn open_export_dialog(&mut self) {
        let Some(ed) = &self.ed else { return };
        let name = ed.project.name.clone();
        let folder = if name.starts_with("Map") || name.starts_with("map") { format!("Map{}", &name[3..]) } else { "Map30".into() };
        self.export_dialog = Some(ExportDialog { levels_dir: self.settings.levels_dir.clone(), folder, replace: false });
    }

    fn open_terrain_dialog(&mut self) {
        let Some(ed) = &self.ed else { return };
        let style = ed.generated.as_ref().map(|g| g.0.clone()).unwrap_or_default();
        let water = style.water_level.is_some();
        let wl = style.water_level.unwrap_or(-60.0);
        self.terrain_dialog = Some(TerrainDialog { style, water, water_level: wl, confirm: ed.generated.is_some() });
    }

    fn open_obj_dialog(&mut self) {
        let Some(ed) = &self.ed else { return };
        let c = self.view2d.center;
        let at = match &ed.sel {
            Sel::Mob(n) => ed.project.mob().and_then(|m| m.find(n).map(|i| m.objects[i].position)),
            Sel::Dsc(s) => ed.dsc_central(s),
            _ => None,
        }
        .unwrap_or([c[0], ed.ground_height(c[0], c[1]).unwrap_or(0.0), c[1]]);
        self.obj_dialog = Some(ObjDialog { path: String::new(), scale: 1.0, yaw: 0.0, at, material: 0, new_material: true, new_name: "custom_model".into(), texture: String::new(), ground: false });
    }

    fn import_file(&mut self) {
        let Some(src) = rfd::FileDialog::new().set_title("File to add to the map").pick_file() else { return };
        let Some(ed) = &mut self.ed else { return };
        let name = src.file_name().unwrap().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        let dir = if lower.ends_with(".dds") || lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            ed.project.files.iter().find(|f| f.path.to_ascii_lowercase().starts_with("scene/textures/")).map_or("Scene/Textures".into(), |f| f.path[..14].to_string())
        } else if lower.ends_with(".scb") || lower.ends_with(".sco") {
            "Scene".to_string()
        } else {
            String::new()
        };
        let (rel, data) = match image_to_dds(&src) {
            Some(Ok(b)) => (format!("{dir}/{}.dds", name.rsplit_once('.').unwrap().0), b),
            Some(Err(e)) => {
                self.error(format!("Could not read the image: {e:#}"));
                return;
            }
            None => match std::fs::read(&src) {
                Ok(b) => (if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") }, b),
                Err(e) => {
                    self.error(format!("{e}"));
                    return;
                }
            },
        };
        let rel = rel.trim_start_matches('/').to_string();
        ed.record("Import file", std::slice::from_ref(&rel));
        ed.project.add_or_replace(MapFile::generated(&rel, data));
        ed.sel = Sel::File(rel.clone());
        ed.touch_all();
        self.info(format!("Added {rel}"));
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let mut rebuild_now = false;
        ui.horizontal(|ui| {
            if let Some(src) = self.ed.as_ref().and_then(|e| e.project.source.clone())
                && ui.add(egui::Button::new(RichText::new("Edit with map tools").strong().color(Color32::from_rgb(140, 190, 255)))).on_hover_text("Edit this map in 2D and 3D: structures, camps, scenery, effects, lights, walls and brush").clicked()
            {
                self.edit_opened_map(src);
            }
            if self.ed.is_some() && ui.add(egui::Button::new(RichText::new("Play  F5").strong().color(Color32::from_rgb(120, 230, 140)))).on_hover_text("Walk this map as Katarina (F5)").clicked() {
                self.start_play();
            }
            self.toolbar_row(ui, &mut rebuild_now);
        });
        if rebuild_now
            && let Some(ed) = &mut self.ed
            && let Some((style, format)) = ed.generated.clone()
        {
            match ed.rebuild_terrain(&style, format) {
                Ok(()) => self.info("Rebuilt the terrain from the navigation grid"),
                Err(e) => self.error(format!("Terrain rebuild failed: {e:#}")),
            }
        }
    }

    fn toolbar_row(&mut self, ui: &mut egui::Ui, rebuild_now: &mut bool) {
        let Some(ed) = &self.ed else {
            return;
        };
        let format = ed.project.scene_format();
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.tab, CentralTab::Map2D, "2D");
            ui.selectable_value(&mut self.tab, CentralTab::Map3D, "3D");
            if self.tab == CentralTab::Map3D {
                if ui.button("Game camera").on_hover_text("The in-game camera angle at the current spot").clicked() {
                    self.view3d.game_camera();
                }
                if ui.button("Overview").clicked() {
                    if let Some(s) = &self.scene {
                        self.view3d.fit(s.bounds);
                    }
                }
            }
            ui.separator();
            let t = &mut self.view2d.tool;
            ui.selectable_value(t, Tool::Select, "Select / move").on_hover_text("Click to select, drag to move (1)");
            ui.selectable_value(t, Tool::Paint, "Paint grid").on_hover_text("Paint walls, brush and ground height (2)");
            ui.selectable_value(t, Tool::Place, "Place object").on_hover_text("Add a new map object (3)");
            ui.selectable_value(t, Tool::Duplicate, "Duplicate").on_hover_text("Click to place a copy of the selection (Ctrl+D)");
            ui.separator();
            match self.view2d.tool {
                Tool::Paint => {
                    egui::ComboBox::from_id_salt("paint").selected_text(self.view2d.paint.label()).show_ui(ui, |ui| {
                        for m in NavPaint::ALL {
                            ui.selectable_value(&mut self.view2d.paint, m, m.label());
                        }
                    });
                    ui.add(egui::Slider::new(&mut self.view2d.radius, 25.0..=1500.0).logarithmic(true).text("radius"));
                    if matches!(self.view2d.paint, NavPaint::Raise | NavPaint::Lower) {
                        ui.add(egui::Slider::new(&mut self.view2d.strength, 1.0..=60.0).text("strength"));
                    }
                }
                Tool::Place => {
                    egui::ComboBox::from_id_salt("placekind").selected_text(self.view2d.place.kind.label()).show_ui(ui, |ui| {
                        for (k, base) in [
                            (ObjectType::Turret, "Turret_T1_C_08"),
                            (ObjectType::Inhibitor, "Barracks_T1_C2"),
                            (ObjectType::Nexus, "HQ_T1"),
                            (ObjectType::SpawnPoint, "__Spawn_T1"),
                            (ObjectType::Barracks, "__P_Order_Spawn_Barracks__C02"),
                            (ObjectType::Shop, "Shop_T1"),
                            (ObjectType::NavPoint, "__NAV_C020"),
                            (ObjectType::InfoPoint, "Info_Point01"),
                            (ObjectType::LevelProp, "LevelProp_Custom"),
                            (ObjectType::Lake, "Lake02"),
                        ] {
                            if ui.selectable_label(self.view2d.place.kind == k, k.label()).clicked() {
                                self.view2d.place.kind = k;
                                self.view2d.place.name = base.into();
                            }
                        }
                    });
                    ui.label("name");
                    ui.add(egui::TextEdit::singleline(&mut self.view2d.place.name).desired_width(200.0));
                }
                _ => {}
            }
            ui.separator();
            let f = match format {
                Some(SceneFormat::Legacy) => "Legacy scene (room.nvr)",
                Some(SceneFormat::Baked) => "Baked scene (room.wgeo)",
                None => "No scene",
            };
            ui.label(RichText::new(f).weak());
            if ed.generated.is_some() {
                ui.label(RichText::new(if ed.terrain_stale { "terrain rebuilds on export" } else { "generated terrain" }).color(Color32::from_rgb(140, 200, 255)));
                if ed.terrain_stale && ui.button("Rebuild terrain now").clicked() {
                    *rebuild_now = true;
                }
            }
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        let Some(ed) = &mut self.ed else { return };
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.left, LeftTab::Objects, "Objects");
            ui.selectable_value(&mut self.left, LeftTab::Files, "Files");
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter…").desired_width(f32::INFINITY));
        let filt = self.filter.to_ascii_lowercase();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.left {
            LeftTab::Objects => {
                let mut groups: std::collections::BTreeMap<String, Vec<(Sel, String, [f32; 3])>> = Default::default();
                if let Some(m) = ed.project.mob() {
                    for o in &m.objects {
                        if !filt.is_empty() && !o.name.to_ascii_lowercase().contains(&filt) {
                            continue;
                        }
                        let g = if o.kind == ObjectType::LevelProp && o.name.starts_with("LevelProp_brush") { "Level props (brush)".to_string() } else { o.kind.label() };
                        groups.entry(g).or_default().push((Sel::Mob(o.name.clone()), o.name.clone(), o.position));
                    }
                }
                for e in ed.project.dsc_entries() {
                    let stem = e.file.rsplit_once('.').map_or(e.file.as_str(), |x| x.0).to_string();
                    if !filt.is_empty() && !stem.to_ascii_lowercase().contains(&filt) {
                        continue;
                    }
                    let c = ed.dsc_central(&stem).unwrap_or([0.0; 3]);
                    groups.entry("Scene objects (room.dsc)".into()).or_default().push((Sel::Dsc(stem.clone()), stem, c));
                }
                for (g, items) in groups {
                    egui::CollapsingHeader::new(format!("{g} ({})", items.len())).default_open(!filt.is_empty() || items.len() < 30).show(ui, |ui| {
                        for (s, n, p) in items {
                            if ui.selectable_label(ed.sel == s, &n).clicked() {
                                ed.sel = s;
                                self.view2d.focus_request = Some([p[0], p[2]]);
                                self.view3d.focus(p);
                            }
                        }
                    });
                }
            }
            LeftTab::Files => {
                let mut dirs: std::collections::BTreeMap<String, Vec<(String, String, bool)>> = Default::default();
                for f in &ed.project.files {
                    if !filt.is_empty() && !f.path.to_ascii_lowercase().contains(&filt) {
                        continue;
                    }
                    let (d, n) = f.path.rsplit_once('/').map_or((String::new(), f.path.clone()), |(a, b)| (a.to_string(), b.to_string()));
                    dirs.entry(d).or_default().push((f.path.clone(), n, f.dirty));
                }
                for (d, files) in dirs {
                    let label = if d.is_empty() { format!("{} ({})", ed.project.name, files.len()) } else { format!("{d} ({})", files.len()) };
                    egui::CollapsingHeader::new(label).default_open(d.is_empty() || (!filt.is_empty())).show(ui, |ui| {
                        for (p, n, dirty) in files {
                            let text = if dirty { RichText::new(format!("● {n}")).color(Color32::from_rgb(255, 210, 120)) } else { RichText::new(n) };
                            if ui.selectable_label(ed.sel == Sel::File(p.clone()), text).clicked() {
                                ed.sel = Sel::File(p);
                            }
                        }
                    });
                }
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        // New map
        let mut create_now = None;
        if let Some(d) = &mut self.new_dialog {
            let mut open = true;
            egui::Window::new("New map").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label("A new map gets its scene, navigation grid, minimap and object layout generated here.");
                ui.label("Gameplay files (level scripts, settings, structure meshes) come from a template map, and its structures are placed proportionally on the new map.");
                ui.separator();
                egui::Grid::new("newmap").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Template map");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.template).desired_width(300.0));
                        if ui.button("Browse…").clicked()
                            && let Some(p) = rfd::FileDialog::new().pick_folder()
                        {
                            d.template = p.to_string_lossy().to_string();
                        }
                    });
                    ui.end_row();
                    ui.label("Map folder name");
                    ui.add(egui::TextEdit::singleline(&mut d.opt.folder_name).desired_width(120.0));
                    ui.end_row();
                    ui.label("Size (units)");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut d.opt.size[0]).range(2000.0..=30000.0).speed(50.0));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut d.opt.size[1]).range(2000.0..=30000.0).speed(50.0));
                        ui.label(RichText::new("Map1 and map11 are about 14500 × 14500").weak());
                    });
                    ui.end_row();
                    ui.label("Starting layout");
                    ui.vertical(|ui| {
                        ui.radio_value(&mut d.opt.layout, StartLayout::TemplateScaled, "The template's lanes, walls and brush, scaled");
                        ui.radio_value(&mut d.opt.layout, StartLayout::Blank, "Open field with a border wall");
                    });
                    ui.end_row();
                    ui.label("Ground height");
                    ui.add(egui::DragValue::new(&mut d.opt.base_height).speed(1.0));
                    ui.end_row();
                    if d.opt.layout == StartLayout::Blank {
                        ui.label("Rolling hills");
                        ui.add(egui::Slider::new(&mut d.opt.hills, 0.0..=400.0));
                        ui.end_row();
                    }
                    ui.label("Border wall (cells)");
                    ui.add(egui::DragValue::new(&mut d.opt.border_cells).range(0..=40));
                    ui.end_row();
                    ui.label("Cliff height");
                    ui.add(egui::DragValue::new(&mut d.opt.style.wall_height).range(50.0..=1000.0));
                    ui.end_row();
                    ui.label("Water");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut d.water, "Water plane at height");
                        ui.add_enabled(d.water, egui::DragValue::new(&mut d.water_level));
                    });
                    ui.end_row();
                    ui.label("Decoration");
                    ui.checkbox(&mut d.opt.keep_decor, "Keep the template's props, particles and lights (moved proportionally)");
                    ui.end_row();
                });
                ui.separator();
                if ui.button("Create map").clicked() {
                    create_now = Some(());
                }
            });
            if !open {
                self.new_dialog = None;
            }
        }
        if create_now.is_some()
            && let Some(mut d) = self.new_dialog.take()
        {
            d.opt.style.water_level = d.water.then_some(d.water_level);
            self.settings.template = d.template.clone();
            self.settings.save();
            self.guard(move |a| {
                a.busy = Some("Generating the map…".into());
                let tx = a.tx.clone();
                std::thread::spawn(move || {
                    let r = (|| {
                        let t = MapProject::open(Path::new(&d.template))?;
                        let format = t.scene_format().ok_or_else(|| anyhow::anyhow!("the template has no scene"))?;
                        let p = create::new_map(&t, &d.opt)?;
                        Ok::<_, anyhow::Error>((p, format))
                    })();
                    let _ = match r {
                        Ok((p, f)) => tx.send(Job::Opened(Ok(p), Some((d.opt.style.clone(), f)))),
                        Err(e) => tx.send(Job::Opened(Err(format!("{e:#}")), None)),
                    };
                });
            });
        }

        // Export
        let mut do_export = None;
        if let Some(d) = &mut self.export_dialog {
            let mut open = true;
            egui::Window::new("Export map").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label("Writes the complete map folder. Put it in the game's LEVELS folder; the folder name selects the map ID (Map30 = map 30).");
                egui::Grid::new("export").num_columns(2).show(ui, |ui| {
                    ui.label("Destination (LEVELS folder)");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.levels_dir).desired_width(320.0));
                        if ui.button("Browse…").clicked()
                            && let Some(p) = rfd::FileDialog::new().pick_folder()
                        {
                            d.levels_dir = p.to_string_lossy().to_string();
                        }
                    });
                    ui.end_row();
                    ui.label("Map folder");
                    ui.add(egui::TextEdit::singleline(&mut d.folder).desired_width(120.0));
                    ui.end_row();
                });
                let dest = Path::new(&d.levels_dir).join(&d.folder);
                ui.label(RichText::new(format!("-> {}", dest.display())).monospace());
                let exists = !d.levels_dir.is_empty() && dest.exists();
                if exists {
                    ui.colored_label(Color32::from_rgb(255, 190, 110), "This folder exists. Its contents will be replaced.");
                    ui.checkbox(&mut d.replace, "Replace it");
                }
                if let Some(ed) = &self.ed {
                    let src = ed.project.source.as_ref().map(|s| s.canonicalize().ok()).flatten();
                    if src.is_some() && src == dest.canonicalize().ok() {
                        ui.label(RichText::new("This is the folder the map was opened from.").weak());
                    }
                    ui.label(format!("{} files, {} changed", ed.project.files.len(), ed.project.dirty_count()));
                }
                let ok = !d.levels_dir.is_empty() && !d.folder.is_empty() && (!exists || d.replace);
                if ui.add_enabled(ok, egui::Button::new("Export")).clicked() {
                    do_export = Some((dest, exists));
                }
            });
            if !open {
                self.export_dialog = None;
            }
        }
        if let Some((dest, replace)) = do_export {
            if let Some(d) = self.export_dialog.take() {
                self.settings.levels_dir = d.levels_dir;
                self.settings.save();
                if let Some(ed) = &mut self.ed {
                    ed.project.name = d.folder;
                }
            }
            self.start_export(dest, replace);
        }

        // Terrain rebuild
        let mut rebuild = None;
        if let (Some(d), Some(ed)) = (&mut self.terrain_dialog, &self.ed) {
            let mut open = true;
            let textures: Vec<String> = ed.project.files.iter().filter(|f| f.kind == FileKind::Dds && f.path.to_ascii_lowercase().starts_with("scene/textures/")).map(|f| f.file_name().to_string()).collect();
            egui::Window::new("Rebuild terrain from the navigation grid").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label("Builds the scene from the grid: walkable cells are ground, walls rise into cliffs, brush cells get grass.");
                if ed.generated.is_none() {
                    ui.colored_label(Color32::from_rgb(255, 170, 100), "This replaces the map's entire scene geometry (all of its original art) with generated terrain.");
                    ui.checkbox(&mut d.confirm, "I want to replace the scene");
                }
                egui::Grid::new("terrain").num_columns(2).show(ui, |ui| {
                    for (label, v, gname) in [("Ground texture", &mut d.style.ground_texture, create::GEN_GROUND), ("Cliff texture", &mut d.style.cliff_texture, create::GEN_CLIFF), ("Brush texture", &mut d.style.brush_texture, create::GEN_BRUSH), ("Water texture", &mut d.style.water_texture, create::GEN_WATER)] {
                        ui.label(label);
                        egui::ComboBox::from_id_salt(label).selected_text(v.as_str()).width(260.0).show_ui(ui, |ui| {
                            ui.selectable_value(v, gname.to_string(), format!("{gname} (generated)"));
                            for t in &textures {
                                if t != gname {
                                    ui.selectable_value(v, t.clone(), t);
                                }
                            }
                        });
                        ui.end_row();
                    }
                    ui.label("Cliff height");
                    ui.add(egui::DragValue::new(&mut d.style.wall_height).range(50.0..=1000.0));
                    ui.end_row();
                    ui.label("Texture repeat (units)");
                    ui.add(egui::DragValue::new(&mut d.style.texture_scale).range(50.0..=5000.0));
                    ui.end_row();
                    ui.label("Grass height");
                    ui.add(egui::DragValue::new(&mut d.style.brush_height).range(20.0..=400.0));
                    ui.end_row();
                    ui.label("Water");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut d.water, "Water plane at height");
                        ui.add_enabled(d.water, egui::DragValue::new(&mut d.water_level));
                    });
                    ui.end_row();
                });
                if ui.add_enabled(d.confirm, egui::Button::new("Rebuild")).clicked() {
                    let mut s = d.style.clone();
                    s.water_level = d.water.then_some(d.water_level);
                    rebuild = Some(s);
                }
            });
            if !open {
                self.terrain_dialog = None;
            }
        }
        if let Some(style) = rebuild {
            self.terrain_dialog = None;
            if let Some(ed) = &mut self.ed {
                let format = ed.project.scene_format().unwrap_or(SceneFormat::Legacy);
                match ed.rebuild_terrain(&style, format) {
                    Ok(()) => self.info("Rebuilt the terrain from the navigation grid"),
                    Err(e) => self.error(format!("Terrain rebuild failed: {e:#}")),
                }
            }
        }

        // OBJ import
        let mut import = false;
        if let (Some(d), Some(ed)) = (&mut self.obj_dialog, &self.ed) {
            let mut open = true;
            let baked = ed.project.scene_format() == Some(SceneFormat::Baked);
            let textures: Vec<String> = ed.project.files.iter().filter(|f| f.kind == FileKind::Dds && f.path.to_ascii_lowercase().starts_with("scene/textures/")).map(|f| f.file_name().to_string()).collect();
            let mats: Vec<String> = match ed.project.file("Scene/room.nvr").map(|f| &f.doc) {
                Some(Doc::Nvr(n)) => n.materials.iter().map(|m| m.name.clone()).collect(),
                _ => Vec::new(),
            };
            egui::Window::new("Import OBJ model").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label(format!("Adds the model to {} as new meshes.", if baked { "room.wgeo" } else { "room.nvr" }));
                egui::Grid::new("obj").num_columns(2).show(ui, |ui| {
                    ui.label("OBJ file");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.path).desired_width(280.0));
                        if ui.button("Browse…").clicked()
                            && let Some(p) = rfd::FileDialog::new().add_filter("Wavefront OBJ", &["obj"]).pick_file()
                        {
                            d.path = p.to_string_lossy().to_string();
                        }
                    });
                    ui.end_row();
                    ui.label("Place at");
                    ui.horizontal(|ui| {
                        for i in 0..3 {
                            ui.add(egui::DragValue::new(&mut d.at[i]).speed(5.0));
                        }
                    });
                    ui.end_row();
                    ui.label("Scale");
                    ui.add(egui::DragValue::new(&mut d.scale).speed(0.05).range(0.001..=1000.0));
                    ui.end_row();
                    ui.label("Rotation (°)");
                    ui.add(egui::DragValue::new(&mut d.yaw).speed(1.0));
                    ui.end_row();
                    if !baked && !mats.is_empty() {
                        ui.label("Material");
                        ui.vertical(|ui| {
                            ui.radio_value(&mut d.new_material, true, "New material");
                            ui.radio_value(&mut d.new_material, false, "Existing material");
                            if !d.new_material {
                                egui::ComboBox::from_id_salt("objmat").selected_text(mats.get(d.material).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                                    for (i, m) in mats.iter().enumerate() {
                                        ui.selectable_value(&mut d.material, i, m);
                                    }
                                });
                            }
                        });
                        ui.end_row();
                    }
                    if baked || d.new_material {
                        ui.label("Material name");
                        ui.text_edit_singleline(&mut d.new_name);
                        ui.end_row();
                        ui.label("Texture");
                        egui::ComboBox::from_id_salt("objtex").selected_text(d.texture.as_str()).width(260.0).show_ui(ui, |ui| {
                            for t in &textures {
                                ui.selectable_value(&mut d.texture, t.clone(), t);
                            }
                        });
                        ui.end_row();
                    }
                    if baked {
                        ui.label("Ground");
                        ui.checkbox(&mut d.ground, "Units can stand on it (adds it to the ground-height buckets)");
                        ui.end_row();
                    }
                });
                ui.label(RichText::new("To add a texture, use File -> Import file into the map (PNG/JPG become DDS).").weak());
                if ui.add_enabled(!d.path.is_empty(), egui::Button::new("Import")).clicked() {
                    import = true;
                }
            });
            if !open {
                self.obj_dialog = None;
            }
        }
        if import && let Some(d) = self.obj_dialog.take() {
            match self.import_obj(&d) {
                Ok(n) => self.info(format!("Imported {} ({n} meshes)", d.path)),
                Err(e) => self.error(format!("OBJ import failed: {e:#}")),
            }
        }

        // Check results
        if let Some(issues) = &self.issues {
            let mut open = true;
            let mut goto = None;
            egui::Window::new("Map check").open(&mut open).default_width(620.0).show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    for i in issues {
                        let (c, t) = match i.severity {
                            Severity::Error => (Color32::from_rgb(255, 110, 100), "Error"),
                            Severity::Warning => (Color32::from_rgb(255, 200, 110), "Warning"),
                            Severity::Info => (Color32::from_gray(200), "Note"),
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(c, t);
                            if let Some(f) = &i.file
                                && ui.link(f).clicked()
                            {
                                goto = Some(f.clone());
                            }
                            ui.label(&i.message);
                        });
                    }
                });
            });
            if let Some(f) = goto
                && let Some(ed) = &mut self.ed
            {
                ed.sel = Sel::File(f);
            }
            if !open {
                self.issues = None;
            }
        }

        // Unsaved changes
        let mut answer = None;
        if self.confirm.is_some() {
            #[allow(deprecated)]
            let id = egui::Id::new("confirm");
            egui::Modal::new(id).show(ctx, |ui| {
                ui.heading("Unsaved changes");
                ui.label("The open map has changes that have not been exported. Discard them?");
                ui.horizontal(|ui| {
                    if ui.button("Discard changes").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
        }
        if let Some(a) = answer
            && let Some(Confirm::Discard(f)) = self.confirm.take()
            && a
        {
            if let Some(ed) = &mut self.ed {
                for file in &mut ed.project.files {
                    file.dirty = false;
                }
                ed.terrain_stale = false;
            }
            f(self);
        }
    }

    fn import_obj(&mut self, d: &ObjDialog) -> anyhow::Result<usize> {
        let ed = self.ed.as_mut().ok_or_else(|| anyhow::anyhow!("no map is open"))?;
        let text = std::fs::read_to_string(&d.path)?;
        let mut g = scene::read_obj(&text)?;
        scene::place_geometry(&mut g, d.scale, d.yaw, d.at);
        match ed.project.scene_format() {
            Some(SceneFormat::Baked) => {
                ed.record("Import model", &["Scene/room.wgeo".to_string()]);
                let f = ed.project.file_mut("Scene/room.wgeo").ok_or_else(|| anyhow::anyhow!("no room.wgeo"))?;
                let Doc::Wgeo(w) = &mut f.doc else { anyhow::bail!("room.wgeo could not be read") };
                let pieces = lol_map_studio::formats::nvr::split_geometry(&g, 65000);
                let n = pieces.len();
                let mut ground = if d.ground { w.bucket_triangles() } else { Vec::new() };
                for p in pieces {
                    if d.ground {
                        for &i in &p.indices {
                            ground.push(p.positions[i as usize]);
                        }
                    }
                    w.meshes.push(WgeoMesh::new(&d.texture, &d.new_name, p.positions, p.uvs, p.indices));
                }
                if d.ground {
                    let side = w.buckets.header.buckets_per_side.max(1);
                    w.buckets = lol_map_studio::formats::wgeo::build_buckets(&ground, side);
                }
                w.total_triangles = w.triangle_count();
                f.dirty = true;
                ed.scene_rev += 1;
                Ok(n)
            }
            _ => {
                ed.record("Import model", &["Scene/room.nvr".to_string()]);
                let f = ed.project.file_mut("Scene/room.nvr").ok_or_else(|| anyhow::anyhow!("no room.nvr"))?;
                let Doc::Nvr(n) = &mut f.doc else { anyhow::bail!("room.nvr could not be read") };
                let mat = if d.new_material || n.materials.is_empty() { n.add_material(Material::new(&d.new_name, &d.texture, 0, 0x10)) } else { d.material as u32 };
                let count = n.append_geometry(mat, &g, -100)?;
                f.dirty = true;
                ed.scene_rev += 1;
                Ok(count)
            }
        }
    }

    /// Enters Play mode on the map shown now (a custom map's live build, or the open map).
    fn start_play(&mut self) {
        let project: Option<std::sync::Arc<MapProject>> = if let Some(d) = self.layout.as_ref().filter(|d| d.layout.base.is_some()) {
            // Opened maps: the edits merged into the original (walls, structures, camps).
            let opt = lol_map_studio::layout_build::BuildOptions { rules_source: None, preview: true, props: self.props_lib.clone() };
            match lol_map_studio::layout_build::build(&d.layout, &opt) {
                Ok(p) => Some(std::sync::Arc::new(p.subset(|f| matches!(f.kind, FileKind::NavGrid | FileKind::Mob | FileKind::Scb | FileKind::Sco | FileKind::Dds) || f.path.ends_with("room.dsc") || f.path.starts_with("Scripts/NeutralMinionSpawn") || f.path == "Particles.dat"))),
                Err(e) => {
                    self.error(format!("Could not prepare the map: {e:#}"));
                    return;
                }
            }
        } else if self.layout.is_some() {
            self.preview_project.clone()
        } else {
            self.ed.as_ref().map(|e| std::sync::Arc::new(e.project.subset(|f| matches!(f.kind, FileKind::NavGrid | FileKind::Mob | FileKind::Scb | FileKind::Sco | FileKind::Dds) || f.path.ends_with("room.dsc") || f.path.starts_with("Scripts/NeutralMinionSpawn"))))
        };
        let Some(project) = project else {
            self.error("The map is still building; press Play again in a moment.");
            return;
        };
        let Some(world) = lol_map_studio::play::World::from_project(&project) else {
            self.error("This map has no navigation grid to walk on.");
            return;
        };
        if self.champion.is_none() {
            let Some(data) = &self.data else {
                self.error("Play needs the game's DATA folder (copy it into game/DATA) for the champion.");
                return;
            };
            match lol_map_studio::champion::Champion::load(&data.root, "KatarinaV2") {
                Ok(c) => self.champion = Some(std::sync::Arc::new(c)),
                Err(e) => {
                    self.error(format!("Could not load Katarina: {e:#}"));
                    return;
                }
            }
        }
        let base_minimap = self.layout.as_ref().and_then(|d| d.layout.base.as_ref()).and_then(|b| lol_map_studio::layout_base::base_project(&b.path).ok()).and_then(|bp| {
            let f = bp.file("INFO/2DLevelMiniMap.dds")?;
            let i = dds::decode(&f.bytes().ok()?, 512).ok()?;
            Some(egui::ColorImage::from_rgba_unmultiplied([i.width, i.height], &i.pixels))
        });
        let minimap = match (&self.layout, &self.ed) {
            _ if base_minimap.is_some() => base_minimap,
            (Some(d), _) => Some(layout_view::LayoutView::canvas_image_pub(&d.layout)),
            (None, Some(ed)) => ed.project.file("INFO/2DLevelMiniMap.dds").and_then(|f| dds::decode(&f.bytes().ok()?, 512).ok()).map(|i| egui::ColorImage::from_rgba_unmultiplied([i.width, i.height], &i.pixels)),
            _ => None,
        };
        // The map's particle effects: the custom map's placed ones, or the opened map's Particles.dat.
        let effects: Vec<(String, [f32; 3])> = match (&self.layout, &self.ed) {
            (Some(d), _) => {
                let t = lol_map_studio::layout_build::terrain(&d.layout);
                d.layout.effects.iter().map(|e| (e.name.clone(), [e.pos[0], t.height_at(e.pos[0], e.pos[1]) + e.lift, e.pos[1]])).collect()
            }
            (None, Some(ed)) => ed.project.text("Particles.dat").map(fx::particles_dat).unwrap_or_default(),
            _ => Vec::new(),
        };
        let map_id = match (&self.layout, &self.ed) {
            (Some(d), _) => d.layout.map_id,
            (None, Some(ed)) => ed.project.name.trim_start_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(0),
            _ => 0,
        };
        let hud = match self.data.as_ref().map(|d| lol_map_studio::hud::Hud::load(&d.root)) {
            Some(Ok(h)) => Some(std::sync::Arc::new(h)),
            Some(Err(e)) => {
                self.info(format!("No game HUD: {e:#}"));
                None
            }
            None => None,
        };
        self.play = Some(play_view::PlayState::new(world, self.champion.clone().unwrap(), minimap, hud, map_id, self.data.clone(), effects));
        self.info("Play mode: right-click to move Katarina, Esc to leave.");
    }

    fn open_project_dialog(&mut self) {
        let Some(p) = rfd::FileDialog::new().add_filter("LoL Map Studio project", &["lmsmap"]).pick_file() else { return };
        match std::fs::read_to_string(&p).map_err(anyhow::Error::from).and_then(|t| lol_map_studio::layout::Layout::load(&t)) {
            Ok(l) => {
                self.info(format!("Opened project {}", p.display()));
                self.start_layout(l, Some(p));
            }
            Err(e) => self.error(format!("Could not open the project: {e:#}")),
        }
    }

    fn start_layout(&mut self, l: lol_map_studio::layout::Layout, path: Option<PathBuf>) {
        self.layout = Some(layout_view::LayoutDoc::new(l, path));
        self.lview = Default::default();
        self.view3d = View3D::default();
        self.preview_rev = 0;
        self.scene = None;
        self.ed = None;
        self.base_scene = None;
        self.tab = CentralTab::Map2D;
    }

    fn save_project(&mut self, ask: bool) {
        let Some(doc) = &mut self.layout else { return };
        let path = match (&doc.path, ask) {
            (Some(p), false) => Some(p.clone()),
            _ => rfd::FileDialog::new().add_filter("LoL Map Studio project", &["lmsmap"]).set_file_name(format!("{}.lmsmap", doc.layout.name)).save_file(),
        };
        let Some(path) = path else { return };
        match doc.layout.save().map_err(anyhow::Error::from).and_then(|t| std::fs::write(&path, t).map_err(Into::into)) {
            Ok(()) => {
                doc.path = Some(path.clone());
                doc.dirty = false;
                self.info(format!("Saved {}", path.display()));
            }
            Err(e) => self.error(format!("Could not save: {e:#}")),
        }
    }

    fn open_build_dialog(&mut self) {
        let Some(doc) = &self.layout else { return };
        self.build_dialog = Some(ExportDialog { levels_dir: self.settings.levels_dir.clone(), folder: format!("Map{}", doc.layout.map_id), replace: false });
    }

    fn start_build(&mut self, dest: PathBuf, replace: bool, open_after: bool) {
        let Some(doc) = &self.layout else { return };
        let layout = doc.layout.clone();
        let rules = (layout.rules == lol_map_studio::layout::Rules::Classic && !self.settings.rules.is_empty()).then(|| PathBuf::from(&self.settings.rules));
        self.busy = Some(format!("Building {}…", dest.display()));
        let tx = self.tx.clone();
        let props = self.props_lib.clone();
        std::thread::spawn(move || {
            let r = (|| {
                let t = std::time::Instant::now();
                let p = lol_map_studio::layout_build::build(&layout, &lol_map_studio::layout_build::BuildOptions { rules_source: rules, preview: false, props })?;
                let issues = validate::validate(&p);
                let errors = issues.iter().filter(|i| i.severity == Severity::Error).count();
                let rep = p.export(&dest, replace)?;
                let mut msg = format!("Built and exported {} files ({:.1} MB) to {} in {:.1}s", rep.files, rep.bytes as f64 / 1e6, dest.display(), t.elapsed().as_secs_f32());
                if errors > 0 {
                    msg += &format!(" — {errors} problems, use Map -> Check map on the exported folder");
                }
                Ok::<_, anyhow::Error>((msg, p))
            })();
            match r {
                Ok((msg, p)) => {
                    let _ = tx.send(Job::Built(Ok(msg)));
                    if open_after {
                        let _ = tx.send(Job::Opened(Ok(p), None));
                    }
                }
                Err(e) => {
                    let _ = tx.send(Job::Built(Err(format!("{e:#}"))));
                }
            }
        });
    }

    fn ai_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.ai else { return };
        let mut open = true;
        let mut ask = false;
        let mut build: Option<bool> = None;
        let mut recipe: Option<&'static str> = None;
        let env_key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
        egui::Window::new("Create a map with AI").open(&mut open).default_width(560.0).collapsible(false).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show(ctx, |ui| {
            ui.label("Describe the map: lanes, mood and season, jungle, river, special objects, decoration, lighting.");
            ui.add(egui::TextEdit::multiline(&mut d.prompt).desired_rows(4).desired_width(f32::INFINITY).hint_text("e.g. A snowy two-lane map with lots of brush and torches along the lanes"));
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Try:").weak());
                for ex in AI_EXAMPLES {
                    if ui.small_button(ex.split(',').next().unwrap_or(ex)).on_hover_text(ex).clicked() {
                        d.prompt = ex.to_string();
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.use_claude, true, "Claude (Anthropic API)");
                ui.radio_value(&mut d.use_claude, false, "Offline (key words only)");
            });
            if d.use_claude {
                ui.horizontal(|ui| {
                    ui.label("API key");
                    ui.add(egui::TextEdit::singleline(&mut self.settings.api_key).password(!d.show_key).desired_width(280.0).hint_text(if env_key.is_empty() { "sk-ant-…" } else { "using ANTHROPIC_API_KEY" }));
                    ui.checkbox(&mut d.show_key, "show");
                });
                ui.horizontal(|ui| {
                    ui.label("Model");
                    if self.settings.ai_model.is_empty() {
                        self.settings.ai_model = "claude-opus-5-5".into();
                    }
                    egui::ComboBox::from_id_salt("aimodel").selected_text(self.settings.ai_model.clone()).show_ui(ui, |ui| {
                        for (m, n) in [("claude-opus-5-5", "Opus 5.5 (best)"), ("claude-sonnet-5-5", "Sonnet 5.5 (faster)"), ("claude-haiku-4-5-20251001", "Haiku 4.5 (fastest)")] {
                            ui.selectable_value(&mut self.settings.ai_model, m.to_string(), n);
                        }
                    });
                });
                ui.label(RichText::new("The key is stored only in your local settings file. The prompt is sent to Anthropic.").weak());
                ui.horizontal(|ui| {
                    ui.radio_value(&mut d.step_by_step, true, "Build it step by step").on_hover_text("Claude uses the editor's tools itself: new map, terrain, every lane, turret and camp, textures, scenery, effects; it looks at the map, runs a playability check and fixes problems. You watch the map being built.");
                    ui.radio_value(&mut d.step_by_step, false, "Quick plan").on_hover_text("One answer turned into a generated map");
                });
                if d.step_by_step {
                    ui.horizontal(|ui| {
                        ui.label("Reference image");
                        if ui.button(d.image.as_ref().and_then(|p| p.file_name()).map_or("Choose concept art…".to_string(), |n| n.to_string_lossy().to_string())).on_hover_text("A picture of the map to follow (concept art, a sketch); it is sent to Claude with the description").clicked()
                            && let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"]).pick_file()
                        {
                            d.image = Some(p);
                        }
                        if d.image.is_some() && ui.small_button("✕").clicked() {
                            d.image = None;
                        }
                    });
                }
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Recipe:").weak());
                if ui.button("Magma Chamber (2010 5v5 concept)").on_hover_text("Built offline with the same editor tools: a wide volcanic cavern, west vs east, three lanes around a lava caldera, Baron and Dragon pits, four jungle quadrants, teleport-pad markers. An approximation from the wiki description; for the exact layout attach the concept art and let Claude build it.").clicked() {
                    recipe = Some(include_str!("../../recipes/magma_chamber.json"));
                }
            });
            ui.horizontal(|ui| {
                if d.busy {
                    ui.spinner();
                    ui.label("Designing your map…");
                } else if ui.add_enabled(!d.prompt.trim().is_empty(), egui::Button::new(RichText::new(if d.use_claude && d.step_by_step { "Build the map" } else { "Design the map" }).strong())).clicked() {
                    ask = true;
                }
            });
            if let Some(e) = &d.error {
                ui.colored_label(Color32::from_rgb(255, 120, 100), e);
            }
            if let Some(p) = &d.plan {
                ui.separator();
                ui.label(RichText::new(if p.name.is_empty() { "Your map" } else { &p.name }).strong().size(16.0));
                if !p.summary.is_empty() {
                    ui.label(&p.summary);
                }
                egui::Grid::new("aiplan").num_columns(2).show(ui, |ui| {
                    let row = |ui: &mut egui::Ui, a: &str, b: String| {
                        ui.label(RichText::new(a).weak());
                        ui.label(b);
                        ui.end_row();
                    };
                    row(ui, "Size", format!("{:.0} units, {} symmetry", p.size, if p.symmetry.is_empty() { "diagonal" } else { &p.symmetry }));
                    row(ui, "Lanes", if p.lanes.is_empty() { "none".into() } else { p.lanes.join(", ") });
                    row(ui, "Theme", p.theme.replace('_', " "));
                    row(ui, "Terrain", format!("walls {:.0}%, brush {:.0}%, hills {:.0}{}", p.walls * 100.0, p.brush * 100.0, p.hills, if p.river { ", river" } else { "" }));
                    row(ui, "Jungle", format!("{}{}", if p.jungle_camps { "camps" } else { "no camps" }, if p.dragon_baron { ", Dragon and Baron" } else { "" }));
                    if !p.mode_objects.is_empty() {
                        row(ui, "Objects", p.mode_objects.join(", ").replace('_', " "));
                    }
                    row(ui, "Decoration", p.decor.iter().map(|x| format!("{} ({})", x.category, x.area)).collect::<Vec<_>>().join(", "));
                    if !p.effects.is_empty() {
                        row(ui, "Effects", p.effects.iter().map(|x| format!("{} on {}", x.effect.trim_end_matches(".troy"), x.area)).collect::<Vec<_>>().join(", "));
                    }
                    if !p.settings.is_empty() {
                        row(ui, "Look & rules", format!("{} settings", p.settings.len()));
                    }
                    if !p.online.is_empty() {
                        row(ui, "Online models", p.online.iter().map(|o| o.query.clone()).collect::<Vec<_>>().join(", "));
                    }
                });
                if !p.online.is_empty() {
                    ui.checkbox(&mut d.online, "Download these models (Poly Haven, free CC0) and restyle them like classic Map1").on_hover_text("Searches polyhaven.com, downloads the models, cuts them to a classic polygon budget, scales them to League size and repaints their textures toward Map1's own. They are saved to your assets (Model Studio) and placed on the map.");
                }
                if d.fetching {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Downloading and restyling models…");
                    });
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!d.fetching, egui::Button::new(RichText::new("Build this map").strong())).on_hover_text("A new map you can keep editing").clicked() {
                        build = Some(false);
                    }
                    if ui.button("Try again").clicked() {
                        ask = true;
                    }
                });
            }
        });
        let key_now = if self.settings.api_key.trim().is_empty() { env_key.clone() } else { self.settings.api_key.clone() };
        if (ask && d.use_claude && d.step_by_step) || recipe.is_some() {
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (prompt, image, model) = (d.prompt.clone(), d.image.clone(), self.settings.ai_model.clone());
            self.settings.save();
            let tx = self.tx.clone();
            let ctx2 = ctx.clone();
            let lib = self.props_lib.clone();
            let map1 = example_maps_dir().map(|e| e.join("Map1").join("Scene").join("Textures"));
            let effects = self.lview.effect_names.clone();
            let stop2 = stop.clone();
            self.ai = None;
            self.agent = Some(AgentRun { stop, log: vec![if recipe.is_some() { "Running the Magma Chamber recipe…".into() } else { "Claude is designing the map…".into() }], done: false, opened: false });
            std::thread::spawn(move || {
                let mut ws = lol_map_studio::ai_agent::Workshop::new(lib, map1, effects);
                let mut last = std::time::Instant::now() - std::time::Duration::from_secs(5);
                let mut send = |ws: &lol_map_studio::ai_agent::Workshop, note: &str, force: bool| {
                    // The map is sent at most a few times a second.
                    if force || last.elapsed() > std::time::Duration::from_millis(400) {
                        last = std::time::Instant::now();
                        let _ = tx.send(Job::AgentStep(Box::new(ws.l.clone()), note.to_string()));
                        ctx2.request_repaint();
                    }
                };
                let r = if let Some(script) = recipe {
                    serde_json::from_str::<serde_json::Value>(script).map_err(|e| e.to_string()).and_then(|v| lol_map_studio::ai_agent::run_script(&v, &mut ws, |w, n| send(w, n, false)).map_err(|e| format!("{e:#}"))).map(|_| ws.done.clone().unwrap_or_default())
                } else {
                    let img = image.as_ref().and_then(|p| std::fs::read(p).ok().map(|b| {
                        let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                        let media = match ext.as_str() { "png" => "image/png", "webp" => "image/webp", "gif" => "image/gif", _ => "image/jpeg" };
                        (media, b)
                    }));
                    let opt = lol_map_studio::ai_agent::AgentOptions { api_key: key_now, model, effort: "high".into(), max_turns: 120 };
                    lol_map_studio::ai_agent::run(&prompt, img.as_ref().map(|(m, b)| (*m, b.as_slice())), &opt, &mut ws, &stop2, |w, n| send(w, n, false)).map_err(|e| format!("{e:#}"))
                };
                send(&ws, "final map", true);
                let _ = tx.send(Job::AgentDone(r));
                ctx2.request_repaint();
            });
            return;
        }
        if ask {
            d.busy = true;
            d.error = None;
            let prompt = d.prompt.clone();
            let (use_claude, key, model) = (d.use_claude, if self.settings.api_key.trim().is_empty() { env_key } else { self.settings.api_key.clone() }, self.settings.ai_model.clone());
            self.settings.save();
            let tx = self.tx.clone();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let r = if use_claude { lol_map_studio::ai_map::plan_with_claude(&prompt, &key, &model).map_err(|e| format!("{e:#}")) } else { Ok(lol_map_studio::ai_map::plan_from_keywords(&prompt)) };
                let _ = tx.send(Job::AiPlan(r));
                ctx2.request_repaint();
            });
        }
        if build.is_some()
            && let Some(mut p) = d.plan.clone()
            && d.online
            && !p.online.is_empty()
        {
            // Models first (in the background), then the map: see Job::AiAssets.
            d.fetching = true;
            let tx = self.tx.clone();
            let ctx2 = ctx.clone();
            let lib = self.props_lib.clone();
            let map1 = example_maps_dir().map(|e| e.join("Map1").join("Scene").join("Textures"));
            std::thread::spawn(move || {
                let log = lol_map_studio::ai_map::fetch_online(&mut p, lib.as_deref(), map1.as_deref());
                let _ = tx.send(Job::AiAssets(p, log));
                ctx2.request_repaint();
            });
            return;
        }
        if build.is_some()
            && let Some(p) = d.plan.clone()
        {
            let l = lol_map_studio::ai_map::build_layout(&p, self.props_lib.as_deref(), 30);
            self.ai = None;
            self.start_layout(l, None);
            self.tab = CentralTab::Edit3D;
            self.info("Your AI map is ready: everything on it can be edited");
            return;
        }
        if !open {
            self.ai = None;
        }
    }

    fn open_ai(&mut self) {
        let has_key = !self.settings.api_key.trim().is_empty() || std::env::var("ANTHROPIC_API_KEY").is_ok_and(|k| !k.is_empty());
        self.ai = Some(AiDialog { prompt: String::new(), use_claude: has_key, busy: false, plan: None, error: None, show_key: false, online: true, fetching: false, step_by_step: true, image: None });
    }

    fn agent_window(&mut self, ctx: &egui::Context) {
        let Some(a) = &mut self.agent else { return };
        let mut close = false;
        egui::Window::new("AI map builder").anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -40.0]).default_width(420.0).collapsible(true).resizable(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if a.done {
                    ui.label(RichText::new("Finished").strong());
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                } else {
                    ui.spinner();
                    ui.label(format!("Working… {} steps", a.log.len().saturating_sub(1)));
                    if ui.button("Stop").clicked() {
                        a.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            });
            if !a.done {
                ui.label(RichText::new("The map updates as the AI builds it; edit it once it has finished.").weak());
            }
            egui::ScrollArea::vertical().max_height(260.0).stick_to_bottom(true).show(ui, |ui| {
                for line in &a.log {
                    ui.label(RichText::new(line).size(12.0));
                }
            });
        });
        if close {
            self.agent = None;
        }
    }

    fn layout_dialogs(&mut self, ctx: &egui::Context) {
        self.ai_dialog(ctx);
        self.agent_window(ctx);
        let mut create = None;
        if let Some(d) = &mut self.custom_dialog {
            let mut open = true;
            egui::Window::new("New custom map").open(&mut open).collapsible(false).resizable(false).pivot(egui::Align2::CENTER_CENTER).default_pos(ctx.content_rect().center()).show(ctx, |ui| {
                ui.label("Paint your map from scratch: ground, walls, brush, rivers, objectives and lanes.");
                egui::Grid::new("custom").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut d.name);
                    ui.end_row();
                    ui.label("Map ID");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut d.map_id).range(1..=99));
                        ui.label(RichText::new(format!("exports as LEVELS/Map{}", d.map_id)).weak());
                    });
                    ui.end_row();
                    ui.label("Size");
                    ui.horizontal(|ui| {
                        for (n, s) in [("Small", 8000.0), ("Medium", 11000.0), ("Rift size", 14500.0)] {
                            ui.selectable_value(&mut d.size, s, n);
                        }
                        ui.add(egui::DragValue::new(&mut d.size).range(3000.0..=20000.0).speed(50.0).suffix(" units"));
                    });
                    ui.end_row();
                    ui.label("Start with");
                    ui.vertical(|ui| {
                        ui.radio_value(&mut d.classic, false, "An empty field with a border wall");
                        ui.radio_value(&mut d.classic, true, "Three lanes, a river, jungle and both bases");
                    });
                    ui.end_row();
                    ui.label("Look");
                    ui.vertical(|ui| {
                        ui.radio_value(&mut d.rift_look, true, "Summoner's Rift textures (from the examples)");
                        ui.radio_value(&mut d.rift_look, false, "Generated textures");
                    });
                    ui.end_row();
                });
                if ui.button("Create").clicked() {
                    create = Some(());
                }
            });
            if !open {
                self.custom_dialog = None;
            }
        }
        if create.is_some()
            && let Some(d) = self.custom_dialog.take()
        {
            let mut l = if d.classic { lol_map_studio::layout::Layout::classic_start(&d.name, d.map_id, d.size) } else { lol_map_studio::layout::Layout::new(&d.name, d.map_id, [d.size, d.size]) };
            if d.rift_look {
                let find = |n: &str| self.library.iter().find(|(name, _)| name.eq_ignore_ascii_case(n)).map(|x| x.1.clone());
                for (n, i) in [("grass_TX_CM.dds", 0), ("order_dirt.dds", 1), ("gravel_stone_TX_CM.dds", 2), ("td_sand1.dds", 3)] {
                    if let Some(r) = find(n) {
                        if let Some(g) = l.theme.ground.get_mut(i) {
                            g.texture = r;
                        }
                    }
                }
                if let Some(r) = find("rocks.dds") {
                    l.theme.cliff = r;
                }
                if let Some(r) = find("WallOfGrass.dds") {
                    l.theme.brush = r;
                }
            }
            self.guard(move |a| a.start_layout(l, None));
        }
        let mut go = None;
        if let Some(d) = &mut self.build_dialog {
            let mut open = true;
            egui::Window::new("Build and export map").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.label("Builds the complete map folder from your layout and writes it to the game's LEVELS folder.");
                egui::Grid::new("build").num_columns(2).show(ui, |ui| {
                    ui.label("LEVELS folder");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.levels_dir).desired_width(320.0));
                        if ui.button("Browse…").clicked()
                            && let Some(p) = rfd::FileDialog::new().pick_folder()
                        {
                            d.levels_dir = p.to_string_lossy().to_string();
                        }
                    });
                    ui.end_row();
                    ui.label("Map folder");
                    ui.add(egui::TextEdit::singleline(&mut d.folder).desired_width(120.0));
                    ui.end_row();
                });
                let dest = Path::new(&d.levels_dir).join(&d.folder);
                ui.label(RichText::new(format!("-> {}", dest.display())).monospace());
                let exists = !d.levels_dir.is_empty() && dest.exists();
                if exists {
                    ui.colored_label(Color32::from_rgb(255, 190, 110), "This folder exists and will be replaced.");
                    ui.checkbox(&mut d.replace, "Replace it");
                }
                let ok = !d.levels_dir.is_empty() && !d.folder.is_empty() && (!exists || d.replace);
                ui.horizontal(|ui| {
                    if ui.add_enabled(ok, egui::Button::new("Build and export")).clicked() {
                        go = Some((dest.clone(), exists, false));
                    }
                    if ui.add_enabled(ok, egui::Button::new("Build, export and inspect")).on_hover_text("Also opens the built map in the inspector").clicked() {
                        go = Some((dest.clone(), exists, true));
                    }
                });
            });
            if !open {
                self.build_dialog = None;
            }
        }
        if let Some((dest, replace, inspect)) = go {
            if let Some(d) = self.build_dialog.take() {
                self.settings.levels_dir = d.levels_dir;
                self.settings.save();
            }
            if inspect {
                self.layout = None;
            }
            self.start_build(dest, replace, inspect);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.play.is_some() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F5)) && (self.layout.is_some() || self.ed.is_some()) {
            self.start_play();
            return;
        }
        let (undo, redo, del, dup, export, fit, t1, t2, t3, esc) = ctx.input_mut(|i| {
            let c = egui::Modifiers::COMMAND;
            (
                i.consume_key(c, egui::Key::Z),
                i.consume_key(c, egui::Key::Y) || i.consume_key(c | egui::Modifiers::SHIFT, egui::Key::Z),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete),
                i.consume_key(c, egui::Key::D),
                i.consume_key(c, egui::Key::E),
                i.consume_key(egui::Modifiers::NONE, egui::Key::F),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Num1),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Num2),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Num3),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if let Some(doc) = &mut self.layout {
            if undo {
                doc.undo();
            }
            if redo {
                doc.redo();
            }
            if export {
                self.open_build_dialog();
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
                self.save_project(false);
            }
            if fit {
                self.lview.reset_view();
            }
            return;
        }
        if undo {
            self.do_undo(false);
        }
        if redo {
            self.do_undo(true);
        }
        if del && self.ed.as_ref().is_some_and(|e| matches!(e.sel, Sel::Mob(_) | Sel::Dsc(_))) {
            self.delete_sel();
        }
        if dup {
            self.view2d.tool = Tool::Duplicate;
        }
        if export && self.ed.is_some() {
            self.open_export_dialog();
        }
        if fit {
            self.view2d.fitted = false;
        }
        if t1 || esc {
            self.view2d.tool = Tool::Select;
        }
        if t2 {
            self.view2d.tool = Tool::Paint;
        }
        if t3 {
            self.view2d.tool = Tool::Place;
        }
    }
}

/// Maps made by the generator carry its material names; their textures give the style back.
fn detect_generated(p: &MapProject) -> Option<(TerrainStyle, SceneFormat)> {
    let mut style = TerrainStyle::default();
    let mut found = false;
    let mut take = |name: &str, tex: &str, style: &mut TerrainStyle| match name {
        "lms_ground" => { style.ground_texture = tex.into(); found = true; }
        "lms_cliff" => style.cliff_texture = tex.into(),
        "lms_brush" => style.brush_texture = tex.into(),
        "lms_water" => style.water_texture = tex.into(),
        _ => {}
    };
    match p.scene_format()? {
        SceneFormat::Legacy => {
            if let Some(Doc::Nvr(n)) = p.file("Scene/room.nvr").map(|f| &f.doc) {
                for m in &n.materials {
                    take(&m.name, &m.channels[0].texture, &mut style);
                }
            }
        }
        SceneFormat::Baked => {
            if let Some(Doc::Wgeo(w)) = p.file("Scene/room.wgeo").map(|f| &f.doc) {
                for m in &w.meshes {
                    take(&m.material, &m.texture, &mut style);
                }
                if w.meshes.iter().any(|m| m.material == "lms_water") {
                    style.water_level = w.meshes.iter().find(|m| m.material == "lms_water").and_then(|m| m.positions.first()).map(|v| v[1]);
                }
            }
        }
    }
    if let Some(Doc::Nvr(n)) = p.file("Scene/room.nvr").map(|f| &f.doc)
        && let Some(wi) = n.materials.iter().position(|m| m.name == "lms_water")
        && let Some(mesh) = n.meshes.iter().position(|m| m.material as usize == wi)
    {
        style.water_level = n.mesh_geometry(mesh).positions.first().map(|v| v[1]);
    }
    found.then(|| (style, p.scene_format().unwrap()))
}

fn image_to_dds(path: &Path) -> Option<anyhow::Result<Vec<u8>>> {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if !(lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg")) {
        return None;
    }
    Some((|| {
        let img = image::open(path)?.to_rgba8();
        let (w, h) = img.dimensions();
        Ok(dds::encode_argb(&dds::Image { width: w as usize, height: h as usize, pixels: img.into_raw() }, true))
    })())
}

impl eframe::App for App {
    /// In the Model Studio, Tab switches Object / Edit mode (as in Blender) instead of moving
    /// keyboard focus between buttons.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if !self.studio_open || ctx.egui_wants_keyboard_input() {
            return;
        }
        if raw.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Tab, pressed: true, repeat: false, .. })) {
            self.studio.tab_pressed = true;
        }
        raw.events.retain(|e| !matches!(e, egui::Event::Key { key: egui::Key::Tab, .. }));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Ghosts and animated units are set again by whichever view draws this frame.
        self.gl.lock().unwrap().extras.clear();
        if self.lview.effect_names.is_empty()
            && let Some(d) = &self.data
        {
            self.lview.effect_names = lol_map_studio::particles::list_effects(&d.root);
            if self.lview.effect_names.is_empty() {
                self.lview.effect_names.push("Fire_Cauldron.troy".into());
            }
        }
        self.poll_jobs(&ctx);
        self.shortcuts(&ctx);

        // Closing with unsaved changes asks first.
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() && !self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(Confirm::Discard(Box::new(|a: &mut App| {
                a.allow_close = true;
            })));
        }
        if self.allow_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let title = match (&self.layout, &self.ed) {
            (Some(d), _) => format!("LoL Map Studio — {}", d.title()),
            (None, e) => match e {
            Some(e) => format!("LoL Map Studio — {}{}", e.project.name, if e.project.dirty_count() > 0 { format!(" ({} changed files)", e.project.dirty_count()) } else { String::new() }),
            None => "LoL Map Studio".into(),
        }};
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // Replace-texture and OBJ requests from the inspector.
        if self.insp.request_obj_import {
            self.insp.request_obj_import = false;
            self.open_obj_dialog();
        }
        if let Some(path) = self.insp.request_replace_texture.take()
            && let Some(src) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "dds"]).pick_file()
        {
            let data = match image_to_dds(&src) {
                Some(r) => r,
                None => std::fs::read(&src).map_err(Into::into),
            };
            match (data, self.ed.as_mut()) {
                (Ok(b), Some(ed)) => {
                    ed.record("Replace texture", std::slice::from_ref(&path));
                    ed.project.add_or_replace(MapFile::generated(&path, b));
                    ed.touch_all();
                    self.info(format!("Replaced {path}"));
                }
                (Err(e), _) => self.error(format!("Could not read the image: {e:#}")),
                _ => {}
            }
        }

        if let Some(key) = self.lview.open_in_studio.take() {
            let lib = self.props_lib.clone();
            self.studio.open_key(&key, lib.as_deref());
            self.studio_open = true;
        }
        if self.studio_open {
            let data_root = self.data.as_ref().map(|d| d.root.clone());
            let lib = self.props_lib.clone();
            let action = self.studio.ui(ui, lib.as_deref(), data_root.as_deref());
            match action {
                Some(model_studio::StudioAction::Close) => self.studio_open = false,
                Some(model_studio::StudioAction::LibraryChanged) => self.reload_assets(),
                Some(model_studio::StudioAction::UseInMap(key)) => {
                    self.reload_assets();
                    self.studio_open = false;
                    if self.layout.is_some() {
                        self.lview.tool = layout_view::LTool::Scenery;
                        self.lview.scenery = layout_view::SceneryMode::Place;
                        self.lview.prop_cat = self.props_lib.as_ref().and_then(|l| l.get(&key)).map_or("My assets".into(), |d| d.category.clone());
                        self.lview.prop_key = Some(key);
                        self.tab = CentralTab::Edit3D;
                        self.info("Your asset is on the cursor: click to place it, drag to turn it");
                    } else {
                        self.info("Saved. Open or create a map, then find it under My assets in the Scenery tool");
                    }
                }
                None => {}
            }
            return;
        }
        if self.play.is_some()
            && let Some(doc) = &self.layout
            && doc.layout.base.is_some()
        {
            let t = self.edit3d.terrain_for(doc);
            let mut g = self.gl.lock().unwrap();
            let more = self.edit3d.base_extras(&mut g, self.data.as_ref(), &doc.layout, self.props_lib.as_deref(), &|x, z| t.height_at(x, z));
            g.extras.extend(more);
        }
        if let Some(play) = &mut self.play {
            egui::CentralPanel::no_frame().show(ui, |ui| play.ui(ui, &self.gl));
            if play.exit {
                self.play = None;
                self.gl.lock().unwrap().character = None;
                self.info("Left Play mode");
            }
            return;
        }
        egui::Panel::top("menu").show(ui, |ui| {
            self.menu(ui);
            if self.layout.is_some() {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, CentralTab::Map2D, "Paint (2D)");
                    ui.selectable_value(&mut self.tab, CentralTab::Edit3D, "Edit (3D)").on_hover_text("Paint, sculpt and place directly in 3D");
                    ui.selectable_value(&mut self.tab, CentralTab::Map3D, "Preview (3D)");
                    if matches!(self.tab, CentralTab::Map3D | CentralTab::Edit3D) {
                        if ui.button("Game camera").clicked() {
                            self.view3d.game_camera();
                        }
                        if ui.button("Overview").clicked()
                            && let Some(s) = &self.scene
                        {
                            self.view3d.fit(s.bounds);
                        }
                    }
                    ui.separator();
                    if let Some(d) = &self.layout {
                        ui.label(RichText::new(d.title()).strong());
                    }
                    if self.preview_busy {
                        ui.spinner();
                        ui.label(RichText::new("updating preview").weak());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(RichText::new("Build and export…").strong()).clicked() {
                            self.open_build_dialog();
                        }
                        if ui.button(RichText::new("AI map").color(Color32::from_rgb(200, 160, 255))).on_hover_text("Create a new map from a description").clicked() {
                            self.open_ai();
                        }
                        if ui.button(RichText::new("Model Studio").color(Color32::from_rgb(140, 190, 255))).on_hover_text("Import, build and edit 3D assets for your maps").clicked() {
                            self.studio_open = true;
                        }
                        if ui.add(egui::Button::new(RichText::new("Play  F5").strong().color(Color32::from_rgb(120, 230, 140)))).on_hover_text("Walk your map as Katarina (F5)").clicked() {
                            self.start_play();
                        }
                        if ui.button("Save").clicked() {
                            self.save_project(false);
                        }
                    });
                });
            } else {
                self.toolbar(ui);
            }
        });
        if let Some(s) = self.lview.status.take() {
            self.log.push((Color32::from_gray(210), s));
        }
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(b) = &self.busy {
                    ui.spinner();
                    ui.label(b);
                }
                if let Some(s) = self.view2d.status.take() {
                    self.log.push((Color32::from_gray(210), s));
                }
                if let Some((c, s)) = self.log.last() {
                    ui.colored_label(*c, s);
                }
                if let Some(ed) = &self.ed {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(s) = &self.scene {
                            ui.label(RichText::new(format!("{} draw batches, {} triangles, {} textures", s.batches.len(), s.triangles, s.textures.len())).weak());
                        }
                        if self.data.is_none() {
                            ui.label(RichText::new("game DATA not found: structures shown as markers").color(Color32::from_rgb(255, 190, 110)));
                        }
                        if ed.project.dirty_count() > 0 {
                            ui.label(RichText::new(format!("{} changed files", ed.project.dirty_count())).color(Color32::from_rgb(255, 210, 120)));
                        }
                    });
                }
            });
            egui::CollapsingHeader::new("Log").default_open(false).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(120.0).stick_to_bottom(true).show(ui, |ui| {
                    for (c, s) in &self.log {
                        ui.colored_label(*c, s);
                    }
                });
            });
        });
        // Imported props join the scenery library as soon as they are added.
        if let (Some(doc), Some(lib)) = (&self.layout, &self.props_lib) {
            let have: Vec<&str> = lib.props.iter().filter(|p| p.key.starts_with("custom:")).map(|p| &p.key[7..]).collect();
            let want: Vec<&str> = doc.layout.custom_props.iter().map(|c| c.name.as_str()).collect();
            if have != want {
                let mut l = (**lib).clone();
                l.set_custom(&doc.layout.custom_props);
                self.props_lib = Some(std::sync::Arc::new(l));
            }
        }
        if let Some(doc) = &self.layout {
            self.monsters = lol_map_studio::jungle::from_layout(&doc.layout);
            // The preview shows the map's picture and fog settings (yours, else the map's own).
            let src = doc.layout.base.as_ref().map(|b| b.path.clone()).unwrap_or_else(|| self.settings.rules.clone());
            if self.look_src.as_ref().is_none_or(|x| x.0 != src) {
                let mut vals = std::collections::HashMap::new();
                if let Ok(p) = lol_map_studio::layout_base::base_project(&src) {
                    for sp in lol_map_studio::map_settings::specs() {
                        if let Some(v) = lol_map_studio::map_settings::read(&p, &sp) {
                            vals.insert(sp.id(), v);
                        }
                    }
                }
                self.look_src = Some((src, vals));
            }
            let base = self.look_src.as_ref().map(|x| &x.1);
            let get = |id: &str, d: f32| -> f32 {
                doc.layout.settings.get(id).or_else(|| base.and_then(|b| b.get(id))).and_then(|v| v.split_whitespace().next()?.parse().ok()).unwrap_or(d)
            };
            let fog_on = get("terrain.inibin|Fog*enable", 0.0) != 0.0;
            let look = gl_scene::Look {
                gamma: get("Gamma.inibin|Gamma*Gamma", 1.0),
                brightness: get("Gamma.inibin|Gamma*Brightness", 1.0),
                contrast: get("Gamma.inibin|Gamma*Contrast", 1.0),
                fog: if fog_on { [get("terrain.inibin|Fog*red", 128.0) / 255.0, get("terrain.inibin|Fog*green", 140.0) / 255.0, get("terrain.inibin|Fog*blue", 160.0) / 255.0, get("terrain.inibin|Fog*opacity", 0.5)] } else { [0.0; 4] },
                fog_heights: [get("terrain.inibin|Fog*min_height", -200.0), get("terrain.inibin|Fog*max_height", 100.0)],
            };
            self.gl.lock().unwrap().look = look;
        } else {
            self.gl.lock().unwrap().look = gl_scene::Look::default();
        }
        if let Some(doc) = &mut self.layout {
            egui::Panel::left("ltools").default_size(250.0).show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.lview.tools_ui(ui, doc, self.props_lib.as_deref()));
            });
            egui::Panel::right("lsettings").default_size(360.0).show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.lview.settings_ui(ui, doc, &self.library, &mut self.settings.rules, self.props_lib.as_deref()));
            });
            // Tool bar (icons) and the active tool's options, around the viewport.
            if matches!(self.tab, CentralTab::Map2D | CentralTab::Edit3D) {
                egui::Panel::left("toolstrip").exact_size(46.0).resizable(false).show(ui, |ui| toolbar::strip(ui, &mut self.lview, doc));
                egui::Panel::top("tooloptions").show(ui, |ui| toolbar::options(ui, &mut self.lview, doc, self.props_lib.as_deref()));
            }
            egui::CentralPanel::no_frame().show(ui, |ui| match self.tab {
                CentralTab::Map2D => self.lview.canvas_ui(ui, doc, &self.gl, self.props_lib.as_deref()),
                CentralTab::Edit3D => self.edit3d.ui(ui, doc, &mut self.lview, &mut self.view3d, &self.gl, self.props_lib.as_deref(), self.data.as_ref(), &self.monsters, &mut self.fx),
                CentralTab::Map3D => {
                    // Animated jungle monsters on the built terrain.
                    if let Some(d) = &self.data {
                        let t = lol_map_studio::layout_build::terrain(&doc.layout);
                        let time = self.gl.lock().unwrap().start.elapsed().as_secs_f32();
                        let mut g = self.gl.lock().unwrap();
                        let m = self.edit3d.models.monsters(&mut g, d, &self.monsters, &|x, z| t.height_at(x, z), time);
                        g.extras.extend(m);
                        let more = self.edit3d.base_extras(&mut g, Some(d), &doc.layout, self.props_lib.as_deref(), &|x, z| t.height_at(x, z));
                        g.extras.extend(more);
                        let all: Vec<(String, [f32; 3])> = doc.layout.effects.iter().map(|e| (e.name.clone(), [e.pos[0], t.height_at(e.pos[0], e.pos[1]) + e.lift, e.pos[1]])).collect();
                        let items = fx::nearby(&all, [self.view3d.target[0], self.view3d.target[2]], self.view3d.distance.max(3000.0) * 1.5, 64);
                        let aspect = ui.available_width() / ui.available_height().max(1.0);
                        if let Some(inv) = self.view3d.inverse(aspect) {
                            let q = self.fx.frame(&mut g, &d.root, &items, &inv);
                            g.extras.extend(q);
                        }
                        ctx.request_repaint();
                    }
                    let mut dummy = Editor::new(MapProject::empty("preview"));
                    self.view3d.ui(ui, &mut dummy, &self.view2d.layers, &self.gl)
                }
            });
            self.layout_dialogs(&ctx);
            self.dialogs(&ctx);
            if self.busy.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            return;
        }
        if self.ed.is_some() {
            egui::Panel::left("left").default_size(250.0).show(ui, |ui| self.left_panel(ui));
            egui::Panel::right("inspector").default_size(380.0).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if let Some(ed) = &mut self.ed {
                        inspector::ui(ui, ed, &mut self.insp, &mut self.keys);
                    }
                });
            });
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if let Some(ed) = &mut self.ed {
                match self.tab {
                    CentralTab::Map2D => self.view2d.ui(ui, ed, &self.gl),
                    CentralTab::Map3D | CentralTab::Edit3D => {
                        // Animated jungle monsters of the open map.
                        let key = std::sync::Arc::as_ptr(&ed.project.files.first().map(|f| f.original.clone()).unwrap_or_default()) as usize;
                        if key != self.monsters_of {
                            self.monsters_of = key;
                            self.monsters = lol_map_studio::jungle::monsters(&ed.project);
                        }
                        if let (Some(d), Some(grid)) = (&self.data, ed.project.navgrid()) {
                            let time = self.gl.lock().unwrap().start.elapsed().as_secs_f32();
                            let mut g = self.gl.lock().unwrap();
                            let m = self.edit3d.models.monsters(&mut g, d, &self.monsters, &|x, z| lol_map_studio::create::grid_height(grid, x, z), time);
                            g.extras.extend(m);
                            if self.particles_of != key {
                                self.particles_of = key;
                                self.map_effects = ed.project.text("Particles.dat").map(fx::particles_dat).unwrap_or_default();
                            }
                            // Only effects near the view run (Summoner's Rift lists 600).
                            if self.view3d.distance < 9000.0 {
                                let items = fx::nearby(&self.map_effects, [self.view3d.target[0], self.view3d.target[2]], self.view3d.distance.max(2500.0) * 1.3, 48);
                                let aspect = ui.available_width() / ui.available_height().max(1.0);
                                if let Some(inv) = self.view3d.inverse(aspect) {
                                    let q = self.fx.frame(&mut g, &d.root, &items, &inv);
                                    g.extras.extend(q);
                                }
                                ctx.request_repaint();
                            }
                            if !self.monsters.is_empty() {
                                ctx.request_repaint();
                            }
                        }
                        self.view3d.ui(ui, ed, &self.view2d.layers, &self.gl)
                    }
                }
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.heading("LoL Map Studio");
                    ui.label("Editor and inspector for League of Legends map folders (LEVELS/MapN).");
                    ui.add_space(16.0);
                    if ui.add_sized([260.0, 36.0], egui::Button::new(RichText::new("New custom map…").size(16.0))).clicked() {
                        self.custom_dialog = Some(CustomDialog { name: "My Map".into(), map_id: 30, size: 12000.0, classic: true, rift_look: true });
                    }
                    if ui.add_sized([260.0, 28.0], egui::Button::new("Open custom map project…")).clicked() {
                        self.open_project_dialog();
                    }
                    ui.add_space(8.0);
                    if ui.add_sized([260.0, 28.0], egui::Button::new("Open map folder (inspect and edit)…")).clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.open_folder(dir);
                    }
                    if ui.add_sized([260.0, 34.0], egui::Button::new(RichText::new("Create a map with AI…").strong().color(Color32::from_rgb(200, 160, 255)))).on_hover_text("Describe a map in words; Claude designs it, you keep editing").clicked() {
                        self.open_ai();
                    }
                    if ui.add_sized([260.0, 28.0], egui::Button::new("Model Studio (3D assets)…")).on_hover_text("Import any 3D model, edit it like in Blender, use it on maps").clicked() {
                        self.studio_open = true;
                    }
                    if ui.button("New map…").clicked() {
                        self.new_dialog = Some(NewMapDialog { template: self.settings.template.clone(), opt: NewMapOptions::default(), water: false, water_level: -60.0 });
                    }
                    if let Some(ex) = example_maps_dir() {
                        ui.add_space(8.0);
                        ui.label(RichText::new("Examples").weak());
                        for name in ["Map1", "map11"] {
                            let p = ex.join(name);
                            if p.is_dir() {
                                if ui.add_sized([260.0, 24.0], egui::Button::new(format!("Edit {name} with the map tools"))).on_hover_text("Paint, place, effects, camps and Play on this map; its art is kept").clicked() {
                                    self.edit_opened_map(p.clone());
                                }
                                if ui.button(format!("Inspect {name}")).on_hover_text("Files, formats and objects").clicked() {
                                    self.open_folder(p.clone());
                                }
                            }
                        }
                    }
                });
            }
        });
        self.layout_dialogs(&ctx);
        self.dialogs(&ctx);
        if self.busy.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

/// The app's look: a dark slate theme with League-style gold and blue accents, rounder widgets
/// and a little more room.
fn apply_style(ctx: &egui::Context) {
    ctx.style_mut_of(egui::Theme::Dark, |st| {
        let gold = Color32::from_rgb(200, 170, 100);
        let blue = Color32::from_rgb(40, 110, 170);
        let v = &mut st.visuals;
        v.panel_fill = Color32::from_rgb(18, 21, 27);
        v.window_fill = Color32::from_rgb(22, 26, 33);
        v.extreme_bg_color = Color32::from_rgb(11, 13, 17);
        v.faint_bg_color = Color32::from_rgb(26, 30, 38);
        v.window_stroke = egui::Stroke::new(1.0, Color32::from_rgb(70, 62, 40));
        v.selection.bg_fill = blue;
        v.selection.stroke = egui::Stroke::new(1.0, Color32::from_rgb(170, 210, 255));
        v.hyperlink_color = gold;
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, Color32::from_rgb(40, 44, 54));
        v.widgets.inactive.bg_fill = Color32::from_rgb(34, 39, 49);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(34, 39, 49);
        v.widgets.hovered.bg_fill = Color32::from_rgb(46, 54, 68);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(46, 54, 68);
        v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, gold);
        v.widgets.active.bg_fill = Color32::from_rgb(52, 62, 80);
        v.widgets.active.weak_bg_fill = Color32::from_rgb(52, 62, 80);
        let r = egui::CornerRadius::same(5);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = r;
        }
        v.window_corner_radius = egui::CornerRadius::same(8);
        st.spacing.item_spacing = egui::vec2(7.0, 5.0);
        st.spacing.button_padding = egui::vec2(8.0, 3.0);
        st.spacing.interact_size.y = 22.0;
    });
}
