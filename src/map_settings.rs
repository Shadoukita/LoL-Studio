//! Map look and behaviour settings: the per-map configuration files the 4.17 client (and a game
//! server) read, with the keys that matter to map makers. Values set by the user are stored in
//! the layout (`Layout::settings`, key "file|Section*Key") and written over the map's own files
//! when it is built; unset values keep what the map (or the rules map) has.

use crate::formats::inibin::{Inibin, Value, ValueKind, hash_key};
use crate::project::{MapFile, MapProject};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Float(f32, f32),
    Int(i32, i32),
    /// 0 / 1 stored as a number.
    Switch,
    /// "r g b" (0..255).
    Color,
    Text,
    Choice(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Store {
    Inibin,
    /// Grass.ini: "Name value" lines.
    NameValue,
    /// Constants.var: "name = value ; comment" lines.
    Var,
}

#[derive(Clone, Debug)]
pub struct Spec {
    pub group: &'static str,
    pub file: &'static str,
    pub store: Store,
    /// "Section*Key" for inibin files, the variable name for text files.
    pub key: String,
    pub label: String,
    pub help: &'static str,
    pub kind: Kind,
    /// Used when neither the map nor the user sets it.
    pub default: String,
    /// Only a game server uses it (the client ignores it).
    pub server: bool,
}

impl Spec {
    pub fn id(&self) -> String {
        format!("{}|{}", self.file, self.key)
    }
}

fn s(group: &'static str, file: &'static str, store: Store, key: &str, label: &str, help: &'static str, kind: Kind, default: &str, server: bool) -> Spec {
    Spec { group, file, store, key: key.into(), label: label.into(), help, kind, default: default.into(), server }
}

/// Every setting, grouped for the panel.
pub fn specs() -> Vec<Spec> {
    use Kind::*;
    use Store::*;
    let mut v = vec![
        // Atmosphere: fog, fog of war, clouds.
        s("Fog", "terrain.inibin", Inibin, "Fog*enable", "Height fog", "Fog in low areas of the map", Switch, "0", false),
        s("Fog", "terrain.inibin", Inibin, "Fog*red", "Fog red", "", Int(0, 255), "128", false),
        s("Fog", "terrain.inibin", Inibin, "Fog*green", "Fog green", "", Int(0, 255), "140", false),
        s("Fog", "terrain.inibin", Inibin, "Fog*blue", "Fog blue", "", Int(0, 255), "160", false),
        s("Fog", "terrain.inibin", Inibin, "Fog*opacity", "Fog opacity", "", Float(0.0, 1.0), "0.5", false),
        s("Fog", "terrain.inibin", Inibin, "Fog*min_height", "Fog bottom height", "", Float(-2000.0, 3000.0), "-200", false),
        s("Fog", "terrain.inibin", Inibin, "Fog*max_height", "Fog top height", "", Float(-2000.0, 3000.0), "100", false),
        s("Fog", "terrain.inibin", Inibin, "Fog*min_radius", "Fog near radius", "Distance from the camera where fog starts", Float(0.0, 20000.0), "0", false),
        s("Fog", "terrain.inibin", Inibin, "Fog*max_radius", "Fog far radius", "", Float(0.0, 20000.0), "5000", false),
        s("Fog of war", "terrain.inibin", Inibin, "FoW*baseMapBrightness", "Brightness under fog of war", "How visible unseen ground is", Float(0.0, 2.0), "1", false),
        s("Fog of war", "terrain.inibin", Inibin, "FoW*fadeStart", "Fade start height", "", Float(-20000.0, 5000.0), "-100", false),
        s("Fog of war", "terrain.inibin", Inibin, "FoW*fadeFinish", "Fade end height", "", Float(-20000.0, 5000.0), "-15000", false),
        s("Fog of war", "terrain.inibin", Inibin, "FoW*edgeEnhancement", "Edge enhancement", "", Float(0.0, 1.0), "0", false),
        s("Fog of war", "terrain.inibin", Inibin, "FoW*edgeTintPoint", "Edge tint", "", Float(0.0, 1.0), "0", false),
        s("Clouds", "clouds.inibin", Inibin, "Clouds*Texture", "Cloud shadow texture", "A texture of the UI package, NONE for no cloud shadows", Text, "Cloud2.tga", false),
        s("Clouds", "clouds.inibin", Inibin, "Clouds*Tile", "Cloud size", "Tiling distance in units", Float(100.0, 50000.0), "12000", false),
        s("Clouds", "clouds.inibin", Inibin, "Clouds*SpeedU", "Cloud speed X", "", Float(-0.05, 0.05), "0.0043", false),
        s("Clouds", "clouds.inibin", Inibin, "Clouds*SpeedV", "Cloud speed Z", "", Float(-0.05, 0.05), "0.002", false),
        // Picture.
        s("Picture", "Gamma.inibin", Inibin, "Gamma*Level", "Level", "", Float(0.5, 2.0), "1", false),
        s("Picture", "Gamma.inibin", Inibin, "Gamma*Gamma", "Gamma", "", Float(0.5, 2.0), "1", false),
        s("Picture", "Gamma.inibin", Inibin, "Gamma*Brightness", "Brightness", "", Float(0.5, 2.0), "1", false),
        s("Picture", "Gamma.inibin", Inibin, "Gamma*Contrast", "Contrast", "", Float(0.5, 2.0), "1", false),
        s("Picture", "Graphics.inibin", Inibin, "TextureQuality*World", "Ground texture sharpness", "Mip bias: lower is sharper", Float(-3.0, 1.0), "-0.5", false),
        s("Picture", "terrain.inibin", Inibin, "AlphaTest*cut_off", "Leaf cut-off", "Alpha below this is see-through (leaves, grass)", Int(0, 255), "1", false),
        // Shadows.
        s("Shadows", "terrain.inibin", Inibin, "Environment*ShadowColor", "Shadow colour", "", Color, "125 135 185", false),
        s("Shadows", "terrain.inibin", Inibin, "Environment*ShadowBlur", "Soft shadows", "", Switch, "1", false),
        s("Shadows", "ShadowSettings.inibin", Inibin, "Shadow*ShadowBias", "Shadow bias", "", Float(0.0, 0.05), "0.006", false),
        s("Shadows", "ShadowSettings.inibin", Inibin, "Shadow*ShadowFadeRange", "Shadow fade range", "", Float(0.0, 2000.0), "100", false),
        s("Shadows", "ShadowSettings.inibin", Inibin, "Shadow*ShadowFadeMinY", "Shadow fade height", "", Float(-2000.0, 500.0), "-300", false),
        // Brush grass.
        s("Brush grass", "Grass.ini", NameValue, "SeeThroughMinAlpha", "See-through when inside", "How transparent brush is around your champion", Float(0.0, 1.0), ".15", false),
        s("Brush grass", "Grass.ini", NameValue, "SeeThroughMaxAlpha", "Opacity elsewhere", "", Float(0.0, 1.0), "1", false),
        s("Brush grass", "Grass.ini", NameValue, "SeeThroughRangeScale", "See-through range", "", Float(0.0, 10.0), "3", false),
        s("Brush grass", "Grass.ini", NameValue, "SwayingRangeScale", "Sway range", "How far grass bends around units", Float(0.0, 10.0), "2.0", false),
        s("Brush grass", "Grass.ini", NameValue, "DriveMode", "Sway motion", "", Choice(&["CubicThenPDClamped", "Cubic", "PDClamped"]), "CubicThenPDClamped", false),
        s("Brush grass", "Grass.ini", NameValue, "AccelerationCapForCubicDriver", "Sway acceleration", "", Float(0.0, 200.0), "30", false),
        s("Brush grass", "Grass.ini", NameValue, "FrequencyForPDClampedDriver", "Sway spring", "", Float(0.0, 10.0), "1.0", false),
        s("Brush grass", "Grass.ini", NameValue, "DampingForPDClampedDriver", "Sway damping", "", Float(0.0, 5.0), "0.5", false),
        // Sound and atmosphere mode.
        s("Sound & mood", "Audio.inibin", Inibin, "Default*ThemeMusic", "Theme music event", "", Text, "", false),
        s("Sound & mood", "Audio.inibin", Inibin, "Default*AmbientEvent", "Ambience event", "", Text, "", false),
        s("Sound & mood", "AtmosphereMutators.inibin", Inibin, "Atmosphere*Mutator1", "Atmosphere variant", "Name of an alternate sun set (sun_<name>.inibin), e.g. NightmareSRVisual", Text, "None", false),
        // Camera.
        s("Camera", "Constants.var", Var, "cam_MinX", "Camera min X", "How far the camera may scroll", Float(-5000.0, 30000.0), "300", false),
        s("Camera", "Constants.var", Var, "cam_MinY", "Camera min Z", "", Float(-5000.0, 30000.0), "300", false),
        s("Camera", "Constants.var", Var, "cam_MaxX", "Camera max X", "", Float(0.0, 40000.0), "14000", false),
        s("Camera", "Constants.var", Var, "cam_MaxY", "Camera max Z", "", Float(0.0, 40000.0), "14200", false),
        s("Camera", "Constants.var", Var, "cam_FarZ", "Draw distance", "", Float(1000.0, 30000.0), "5000", false),
        // Economy and rules (server).
        s("Gold & regen", "Constants.var", Var, "ai_StartingGold", "Starting gold", "", Float(0.0, 100000.0), "475", true),
        s("Gold & regen", "Constants.var", Var, "ai_AmbientGoldAmount", "Passive gold per tick", "", Float(0.0, 1000.0), "9.5", true),
        s("Gold & regen", "Constants.var", Var, "ai_AmbientGoldInterval", "Passive gold interval (s)", "", Float(0.1, 60.0), "5", true),
        s("Gold & regen", "Constants.var", Var, "ai_AmbientGoldDelay", "Passive gold starts at (s)", "", Float(0.0, 1200.0), "90", true),
        s("Gold & regen", "Constants.var", Var, "ai_ExpRadius2", "Experience range²", "", Float(0.0, 10000000.0), "1600", true),
        s("Gold & regen", "Constants.var", Var, "ai_GoldRadius2", "Gold share range²", "", Float(0.0, 10000000.0), "1000", true),
        s("Gold & regen", "Constants.var", Var, "sp_HealthRegenPercent", "Fountain health regen", "Fraction of max health per second", Float(0.0, 1.0), "0.085", true),
        s("Gold & regen", "Constants.var", Var, "sp_ManaRegenPercent", "Fountain mana regen", "", Float(0.0, 1.0), "0.085", true),
        s("Gold & regen", "Constants.var", Var, "sp_RegenRadius", "Fountain radius", "", Float(0.0, 5000.0), "1100", true),
        s("Respawn", "DeathTimes.inibin", Inibin, "DeathTimeScaling*StartTime", "Longer respawns from (s)", "", Int(0, 7200), "1500", true),
        s("Respawn", "DeathTimes.inibin", Inibin, "DeathTimeScaling*IncrementTime", "Every (s)", "", Int(1, 7200), "60", true),
        s("Respawn", "DeathTimes.inibin", Inibin, "DeathTimeScaling*PercentIncrease", "By (fraction)", "", Float(0.0, 1.0), "0.02", true),
        s("Respawn", "DeathTimes.inibin", Inibin, "DeathTimeScaling*PercentCap", "Up to (×)", "", Float(1.0, 5.0), "1.5", true),
        s("Respawn", "DeathTimes.inibin", Inibin, "DeathWaveRespawn*WaveRespawnInterval", "Respawn in waves every (s)", "0 = everyone respawns on their own timer", Float(0.0, 120.0), "0", true),
    ];
    for lvl in 1..=18 {
        v.push(Spec {
            group: "Respawn time per level",
            file: "DeathTimes.inibin",
            store: Store::Inibin,
            key: format!("TimeDeadPerLevel*Level{lvl:02}"),
            label: format!("Level {lvl}"),
            help: "Seconds dead at this level",
            kind: Kind::Float(0.0, 300.0),
            default: format!("{}", lvl as f32 * 2.5 + 5.0),
            server: true,
        });
    }
    let mut total = 0;
    for lvl in 2..=18 {
        total += 100 * lvl - 80;
        v.push(Spec {
            group: "Experience per level",
            file: "ExpCurve.inibin",
            store: Store::Inibin,
            key: format!("EXP*Level{lvl}"),
            label: format!("Level {lvl}"),
            help: "Total experience to reach this level",
            kind: Kind::Float(0.0, 200000.0),
            default: format!("{total}"),
            server: true,
        });
    }
    v
}

/// The current value of a setting in a built or opened map (None when not set there).
pub fn read(p: &MapProject, sp: &Spec) -> Option<String> {
    let f = p.file(sp.file)?;
    let bytes = f.bytes().ok()?;
    match sp.store {
        Store::Inibin => {
            let (sec, key) = sp.key.split_once('*')?;
            let ini = Inibin::parse(&bytes).ok()?;
            ini.get(hash_key(sec, key)).map(|v| v.to_text())
        }
        Store::NameValue => {
            let t = String::from_utf8_lossy(&bytes);
            t.lines().find_map(|l| {
                let mut it = l.split_whitespace();
                (it.next()?.eq_ignore_ascii_case(&sp.key)).then(|| it.next().unwrap_or("").to_string())
            })
        }
        Store::Var => {
            let t = String::from_utf8_lossy(&bytes);
            t.lines().find_map(|l| {
                let (a, b) = l.split_once('=')?;
                (a.trim() == sp.key).then(|| b.split(';').next().unwrap_or("").trim().to_string())
            })
        }
    }
}

fn new_value(kind: Kind, text: &str) -> Option<Value> {
    Some(match kind {
        Kind::Float(..) => Value::F32(text.trim().parse().ok()?),
        Kind::Int(..) | Kind::Switch => Value::I32(text.trim().parse::<f32>().ok()? as i32),
        Kind::Color | Kind::Text | Kind::Choice(_) => Value::Str(text.to_string()),
    })
}

/// Writes the user's settings into the map's files (creating files that do not exist yet).
pub fn apply(p: &mut MapProject, settings: &BTreeMap<String, String>) {
    if settings.is_empty() {
        return;
    }
    let all = specs();
    let mut by_file: BTreeMap<&str, Vec<(&Spec, &String)>> = BTreeMap::new();
    for sp in &all {
        if let Some(v) = settings.get(&sp.id()) {
            by_file.entry(sp.file).or_default().push((sp, v));
        }
    }
    for (file, items) in by_file {
        let path = p.files.iter().find(|f| f.path.eq_ignore_ascii_case(file)).map(|f| f.path.clone()).unwrap_or_else(|| file.to_string());
        let old = p.file(&path).and_then(|f| f.bytes().ok());
        let out: Vec<u8> = match items[0].0.store {
            Store::Inibin => {
                let mut ini = old.as_deref().and_then(|b| Inibin::parse(b).ok()).unwrap_or_default();
                for (sp, v) in items {
                    let Some((sec, key)) = sp.key.split_once('*') else { continue };
                    let h = hash_key(sec, key);
                    // Keep the type the map already stores the value in.
                    let val = match ini.get(h).map(|x| x.kind()) {
                        Some(k) if k != ValueKind::Str || matches!(sp.kind, Kind::Text | Kind::Color | Kind::Choice(_)) => Value::parse_text(k, v).ok().or_else(|| new_value(sp.kind, v)),
                        Some(_) => Some(Value::Str(v.clone())),
                        None => new_value(sp.kind, v),
                    };
                    if let Some(val) = val {
                        ini.set(h, val);
                    }
                }
                match ini.write() {
                    Ok(b) => b,
                    Err(_) => continue,
                }
            }
            Store::NameValue => {
                let mut lines: Vec<String> = old.as_deref().map(|b| String::from_utf8_lossy(b).lines().map(str::to_string).collect()).unwrap_or_default();
                for (sp, v) in items {
                    let line = format!("{} {}", sp.key, v);
                    match lines.iter_mut().find(|l| l.split_whitespace().next().is_some_and(|n| n.eq_ignore_ascii_case(&sp.key))) {
                        Some(l) => *l = line,
                        None => lines.push(line),
                    }
                }
                (lines.join("\r\n") + "\r\n").into_bytes()
            }
            Store::Var => {
                let mut lines: Vec<String> = old.as_deref().map(|b| String::from_utf8_lossy(b).lines().map(str::to_string).collect()).unwrap_or_default();
                for (sp, v) in items {
                    let hit = lines.iter_mut().find(|l| l.split_once('=').is_some_and(|(a, _)| a.trim() == sp.key));
                    match hit {
                        Some(l) => {
                            let comment = l.split_once(';').map(|x| format!(" ;{}", x.1)).unwrap_or_default();
                            *l = format!("{:<44}= {:>45}{comment}", sp.key, v);
                        }
                        None => lines.push(format!("{:<44}= {:>45} ; set in LoL Map Studio", sp.key, v)),
                    }
                }
                (lines.join("\r\n") + "\r\n").into_bytes()
            }
        };
        p.add_or_replace(MapFile::generated(&path, out));
    }
}
