//! Maps from a text prompt. A prompt becomes a `MapPlan` (with Claude through the Anthropic API,
//! or offline by reading key words), and the plan becomes a normal editable layout: lanes and
//! structures, generated terrain, river, jungle camps, decoration, effects, light and atmosphere.

use crate::layout::{Lane, Layout, ObjKind, Surface, Symmetry, TurretTier};
use crate::layout_build::hash01;
use crate::props::PropLibrary;
use crate::worldtools::{GenOptions, generate};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

fn d_size() -> f32 {
    12000.0
}
fn d_lanes() -> Vec<String> {
    vec!["top".into(), "mid".into(), "bot".into()]
}
fn d_true() -> bool {
    true
}
fn d_walls() -> f32 {
    0.38
}
fn d_feature() -> f32 {
    1400.0
}
fn d_brush() -> f32 {
    0.12
}
fn d_hills() -> f32 {
    60.0
}
fn d_theme() -> String {
    "summoners_rift".into()
}

/// A decoration rule: props of a scenery category (optionally whose name contains `name`)
/// scattered over a part of the map.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Decor {
    pub category: String,
    #[serde(default)]
    pub name: String,
    /// "walls", "jungle", "lanes", "river", "bases" or "everywhere".
    pub area: String,
    /// 0..1, how full the area gets.
    pub density: f32,
}

/// A 3D model to fetch online (Poly Haven, CC0) and place, restyled like classic Map1 art.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OnlineDecor {
    /// A few English words ("wooden barrel", "mossy boulder").
    pub query: String,
    #[serde(default)]
    pub category: String,
    /// Same areas as decor.
    #[serde(default = "d_area")]
    pub area: String,
    #[serde(default = "d_density")]
    pub density: f32,
}

fn d_area() -> String {
    "jungle".into()
}

fn d_density() -> f32 {
    0.15
}

/// Particle effects along lanes, at camps or bases.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EffectRule {
    pub effect: String,
    /// "lanes", "camps", "bases" or "river".
    pub area: String,
    pub count: u32,
}

/// What the map should be. Every field has a default, so partial answers still work.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MapPlan {
    #[serde(default)]
    pub name: String,
    #[serde(default = "d_size")]
    pub size: f32,
    /// "diagonal", "rotational", "mirror_x", "mirror_z", "none".
    #[serde(default)]
    pub symmetry: String,
    /// Which lanes: "top", "mid", "bot".
    #[serde(default = "d_lanes")]
    pub lanes: Vec<String>,
    #[serde(default = "d_true")]
    pub structures: bool,
    #[serde(default = "d_walls")]
    pub walls: f32,
    #[serde(default = "d_feature")]
    pub wall_size: f32,
    #[serde(default = "d_brush")]
    pub brush: f32,
    #[serde(default = "d_hills")]
    pub hills: f32,
    #[serde(default = "d_true")]
    pub river: bool,
    #[serde(default = "d_true")]
    pub jungle_camps: bool,
    #[serde(default = "d_true")]
    pub dragon_baron: bool,
    /// Dominion / ARAM objects: "health_relics", "capture_points", "speed_shrines".
    #[serde(default)]
    pub mode_objects: Vec<String>,
    /// "summoners_rift", "snow", "desert", "night", "volcanic", "autumn", "shadow_isles".
    #[serde(default = "d_theme")]
    pub theme: String,
    #[serde(default)]
    pub trees_on_walls: Option<bool>,
    #[serde(default)]
    pub decor: Vec<Decor>,
    /// Extra models fetched online.
    #[serde(default)]
    pub online: Vec<OnlineDecor>,
    #[serde(default)]
    pub effects: Vec<EffectRule>,
    /// Overrides as "r g b" (0..255) and a direction.
    #[serde(default)]
    pub sun_color: Option<[f32; 3]>,
    #[serde(default)]
    pub ambient_color: Option<[f32; 3]>,
    /// Look & behaviour settings ("file|Section*Key" -> value), see map_settings.
    #[serde(default)]
    pub settings: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub seed: u32,
    /// What the planner says it made (shown to the user).
    #[serde(default)]
    pub summary: String,
}

impl Default for MapPlan {
    fn default() -> Self {
        serde_json::from_str("{}").unwrap()
    }
}

// ------------------------------------------------------------------ offline planner

/// Reads a prompt without AI: key words for lanes, theme, terrain and features.
pub fn plan_from_keywords(prompt: &str) -> MapPlan {
    let p = prompt.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| p.contains(x));
    let mut m = MapPlan { name: "AI Map".into(), ..Default::default() };
    // Objects named in the prompt that are fetched online.
    for (word, query, cat, area) in [
        ("barrel", "wooden barrel", "Props", "bases"),
        ("crate", "wooden crate", "Props", "bases"),
        ("cart", "wooden cart", "Props", "lanes"),
        ("wagon", "wooden cart", "Props", "lanes"),
        ("lantern", "lantern", "Lights", "lanes"),
        ("statue", "stone statue", "Statues & ruins", "jungle"),
        ("boulder", "boulder", "Rocks", "jungle"),
        ("stump", "tree stump", "Trees", "jungle"),
        ("log", "dead tree trunk", "Trees", "jungle"),
        ("dead tree", "dead tree trunk", "Trees", "walls"),
        ("fence", "wooden fence", "Props", "lanes"),
        ("bench", "wooden bench", "Props", "bases"),
        ("well", "stone well", "Props", "bases"),
        ("pot", "clay pot", "Props", "bases"),
        ("ruin", "stone ruins", "Statues & ruins", "jungle"),
    ] {
        if p.contains(word) && !m.online.iter().any(|o| o.query == query) && m.online.len() < 5 {
            m.online.push(OnlineDecor { query: query.into(), category: cat.into(), area: area.into(), density: 0.15 });
        }
    }
    m.seed = prompt.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 100000;
    if has(&["one lane", "1 lane", "single lane", "aram", "howling", "bridge"]) {
        m.lanes = vec!["mid".into()];
        m.jungle_camps = false;
        m.dragon_baron = false;
        m.river = false;
        m.walls = 0.6;
        m.mode_objects.push("health_relics".into());
        m.size = 9000.0;
    } else if has(&["two lane", "2 lane", "twisted treeline", "3v3", "treeline"]) {
        m.lanes = vec!["top".into(), "bot".into()];
        m.size = 10000.0;
    }
    if has(&["no lane", "jungle only", "arena"]) {
        m.lanes.clear();
        m.structures = false;
    }
    for (w, s) in [(["small", "tiny"], 8000.0), (["big", "huge"], 16000.0)] {
        if has(&w) {
            m.size = s;
        }
    }
    if has(&["no river", "without river", "dry"]) {
        m.river = false;
    }
    if has(&["no jungle", "no camps", "without jungle"]) {
        m.jungle_camps = false;
    }
    if has(&["open", "few walls", "sparse"]) {
        m.walls = 0.2;
    }
    if has(&["dense", "maze", "many walls", "labyrinth"]) {
        m.walls = 0.55;
        m.wall_size = 900.0;
    }
    if has(&["lots of brush", "much brush", "bushy", "grass everywhere"]) {
        m.brush = 0.3;
    }
    if has(&["mountain", "hill", "rugged"]) {
        m.hills = 200.0;
    }
    if has(&["flat"]) {
        m.hills = 0.0;
    }
    if has(&["dominion", "capture point", "crystal scar"]) {
        m.mode_objects.extend(["capture_points".into(), "speed_shrines".into()]);
    }
    if has(&["relic", "health pack"]) && !m.mode_objects.iter().any(|x| x == "health_relics") {
        m.mode_objects.push("health_relics".into());
    }
    m.theme = if has(&["snow", "winter", "ice", "frozen", "freljord"]) {
        "snow"
    } else if has(&["desert", "sand", "shurima"]) {
        "desert"
    } else if has(&["lava", "volcan", "fire", "hell"]) {
        "volcanic"
    } else if has(&["shadow isles", "haunted", "spooky", "ghost", "undead"]) {
        "shadow_isles"
    } else if has(&["night", "dark", "moon"]) {
        "night"
    } else if has(&["autumn", "fall"]) {
        "autumn"
    } else {
        "summoners_rift"
    }
    .into();
    if has(&["torch", "lit lane", "light"]) {
        m.effects.push(EffectRule { effect: "FireTorch_Med.troy".into(), area: "lanes".into(), count: 24 });
    }
    if has(&["flower", "colorful", "colourful"]) {
        m.decor.push(Decor { category: "Plants & flowers".into(), name: String::new(), area: "jungle".into(), density: 0.4 });
    }
    if has(&["rock", "stone", "boulder"]) {
        m.decor.push(Decor { category: "Rocks".into(), name: String::new(), area: "jungle".into(), density: 0.3 });
    }
    if has(&["ruin", "statue", "ancient"]) {
        m.decor.push(Decor { category: "Statues & ruins".into(), name: String::new(), area: "jungle".into(), density: 0.2 });
    }
    if m.decor.is_empty() {
        m.decor.push(Decor { category: "Plants & flowers".into(), name: String::new(), area: "jungle".into(), density: 0.15 });
        m.decor.push(Decor { category: "Rocks".into(), name: String::new(), area: "jungle".into(), density: 0.1 });
    }
    m.summary = format!(
        "{} lane(s), {} theme, {}{}{}walls {:.0}%, brush {:.0}%",
        m.lanes.len(),
        m.theme.replace('_', " "),
        if m.river { "river, " } else { "" },
        if m.jungle_camps { "jungle camps, " } else { "" },
        if m.mode_objects.is_empty() { String::new() } else { format!("{}, ", m.mode_objects.join(", ").replace('_', " ")) },
        m.walls * 100.0,
        m.brush * 100.0
    );
    m
}

// ------------------------------------------------------------------ Claude

const SYSTEM: &str = r#"You design League of Legends maps for LoL Map Studio (game client 4.17). Reply with ONE JSON object and nothing else, following this schema (every field optional):
{
 "name": string,
 "summary": string (one or two sentences describing your design to the user),
 "size": number (map width in game units, 6000-20000; Summoner's Rift is 14870, ARAM about 12000 but narrow),
 "symmetry": "diagonal" | "rotational" | "mirror_x" | "mirror_z" | "none",
 "lanes": array of "top" | "mid" | "bot" (empty = no lanes),
 "structures": bool (turrets, inhibitors, nexus, fountains on the lanes),
 "walls": number 0-0.8 (share of the map that is jungle wall),
 "wall_size": number 300-4000 (size of wall blobs; small = maze-like),
 "brush": number 0-0.5 (share of open ground covered by tall grass),
 "hills": number 0-300 (height variation),
 "river": bool,
 "jungle_camps": bool,
 "dragon_baron": bool,
 "mode_objects": array of "health_relics" | "capture_points" | "speed_shrines",
 "theme": "summoners_rift" | "snow" | "desert" | "night" | "volcanic" | "autumn" | "shadow_isles",
 "trees_on_walls": bool,
 "decor": [ {"category": "Trees"|"Rocks"|"Plants & flowers"|"Lights"|"Statues & ruins"|"Props"|"Base & walls"|"Decals", "name": optional substring of a prop name, "area": "walls"|"jungle"|"lanes"|"river"|"bases"|"everywhere", "density": 0-1} ],
 "online": [ {"query": "2-3 English words naming one object to download as a free 3D model, e.g. "wooden barrel", "mossy boulder", "dead tree trunk", "stone statue", "wooden cart", "tree stump"", "category": one of the decor categories, "area": as in decor, "density": 0-0.4} ] (at most 5; for objects the classic scenery lacks; the studio downloads CC0 models and restyles them to look like classic 2014 Summoner's Rift art, so ask for old, natural or medieval-fantasy things, never modern ones),
 "effects": [ {"effect": particle file such as "FireTorch_Med.troy", "Fire_Cauldron.troy", "WaterRipples.troy", "area": "lanes"|"camps"|"bases"|"river", "count": number} ],
 "sun_color": [r,g,b] 0-255, "ambient_color": [r,g,b] 0-255,
 "settings": object of map settings, keys like "terrain.inibin|Fog*enable" "1", "terrain.inibin|Fog*red" "200", "terrain.inibin|Fog*green", "terrain.inibin|Fog*blue", "terrain.inibin|Fog*opacity" "0.4", "terrain.inibin|Fog*min_height", "terrain.inibin|Fog*max_height", "Gamma.inibin|Gamma*Brightness" "1.1", "Gamma.inibin|Gamma*Contrast", "Gamma.inibin|Gamma*Gamma", "clouds.inibin|Clouds*SpeedU", "Grass.ini|SeeThroughMinAlpha", "Constants.var|ai_StartingGold" "1400", "Constants.var|ai_AmbientGoldAmount", "DeathTimes.inibin|DeathTimeScaling*PercentCap", values as strings,
 "seed": number
}
Design like Riot's level designers: readable lanes, fair symmetry for competitive maps, jungle paths wide enough to walk, brush near lanes for ganks, decoration that matches the theme."#;

/// Asks Claude for a plan. `model` e.g. "claude-opus-5-5".
pub fn plan_with_claude(prompt: &str, api_key: &str, model: &str) -> Result<MapPlan> {
    if api_key.trim().is_empty() {
        bail!("no Anthropic API key");
    }
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 4000,
        "system": SYSTEM,
        "messages": [{ "role": "user", "content": prompt }],
    });
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(std::time::Duration::from_secs(180))).build().into();
    let mut resp = agent
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key.trim())
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .send(body.to_string())
        .context("could not reach the Anthropic API")?;
    let status = resp.status();
    let text = resp.body_mut().read_to_string().context("reading the API answer")?;
    let v: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("unexpected API answer: {}", &text.chars().take(300).collect::<String>()))?;
    if !status.is_success() {
        let msg = v.pointer("/error/message").and_then(|m| m.as_str()).unwrap_or(&text);
        bail!("the API answered {status}: {msg}");
    }
    let out: String = v.get("content").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("")).unwrap_or_default();
    let (a, b) = (out.find('{').context("the answer had no map plan")?, out.rfind('}').context("the answer had no map plan")?);
    let mut plan: MapPlan = serde_json::from_str(&out[a..=b]).context("the map plan could not be read")?;
    if plan.seed == 0 {
        plan.seed = prompt.len() as u32 * 7919 % 100000;
    }
    Ok(plan)
}

// ------------------------------------------------------------------ plan -> layout

fn theme_preset(t: &str) -> ([f32; 3], [f32; 3], [&'static str; 4], &'static str, Vec<(&'static str, &'static str)>) {
    // sun colour, ambient, ground layers, cliff texture, settings
    match t {
        "snow" => ([150.0, 160.0, 185.0], [40.0, 70.0, 120.0], ["gen:snow", "gen:path", "gen:dirt", "gen:grass"], "gen:rock", vec![("terrain.inibin|Fog*enable", "1"), ("terrain.inibin|Fog*red", "220"), ("terrain.inibin|Fog*green", "230"), ("terrain.inibin|Fog*blue", "245"), ("terrain.inibin|Fog*opacity", "0.25")]),
        "desert" => ([175.0, 150.0, 110.0], [70.0, 55.0, 40.0], ["gen:sand", "gen:dirt", "gen:path", "gen:grass"], "gen:rock", vec![("Gamma.inibin|Gamma*Contrast", "1.1")]),
        "night" => ([60.0, 80.0, 140.0], [10.0, 25.0, 70.0], ["gen:grass", "gen:dirt", "gen:path", "gen:sand"], "gen:rock", vec![("Gamma.inibin|Gamma*Brightness", "0.85"), ("terrain.inibin|Fog*enable", "1"), ("terrain.inibin|Fog*red", "20"), ("terrain.inibin|Fog*green", "30"), ("terrain.inibin|Fog*blue", "70"), ("terrain.inibin|Fog*opacity", "0.4")]),
        "volcanic" => ([180.0, 100.0, 60.0], [50.0, 20.0, 15.0], ["gen:ash", "gen:dirt", "gen:path", "gen:sand"], "gen:rock", vec![("terrain.inibin|Fog*enable", "1"), ("terrain.inibin|Fog*red", "120"), ("terrain.inibin|Fog*green", "40"), ("terrain.inibin|Fog*blue", "20"), ("terrain.inibin|Fog*opacity", "0.2"), ("Gamma.inibin|Gamma*Contrast", "1.15")]),
        "autumn" => ([170.0, 135.0, 95.0], [50.0, 45.0, 80.0], ["gen:dirt", "gen:grass", "gen:path", "gen:sand"], "gen:rock", vec![]),
        "shadow_isles" => ([90.0, 140.0, 120.0], [20.0, 50.0, 45.0], ["gen:ash", "gen:dirt", "gen:path", "gen:grass"], "gen:rock", vec![("terrain.inibin|Fog*enable", "1"), ("terrain.inibin|Fog*red", "40"), ("terrain.inibin|Fog*green", "90"), ("terrain.inibin|Fog*blue", "80"), ("terrain.inibin|Fog*opacity", "0.45"), ("Gamma.inibin|Gamma*Brightness", "0.9")]),
        _ => ([137.0, 149.0, 180.0], [19.0, 92.0, 194.0], ["gen:grass", "gen:dirt", "gen:path", "gen:sand"], "gen:rock", vec![]),
    }
}

/// Builds the layout a plan describes.
/// Fetches the plan's online models (restyled like classic Map1 art, saved to the asset library)
/// and turns them into decoration rules. Returns what happened, one line per model.
pub fn fetch_online(plan: &mut MapPlan, lib: Option<&PropLibrary>, map1_textures: Option<&std::path::Path>) -> Vec<String> {
    use crate::online_assets as oa;
    let mut log = Vec::new();
    if plan.online.is_empty() {
        return log;
    }
    let cat = match oa::catalogue() {
        Ok(c) => c,
        Err(e) => {
            log.push(format!("Online models skipped: {e:#}"));
            return log;
        }
    };
    for o in plan.online.clone().into_iter().take(6) {
        let hits = oa::search(&cat, &o.query);
        // Two models per request for some variety.
        let mut got = 0;
        for m in hits.into_iter().take(4) {
            if got == 2 {
                break;
            }
            let category = oa::category_for(m, &o.category);
            match oa::add_classic(m, &category, lib, map1_textures) {
                Ok(key) => {
                    let name = key.trim_start_matches("asset:").to_string();
                    log.push(format!("\"{}\": {} (Poly Haven, CC0), restyled", o.query, m.name));
                    // Small objects are dotted around; rocks and trees may be denser.
                    let cap = if matches!(category.as_str(), "Rocks" | "Trees" | "Plants & flowers") { 0.05 } else { 0.03 };
                    plan.decor.push(Decor { category, name, area: o.area.clone(), density: o.density.clamp(0.0, cap) });
                    got += 1;
                }
                Err(e) => log.push(format!("\"{}\": {} failed: {e:#}", o.query, m.name)),
            }
        }
        if got == 0 {
            log.push(format!("\"{}\": nothing suitable found online", o.query));
        }
    }
    log
}

pub fn build_layout(plan: &MapPlan, lib: Option<&PropLibrary>, map_id: u32) -> Layout {
    let size = plan.size.clamp(5000.0, 20000.0);
    let name = if plan.name.trim().is_empty() { "AI Map".to_string() } else { plan.name.clone() };
    let mut l = Layout::classic_start(&name, map_id, size);
    // Lanes and their structures.
    let keep: Vec<Lane> = plan
        .lanes
        .iter()
        .filter_map(|s| match s.to_ascii_lowercase().as_str() {
            "top" => Some(Lane::Top),
            "mid" | "middle" => Some(Lane::Mid),
            "bot" | "bottom" => Some(Lane::Bot),
            _ => None,
        })
        .collect();
    l.lanes.retain(|lp| keep.contains(&lp.lane));
    l.objectives.retain(|o| !o.kind.has_lane() || keep.contains(&o.lane) || matches!(o.tier, TurretTier::NexusA | TurretTier::NexusB));
    if !plan.structures || keep.is_empty() {
        l.objectives.retain(|o| matches!(o.kind, ObjKind::SpawnPoint | ObjKind::FountainTurret | ObjKind::Shop) || o.kind.is_camp());
    }
    if !plan.jungle_camps {
        l.objectives.retain(|o| !o.kind.is_camp() || (plan.dragon_baron && o.kind.is_epic()));
    }
    if !plan.dragon_baron {
        l.objectives.retain(|o| !matches!(o.kind, ObjKind::Dragon | ObjKind::Baron));
    }
    l.symmetry = match plan.symmetry.as_str() {
        "none" => Symmetry::None,
        "rotational" => Symmetry::Point,
        "mirror_x" => Symmetry::MirrorX,
        "mirror_z" => Symmetry::MirrorZ,
        _ => Symmetry::Diagonal,
    };
    // Terrain.
    let seed = if plan.seed == 0 { 1 } else { plan.seed };
    generate(&mut l, &GenOptions { seed, walls: plan.walls.clamp(0.0, 0.8), feature: plan.wall_size.clamp(300.0, 4000.0), brush: plan.brush.clamp(0.0, 0.5), hills: plan.hills.clamp(0.0, 400.0), keep_lanes: true, river: plan.river });
    // Game-mode objects, spread around the middle.
    let c = [size / 2.0, size / 2.0];
    for (k, name) in plan.mode_objects.iter().enumerate() {
        let kind = match name.as_str() {
            "health_relics" => ObjKind::HealthRelic,
            "capture_points" => ObjKind::CapturePoint,
            "speed_shrines" => ObjKind::SpeedShrine,
            _ => continue,
        };
        let n = if kind == ObjKind::CapturePoint { 5 } else { 2 };
        for i in 0..n {
            let a = (i as f32 / n as f32 + k as f32 * 0.13) * std::f32::consts::TAU;
            let r = size * if kind == ObjKind::CapturePoint { 0.3 } else { 0.18 };
            let p = [c[0] + a.cos() * r, c[1] + a.sin() * r];
            let saved = l.symmetry;
            l.symmetry = Symmetry::None;
            l.paint_surface(p[0], p[1], 300.0, false, Surface::Ground);
            l.add_objective(kind, 0, Lane::Mid, TurretTier::Outer, p);
            l.symmetry = saved;
        }
    }
    // Look.
    let (sun, amb, ground, cliff, settings) = theme_preset(&plan.theme);
    l.theme.sun_color = plan.sun_color.unwrap_or(sun);
    l.theme.ambient = plan.ambient_color.unwrap_or(amb);
    for (i, t) in ground.iter().enumerate() {
        if let Some(g) = l.theme.ground.get_mut(i) {
            g.texture = t.to_string();
            g.name = t.trim_start_matches("gen:").to_string();
        }
    }
    l.theme.cliff = cliff.into();
    // Classic Summoner's Rift looks use Map1's own textures (found through its props), not generated ones.
    if matches!(plan.theme.as_str(), "summoners_rift" | "night" | "")
        && let Some(dir) = lib.and_then(|lb| lb.props.iter().find(|d| d.key.starts_with("Map1/")).map(|d| d.texture_dir.clone()))
    {
        let find = |n: &str| std::fs::read_dir(&dir).ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(n))));
        for (i, (n, label)) in [("grass_TX_CM.dds", "grass"), ("order_dirt.dds", "dirt"), ("gravel_stone_TX_CM.dds", "path"), ("td_sand1.dds", "sand")].into_iter().enumerate() {
            if let (Some(f), Some(g)) = (find(n), l.theme.ground.get_mut(i)) {
                g.texture = f.to_string_lossy().to_string();
                g.name = label.into();
            }
        }
        if let Some(f) = find("rocks.dds") {
            l.theme.cliff = f.to_string_lossy().to_string();
        }
        if let Some(f) = find("WallOfGrass.dds") {
            l.theme.brush = f.to_string_lossy().to_string();
        }
    }
    l.theme.forest.enabled = plan.trees_on_walls.unwrap_or(plan.theme != "desert" && plan.theme != "volcanic");
    for (k, v) in settings {
        l.settings.insert(k.into(), v.into());
    }
    for (k, v) in &plan.settings {
        l.settings.insert(k.clone(), v.clone());
    }
    // Decoration.
    if let Some(lib) = lib {
        let surface_at = |l: &Layout, p: [f32; 2]| l.cell_of(p[0], p[1]).map(|(i, j)| l.surface_at(i, j));
        let near_lane = |l: &Layout, p: [f32; 2], d: f32| l.lanes.iter().any(|lp| lp.points.windows(2).any(|w| seg_dist(p, w[0], w[1]) < d));
        for (r, rule) in plan.decor.iter().enumerate() {
            let props: Vec<&crate::props::PropDef> = lib.props.iter().filter(|d| crate::props::PropLibrary::allowed(&d.key, &l.asset_maps) && d.category.eq_ignore_ascii_case(&rule.category) && (rule.name.is_empty() || d.name.to_ascii_lowercase().contains(&rule.name.to_ascii_lowercase()))).collect();
            if props.is_empty() {
                continue;
            }
            let step = 260.0;
            let n = (size / step) as u32;
            for b in 0..n {
                for a in 0..n {
                    let h = hash01(a, b, 100 + r as u32 + seed);
                    if h > rule.density.clamp(0.0, 1.0) * 0.6 {
                        continue;
                    }
                    let p = [(a as f32 + 0.2 + 0.6 * hash01(a, b, 3)) * step, (b as f32 + 0.2 + 0.6 * hash01(a, b, 4)) * step];
                    let Some(s) = surface_at(&l, p) else { continue };
                    let ok = match rule.area.as_str() {
                        "walls" => s == Surface::Wall,
                        "river" => s == Surface::River,
                        "lanes" => s.walkable() && near_lane(&l, p, 600.0) && !near_lane(&l, p, 250.0),
                        "bases" => s.walkable() && l.objectives.iter().any(|o| o.kind == ObjKind::Nexus && (o.pos[0] - p[0]).hypot(o.pos[1] - p[1]) < 2200.0),
                        "everywhere" => s != Surface::Void,
                        _ => s.walkable() && !near_lane(&l, p, 400.0) && !l.objectives.iter().any(|o| (o.pos[0] - p[0]).hypot(o.pos[1] - p[1]) < o.kind.radius() + 150.0),
                    };
                    if !ok {
                        continue;
                    }
                    let d = props[(hash01(a, b, 9) * props.len() as f32) as usize % props.len()];
                    let key = d.key.clone();
                    let saved = l.symmetry;
                    l.symmetry = Symmetry::None;
                    l.add_prop(&key, p, 360.0 * hash01(a, b, 5), 0.85 + 0.35 * hash01(a, b, 6));
                    l.symmetry = saved;
                }
            }
        }
    }
    // Effects.
    for rule in &plan.effects {
        let spots: Vec<[f32; 2]> = match rule.area.as_str() {
            "camps" => l.objectives.iter().filter(|o| o.kind.is_camp()).map(|o| [o.pos[0] + 150.0, o.pos[1] + 150.0]).collect(),
            "bases" => l.objectives.iter().filter(|o| matches!(o.kind, ObjKind::Nexus | ObjKind::SpawnPoint)).map(|o| [o.pos[0] + 250.0, o.pos[1]]).collect(),
            "river" => (0..rule.count).map(|k| {
                let t = (k as f32 + 0.5) / rule.count.max(1) as f32;
                [size * t, size * (1.0 - t)]
            }).collect(),
            _ => {
                let mut v = Vec::new();
                let per = (rule.count as usize / l.lanes.len().max(1)).max(1);
                for lp in &l.lanes {
                    for k in 0..per {
                        let p = crate::layout::point_along(&lp.points, (k as f32 + 0.5) / per as f32);
                        v.push([p[0] + 280.0, p[1] - 280.0]);
                    }
                }
                v
            }
        };
        for p in spots.into_iter().take(rule.count.max(1) as usize * 2) {
            l.add_effect(&rule.effect, p, 0.0, 0.0);
        }
    }
    l
}

fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dz) / (dx * dx + dz * dz).max(1e-6)).clamp(0.0, 1.0);
    (p[0] - a[0] - dx * t).hypot(p[1] - a[1] - dz * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_to_map() {
        let p = plan_from_keywords("A snowy two lane map with torches along the lanes and no river");
        assert_eq!(p.lanes.len(), 2);
        assert_eq!(p.theme, "snow");
        assert!(!p.river);
        let l = build_layout(&p, None, 40);
        assert_eq!(l.lanes.len(), 2);
        assert!(l.objectives.iter().all(|o| !o.kind.has_lane() || o.lane != Lane::Mid || matches!(o.tier, TurretTier::NexusA | TurretTier::NexusB)));
        assert!(!l.effects.is_empty());
        assert_eq!(l.settings.get("terrain.inibin|Fog*enable").map(|s| s.as_str()), Some("1"));
        let json = r#"{"lanes":["mid"],"theme":"volcanic","walls":0.5,"mode_objects":["health_relics"]}"#;
        let plan: MapPlan = serde_json::from_str(json).unwrap();
        let l = build_layout(&plan, None, 41);
        assert_eq!(l.lanes.len(), 1);
        assert!(l.objectives.iter().any(|o| o.kind == ObjKind::HealthRelic));
    }
}
