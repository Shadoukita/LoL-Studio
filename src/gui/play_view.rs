//! Play mode inside the editor: walk the map as a champion the way the 4.17 client plays.
//!
//! The camera is the client's HudCameraLogic (top-down mode): 56 degree pitch, 50 degree field
//! of view (40 on map 11, with the same zoom scale correction), zoom distance 1000..2250 eased
//! over 0.2 s, 120 per mouse-wheel notch, edge/arrow scrolling at 1000 * 10/9 units per second.
//! The HUD is the game's own, read from DATA/menu/hud and drawn with its atlas.
//!
//! Controls: right-click moves (pathfinding on the navigation grid), S stops, Q/W/E/R spell
//! animations, D Flash, B recalls, Ctrl+1..4 emotes, Y toggles the camera lock, Space centres
//! the camera while held, the minimap moves the camera (left) or the champion (right), Esc leaves.

use super::gl_scene::{self, CharacterDraw, Mat4, Shared};
use eframe::egui::{self, Align2, Color32, FontFamily, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use lol_map_studio::champion::Champion;
use lol_map_studio::formats::dds::{self, Image};
use lol_map_studio::hud::{Align, Hud, Kind};
use lol_map_studio::play::{Action, Hero, RECALL_TIME, World};
use lol_map_studio::render::RVertex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// The client's camera (4.17).
const PITCH: f32 = 56.0;
const ZOOM_MIN: f32 = 1000.0;
const ZOOM_MAX: f32 = 2250.0;
const TOPDOWN_ZOOM: f32 = 600.0;
const ZOOM_EASE_TIME: f32 = 0.2;
const ZOOM_MIN_SPEED: f32 = 3.0;
/// One wheel notch (WHEEL_DELTA) times RollerButtonSpeed 1.
const WHEEL_NOTCH: f32 = 120.0;
/// cr_MouseScrollSpeed 1000 at MapScrollSpeed 0.5, times 10/9 (HudInputLogic).
const SCROLL_SPEED: f32 = 1000.0 * 10.0 / 9.0;
const ACCEL_TIME_MOUSE: f32 = 0.1;
const ACCEL_TIME_KEYBOARD: f32 = 0.05;
const NEAR: f32 = 50.0;
const FAR: f32 = 5000.0;
/// cam_MinX/cam_MinY 300, cam_MaxX/cam_MaxY 14545 on a 14870 map.
const CAM_MARGIN: [f32; 2] = [300.0, 325.0];
/// Flash range.
const FLASH_RANGE: f32 = 400.0;

static ARIAL_TRIED: AtomicBool = AtomicBool::new(false);
static ARIAL: AtomicBool = AtomicBool::new(false);

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

pub fn invert_pub(m: &Mat4) -> Option<Mat4> {
    invert(m)
}

/// General 4x4 inverse (column-major).
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

const MIRROR: Mat4 = [-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// Eased approach to a target, as the client's camera zoom moves.
fn ease_in_out(elapsed: f32, ease: f32, target: f32, cur: &mut f32, vel: &mut f32) {
    if elapsed < 1e-15 {
        return;
    }
    if ease < 1e-15 {
        *cur = target;
        *vel = 0.0;
        return;
    }
    let dt = ease.min(elapsed);
    let omega = 2.0 / ease;
    let x = dt * omega;
    let exp = 1.0 / (x + 1.0 + 0.48 * x * x * 0.235 * x * x * x);
    let change = *cur - target;
    let temp = (*vel + omega * change) * dt;
    *vel = (*vel - omega * temp) * exp;
    *cur = (temp + change) * exp + target;
}

/// The client's top-down camera.
struct Camera {
    pos: [f32; 2],
    zoom: f32,
    desired: f32,
    velocity: f32,
    fov: f32,
    accel_timer: f32,
}

impl Camera {
    /// Zoom::mScale: keeps the framing when the field of view is not 50 degrees.
    fn scale(&self) -> f32 {
        (25f32.to_radians()).tan() / (self.fov.to_radians() * 0.5).tan()
    }

    /// Zoom::Update.
    fn update_zoom(&mut self, dt: f32) {
        let before = self.zoom;
        ease_in_out(dt, ZOOM_EASE_TIME, self.desired, &mut self.zoom, &mut self.velocity);
        if (self.desired - self.zoom).abs() < ZOOM_MIN_SPEED {
            self.zoom = self.desired;
            self.velocity = 0.0;
        } else if (self.zoom - before).abs() < ZOOM_MIN_SPEED {
            self.zoom = if self.desired > before { before + ZOOM_MIN_SPEED } else { before - ZOOM_MIN_SPEED };
        }
        self.zoom = self.zoom.clamp(ZOOM_MIN, ZOOM_MAX);
    }

    /// SetTopdownCamera: eye and look-at point (the camera looks at ground level y = 0).
    fn eye_target(&self) -> ([f32; 3], [f32; 3]) {
        let nz = (self.zoom + self.zoom) / 1500.0 * self.scale();
        let p = PITCH.to_radians();
        let t = [self.pos[0], 0.0, self.pos[1]];
        (([t[0], p.sin() * TOPDOWN_ZOOM * nz, t[2] - p.cos() * TOPDOWN_ZOOM * nz]), t)
    }

    fn mvp(&self, aspect: f32) -> Mat4 {
        let (e, t) = self.eye_target();
        let m = |v: [f32; 3]| [-v[0], v[1], v[2]];
        gl_scene::mul(&perspective(self.fov.to_radians(), aspect, NEAR, FAR), &gl_scene::mul(&look_at(m(e), m(t)), &MIRROR))
    }
}

struct HudTex {
    blend: TextureHandle,
    opaque: TextureHandle,
}

pub struct PlayState {
    pub world: World,
    pub hero: Hero,
    champ: Arc<Champion>,
    hud: Option<Arc<Hud>>,
    cam: Camera,
    locked: bool,
    click_fx: Option<([f32; 3], f32)>,
    last: std::time::Instant,
    textures: HashMap<String, Option<HudTex>>,
    minimap: Option<TextureHandle>,
    minimap_img: Option<egui::ColorImage>,
    circle: Option<TextureHandle>,
    idle: Option<String>,
    run: Option<String>,
    fps: f32,
    time: f32,
    middle_origin: Option<Pos2>,
    pub exit: bool,
    uploaded: bool,
    data: Option<lol_map_studio::gamedata::GameData>,
    models: super::models::Models,
    /// The map's particle effects (Particles.dat), run near the camera.
    effects: Vec<(String, [f32; 3])>,
    fx: super::fx::Fx,
}

impl PlayState {
    pub fn new(world: World, champ: Arc<Champion>, minimap: Option<egui::ColorImage>, hud: Option<Arc<Hud>>, map_id: u32, data: Option<lol_map_studio::gamedata::GameData>, effects: Vec<(String, [f32; 3])>) -> Self {
        let spawn = world.spawn;
        let speed = champ.move_speed;
        let idle = champ.find_anim(&["idle1", "idle"]);
        let run = champ.find_anim(&["run"]);
        // TryCustomFOV: Summoner's Rift (map 11) uses 40 degrees.
        let fov = if map_id == 11 { 40.0 } else { 50.0 };
        Self {
            world,
            hero: Hero::new(spawn, speed),
            champ,
            hud,
            cam: Camera { pos: spawn, zoom: ZOOM_MAX, desired: ZOOM_MAX, velocity: 0.0, fov, accel_timer: 0.0 },
            // Game.cfg LockCamera=0: the camera starts free, centred on the champion.
            locked: false,
            click_fx: None,
            last: std::time::Instant::now(),
            textures: HashMap::new(),
            minimap: None,
            minimap_img: minimap,
            circle: None,
            idle,
            run,
            fps: 60.0,
            time: 0.0,
            middle_origin: None,
            exit: false,
            uploaded: false,
            data,
            models: super::models::Models::default(),
            effects,
            fx: super::fx::Fx::default(),
        }
    }

    fn ground_at(&self, inv: &Mat4, rect: Rect, p: Pos2) -> Option<[f32; 3]> {
        let ndc = [(p.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.top()) / rect.height() * 2.0];
        let un = |z: f32| {
            let v = gl_scene::transform(inv, [ndc[0], ndc[1], z]);
            [v[0] / v[3], v[1] / v[3], v[2] / v[3]]
        };
        let (a, b) = (un(-1.0), un(1.0));
        let dir = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        let d = dir.map(|v| v / len);
        let mut prev = a;
        let mut t = 0.0;
        while t < len {
            t += 20.0;
            let q = [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t];
            if q[1] <= self.world.height(q[0], q[2]) {
                let (mut lo, mut hi) = (prev, q);
                for _ in 0..12 {
                    let m = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
                    if m[1] <= self.world.height(m[0], m[2]) {
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

    /// Where a screen point's ray meets the plane y = 0 (for the minimap view outline), in world X/Z.
    fn plane_at(inv: &Mat4, ndc: [f32; 2]) -> Option<[f32; 2]> {
        let un = |z: f32| {
            let v = gl_scene::transform(inv, [ndc[0], ndc[1], z]);
            [v[0] / v[3], v[1] / v[3], v[2] / v[3]]
        };
        let (a, b) = (un(-1.0), un(1.0));
        if (a[1] - b[1]).abs() < 1e-6 {
            return None;
        }
        let t = (a[1] / (a[1] - b[1])).clamp(0.0, 1.0);
        Some([a[0] + (b[0] - a[0]) * t, a[2] + (b[2] - a[2]) * t])
    }

    fn anim_now(&self) -> (Option<String>, f32, bool) {
        match &self.hero.action {
            Action::Idle => (self.idle.clone(), self.hero.anim_time, true),
            Action::Run => (self.run.clone(), self.hero.anim_time, true),
            Action::Once(n, t) => (Some(n.clone()), *t, false),
            Action::Recall(t) => (self.champ.find_anim(&["recall"]), *t, false),
        }
    }

    fn data_root(&self) -> Option<std::path::PathBuf> {
        Some(self.hud.as_ref()?.root.parent()?.parent()?.to_path_buf())
    }

    /// The image behind a HUD texture name: the atlas files, or the champion's icons.
    fn hud_image(&self, name: &str) -> Option<Image> {
        let icon = |stem: &str| {
            let dir = self.data_root()?.join("Spells").join("Icons2D");
            let p = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(stem)))?;
            dds::decode(&std::fs::read(p).ok()?, 128).ok()
        };
        match name {
            "SpellIcon0" | "SpellIcon1" | "SpellIcon2" | "SpellIcon3" => self.champ.spell_icons[(name.as_bytes()[9] - b'0') as usize].clone(),
            "PassiveIcon" => self.champ.passive_icon.clone(),
            "AvatarImage" => self.champ.portrait.clone(),
            "RecallIcon" => icon("RecallHome"),
            "SummonerSpellIcon0" => icon("Summoner_flash"),
            "SummonerSpellIcon1" => icon("SummonerIgnite"),
            _ => {
                let p = self.hud.as_ref()?.texture_path(name)?;
                dds::decode(&std::fs::read(p).ok()?, 1024).ok()
            }
        }
    }

    fn texture(&mut self, ctx: &egui::Context, name: &str) -> Option<&HudTex> {
        if !self.textures.contains_key(name) {
            let t = self.hud_image(name).map(|img| {
                let blend = ctx.load_texture(format!("hud:{name}"), egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.pixels), TextureOptions::LINEAR);
                let mut px = img.pixels.clone();
                for a in px.iter_mut().skip(3).step_by(4) {
                    *a = 255;
                }
                let opaque = ctx.load_texture(format!("hud-opaque:{name}"), egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &px), TextureOptions::LINEAR);
                HudTex { blend, opaque }
            });
            self.textures.insert(name.to_string(), t);
        }
        self.textures.get(name).and_then(|t| t.as_ref())
    }

    /// Text the game fills into a HUD text element.
    fn hud_text(&self, name: &str) -> Option<String> {
        let s = &self.champ.stats;
        let gold = 475.0 + ((self.time - 90.0).max(0.0) * 1.9).floor();
        Some(match name {
            "SpellHotKey0" => "Q".into(),
            "SpellHotKey1" => "W".into(),
            "SpellHotKey2" => "E".into(),
            "SpellHotKey3" => "R".into(),
            "SummonerSpellHotKey0" => "D".into(),
            "SummonerSpellHotKey1" => "F".into(),
            "RecallHotKey" => "B".into(),
            "InventoryItem0_HotKey" => "1".into(),
            "InventoryItem1_HotKey" => "2".into(),
            "InventoryItem2_HotKey" => "3".into(),
            "InventoryItem3_HotKey" => "5".into(),
            "InventoryItem4_HotKey" => "6".into(),
            "InventoryItem5_HotKey" => "7".into(),
            "InventoryItem6_HotKey" => "4".into(),
            "AvatarName" => self.champ.name.trim_end_matches(|c: char| c == 'V' || c.is_ascii_digit()).to_string(),
            "LevelStat" => "1".into(),
            "GoldStat" => format!("{gold:.0}"),
            "ADStat" => format!("{:.0}", s.ad),
            "APStat" => "0".into(),
            "ASStat" => format!("{:.2}", s.attack_speed),
            "MSStat" => format!("{:.0}", self.hero.speed),
            "ArmorStat" => format!("{:.0}", s.armor),
            "MRStat" => format!("{:.0}", s.mr),
            "HPStat" => format!("{:.0} / {:.0}", s.hp, s.hp),
            "MPStat" if s.has_mana => format!("{:.0} / {:.0}", s.mana, s.mana),
            "GameTime" => format!("{:02}:{:02}", (self.time as u32) / 60, (self.time as u32) % 60),
            "KillsStat" | "DeathsStat" | "AssistsStat" | "MinionKillsStat" | "TeamKillsStat" | "EnemyKillsStat" => "0".into(),
            "FPS" => format!("FPS: {:.0}", self.fps),
            _ => return None,
        })
    }

    /// Elements the game's code hides or resizes for this champion's state.
    fn hud_state(&self, name: &str, file: &str) -> Option<f32> {
        let mana = self.champ.stats.has_mana;
        if file.eq_ignore_ascii_case("CenterHUD_LevelUpBar") {
            return None;
        }
        match name {
            "HPBarLost" | "MPBarLost" | "XPBar" => None,
            "MPBar" | "MPBarEmpty" | "MPBarTipLeft" | "MPBarTipRight" | "MPBarEndCapLeft" | "MPBarEndCapRight" if !mana => None,
            _ => Some(1.0),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, gl: &Shared) {
        let now = std::time::Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.time += dt;
        self.fps = self.fps * 0.95 + 0.05 / dt.max(1e-4);
        let ctx = ui.ctx().clone();
        // The HUD's text font is Arial (from Windows; egui's default font otherwise).
        if !ARIAL_TRIED.swap(true, Ordering::Relaxed) {
            if let Ok(b) = std::fs::read("C:/Windows/Fonts/arial.ttf") {
                ARIAL.store(true, Ordering::Relaxed);
                ctx.add_font(egui::epaint::text::FontInsert::new(
                    "Arial",
                    egui::FontData::from_owned(b),
                    vec![egui::epaint::text::InsertFontFamily { family: FontFamily::Name("Arial".into()), priority: egui::epaint::text::FontPriority::Highest }],
                ));
            }
        }
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let (w, h) = (rect.width(), rect.height());
        let aspect = w / h.max(1.0);

        // HUD layout for this frame (needed for hit tests before the world reacts to clicks).
        let hud = self.hud.clone();
        let to_rect = |r: [f32; 4]| Rect::from_min_max(rect.min + Vec2::new(r[0], r[1]), rect.min + Vec2::new(r[2], r[3]));
        let content = hud.as_ref().and_then(|hd| hd.get("MinimapContent")).map(|e| to_rect(e.screen_rect(w, h))).unwrap_or_else(|| Rect::from_min_size(rect.right_bottom() - Vec2::splat(h * 0.25), Vec2::splat(h * 0.24)));
        let blockers: Vec<Rect> = hud
            .as_ref()
            .map(|hd| hd.elements.iter().filter(|e| e.enabled && e.kind == Kind::Icon && matches!(e.name.as_str(), "HUDCenterFrame" | "HUDLeftFrame" | "HUDLeftPillar" | "MinimapFrame" | "MinimapPillar" | "HUDCenterUnderlay")).map(|e| to_rect(e.screen_rect(w, h))).collect())
            .unwrap_or_default();
        let pointer = ui.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()));
        let over_hud = pointer.is_some_and(|p| content.contains(p) || blockers.iter().any(|b| b.contains(p) && p.y > b.top() + b.height() * 0.12));

        // Input.
        let (keys, mods, notches, arrows, space) = ui.input(|i| {
            use egui::Key::*;
            let k = |key| i.key_pressed(key);
            let notches: f32 = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                        egui::MouseWheelUnit::Line => delta.y,
                        egui::MouseWheelUnit::Page => delta.y * 3.0,
                        egui::MouseWheelUnit::Point => delta.y / 50.0,
                    }),
                    _ => None,
                })
                .sum();
            let arrows = [i.key_down(ArrowLeft), i.key_down(ArrowRight), i.key_down(ArrowUp), i.key_down(ArrowDown)];
            ([k(Q), k(W), k(E), k(R), k(B), k(S), k(Y), k(D), k(Escape), k(Num1), k(Num2), k(Num3), k(Num4), k(F)], i.modifiers, notches, arrows, i.key_down(Space))
        });
        let mvp = self.cam.mvp(aspect);
        let inv = invert(&mvp);
        let right_down = ui.input(|i| i.pointer.secondary_down());
        let right = (resp.secondary_clicked() || (resp.dragged_by(egui::PointerButton::Secondary) && right_down)) && !over_hud;
        if right
            && let (Some(inv), Some(p)) = (inv, resp.interact_pointer_pos())
            && let Some(g) = self.ground_at(&inv, rect, p)
        {
            self.hero.move_to(&self.world, [g[0], g[2]]);
            if resp.secondary_clicked() || self.click_fx.as_ref().is_none_or(|c| c.1 > 0.25) {
                self.click_fx = Some((g, 0.0));
            }
        }
        let champ = self.champ.clone();
        let spell = |n: usize| champ.find_anim(&[&format!("spell{n}")]);
        if !mods.ctrl {
            for (k, n) in [(0usize, 1usize), (1, 2), (2, 3), (3, 4)] {
                if keys[k]
                    && let Some(a) = spell(n)
                {
                    self.hero.play(a);
                }
            }
        } else {
            for (k, key) in [(9usize, "joke"), (10, "taunt"), (11, "dance"), (12, "laugh")] {
                if keys[k]
                    && let Some(a) = champ.find_anim(&[key])
                {
                    self.hero.play(a);
                }
            }
        }
        // D: Flash toward the cursor.
        if keys[7]
            && let (Some(inv), Some(p)) = (inv, pointer)
            && let Some(g) = self.ground_at(&inv, rect, p)
        {
            let (dx, dz) = (g[0] - self.hero.pos[0], g[2] - self.hero.pos[1]);
            let d = dx.hypot(dz).max(1e-3);
            let k = d.min(FLASH_RANGE) / d;
            let to = [self.hero.pos[0] + dx * k, self.hero.pos[1] + dz * k];
            if let Some(t) = self.world.nearest_walkable(to) {
                self.hero.pos = t;
                self.hero.facing = dx.atan2(dz);
                self.hero.stop();
            }
        }
        if keys[4] && champ.find_anim(&["recall"]).is_some() {
            self.hero.path.clear();
            self.hero.action = Action::Recall(0.0);
        }
        if keys[5] {
            self.hero.stop();
        }
        if keys[6] {
            self.locked = !self.locked;
        }
        if keys[8] {
            self.exit = true;
        }
        if notches != 0.0 && resp.hovered() {
            self.cam.desired = (self.cam.desired - notches * WHEEL_NOTCH).clamp(ZOOM_MIN, ZOOM_MAX);
        }

        // Simulation.
        let lens = |n: &str| champ.anims.get(n).map_or(1.0, |a| a.duration());
        self.hero.update(dt, &self.world, lens);

        // Camera movement (UpdateWorldPosition with the smoothing ramp).
        let mut dir = [0.0f32, 0.0];
        let mut keyboard = false;
        if arrows.iter().any(|a| *a) {
            keyboard = true;
            dir = [(arrows[1] as i32 - arrows[0] as i32) as f32, (arrows[2] as i32 - arrows[3] as i32) as f32];
        } else if let Some(hp) = ui.input(|i| i.pointer.hover_pos())
            && rect.contains(hp)
            && ctx.input(|i| i.viewport().focused.unwrap_or(true))
        {
            let edge = 3.0;
            if hp.x <= rect.left() + edge {
                dir[0] -= 1.0;
            }
            if hp.x >= rect.right() - edge {
                dir[0] += 1.0;
            }
            if hp.y <= rect.top() + edge {
                dir[1] += 1.0;
            }
            if hp.y >= rect.bottom() - edge {
                dir[1] -= 1.0;
            }
        }
        // Middle mouse drag scrolls toward the pointer.
        if ui.input(|i| i.pointer.middle_down()) {
            let p = pointer.unwrap_or(rect.center());
            let o = *self.middle_origin.get_or_insert(p);
            let v = p - o;
            if v.length() >= 4.0 {
                dir = [v.x, -v.y];
            }
        } else {
            self.middle_origin = None;
        }
        let len = dir[0].hypot(dir[1]);
        if len > 0.0 && !self.locked {
            self.cam.accel_timer += dt;
            let acc = (self.cam.accel_timer / if keyboard { ACCEL_TIME_KEYBOARD } else { ACCEL_TIME_MOUSE }).min(1.0);
            self.cam.pos[0] += dir[0] / len * SCROLL_SPEED * acc * dt;
            self.cam.pos[1] += dir[1] / len * SCROLL_SPEED * acc * dt;
        } else {
            self.cam.accel_timer = 0.0;
        }
        if self.locked || space {
            self.cam.pos = self.hero.pos;
        }
        // Minimap: left button moves the camera, right button moves the champion.
        if let Some(p) = pointer
            && content.contains(p)
        {
            let b = self.world.bounds;
            let wpt = [b[0] + (p.x - content.left()) / content.width() * (b[2] - b[0]), b[1] + (content.bottom() - p.y) / content.height() * (b[3] - b[1])];
            if ui.input(|i| i.pointer.primary_down()) {
                self.cam.pos = wpt;
            }
            if ui.input(|i| i.pointer.secondary_pressed()) {
                self.hero.move_to(&self.world, wpt);
            }
        }
        let b = self.world.bounds;
        self.cam.pos[0] = self.cam.pos[0].clamp(b[0] + CAM_MARGIN[0], (b[2] - CAM_MARGIN[1]).max(b[0] + CAM_MARGIN[0]));
        self.cam.pos[1] = self.cam.pos[1].clamp(b[1] + CAM_MARGIN[0], (b[3] - CAM_MARGIN[1]).max(b[1] + CAM_MARGIN[0]));
        self.cam.update_zoom(dt);
        if let Some(fx) = &mut self.click_fx {
            fx.1 += dt;
            if fx.1 > 0.6 {
                self.click_fx = None;
            }
        }

        // Camera for this frame (after movement).
        let mvp = self.cam.mvp(aspect);
        let inv = invert(&mvp);

        // Champion mesh for this frame.
        let (anim, t, looping) = self.anim_now();
        let mats = champ.pose(anim.as_deref(), t, looping);
        let (pos, nor) = champ.skin(&mats);
        let y = self.world.height(self.hero.pos[0], self.hero.pos[1]);
        let (sn, cs) = self.hero.facing.sin_cos();
        let k = champ.scale;
        let verts: Vec<RVertex> = pos
            .iter()
            .zip(&nor)
            .zip(&champ.mesh.uvs)
            .map(|((p, n), uv)| {
                let (x, z) = (p[0] * k, p[2] * k);
                RVertex {
                    pos: [self.hero.pos[0] + cs * x + sn * z, y + p[1] * k, self.hero.pos[1] - sn * x + cs * z],
                    normal: [cs * n[0] + sn * n[2], n[1], -sn * n[0] + cs * n[2]],
                    uv0: *uv,
                    uv1: *uv,
                    color: [0.06, 0.06, 0.06, 1.0],
                }
            })
            .collect();
        let in_brush = self.world.in_brush(self.hero.pos[0], self.hero.pos[1]);
        {
            let mut s = gl.lock().unwrap();
            if !self.uploaded {
                if let Some(t) = &champ.texture {
                    s.set_character_texture(t.width, t.height, t.pixels.clone());
                }
                self.uploaded = true;
            }
            // Jungle monsters breathing in their idle animations.
            if let Some(d) = &self.data {
                let w = &self.world;
                let m = self.models.monsters(&mut s, d, &w.monsters, &|x, z| w.height(x, z), self.time);
                s.extras.extend(m);
                // Particle effects around the camera (and the relics' and shrines' auras).
                let mut items = super::fx::nearby(&self.effects, self.cam.pos, 3500.0, 48);
                for (i, mo) in w.monsters.iter().enumerate() {
                    if let Some(e) = super::fx::unit_effect(&mo.name) {
                        items.push((format!("m{i}"), e.to_string(), [mo.pos[0], w.height(mo.pos[0], mo.pos[2]) + 20.0, mo.pos[2]]));
                    }
                }
                if let Some(inv) = &inv {
                    let q = self.fx.frame(&mut s, &d.root, &items, inv);
                    s.extras.extend(q);
                }
            }
            s.character = Some(CharacterDraw { vertices: verts, indices: champ.mesh.indices.clone(), alpha: if in_brush { 0.55 } else { 1.0 } });
        }
        let (eye, _) = self.cam.eye_target();
        let g = gl.clone();
        let cb = egui_glow::CallbackFn::new(move |info, p| {
            let mut s = g.lock().unwrap();
            unsafe { s.paint(p.gl(), &info, &mvp, eye, [0.0, 0.0, 0.0, 1.0]) };
        });
        ui.painter().add(egui::PaintCallback { rect, callback: Arc::new(cb) });

        // World overlays: move marker, path, overhead bar.
        let painter = ui.painter_at(rect);
        let project = |p: [f32; 3]| -> Option<Pos2> {
            let c = gl_scene::transform(&mvp, p);
            (c[3] > 1.0).then(|| Pos2::new(rect.left() + (c[0] / c[3] + 1.0) / 2.0 * w, rect.top() + (1.0 - c[1] / c[3]) / 2.0 * h))
        };
        let ring = |c: [f32; 2], r: f32, col: Color32, wd: f32| {
            let pts: Vec<Pos2> = (0..=32)
                .filter_map(|i| {
                    let a = i as f32 / 32.0 * std::f32::consts::TAU;
                    let (x, z) = (c[0] + a.cos() * r, c[1] + a.sin() * r);
                    project([x, self.world.height(x, z) + 4.0, z])
                })
                .collect();
            painter.add(egui::Shape::line(pts, Stroke::new(wd, col)));
        };
        if let Some((g, t)) = self.click_fx {
            let f = 1.0 - t / 0.6;
            ring([g[0], g[2]], 20.0 + 40.0 * f, Color32::from_rgba_unmultiplied(80, 255, 110, (255.0 * f) as u8), 2.5);
            ring([g[0], g[2]], 8.0 + 16.0 * f, Color32::from_rgba_unmultiplied(80, 255, 110, (200.0 * f) as u8), 2.0);
        }
        let ui_scale = h / 768.0;
        // Overhead health bar (level box + green bar with a tick per 100 health).
        if let Some(p) = project([self.hero.pos[0], y + 250.0, self.hero.pos[1]]) {
            let bw = 104.0 * ui_scale;
            let bh = 11.0 * ui_scale;
            let bar = Rect::from_center_size(p, Vec2::new(bw, bh));
            let lvl = Rect::from_min_size(bar.left_top() - Vec2::new(bh + 4.0 * ui_scale, 2.0 * ui_scale), Vec2::splat(bh + 4.0 * ui_scale));
            painter.rect_filled(bar.expand(2.0 * ui_scale), 1.0, Color32::from_rgb(10, 10, 10));
            painter.rect_filled(lvl, 1.0, Color32::from_rgb(10, 10, 10));
            painter.rect_stroke(lvl, 1.0, Stroke::new(1.0, Color32::from_rgb(120, 100, 60)), egui::StrokeKind::Inside);
            painter.text(lvl.center(), Align2::CENTER_CENTER, "1", self.font(9.0 * ui_scale), Color32::WHITE);
            painter.rect_filled(bar, 0.0, Color32::from_rgb(40, 170, 50));
            painter.rect_filled(Rect::from_min_size(bar.min, Vec2::new(bw, bh * 0.4)), 0.0, Color32::from_rgb(90, 215, 90));
            let hp = champ.stats.hp.max(1.0);
            let mut tick = 100.0;
            while tick < hp {
                let x = bar.left() + bw * tick / hp;
                let big = (tick as i32) % 1000 == 0;
                painter.line_segment([Pos2::new(x, bar.top()), Pos2::new(x, bar.top() + bh * if big { 1.0 } else { 0.55 })], Stroke::new(1.0, Color32::from_rgb(10, 30, 10)));
                tick += 100.0;
            }
        }
        // Recall channel bar (ChannelBar sits above the centre HUD).
        if let Action::Recall(t) = self.hero.action {
            let r = Rect::from_center_size(Pos2::new(rect.center().x, rect.top() + h * 0.76), Vec2::new(200.0 * ui_scale, 9.0 * ui_scale));
            painter.rect_filled(r.expand(2.0), 2.0, Color32::from_rgb(10, 10, 14));
            painter.rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width() * (t / RECALL_TIME).min(1.0), r.height())), 1.0, Color32::from_rgb(70, 140, 255));
            painter.text(r.center_top() - Vec2::new(0.0, 3.0 * ui_scale), Align2::CENTER_BOTTOM, "Recall", self.font(12.0 * ui_scale), Color32::from_rgb(230, 220, 190));
        }

        // Minimap content (drawn under the HUD frame).
        if self.minimap.is_none()
            && let Some(img) = self.minimap_img.take()
        {
            self.minimap = Some(ctx.load_texture("play-minimap", img, TextureOptions::LINEAR));
        }
        if self.circle.is_none()
            && let Some(c) = &champ.circle
        {
            self.circle = Some(ctx.load_texture("play-circle", egui::ColorImage::from_rgba_unmultiplied([c.width, c.height], &c.pixels), TextureOptions::LINEAR));
        }
        let mm = painter.with_clip_rect(content);
        mm.rect_filled(content, 0.0, Color32::BLACK);
        if let Some(t) = &self.minimap {
            mm.image(t.id(), content, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        let to_mm = |x: f32, z: f32| Pos2::new(content.left() + (x - b[0]) / (b[2] - b[0]) * content.width(), content.bottom() - (z - b[1]) / (b[3] - b[1]) * content.height());
        for (name, team, x, z) in &self.world.structures {
            let col = if *team == 1 { Color32::from_rgb(60, 140, 255) } else { Color32::from_rgb(255, 60, 50) };
            let p = to_mm(*x, *z);
            let n = name.to_ascii_lowercase();
            let s = if n.contains("nexus") || n.contains("hq") { 7.0 } else if n.contains("barracks") || n.contains("inhib") { 5.0 } else { 4.0 } * ui_scale;
            mm.rect_filled(Rect::from_center_size(p, Vec2::splat(s * 2.0)), 1.0, Color32::BLACK);
            mm.rect_filled(Rect::from_center_size(p, Vec2::splat(s * 2.0 - 2.0)), 1.0, col);
        }
        // Jungle camps (one icon per camp; buffs, Dragon and Baron larger).
        let mut shown = std::collections::HashSet::new();
        for m in &self.world.monsters {
            if !shown.insert(m.camp) {
                continue;
            }
            let p = to_mm(m.pos[0], m.pos[2]);
            let (col, r) = match m.name.as_str() {
                "Dragon" => (Color32::from_rgb(255, 170, 40), 6.0),
                "Worm" => (Color32::from_rgb(190, 90, 255), 6.5),
                "AncientGolem" | "LizardElder" => (Color32::from_rgb(250, 230, 150), 4.5),
                _ => (Color32::from_rgb(230, 220, 170), 3.2),
            };
            mm.circle_filled(p, r * ui_scale, Color32::from_black_alpha(200));
            mm.circle_filled(p, (r - 1.2) * ui_scale, col);
        }
        // The camera's view on the ground, as the white outline the client draws.
        if let Some(inv) = inv {
            let pts: Vec<Pos2> = [[-1.0f32, 1.0], [1.0, 1.0], [1.0, -1.0], [-1.0, -1.0], [-1.0, 1.0]].iter().filter_map(|c| Self::plane_at(&inv, *c)).map(|q| to_mm(q[0], q[1])).collect();
            mm.add(egui::Shape::line(pts, Stroke::new(1.0, Color32::WHITE)));
        }
        let hp = to_mm(self.hero.pos[0], self.hero.pos[1]);
        let icon = 42.0 * 0.575 * ui_scale;
        let ir = Rect::from_center_size(hp, Vec2::splat(icon));
        if let Some(t) = &self.circle {
            mm.image(t.id(), ir, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        mm.circle_stroke(hp, icon / 2.0, Stroke::new(1.5 * ui_scale, Color32::from_rgb(40, 90, 255)));

        // The game's HUD.
        if let Some(hd) = &hud {
            // The client switches the classic top bar on for Summoner's Rift style games.
            let overrides = [("TitanBarClassic", true)];
            let mut order: Vec<usize> = (0..hd.elements.len()).filter(|&i| hd.elements[i].enabled && hd.scene_on(&hd.elements[i], &overrides)).collect();
            order.sort_by_key(|&i| (hd.scene_layer(&hd.elements[i]), hd.elements[i].layer));
            let hp = painter.with_clip_rect(rect);
            for i in order {
                let e = &hd.elements[i];
                let Some(frac) = self.hud_state(&e.name, &e.file) else { continue };
                let r = to_rect(e.screen_rect(w, h));
                match e.kind {
                    Kind::Icon => {
                        if e.texture.is_empty() || e.texture == "None" {
                            continue;
                        }
                        let col = Color32::from_rgba_unmultiplied(e.color[0], e.color[1], e.color[2], e.color[3]);
                        let blend = e.blend;
                        let Some(t) = self.texture(&ctx, &e.texture) else { continue };
                        let id = if blend { t.blend.id() } else { t.opaque.id() };
                        let mut uv = e.uv;
                        let mut rr = r;
                        if frac < 1.0 {
                            rr.max.x = rr.min.x + rr.width() * frac;
                            if e.per_pixel_uvs_x {
                                uv[2] = uv[0] + (uv[2] - uv[0]) * frac;
                            }
                        }
                        hp.image(id, rr, Rect::from_min_max(Pos2::new(uv[0], uv[1]), Pos2::new(uv[2], uv[3])), col);
                    }
                    Kind::Text => {
                        let Some(s) = self.hud_text(&e.name) else { continue };
                        let font = self.font(e.font_px(h));
                        let (anchor, at) = match e.align {
                            Align::Left => (Align2::LEFT_TOP, r.left_top()),
                            Align::Center => (Align2::CENTER_TOP, r.center_top()),
                            Align::Right => (Align2::RIGHT_TOP, r.right_top()),
                        };
                        if e.drop_shadow {
                            hp.text(at + Vec2::splat(1.0), anchor, &s, font.clone(), Color32::from_black_alpha(220));
                        }
                        hp.text(at, anchor, &s, font, Color32::from_rgba_unmultiplied(e.color[0], e.color[1], e.color[2], e.color[3]));
                    }
                    Kind::Effect => {}
                }
            }
        }
        // How to leave.
        painter.text(rect.left_top() + Vec2::new(8.0, 6.0), Align2::LEFT_TOP, if self.locked { "Esc: leave Play mode    Camera locked (Y)" } else { "Esc: leave Play mode" }, self.font(11.0 * ui_scale.max(1.0)), Color32::from_white_alpha(140));
        ctx.request_repaint();
    }

    fn font(&self, px: f32) -> FontId {
        if ARIAL.load(Ordering::Relaxed) { FontId::new(px.max(4.0), FontFamily::Name("Arial".into())) } else { FontId::proportional(px.max(4.0)) }
    }
}
