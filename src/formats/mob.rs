//! `Scene/MapObjects.mob`: the map object table (`Riot::MapObjects`, file id "OPAM", version 2).
//! 16-byte header followed by 128-byte `MapObject` records, read by the game with a single fread.

use super::bin::{Reader, Writer, fixed_str};
use anyhow::{Result, bail};

pub const FILE_ID: u32 = 0x4d41504f;
pub const VERSION: u32 = 2;
pub const RECORD_SIZE: usize = 0x80;
/// `MapObject::kMaxNameSize` is 62; AddMapObject copies at most 0x3d characters.
pub const MAX_NAME: usize = 61;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObjectType {
    Barracks,
    SpawnPoint,
    LevelSize,
    Inhibitor,
    Nexus,
    Turret,
    Shop,
    Lake,
    NavPoint,
    InfoPoint,
    LevelProp,
    Other(u8),
}

impl ObjectType {
    pub const ALL: [ObjectType; 11] = [
        ObjectType::Barracks,
        ObjectType::SpawnPoint,
        ObjectType::LevelSize,
        ObjectType::Inhibitor,
        ObjectType::Nexus,
        ObjectType::Turret,
        ObjectType::Shop,
        ObjectType::Lake,
        ObjectType::NavPoint,
        ObjectType::InfoPoint,
        ObjectType::LevelProp,
    ];

    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Barracks,
            1 => Self::SpawnPoint,
            2 => Self::LevelSize,
            3 => Self::Inhibitor,
            4 => Self::Nexus,
            5 => Self::Turret,
            6 => Self::Shop,
            7 => Self::Lake,
            8 => Self::NavPoint,
            9 => Self::InfoPoint,
            10 => Self::LevelProp,
            v => Self::Other(v),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::Barracks => 0,
            Self::SpawnPoint => 1,
            Self::LevelSize => 2,
            Self::Inhibitor => 3,
            Self::Nexus => 4,
            Self::Turret => 5,
            Self::Shop => 6,
            Self::Lake => 7,
            Self::NavPoint => 8,
            Self::InfoPoint => 9,
            Self::LevelProp => 10,
            Self::Other(v) => v,
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Barracks => "Barracks (minion spawn)".into(),
            Self::SpawnPoint => "Spawn point".into(),
            Self::LevelSize => "Level size".into(),
            Self::Inhibitor => "Inhibitor".into(),
            Self::Nexus => "Nexus".into(),
            Self::Turret => "Turret".into(),
            Self::Shop => "Shop".into(),
            Self::Lake => "Lake".into(),
            Self::NavPoint => "Nav point".into(),
            Self::InfoPoint => "Info point".into(),
            Self::LevelProp => "Level prop".into(),
            Self::Other(v) => format!("Type {v}"),
        }
    }

    /// The name prefix the game matches to create the object's class.
    pub fn name_prefix(self) -> &'static str {
        match self {
            Self::Barracks => "__P_Order_Spawn_Barracks__",
            Self::SpawnPoint => "__Spawn_T",
            Self::LevelSize => "__LevelSize_",
            Self::Inhibitor => "Barracks_T",
            Self::Nexus => "HQ_T",
            Self::Turret => "Turret_T",
            Self::Shop => "Shop",
            Self::Lake => "Lake",
            Self::NavPoint => "__NAV_",
            Self::InfoPoint => "Info",
            Self::LevelProp => "LevelProp_",
            Self::Other(_) => "",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapObject {
    pub name: String,
    pub kind: ObjectType,
    pub ignore_collision_on_placement: u8,
    pub position: [f32; 3],
    pub pitch_yaw_roll: [f32; 3],
    pub scale: [f32; 3],
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    pub skin_id: i32,
    /// The original 62-byte name field, kept so unchanged names round-trip exactly.
    name_field: Option<Vec<u8>>,
}

impl MapObject {
    pub fn new(name: &str, kind: ObjectType, position: [f32; 3]) -> Self {
        Self {
            name: name.to_string(),
            kind,
            ignore_collision_on_placement: 0,
            position,
            pitch_yaw_roll: [0.0; 3],
            scale: [1.0; 3],
            bbox_min: [0.0; 3],
            bbox_max: [0.0; 3],
            skin_id: 0,
            name_field: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapObjects {
    pub reserved: u32,
    pub objects: Vec<MapObject>,
}

impl MapObjects {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let id = r.u32()?;
        if id != FILE_ID {
            bail!("not a MapObjects.mob file (id {id:#x})");
        }
        let version = r.u32()?;
        if version != VERSION {
            bail!("MapObjects.mob version {version}; the game only loads version {VERSION}");
        }
        let count = r.u32()? as usize;
        let reserved = r.u32()?;
        if r.remaining() != count * RECORD_SIZE {
            bail!("MapObjects.mob holds {} bytes of records for {count} objects", r.remaining());
        }
        let mut objects = Vec::with_capacity(count);
        for _ in 0..count {
            let field = r.bytes(62)?;
            let kind = ObjectType::from_u8(r.u8()?);
            let ignore = r.u8()?;
            objects.push(MapObject {
                name: fixed_str(field),
                kind,
                ignore_collision_on_placement: ignore,
                position: r.vec3()?,
                pitch_yaw_roll: r.vec3()?,
                scale: r.vec3()?,
                bbox_min: r.vec3()?,
                bbox_max: r.vec3()?,
                skin_id: r.i32()?,
                name_field: Some(field.to_vec()),
            });
        }
        Ok(Self { reserved, objects })
    }

    pub fn write(&self) -> Result<Vec<u8>> {
        let mut w = Writer::with_capacity(16 + self.objects.len() * RECORD_SIZE);
        w.u32(FILE_ID);
        w.u32(VERSION);
        w.u32(self.objects.len() as u32);
        w.u32(self.reserved);
        for o in &self.objects {
            match &o.name_field {
                Some(f) if fixed_str(f) == o.name => w.bytes(f),
                _ => {
                    if o.name.len() > MAX_NAME {
                        bail!("map object name \"{}\" is longer than {MAX_NAME} characters", o.name);
                    }
                    w.fixed_str(&o.name, 62)?;
                }
            }
            w.u8(o.kind.to_u8());
            w.u8(o.ignore_collision_on_placement);
            for v in [o.position, o.pitch_yaw_roll, o.scale, o.bbox_min, o.bbox_max] {
                w.f32s(&v);
            }
            w.i32(o.skin_id);
        }
        Ok(w.data)
    }

    /// The game's files keep the records sorted by name (byte order); new objects go in their place.
    pub fn insert_sorted(&mut self, obj: MapObject) -> usize {
        let at = self
            .objects
            .iter()
            .position(|o| o.name.as_bytes() > obj.name.as_bytes())
            .unwrap_or(self.objects.len());
        self.objects.insert(at, obj);
        at
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.objects.iter().position(|o| o.name == name)
    }

    /// A name that is not yet used, built from `base` and a number.
    pub fn unique_name(&self, base: &str) -> String {
        if self.find(base).is_none() {
            return base.to_string();
        }
        (1..)
            .map(|i| format!("{base}{i}"))
            .find(|n| self.find(n).is_none())
            .unwrap()
    }
}
