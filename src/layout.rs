//! A custom map as you paint it: a grid of surfaces (ground, wall, brush, river, deep water,
//! void), ground texture weights and heights, plus objectives and minion lanes. Saved as a
//! `.lmsmap` project; `layout_build` turns it into a complete LEVELS/MapN folder.

use base64::Engine;
use serde::{Deserialize, Serialize};

pub const CELL: f32 = 25.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Surface {
    Void = 0,
    Ground = 1,
    Wall = 2,
    Brush = 3,
    River = 4,
    DeepWater = 5,
}

impl Surface {
    pub const ALL: [Surface; 6] = [Surface::Ground, Surface::Wall, Surface::Brush, Surface::River, Surface::DeepWater, Surface::Void];
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Ground,
            2 => Self::Wall,
            3 => Self::Brush,
            4 => Self::River,
            5 => Self::DeepWater,
            _ => Self::Void,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Void => "Empty (off the map)",
            Self::Ground => "Ground",
            Self::Wall => "Wall / cliff",
            Self::Brush => "Brush",
            Self::River => "River (walkable water)",
            Self::DeepWater => "Deep water",
        }
    }
    pub fn walkable(self) -> bool {
        matches!(self, Self::Ground | Self::Brush | Self::River)
    }
    /// Colour on the painting canvas.
    pub fn color(self) -> [u8; 3] {
        match self {
            Self::Void => [12, 12, 16],
            Self::Ground => [96, 132, 74],
            Self::Wall => [92, 84, 78],
            Self::Brush => [34, 104, 40],
            Self::River => [62, 128, 150],
            Self::DeepWater => [30, 70, 110],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum Lane {
    Top,
    Mid,
    Bot,
}

impl Lane {
    pub const ALL: [Lane; 3] = [Lane::Top, Lane::Mid, Lane::Bot];
    /// Letter used in object names (Riot's: L = top, C = mid, R = bottom).
    pub fn letter(self) -> char {
        match self {
            Lane::Top => 'L',
            Lane::Mid => 'C',
            Lane::Bot => 'R',
        }
    }
    /// Lane index the scripts use (CreateChildTurret: 0 = bottom, 1 = mid, 2 = top).
    pub fn index(self) -> u8 {
        match self {
            Lane::Bot => 0,
            Lane::Mid => 1,
            Lane::Top => 2,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Lane::Top => "Top",
            Lane::Mid => "Mid",
            Lane::Bot => "Bottom",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ObjKind {
    Nexus,
    Inhibitor,
    Turret,
    /// Where champions start and respawn.
    SpawnPoint,
    /// The fountain's shrine turret.
    FountainTurret,
    Shop,
    /// Where a lane's minion waves start (barracks spawn point).
    MinionSpawn,
    /// Older projects: spawn, shrine turret and shop in one (split when loaded).
    Fountain,
    // Jungle camps (NeutralMinionSpawn): team is the buff side; Dragon and Baron are neutral.
    BlueBuff,
    RedBuff,
    Wolves,
    Wraiths,
    Golems,
    Wight,
    Dragon,
    Baron,
    // Dominion and ARAM objects (spawned by the server; exported as markers and camps).
    HealthRelic,
    CapturePoint,
    SpeedShrine,
    CenterRelic,
    Crystal,
}

impl ObjKind {
    pub const ALL: [ObjKind; 7] = [ObjKind::Nexus, ObjKind::Inhibitor, ObjKind::Turret, ObjKind::SpawnPoint, ObjKind::FountainTurret, ObjKind::Shop, ObjKind::MinionSpawn];
    pub const CAMPS: [ObjKind; 8] = [ObjKind::BlueBuff, ObjKind::RedBuff, ObjKind::Wolves, ObjKind::Wraiths, ObjKind::Golems, ObjKind::Wight, ObjKind::Dragon, ObjKind::Baron];
    pub const MODES: [ObjKind; 5] = [ObjKind::HealthRelic, ObjKind::CapturePoint, ObjKind::SpeedShrine, ObjKind::CenterRelic, ObjKind::Crystal];
    /// Dominion / ARAM objects.
    pub fn is_mode_object(self) -> bool {
        Self::MODES.contains(&self)
    }
    pub fn is_camp(self) -> bool {
        Self::CAMPS.contains(&self)
    }
    /// Neutral camps that exist once (no buff side, not mirrored).
    pub fn is_epic(self) -> bool {
        matches!(self, ObjKind::Dragon | ObjKind::Baron | ObjKind::CenterRelic)
    }
    pub fn label(self) -> &'static str {
        match self {
            ObjKind::Nexus => "Nexus",
            ObjKind::Inhibitor => "Inhibitor",
            ObjKind::Turret => "Turret",
            ObjKind::SpawnPoint => "Spawn point",
            ObjKind::FountainTurret => "Fountain turret",
            ObjKind::Shop => "Shop",
            ObjKind::MinionSpawn => "Minion spawn",
            ObjKind::Fountain => "Fountain",
            ObjKind::BlueBuff => "Blue buff (Ancient Golem)",
            ObjKind::RedBuff => "Red buff (Lizard Elder)",
            ObjKind::Wolves => "Wolves",
            ObjKind::Wraiths => "Wraiths",
            ObjKind::Golems => "Golems",
            ObjKind::Wight => "Wight (Great Wraith)",
            ObjKind::Dragon => "Dragon",
            ObjKind::Baron => "Baron Nashor",
            ObjKind::HealthRelic => "Health relic (ARAM)",
            ObjKind::CapturePoint => "Capture point (Dominion)",
            ObjKind::SpeedShrine => "Speed shrine",
            ObjKind::CenterRelic => "Center relic (Dominion)",
            ObjKind::Crystal => "Nexus crystal (Dominion)",
        }
    }
    pub fn has_lane(self) -> bool {
        matches!(self, ObjKind::Inhibitor | ObjKind::Turret | ObjKind::MinionSpawn)
    }
    /// Footprint radius kept walkable-free of walls and drawn on the canvas.
    pub fn radius(self) -> f32 {
        match self {
            ObjKind::Nexus => 360.0,
            ObjKind::Inhibitor => 220.0,
            ObjKind::Turret => 120.0,
            ObjKind::Fountain | ObjKind::SpawnPoint => 280.0,
            ObjKind::FountainTurret => 120.0,
            ObjKind::Shop => 180.0,
            ObjKind::MinionSpawn => 60.0,
            ObjKind::Dragon | ObjKind::Baron => 300.0,
            ObjKind::BlueBuff | ObjKind::RedBuff | ObjKind::Wraiths => 200.0,
            ObjKind::Wolves | ObjKind::Golems => 180.0,
            ObjKind::Wight => 120.0,
            ObjKind::HealthRelic | ObjKind::SpeedShrine | ObjKind::CenterRelic => 110.0,
            ObjKind::CapturePoint => 160.0,
            ObjKind::Crystal => 200.0,
        }
    }
}

/// Summoner's Rift's 14 camps (Map1 NeutralMinionSpawn: first spawn point of each camp).
pub const SR_CAMPS: [(ObjKind, u8, [f32; 2]); 14] = [
    (ObjKind::BlueBuff, 1, [3632.7, 7600.4]),
    (ObjKind::Wolves, 1, [3373.7, 6223.3]),
    (ObjKind::Wraiths, 1, [6300.1, 5300.1]),
    (ObjKind::RedBuff, 1, [7455.6, 3890.2]),
    (ObjKind::Golems, 1, [7916.8, 2534.0]),
    (ObjKind::Dragon, 0, [9459.5, 4193.0]),
    (ObjKind::BlueBuff, 2, [10386.6, 6811.1]),
    (ObjKind::Wolves, 2, [10651.5, 8116.4]),
    (ObjKind::Wraiths, 2, [7580.4, 9250.4]),
    (ObjKind::RedBuff, 2, [6504.2, 10584.6]),
    (ObjKind::Golems, 2, [5810.5, 11925.5]),
    (ObjKind::Baron, 0, [4600.5, 10250.5]),
    (ObjKind::Wight, 1, [1684.0, 8207.0]),
    (ObjKind::Wight, 2, [12337.0, 6263.0]),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurretTier {
    Outer,
    Inner,
    Inhibitor,
    NexusA,
    NexusB,
}

impl TurretTier {
    pub const ALL: [TurretTier; 5] = [TurretTier::Outer, TurretTier::Inner, TurretTier::Inhibitor, TurretTier::NexusA, TurretTier::NexusB];
    /// Tier number the scripts use (3 outer, 2 inner, 1 inhibitor, 5/4 nexus, 0 fountain).
    pub fn from_code(c: u8) -> Self {
        match c {
            2 => TurretTier::Inner,
            1 => TurretTier::Inhibitor,
            5 => TurretTier::NexusA,
            4 => TurretTier::NexusB,
            _ => TurretTier::Outer,
        }
    }
    pub fn code(self) -> u8 {
        match self {
            TurretTier::Outer => 3,
            TurretTier::Inner => 2,
            TurretTier::Inhibitor => 1,
            TurretTier::NexusA => 5,
            TurretTier::NexusB => 4,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            TurretTier::Outer => "Outer",
            TurretTier::Inner => "Inner",
            TurretTier::Inhibitor => "Inhibitor",
            TurretTier::NexusA => "Nexus (1)",
            TurretTier::NexusB => "Nexus (2)",
        }
    }
    /// The character skin the game uses for this tier.
    pub fn skin(self, team: u8) -> &'static str {
        match (team, self) {
            (1, TurretTier::Outer) => "OrderTurretNormal",
            (1, TurretTier::Inner) => "OrderTurretNormal2",
            (1, TurretTier::Inhibitor) => "OrderTurretDragon",
            (1, _) => "OrderTurretAngel",
            (_, TurretTier::Outer) => "ChaosTurretWorm",
            (_, TurretTier::Inner) => "ChaosTurretWorm2",
            (_, TurretTier::Inhibitor) => "ChaosTurretGiant",
            (_, _) => "ChaosTurretNormal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Objective {
    pub id: u32,
    pub kind: ObjKind,
    /// 1 = blue (Order), 2 = red (Chaos).
    pub team: u8,
    pub lane: Lane,
    pub tier: TurretTier,
    pub pos: [f32; 2],
    /// Facing in degrees (0 = the game's default).
    pub yaw: f32,
    /// The mirrored copy on the other team, kept in sync when symmetry is on.
    #[serde(default)]
    pub mirror: Option<u32>,
    /// Object name kept from an imported map (otherwise named by kind, team, lane and tier).
    #[serde(default)]
    pub name: Option<String>,
    /// Character skin kept from an imported map (turrets).
    #[serde(default)]
    pub skin: Option<String>,
}

/// An opened map being edited: its folder (scene art, textures and rules are kept) and what was
/// imported from it, so the export only rewrites the parts that changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaseMap {
    pub path: String,
    pub objectives: Vec<Objective>,
    pub lanes: Vec<LanePath>,
    pub effects: Vec<EffectPlace>,
    pub lights: Vec<LightPlace>,
    /// The map's own jungle script could not be read, so it is kept and camps are not exported.
    #[serde(default)]
    pub keep_camps: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LanePath {
    pub lane: Lane,
    /// From the blue base to the red base.
    pub points: Vec<[f32; 2]>,
}

/// A particle effect placed on the map (Particles.dat), e.g. "Fire_Cauldron.troy".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectPlace {
    pub id: u32,
    pub name: String,
    pub pos: [f32; 2],
    /// Height above the ground.
    #[serde(default)]
    pub lift: f32,
    #[serde(default)]
    pub yaw: f32,
}

/// A point light (Light_Env.dat): colour, inner and outer radius, intensity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightPlace {
    pub id: u32,
    pub pos: [f32; 2],
    #[serde(default)]
    pub lift: f32,
    pub color: [u8; 3],
    pub inner: f32,
    pub outer: f32,
    pub intensity: f32,
}

/// A placed scenery prop (tree, rock, ...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PropInstance {
    pub id: u32,
    /// Library key ("Map1/Tree01 #2" or "custom:<name>").
    pub prop: String,
    pub pos: [f32; 2],
    #[serde(default)]
    pub lift: f32,
    pub yaw: f32,
    pub scale: f32,
    /// Tilt around X and Z in degrees (applied before the turn).
    #[serde(default)]
    pub tilt: [f32; 2],
    /// Per-axis stretch on top of the uniform size.
    #[serde(default = "one3")]
    pub stretch: [f32; 3],
    /// Decoration only: units walk through it (no collision on the navigation grid).
    #[serde(default)]
    pub walk_through: bool,
}

fn one3() -> [f32; 3] {
    [1.0; 3]
}

/// A prop you imported: an OBJ model and its texture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomProp {
    pub name: String,
    pub obj: String,
    pub texture: String,
    #[serde(default = "one")]
    pub scale: f32,
}

fn one() -> f32 {
    1.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Symmetry {
    None,
    /// Summoner's Rift style: the red base is the blue base reflected across the other diagonal.
    Diagonal,
    /// Rotated half a turn around the centre.
    Point,
    MirrorX,
    MirrorZ,
}

impl Symmetry {
    pub const ALL: [Symmetry; 5] = [Symmetry::None, Symmetry::Diagonal, Symmetry::Point, Symmetry::MirrorX, Symmetry::MirrorZ];
    pub fn label(self) -> &'static str {
        match self {
            Symmetry::None => "No symmetry",
            Symmetry::Diagonal => "Diagonal (like Summoner's Rift)",
            Symmetry::Point => "Rotational (half turn)",
            Symmetry::MirrorX => "Mirror left / right",
            Symmetry::MirrorZ => "Mirror top / bottom",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rules {
    /// The standard 5v5 rules (minion waves, items, experience) copied from a rules map.
    Classic,
    /// Generated scripts only: structures and spawns, no minion waves.
    Sandbox,
}

pub const MAX_LAYERS: usize = 16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GroundLayer {
    pub name: String,
    /// "gen:grass" (built in) or a texture file (.dds, .png, .jpg).
    pub texture: String,
}

/// Trees on wall tops ("jungle walls"), placed when the map is built.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Forest {
    pub enabled: bool,
    /// Props to choose from (library keys).
    pub props: Vec<String>,
    /// Average distance between trees.
    pub spacing: f32,
    pub scale: [f32; 2],
    /// How far from a wall's edge trees start (keeps cliff faces visible).
    pub inset: f32,
    /// League walls: wall areas stay at ground level and are dressed with 3D rocks and trees.
    #[serde(default)]
    pub league_walls: bool,
    /// Rocks lining the wall edges (library keys; empty = Summoner's Rift's big rocks).
    #[serde(default)]
    pub edge_props: Vec<String>,
    #[serde(default = "edge_spacing")]
    pub edge_spacing: f32,
    /// Dress walls automatically when the map is built (off after "Bake into props").
    #[serde(default = "yes")]
    pub auto_dress: bool,
    /// Edge rock size range (across, in units).
    #[serde(default = "edge_size")]
    pub edge_size: [f32; 2],
    /// How far inside the wall edge rocks sit, as a fraction of their size.
    #[serde(default = "edge_inset")]
    pub edge_inset: f32,
    /// Turn edge models along the wall outline.
    #[serde(default = "yes")]
    pub edge_align: bool,
    /// Share of edge spots that get a tree instead of a rock (1 = classic: trees all round).
    #[serde(default = "edge_tree_share")]
    pub edge_tree_share: f32,
    /// Height of the mossy mound in the middle of a wall (League style).
    #[serde(default = "mound")]
    pub mound: f32,
    /// Texture of the mound (empty = classic Summoner's Rift's foliage, next to the cliff texture).
    #[serde(default)]
    pub mound_texture: String,
}

fn mound() -> f32 {
    90.0
}

fn yes() -> bool {
    true
}

fn edge_size() -> [f32; 2] {
    [200.0, 320.0]
}

fn edge_inset() -> f32 {
    0.3
}

fn edge_tree_share() -> f32 {
    1.0
}

fn edge_spacing() -> f32 {
    150.0
}

impl Default for Forest {
    fn default() -> Self {
        Self { enabled: true, props: Vec::new(), spacing: 340.0, scale: [0.85, 1.25], inset: 170.0, league_walls: true, edge_props: Vec::new(), edge_spacing: edge_spacing(), auto_dress: true, edge_size: edge_size(), edge_inset: edge_inset(), edge_align: true, edge_tree_share: edge_tree_share(), mound: mound(), mound_texture: String::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    /// Ground texture layers (layer 0 is the base). Any number up to 16.
    #[serde(default)]
    pub ground: Vec<GroundLayer>,
    /// Older projects: four fixed layers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layer_names: Vec<String>,
    #[serde(default)]
    pub forest: Forest,
    pub cliff: String,
    pub brush: String,
    /// World units per texture repeat.
    pub tiling: f32,
    pub base_height: f32,
    pub wall_height: f32,
    pub river_depth: f32,
    pub sun_dir: [f32; 3],
    pub sun_color: [f32; 3],
    pub ambient: [f32; 3],
    /// Deep water is lava (a glowing surface instead of the water plane), e.g. Magma Chamber.
    #[serde(default)]
    pub lava: bool,
    /// Lava only from this x onward (west of it deep water stays water): one volcanic side.
    #[serde(default)]
    pub lava_from_x: Option<f32>,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            ground: [("Grass", "gen:grass"), ("Dirt", "gen:dirt"), ("Stone path", "gen:path"), ("Sand", "gen:sand")].iter().map(|(n, t)| GroundLayer { name: n.to_string(), texture: t.to_string() }).collect(),
            layers: Vec::new(),
            layer_names: Vec::new(),
            forest: Forest::default(),
            cliff: "gen:rock".into(),
            brush: "gen:brush".into(),
            tiling: 512.0,
            base_height: 50.0,
            wall_height: 300.0,
            river_depth: 110.0,
            sun_dir: [0.3822, -0.9227, -0.0499],
            sun_color: [137.0, 149.0, 180.0],
            ambient: [19.0, 92.0, 194.0],
            lava: false,
            lava_from_x: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub name: String,
    pub map_id: u32,
    pub nx: usize,
    pub ny: usize,
    pub surface: Vec<u8>,
    /// Ground layer weights per cell (0..255, summing to 255).
    pub paint: Vec<[u8; MAX_LAYERS]>,
    /// Height offset per cell (sculpting).
    pub height: Vec<f32>,
    pub theme: Theme,
    pub objectives: Vec<Objective>,
    pub lanes: Vec<LanePath>,
    pub symmetry: Symmetry,
    pub rules: Rules,
    pub next_id: u32,
    pub props: Vec<PropInstance>,
    pub custom_props: Vec<CustomProp>,
    pub effects: Vec<EffectPlace>,
    pub lights: Vec<LightPlace>,
    /// Set when this layout edits an opened map instead of generating one.
    pub base: Option<BaseMap>,
    /// Look and behaviour settings ("file|Section*Key" -> value), see map_settings.
    pub settings: std::collections::BTreeMap<String, String>,
    /// Ids of locked objects (cannot be selected in the viewport).
    pub locked: Vec<u32>,
    /// The maps whose scenery this map draws from (folder names, e.g. "Map1" for the classic
    /// Summoner's Rift look); empty = every map. Your own models are always available.
    pub asset_maps: Vec<String>,
}

/// Classic Summoner's Rift (Map1) scenery, the default for new maps and older projects.
pub fn classic_assets() -> Vec<String> {
    vec!["Map1".into()]
}

#[derive(Serialize, Deserialize)]
struct LayoutFile {
    format: u32,
    name: String,
    map_id: u32,
    nx: usize,
    ny: usize,
    cell: f32,
    surface: String,
    paint: String,
    height: String,
    theme: Theme,
    objectives: Vec<Objective>,
    lanes: Vec<LanePath>,
    symmetry: Symmetry,
    rules: Rules,
    next_id: u32,
    #[serde(default)]
    props: Vec<PropInstance>,
    #[serde(default)]
    custom_props: Vec<CustomProp>,
    #[serde(default)]
    effects: Vec<EffectPlace>,
    #[serde(default)]
    lights: Vec<LightPlace>,
    #[serde(default)]
    base: Option<BaseMap>,
    #[serde(default)]
    settings: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    locked: Vec<u32>,
    #[serde(default = "classic_assets")]
    asset_maps: Vec<String>,
}

fn base_weights() -> [u8; MAX_LAYERS] {
    let mut w = [0u8; MAX_LAYERS];
    w[0] = 255;
    w
}

impl Layout {
    /// A blank canvas of ground with a wall border.
    pub fn new(name: &str, map_id: u32, size: [f32; 2]) -> Self {
        let nx = (size[0] / CELL).ceil() as usize;
        let ny = (size[1] / CELL).ceil() as usize;
        let mut l = Self {
            name: name.into(),
            map_id,
            nx,
            ny,
            surface: vec![Surface::Ground as u8; nx * ny],
            paint: vec![base_weights(); nx * ny],
            height: vec![0.0; nx * ny],
            theme: Theme::default(),
            objectives: Vec::new(),
            lanes: Vec::new(),
            symmetry: Symmetry::Diagonal,
            rules: Rules::Classic,
            next_id: 1,
            props: Vec::new(),
            custom_props: Vec::new(),
            effects: Vec::new(),
            lights: Vec::new(),
            base: None,
            settings: Default::default(),
            locked: Vec::new(),
            asset_maps: classic_assets(),
        };
        let border = (300.0 / CELL) as usize;
        for j in 0..ny {
            for i in 0..nx {
                if i < border || j < border || i + border >= nx || j + border >= ny {
                    l.surface[j * nx + i] = Surface::Wall as u8;
                }
            }
        }
        l
    }

    pub fn size(&self) -> [f32; 2] {
        [self.nx as f32 * CELL, self.ny as f32 * CELL]
    }

    pub fn surface_at(&self, i: usize, j: usize) -> Surface {
        Surface::from_u8(self.surface[j * self.nx + i])
    }

    pub fn cell_of(&self, x: f32, z: f32) -> Option<(usize, usize)> {
        let (i, j) = ((x / CELL).floor(), (z / CELL).floor());
        (i >= 0.0 && j >= 0.0 && (i as usize) < self.nx && (j as usize) < self.ny).then(|| (i as usize, j as usize))
    }

    /// The symmetric copies of a point (the point itself first).
    pub fn mirrored(&self, p: [f32; 2]) -> Vec<[f32; 2]> {
        let [w, d] = self.size();
        let m = match self.symmetry {
            Symmetry::None => return vec![p],
            Symmetry::Diagonal => [w - p[1] * w / d, d - p[0] * d / w],
            Symmetry::Point => [w - p[0], d - p[1]],
            Symmetry::MirrorX => [w - p[0], p[1]],
            Symmetry::MirrorZ => [p[0], d - p[1]],
        };
        vec![p, m]
    }

    /// The mirror of one point (or itself without symmetry).
    pub fn mirror_point(&self, p: [f32; 2]) -> [f32; 2] {
        *self.mirrored(p).last().unwrap()
    }

    /// Applies `f` to every cell within `radius` of (x, z) and its symmetric copies.
    /// `f` gets the cell index and a 0..1 falloff (1 at the centre).
    pub fn brush(&mut self, x: f32, z: f32, radius: f32, square: bool, mut f: impl FnMut(&mut Layout, usize, f32)) {
        let mut done = std::collections::HashSet::new();
        for c in self.mirrored([x, z]) {
            let r = (radius / CELL).ceil() as i64 + 1;
            let (ci, cj) = ((c[0] / CELL) as i64, (c[1] / CELL) as i64);
            for dj in -r..=r {
                for di in -r..=r {
                    let (i, j) = (ci + di, cj + dj);
                    if i < 0 || j < 0 || i as usize >= self.nx || j as usize >= self.ny {
                        continue;
                    }
                    let (px, pz) = ((i as f32 + 0.5) * CELL, (j as f32 + 0.5) * CELL);
                    let d = if square { (px - c[0]).abs().max((pz - c[1]).abs()) } else { (px - c[0]).hypot(pz - c[1]) };
                    if d <= radius && done.insert((i, j)) {
                        let k = j as usize * self.nx + i as usize;
                        f(self, k, 1.0 - d / radius.max(1.0));
                    }
                }
            }
        }
    }

    pub fn paint_surface(&mut self, x: f32, z: f32, radius: f32, square: bool, s: Surface) {
        self.brush(x, z, radius, square, |l, k, _| l.surface[k] = s as u8);
    }

    /// Fills the polygon (and its symmetric copy) with a surface.
    pub fn fill_polygon(&mut self, poly: &[[f32; 2]], s: Surface) {
        if poly.len() < 3 {
            return;
        }
        let polys: Vec<Vec<[f32; 2]>> = if self.symmetry == Symmetry::None { vec![poly.to_vec()] } else { vec![poly.to_vec(), poly.iter().map(|p| self.mirror_point(*p)).collect()] };
        for poly in polys {
            let (x0, x1) = poly.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
            let (z0, z1) = poly.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
            let (i0, i1) = (((x0 / CELL).floor().max(0.0)) as usize, ((x1 / CELL).ceil() as usize).min(self.nx));
            let (j0, j1) = (((z0 / CELL).floor().max(0.0)) as usize, ((z1 / CELL).ceil() as usize).min(self.ny));
            for j in j0..j1 {
                for i in i0..i1 {
                    let (px, pz) = ((i as f32 + 0.5) * CELL, (j as f32 + 0.5) * CELL);
                    if point_in_polygon(&poly, px, pz) {
                        self.surface[j * self.nx + i] = s as u8;
                    }
                }
            }
        }
    }

    /// Flood-fills the connected area of the same surface (and its mirror).
    pub fn flood_fill(&mut self, x: f32, z: f32, s: Surface) {
        for p in self.mirrored([x, z]) {
            let Some((i, j)) = self.cell_of(p[0], p[1]) else { continue };
            let from = self.surface[j * self.nx + i];
            if from == s as u8 {
                continue;
            }
            let mut stack = vec![(i, j)];
            while let Some((i, j)) = stack.pop() {
                let k = j * self.nx + i;
                if self.surface[k] != from {
                    continue;
                }
                self.surface[k] = s as u8;
                if i > 0 {
                    stack.push((i - 1, j));
                }
                if j > 0 {
                    stack.push((i, j - 1));
                }
                if i + 1 < self.nx {
                    stack.push((i + 1, j));
                }
                if j + 1 < self.ny {
                    stack.push((i, j + 1));
                }
            }
        }
    }

    /// Paints a ground texture layer: its weight grows, the others shrink.
    pub fn paint_texture(&mut self, x: f32, z: f32, radius: f32, layer: usize, strength: f32) {
        let layer = layer.min(MAX_LAYERS - 1);
        self.brush(x, z, radius, false, |l, k, fall| {
            let a = (strength * (0.3 + 0.7 * fall)).clamp(0.0, 1.0);
            set_weight(&mut l.paint[k], layer, a);
        });
    }

    /// The dominant ground layer of a cell.
    pub fn layer_at(&self, k: usize) -> usize {
        let w = &self.paint[k];
        (0..MAX_LAYERS).max_by_key(|&i| w[i]).unwrap_or(0)
    }

    /// Removes a ground layer; its paint goes back to the base layer.
    pub fn remove_layer(&mut self, layer: usize) {
        if layer == 0 || layer >= self.theme.ground.len() {
            return;
        }
        self.theme.ground.remove(layer);
        for w in &mut self.paint {
            w[0] = w[0].saturating_add(w[layer]);
            for i in layer..MAX_LAYERS - 1 {
                w[i] = w[i + 1];
            }
            w[MAX_LAYERS - 1] = 0;
        }
    }

    pub fn add_effect(&mut self, name: &str, pos: [f32; 2], lift: f32, yaw: f32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.effects.push(EffectPlace { id, name: name.to_string(), pos, lift, yaw });
        id
    }

    pub fn add_light(&mut self, pos: [f32; 2], lift: f32, color: [u8; 3], inner: f32, outer: f32, intensity: f32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.lights.push(LightPlace { id, pos, lift, color, inner, outer, intensity });
        id
    }

    pub fn add_prop(&mut self, prop: &str, pos: [f32; 2], yaw: f32, scale: f32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.props.push(PropInstance { id, prop: prop.into(), pos, lift: 0.0, yaw, scale, tilt: [0.0; 2], stretch: [1.0; 3], walk_through: false });
        id
    }

    /// Scatters props in a circle (and its symmetric copy): about one per `spacing` units.
    pub fn scatter_props(&mut self, props: &[String], x: f32, z: f32, radius: f32, spacing: f32, scale: [f32; 2], seed: u32) {
        if props.is_empty() {
            return;
        }
        let n = ((std::f32::consts::PI * radius * radius) / (spacing * spacing)).round().max(1.0) as u32;
        for c in self.mirrored([x, z]) {
            for i in 0..n {
                let r = radius * crate::layout_build::hash01(seed, i, 1).sqrt();
                let a = std::f32::consts::TAU * crate::layout_build::hash01(seed, i, 2);
                let p = [c[0] + r * a.cos(), c[1] + r * a.sin()];
                let too_close = self.props.iter().any(|q| (q.pos[0] - p[0]).hypot(q.pos[1] - p[1]) < spacing * 0.55);
                if too_close {
                    continue;
                }
                let pick = &props[(crate::layout_build::hash01(seed, i, 3) * props.len() as f32) as usize % props.len()];
                let s = scale[0] + (scale[1] - scale[0]) * crate::layout_build::hash01(seed, i, 4);
                self.add_prop(pick, p, 360.0 * crate::layout_build::hash01(seed, i, 5), s);
            }
        }
    }

    /// Removes props within a circle (and its symmetric copy).
    pub fn erase_props(&mut self, x: f32, z: f32, radius: f32) {
        let centres = self.mirrored([x, z]);
        self.props.retain(|p| centres.iter().all(|c| (p.pos[0] - c[0]).hypot(p.pos[1] - c[1]) > radius));
    }

    /// Places spawn point, fountain turret and shop together (a fountain).
    pub fn add_fountain(&mut self, team: u8, pos: [f32; 2]) -> u32 {
        let [w, d] = self.size();
        let to_c = |p: [f32; 2], dist: f32, side: f32| {
            let (dx, dz) = (w / 2.0 - p[0], d / 2.0 - p[1]);
            let l = dx.hypot(dz).max(1.0);
            let (ux, uz) = (dx / l, dz / l);
            [p[0] + ux * dist - uz * side, p[1] + uz * dist + ux * side]
        };
        let id = self.add_objective(ObjKind::SpawnPoint, team, Lane::Mid, TurretTier::Outer, pos);
        self.add_objective(ObjKind::FountainTurret, team, Lane::Mid, TurretTier::Outer, to_c(pos, 260.0, 0.0));
        self.add_objective(ObjKind::Shop, team, Lane::Mid, TurretTier::Outer, to_c(pos, -120.0, 300.0));
        id
    }

    pub fn sculpt(&mut self, x: f32, z: f32, radius: f32, amount: f32, mode: SculptMode, target: f32) {
        let snapshot = if mode == SculptMode::Smooth { self.height.clone() } else { Vec::new() };
        let nx = self.nx;
        let ny = self.ny;
        self.brush(x, z, radius, false, |l, k, fall| {
            let h = l.height[k];
            l.height[k] = match mode {
                SculptMode::Raise => h + amount * fall,
                SculptMode::Lower => h - amount * fall,
                SculptMode::Flatten => h + (target - h) * (0.3 + 0.7 * fall),
                SculptMode::Noise => {
                    let (i, j) = ((k % nx) as f32, (k / nx) as f32);
                    h + (crate::layout_build::noise2(i * CELL / 250.0, j * CELL / 250.0, 77) - 0.5) * 2.0 * amount * fall
                }
                SculptMode::Ramp => h,
                SculptMode::Smooth => {
                    let (i, j) = ((k % nx) as i64, (k / nx) as i64);
                    let (mut s, mut c) = (0.0, 0.0);
                    for dj in -2..=2i64 {
                        for di in -2..=2i64 {
                            let (ii, jj) = (i + di, j + dj);
                            if ii >= 0 && jj >= 0 && (ii as usize) < nx && (jj as usize) < ny {
                                s += snapshot[jj as usize * nx + ii as usize];
                                c += 1.0;
                            }
                        }
                    }
                    h + (s / c - h) * fall
                }
            };
        });
    }

    pub fn add_objective(&mut self, kind: ObjKind, team: u8, lane: Lane, tier: TurretTier, pos: [f32; 2]) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let team = if kind.is_epic() { 0 } else { team };
        self.objectives.push(Objective { id, kind, team, lane, tier, pos, yaw: 0.0, mirror: None, name: None, skin: None });
        if self.symmetry != Symmetry::None && !kind.is_epic() {
            let mid = self.next_id;
            self.next_id += 1;
            let mp = self.mirror_point(pos);
            self.objectives.push(Objective { id: mid, kind, team: 3 - team, lane, tier, pos: mp, yaw: 0.0, mirror: Some(id), name: None, skin: None });
            self.objectives.iter_mut().find(|o| o.id == id).unwrap().mirror = Some(mid);
        }
        id
    }

    pub fn objective(&self, id: u32) -> Option<&Objective> {
        self.objectives.iter().find(|o| o.id == id)
    }

    /// Moves an objective; its mirrored partner follows when symmetry is on.
    pub fn move_objective(&mut self, id: u32, pos: [f32; 2]) {
        let mp = self.mirror_point(pos);
        let mut partner = None;
        if let Some(o) = self.objectives.iter_mut().find(|o| o.id == id) {
            o.pos = pos;
            partner = o.mirror;
        }
        if self.symmetry != Symmetry::None
            && let Some(m) = partner
            && let Some(o) = self.objectives.iter_mut().find(|o| o.id == m)
        {
            o.pos = mp;
        }
    }

    pub fn remove_objective(&mut self, id: u32) {
        let partner = self.objective(id).and_then(|o| o.mirror);
        self.objectives.retain(|o| o.id != id && Some(o.id) != partner);
    }

    /// Lane path with its mirror: the red half of a symmetric lane is the reversed mirror.
    pub fn set_lane(&mut self, lane: Lane, points: Vec<[f32; 2]>) {
        self.lanes.retain(|l| l.lane != lane);
        self.lanes.push(LanePath { lane, points });
    }

    /// Lays out the standard structures for each lane along its path (blue side, mirrored).
    pub fn auto_structures(&mut self) {
        let lanes = self.lanes.clone();
        self.objectives.retain(|o| !matches!(o.kind, ObjKind::Turret | ObjKind::Inhibitor | ObjKind::MinionSpawn));
        let sym = self.symmetry;
        if sym == Symmetry::None {
            self.symmetry = Symmetry::Diagonal;
        }
        for lp in &lanes {
            let n = lp.points.len();
            if n < 2 {
                continue;
            }
            let at = |t: f32| point_along(&lp.points, t);
            self.add_objective(ObjKind::MinionSpawn, 1, lp.lane, TurretTier::Outer, at(0.03));
            self.add_objective(ObjKind::Inhibitor, 1, lp.lane, TurretTier::Outer, at(0.08));
            self.add_objective(ObjKind::Turret, 1, lp.lane, TurretTier::Inhibitor, at(0.13));
            self.add_objective(ObjKind::Turret, 1, lp.lane, TurretTier::Inner, at(0.25));
            self.add_objective(ObjKind::Turret, 1, lp.lane, TurretTier::Outer, at(0.38));
        }
        self.symmetry = sym;
    }

    pub fn save(&self) -> anyhow::Result<String> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let paint: Vec<u8> = self.paint.iter().flat_map(|w| w.iter().copied()).collect();
        let mut theme = self.theme.clone();
        theme.layers.clear();
        theme.layer_names.clear();
        let height: Vec<u8> = self.height.iter().flat_map(|h| h.to_le_bytes()).collect();
        let f = LayoutFile {
            format: 2,
            name: self.name.clone(),
            map_id: self.map_id,
            nx: self.nx,
            ny: self.ny,
            cell: CELL,
            surface: b64.encode(&self.surface),
            paint: b64.encode(paint),
            height: b64.encode(height),
            theme,
            objectives: self.objectives.clone(),
            lanes: self.lanes.clone(),
            symmetry: self.symmetry,
            rules: self.rules,
            next_id: self.next_id,
            props: self.props.clone(),
            custom_props: self.custom_props.clone(),
            effects: self.effects.clone(),
            lights: self.lights.clone(),
            base: self.base.clone(),
            settings: self.settings.clone(),
            locked: self.locked.clone(),
            asset_maps: self.asset_maps.clone(),
        };
        Ok(serde_json::to_string_pretty(&f)?)
    }

    pub fn load(text: &str) -> anyhow::Result<Self> {
        let f: LayoutFile = serde_json::from_str(text)?;
        let b64 = base64::engine::general_purpose::STANDARD;
        let n = f.nx * f.ny;
        let surface = b64.decode(&f.surface)?;
        let paint_raw = b64.decode(&f.paint)?;
        let height_raw = b64.decode(&f.height)?;
        let per = if f.format >= 2 { MAX_LAYERS } else { 3 };
        if surface.len() != n || paint_raw.len() != n * per || height_raw.len() != n * 4 {
            anyhow::bail!("the project's grids do not match its size");
        }
        let paint: Vec<[u8; MAX_LAYERS]> = if per == MAX_LAYERS {
            paint_raw.chunks_exact(MAX_LAYERS).map(|c| c.try_into().unwrap()).collect()
        } else {
            // Format 1 stored a lerp chain (r, g, b over layer 0); convert it to weights.
            paint_raw
                .chunks_exact(3)
                .map(|c| {
                    let (r, g, b) = (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
                    let w3 = b;
                    let w2 = g * (1.0 - b);
                    let w1 = r * (1.0 - g) * (1.0 - b);
                    let mut w = [0u8; MAX_LAYERS];
                    w[1] = (w1 * 255.0).round() as u8;
                    w[2] = (w2 * 255.0).round() as u8;
                    w[3] = (w3 * 255.0).round() as u8;
                    w[0] = 255u8.saturating_sub(w[1].saturating_add(w[2]).saturating_add(w[3]));
                    w
                })
                .collect()
        };
        let mut theme = f.theme;
        if theme.ground.is_empty() {
            let names = if theme.layer_names.len() == theme.layers.len() { theme.layer_names.clone() } else { theme.layers.iter().enumerate().map(|(i, _)| format!("Layer {}", i + 1)).collect() };
            theme.ground = theme.layers.iter().zip(names).map(|(t, n)| GroundLayer { name: n, texture: t.clone() }).collect();
            theme.layers.clear();
            theme.layer_names.clear();
        }
        if theme.ground.is_empty() {
            theme.ground = Theme::default().ground;
        }
        // Older fountains become a spawn point, fountain turret and shop.
        let mut objectives = Vec::new();
        let mut next_id = f.next_id;
        for o in f.objectives {
            if o.kind == ObjKind::Fountain {
                let mut spawn = o.clone();
                spawn.kind = ObjKind::SpawnPoint;
                spawn.mirror = None;
                objectives.push(spawn);
                for (kind, off) in [(ObjKind::FountainTurret, 260.0f32), (ObjKind::Shop, -320.0)] {
                    let mut x = o.clone();
                    x.id = next_id;
                    next_id += 1;
                    x.kind = kind;
                    x.mirror = None;
                    x.pos = [o.pos[0] + off * 0.7, o.pos[1] + off * 0.7];
                    if o.team == 2 {
                        x.pos = [o.pos[0] - off * 0.7, o.pos[1] - off * 0.7];
                    }
                    objectives.push(x);
                }
            } else {
                objectives.push(o);
            }
        }
        Ok(Self {
            name: f.name,
            map_id: f.map_id,
            nx: f.nx,
            ny: f.ny,
            surface,
            paint,
            height: height_raw.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
            theme,
            objectives,
            lanes: f.lanes,
            symmetry: f.symmetry,
            rules: f.rules,
            next_id,
            props: f.props,
            custom_props: f.custom_props,
            effects: f.effects,
            lights: f.lights,
            base: f.base,
            settings: f.settings,
            locked: f.locked,
            asset_maps: f.asset_maps,
        })
    }

    /// A ready-to-play starting point: three lanes, a river, jungle walls and both bases.
    pub fn classic_start(name: &str, map_id: u32, size: f32) -> Self {
        let mut l = Self::new(name, map_id, [size, size]);
        l.symmetry = Symmetry::Diagonal;
        let s = size;
        // Jungle walls, then carve lanes, bases and the river.
        for j in 0..l.ny {
            for i in 0..l.nx {
                let k = j * l.nx + i;
                if l.surface[k] == Surface::Ground as u8 {
                    let (x, z) = ((i as f32 + 0.5) * CELL / s, (j as f32 + 0.5) * CELL / s);
                    let n = crate::layout_build::noise2(x * 9.0, z * 9.0, 3);
                    if n > 0.56 {
                        l.surface[k] = Surface::Wall as u8;
                    }
                }
            }
        }
        let m = 0.09 * s;
        let top = vec![[m, 0.2 * s], [m, s - m], [0.8 * s, s - m]];
        let bot = vec![[0.2 * s, m], [s - m, m], [s - m, 0.8 * s]];
        let mid = vec![[0.16 * s, 0.16 * s], [0.84 * s, 0.84 * s]];
        for (lane, pts) in [(Lane::Top, top), (Lane::Bot, bot), (Lane::Mid, mid)] {
            l.carve_path(&pts, 420.0, Surface::Ground, Some(2));
            l.set_lane(lane, pts);
        }
        // River across the other diagonal.
        l.carve_path(&[[0.12 * s, 0.88 * s], [0.88 * s, 0.12 * s]], 360.0, Surface::River, None);
        // Bases.
        for c in [[0.12 * s, 0.12 * s], [0.88 * s, 0.88 * s]] {
            l.symmetry = Symmetry::None;
            l.paint_surface(c[0], c[1], 0.15 * s, false, Surface::Ground);
            l.brush(c[0], c[1], 0.14 * s, false, |l, k, _| set_weight(&mut l.paint[k], 2, 1.0));
            l.symmetry = Symmetry::Diagonal;
        }
        // Brush patches beside the lanes.
        for t in [0.3f32, 0.5, 0.7] {
            for lp in l.lanes.clone() {
                let p = point_along(&lp.points, t);
                l.paint_surface(p[0] + 330.0, p[1] - 330.0, 140.0, false, Surface::Brush);
            }
        }
        l.symmetry = Symmetry::Diagonal;
        let blue = [0.07 * s, 0.07 * s];
        l.add_fountain(1, blue);
        l.add_objective(ObjKind::Nexus, 1, Lane::Mid, TurretTier::Outer, [0.14 * s, 0.14 * s]);
        l.add_objective(ObjKind::Turret, 1, Lane::Mid, TurretTier::NexusA, [0.125 * s, 0.155 * s]);
        l.add_objective(ObjKind::Turret, 1, Lane::Mid, TurretTier::NexusB, [0.155 * s, 0.125 * s]);
        l.auto_structures();
        l.classic_jungle();
        l
    }

    /// Summoner's Rift's jungle (Map1 NeutralMinionSpawn camp positions) scaled to this map.
    pub fn classic_jungle(&mut self) {
        let [w, d] = self.size();
        let (kx, kz) = (w / 14870.0, d / 14870.0);
        let sym = self.symmetry;
        self.symmetry = Symmetry::None;
        let camps = SR_CAMPS;
        for (k, t, p) in camps {
            let at = [p[0] * kx, p[1] * kz];
            // A walkable glade around the camp (the terrain may not be Summoner's Rift's).
            let r = if k.is_epic() { 520.0 } else { 380.0 } * kx.min(kz).max(0.6);
            // Only walls give way: rivers and paths through the camp stay.
            let n = (r / CELL).ceil() as i64;
            if let Some((ci, cj)) = self.cell_of(at[0], at[1]) {
                for dj in -n..=n {
                    for di in -n..=n {
                        let (i, j) = (ci as i64 + di, cj as i64 + dj);
                        if i < 0 || j < 0 || i as usize >= self.nx || j as usize >= self.ny || ((di * di + dj * dj) as f32).sqrt() * CELL > r {
                            continue;
                        }
                        let k = j as usize * self.nx + i as usize;
                        if self.surface[k] == Surface::Wall as u8 {
                            self.surface[k] = Surface::Ground as u8;
                        }
                    }
                }
            }
            self.add_objective(k, t, Lane::Mid, TurretTier::Outer, at);
        }
        self.symmetry = sym;
    }

    /// Paints a wide path of `s` along a polyline (texture layer `layer` too, if given).
    pub fn carve_path(&mut self, pts: &[[f32; 2]], width: f32, s: Surface, layer: Option<usize>) {
        let sym = self.symmetry;
        self.symmetry = Symmetry::None;
        for w in pts.windows(2) {
            let len = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
            let steps = (len / (CELL * 2.0)).ceil().max(1.0) as usize;
            for k in 0..=steps {
                let t = k as f32 / steps as f32;
                let (x, z) = (w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t);
                self.paint_surface(x, z, width / 2.0, false, s);
                if let Some(layer) = layer {
                    self.brush(x, z, width * 0.32, false, |l, k, _| set_weight(&mut l.paint[k], layer, 1.0));
                }
            }
        }
        self.symmetry = sym;
    }
}

/// Moves a weight set toward `layer` by `a` (0..1), keeping the sum at 255.
pub fn set_weight(w: &mut [u8; MAX_LAYERS], layer: usize, a: f32) {
    let mut f: Vec<f32> = w.iter().map(|&v| v as f32 / 255.0).collect();
    for (i, v) in f.iter_mut().enumerate() {
        *v = if i == layer { *v + (1.0 - *v) * a } else { *v * (1.0 - a) };
    }
    let sum: f32 = f.iter().sum::<f32>().max(1e-6);
    let mut out = [0u8; MAX_LAYERS];
    let mut total = 0u32;
    for i in 0..MAX_LAYERS {
        out[i] = (f[i] / sum * 255.0).round() as u8;
        total += out[i] as u32;
    }
    // Rounding drift goes to the painted layer.
    let drift = 255i32 - total as i32;
    out[layer] = (out[layer] as i32 + drift).clamp(0, 255) as u8;
    *w = out;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SculptMode {
    Raise,
    Lower,
    Flatten,
    Smooth,
    /// Random bumps (Unreal's Noise).
    Noise,
    /// Drag a ramp between two heights (applied on release).
    Ramp,
}

impl Theme {
    /// Whether deep water at x is lava.
    pub fn lava_at(&self, x: f32) -> bool {
        self.lava && self.lava_from_x.is_none_or(|f| x >= f)
    }
}

pub fn point_in_polygon(poly: &[[f32; 2]], x: f32, z: f32) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > z) != (b[1] > z) && x < (b[0] - a[0]) * (z - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The point a fraction `t` (0..1) of the way along a polyline.
pub fn point_along(pts: &[[f32; 2]], t: f32) -> [f32; 2] {
    let total: f32 = pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
    let mut want = total * t.clamp(0.0, 1.0);
    for w in pts.windows(2) {
        let l = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
        if want <= l || l == 0.0 {
            let f = if l > 0.0 { want / l } else { 0.0 };
            return [w[0][0] + (w[1][0] - w[0][0]) * f, w[0][1] + (w[1][1] - w[0][1]) * f];
        }
        want -= l;
    }
    *pts.last().unwrap()
}

/// Anything on the map that can be selected, moved, copied and deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemRef {
    Prop(u32),
    Objective(u32),
    Effect(u32),
    Light(u32),
}

/// Copied map items, positioned relative to their centre.
#[derive(Clone, Debug, Default)]
pub struct Clipboard {
    pub props: Vec<PropInstance>,
    pub objectives: Vec<Objective>,
    pub effects: Vec<EffectPlace>,
    pub lights: Vec<LightPlace>,
}

impl Clipboard {
    pub fn is_empty(&self) -> bool {
        self.props.is_empty() && self.objectives.is_empty() && self.effects.is_empty() && self.lights.is_empty()
    }
    pub fn len(&self) -> usize {
        self.props.len() + self.objectives.len() + self.effects.len() + self.lights.len()
    }
}

impl Layout {
    pub fn item_pos(&self, r: ItemRef) -> Option<[f32; 2]> {
        match r {
            ItemRef::Prop(id) => self.props.iter().find(|p| p.id == id).map(|p| p.pos),
            ItemRef::Objective(id) => self.objective(id).map(|o| o.pos),
            ItemRef::Effect(id) => self.effects.iter().find(|e| e.id == id).map(|e| e.pos),
            ItemRef::Light(id) => self.lights.iter().find(|l| l.id == id).map(|l| l.pos),
        }
    }

    /// Moves an item by an offset (objectives move without their mirrored partner).
    pub fn shift_item(&mut self, r: ItemRef, d: [f32; 2]) {
        let add = |p: &mut [f32; 2]| {
            p[0] += d[0];
            p[1] += d[1];
        };
        match r {
            ItemRef::Prop(id) => self.props.iter_mut().filter(|p| p.id == id).for_each(|p| add(&mut p.pos)),
            ItemRef::Objective(id) => self.objectives.iter_mut().filter(|o| o.id == id).for_each(|o| add(&mut o.pos)),
            ItemRef::Effect(id) => self.effects.iter_mut().filter(|e| e.id == id).for_each(|e| add(&mut e.pos)),
            ItemRef::Light(id) => self.lights.iter_mut().filter(|l| l.id == id).for_each(|l| add(&mut l.pos)),
        }
    }

    pub fn remove_item(&mut self, r: ItemRef) {
        match r {
            ItemRef::Prop(id) => self.props.retain(|p| p.id != id),
            ItemRef::Objective(id) => self.objectives.retain(|o| o.id != id),
            ItemRef::Effect(id) => self.effects.retain(|e| e.id != id),
            ItemRef::Light(id) => self.lights.retain(|l| l.id != id),
        }
    }

    /// Copies items; positions are stored relative to their common centre.
    pub fn copy_items(&self, items: &[ItemRef]) -> Clipboard {
        let pts: Vec<[f32; 2]> = items.iter().filter_map(|r| self.item_pos(*r)).collect();
        if pts.is_empty() {
            return Clipboard::default();
        }
        let c = [pts.iter().map(|p| p[0]).sum::<f32>() / pts.len() as f32, pts.iter().map(|p| p[1]).sum::<f32>() / pts.len() as f32];
        let rel = |p: [f32; 2]| [p[0] - c[0], p[1] - c[1]];
        let mut cb = Clipboard::default();
        for r in items {
            match *r {
                ItemRef::Prop(id) => cb.props.extend(self.props.iter().filter(|p| p.id == id).map(|p| PropInstance { pos: rel(p.pos), ..p.clone() })),
                ItemRef::Objective(id) => cb.objectives.extend(self.objectives.iter().filter(|o| o.id == id).map(|o| Objective { pos: rel(o.pos), mirror: None, ..o.clone() })),
                ItemRef::Effect(id) => cb.effects.extend(self.effects.iter().filter(|e| e.id == id).map(|e| EffectPlace { pos: rel(e.pos), ..e.clone() })),
                ItemRef::Light(id) => cb.lights.extend(self.lights.iter().filter(|l| l.id == id).map(|l| LightPlace { pos: rel(l.pos), ..l.clone() })),
            }
        }
        cb
    }

    /// Pastes copied items centred at `at`, with new ids (copied objectives lose their imported names).
    pub fn paste(&mut self, cb: &Clipboard, at: [f32; 2]) -> Vec<ItemRef> {
        let mut out = Vec::new();
        let abs = |p: [f32; 2]| [p[0] + at[0], p[1] + at[1]];
        for p in &cb.props {
            let id = self.next_id;
            self.next_id += 1;
            self.props.push(PropInstance { id, pos: abs(p.pos), ..p.clone() });
            out.push(ItemRef::Prop(id));
        }
        for o in &cb.objectives {
            let id = self.next_id;
            self.next_id += 1;
            self.objectives.push(Objective { id, pos: abs(o.pos), mirror: None, name: None, ..o.clone() });
            out.push(ItemRef::Objective(id));
        }
        for e in &cb.effects {
            let id = self.next_id;
            self.next_id += 1;
            self.effects.push(EffectPlace { id, pos: abs(e.pos), ..e.clone() });
            out.push(ItemRef::Effect(id));
        }
        for l in &cb.lights {
            let id = self.next_id;
            self.next_id += 1;
            self.lights.push(LightPlace { id, pos: abs(l.pos), ..l.clone() });
            out.push(ItemRef::Light(id));
        }
        out
    }
}

/// Tilt (X, Z degrees) that stands a prop turned by `yaw` on ground with height gradient
/// (dh/dx, dh/dz): the prop's up axis follows the ground normal.
pub fn slope_tilt(grad: [f32; 2], yaw: f32) -> [f32; 2] {
    let n = [-grad[0], 1.0, -grad[1]];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    let n = [n[0] / l, n[1] / l, n[2] / l];
    // Undo the turn (props::place_vertex turns x' = c x - s z, z' = s x + c z).
    let (s, c) = yaw.to_radians().sin_cos();
    let m = [c * n[0] + s * n[2], n[1], -s * n[0] + c * n[2]];
    [m[2].clamp(-1.0, 1.0).asin().to_degrees(), (-m[0]).atan2(m[1]).to_degrees()]
}

/// Spots along a straight path from a to b, `step` apart, each turned along the path
/// (props::place_vertex convention) plus `turn` degrees.
/// A row along a drawn line: smoothed, then exact steps along it, each turned along the line
/// (same turn convention as `path_spots`).
pub fn line_spots(line: &[[f32; 2]], step: f32, turn: f32) -> Vec<([f32; 2], f32)> {
    if line.len() < 2 {
        return Vec::new();
    }
    // Moving average keeps the ends where they were drawn.
    let mut l = line.to_vec();
    for _ in 0..2 {
        let n = l.len();
        if n < 3 {
            break;
        }
        let mut m = l.clone();
        for k in 1..n - 1 {
            m[k] = [(l[k - 1][0] + 2.0 * l[k][0] + l[k + 1][0]) / 4.0, (l[k - 1][1] + 2.0 * l[k][1] + l[k + 1][1]) / 4.0];
        }
        l = m;
    }
    crate::autodress::resample(&l, step.max(10.0)).into_iter().map(|(p, a)| (p, a - 90.0 + turn)).collect()
}

pub fn path_spots(a: [f32; 2], b: [f32; 2], step: f32, turn: f32) -> Vec<([f32; 2], f32)> {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dz);
    let yaw = -dx.atan2(dz).to_degrees() + turn;
    let n = (len / step.max(10.0)).floor() as usize;
    (0..=n).map(|k| {
        let t = if n == 0 { 0.0 } else { k as f32 / n as f32 };
        ([a[0] + dx * t, a[1] + dz * t], yaw)
    }).collect()
}

impl Layout {
    /// Raises or lowers an item (props, effects, lights).
    pub fn lift_item(&mut self, r: ItemRef, dy: f32) {
        match r {
            ItemRef::Prop(id) => self.props.iter_mut().filter(|p| p.id == id).for_each(|p| p.lift += dy),
            ItemRef::Effect(id) => self.effects.iter_mut().filter(|e| e.id == id).for_each(|e| e.lift += dy),
            ItemRef::Light(id) => self.lights.iter_mut().filter(|l| l.id == id).for_each(|l| l.lift += dy),
            ItemRef::Objective(_) => {}
        }
    }

    /// Turns an item around itself (degrees, in the item's own turn convention).
    pub fn turn_item(&mut self, r: ItemRef, deg: f32) {
        match r {
            ItemRef::Prop(id) => self.props.iter_mut().filter(|p| p.id == id).for_each(|p| p.yaw -= deg),
            ItemRef::Objective(id) => self.objectives.iter_mut().filter(|o| o.id == id).for_each(|o| o.yaw += deg),
            ItemRef::Effect(id) => self.effects.iter_mut().filter(|e| e.id == id).for_each(|e| e.yaw += deg),
            ItemRef::Light(_) => {}
        }
    }
}
