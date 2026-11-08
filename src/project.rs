//! A map folder (`LEVELS/MapN`) held in memory. Every file keeps its original bytes; files that
//! are edited are re-encoded on export, all others are written back byte for byte.

use crate::formats::{aimesh::AiMesh, inibin::Inibin, mob::MapObjects, navgrid::NavGrid, nvr::Nvr, scb::{Sco, Scb}, wgeo::Wgeo};
use anyhow::{Context, Result, bail};
use rayon::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    Nvr,
    Wgeo,
    NavGrid,
    AiMesh,
    Mob,
    Scb,
    Sco,
    Inibin,
    Dds,
    Lua,
    Text,
    Binary,
}

impl FileKind {
    pub fn of(path: &str) -> Self {
        let lower = path.to_ascii_lowercase();
        let ext = lower.rsplit_once('.').map_or("", |x| x.1);
        match ext {
            "nvr" => Self::Nvr,
            "wgeo" => Self::Wgeo,
            "aimesh_ngrid" => Self::NavGrid,
            "aimesh" => Self::AiMesh,
            "mob" => Self::Mob,
            "scb" => Self::Scb,
            "sco" => Self::Sco,
            "inibin" | "cfgbin" | "troybin" => Self::Inibin,
            "dds" => Self::Dds,
            "luaobj" => Self::Lua,
            "dat" | "ini" | "cfg" | "mat" | "dsc" | "var" | "json" | "txt" | "troy" | "lua" | "xml" => Self::Text,
            _ => Self::Binary,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Nvr => "Environment mesh (NVR)",
            Self::Wgeo => "Baked environment (WGEO)",
            Self::NavGrid => "Navigation grid",
            Self::AiMesh => "AI mesh",
            Self::Mob => "Map objects",
            Self::Scb => "Object mesh (binary)",
            Self::Sco => "Object mesh (text)",
            Self::Inibin => "Binary settings",
            Self::Dds => "Texture",
            Self::Lua => "Compiled script",
            Self::Text => "Text",
            Self::Binary => "Binary",
        }
    }
}

#[derive(Clone)]
pub enum Doc {
    Raw,
    Text(String),
    Inibin(Inibin),
    Mob(MapObjects),
    Nvr(Box<Nvr>),
    Wgeo(Box<Wgeo>),
    NavGrid(Box<NavGrid>),
    AiMesh(AiMesh),
    Scb(Scb),
    Sco(Sco),
}

#[derive(Clone)]
pub struct MapFile {
    /// Path inside the map folder, with forward slashes, in its original case.
    pub path: String,
    pub kind: FileKind,
    pub original: Arc<Vec<u8>>,
    pub doc: Doc,
    pub dirty: bool,
    pub parse_error: Option<String>,
}

impl MapFile {
    pub fn new(path: &str, data: Vec<u8>) -> Self {
        let kind = FileKind::of(path);
        let (doc, parse_error) = match parse_doc(kind, &data) {
            Ok(d) => (d, None),
            Err(e) => (Doc::Raw, Some(format!("{e:#}"))),
        };
        Self { path: path.to_string(), kind, original: Arc::new(data), doc, dirty: false, parse_error }
    }

    /// A file created by the editor (it is always written).
    pub fn generated(path: &str, data: Vec<u8>) -> Self {
        let mut f = Self::new(path, data);
        f.dirty = true;
        f
    }

    pub fn bytes(&self) -> Result<Vec<u8>> {
        if !self.dirty {
            return Ok(self.original.as_ref().clone());
        }
        encode_doc(&self.doc, &self.original).with_context(|| format!("encoding {}", self.path))
    }

    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn stem(&self) -> &str {
        let n = self.file_name();
        n.rsplit_once('.').map_or(n, |x| x.0)
    }
}

pub fn parse_doc(kind: FileKind, data: &[u8]) -> Result<Doc> {
    Ok(match kind {
        FileKind::Nvr => Doc::Nvr(Box::new(Nvr::parse(data)?)),
        FileKind::Wgeo => Doc::Wgeo(Box::new(Wgeo::parse(data)?)),
        FileKind::NavGrid => Doc::NavGrid(Box::new(NavGrid::parse(data.to_vec())?)),
        FileKind::AiMesh => Doc::AiMesh(AiMesh::parse(data)?),
        FileKind::Mob => Doc::Mob(MapObjects::parse(data)?),
        FileKind::Scb => Doc::Scb(Scb::parse(data)?),
        FileKind::Sco => Doc::Sco(Sco::parse(data)?),
        FileKind::Inibin => Doc::Inibin(Inibin::parse(data)?),
        FileKind::Text => Doc::Text(crate::formats::bin::latin1_decode(data)),
        FileKind::Dds | FileKind::Lua | FileKind::Binary => Doc::Raw,
    })
}

fn encode_doc(doc: &Doc, original: &[u8]) -> Result<Vec<u8>> {
    Ok(match doc {
        Doc::Raw => original.to_vec(),
        Doc::Text(s) => crate::formats::bin::latin1_encode(s),
        Doc::Inibin(d) => d.write()?,
        Doc::Mob(d) => d.write()?,
        Doc::Nvr(d) => d.write()?,
        Doc::Wgeo(d) => d.write()?,
        Doc::NavGrid(d) => d.write(),
        Doc::AiMesh(d) => d.write(),
        Doc::Scb(d) => d.write()?,
        Doc::Sco(d) => d.write(),
    })
}

/// One line of `room.dsc`: an object file and the world quality it needs (-100 = always).
#[derive(Clone, Debug, PartialEq)]
pub struct DscEntry {
    pub file: String,
    pub quality: i32,
}

pub fn parse_dsc(text: &str) -> Vec<DscEntry> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let file = it.next()?.to_string();
            let quality = it.next().and_then(|q| q.parse().ok()).unwrap_or(0);
            Some(DscEntry { file, quality })
        })
        .collect()
}

pub fn write_dsc(entries: &[DscEntry]) -> String {
    entries.iter().map(|e| format!("{} {}\r\n", e.file, e.quality)).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneFormat {
    /// `room.nvr` + `room.mat` (Map1 and older maps).
    Legacy,
    /// `room.wgeo` + `terrain.inibin` Renderer=baked (map11).
    Baked,
}

#[derive(Clone)]
pub struct MapProject {
    pub source: Option<PathBuf>,
    /// Folder name, e.g. "Map1".
    pub name: String,
    pub files: Vec<MapFile>,
    index: HashMap<String, usize>,
}

pub const TERRAIN_RENDERER_KEY: (&str, &str) = ("Environment", "Renderer");

impl MapProject {
    /// A copy holding only the files `keep` selects (for background work on part of a map).
    pub fn subset(&self, keep: impl Fn(&MapFile) -> bool) -> Self {
        let mut p = Self { source: self.source.clone(), name: self.name.clone(), files: self.files.iter().filter(|f| keep(f)).cloned().collect(), index: HashMap::new() };
        p.sort();
        p
    }

    pub fn empty(name: &str) -> Self {
        Self { source: None, name: name.to_string(), files: Vec::new(), index: HashMap::new() }
    }

    pub fn open(dir: &Path) -> Result<Self> {
        if !dir.is_dir() {
            bail!("{} is not a folder", dir.display());
        }
        let mut paths = Vec::new();
        collect(dir, dir, &mut paths)?;
        if paths.is_empty() {
            bail!("{} is empty", dir.display());
        }
        let files: Vec<MapFile> = paths
            .par_iter()
            .map(|rel| {
                let data = std::fs::read(dir.join(rel)).with_context(|| format!("reading {rel}"))?;
                Ok(MapFile::new(rel, data))
            })
            .collect::<Result<_>>()?;
        let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Map".into());
        let mut p = Self { source: Some(dir.to_path_buf()), name, files, index: HashMap::new() };
        p.sort();
        Ok(p)
    }

    fn sort(&mut self) {
        self.files.sort_by(|a, b| a.path.to_ascii_lowercase().cmp(&b.path.to_ascii_lowercase()));
        self.index = self.files.iter().enumerate().map(|(i, f)| (f.path.to_ascii_lowercase(), i)).collect();
    }

    /// Finds a file case-insensitively (the game's package lookups ignore case on Windows).
    pub fn find(&self, path: &str) -> Option<usize> {
        self.index.get(&path.replace('\\', "/").to_ascii_lowercase()).copied()
    }

    pub fn file(&self, path: &str) -> Option<&MapFile> {
        self.find(path).map(|i| &self.files[i])
    }

    pub fn file_mut(&mut self, path: &str) -> Option<&mut MapFile> {
        self.find(path).map(|i| &mut self.files[i])
    }

    pub fn add_or_replace(&mut self, f: MapFile) -> usize {
        if let Some(i) = self.find(&f.path) {
            let keep_case = self.files[i].path.clone();
            self.files[i] = MapFile { path: keep_case, ..f };
            self.files[i].dirty = true;
            return i;
        }
        let path = f.path.clone();
        self.files.push(f);
        self.sort();
        self.find(&path).unwrap()
    }

    pub fn remove(&mut self, path: &str) -> bool {
        match self.find(path) {
            Some(i) => {
                self.files.remove(i);
                self.sort();
                true
            }
            None => false,
        }
    }

    pub fn dirty_count(&self) -> usize {
        self.files.iter().filter(|f| f.dirty).count()
    }

    pub fn scene_format(&self) -> Option<SceneFormat> {
        let baked = self.renderer_setting().is_some_and(|r| r.eq_ignore_ascii_case("baked"));
        match (self.find("Scene/room.nvr").is_some(), self.find("Scene/room.wgeo").is_some()) {
            (_, true) if baked => Some(SceneFormat::Baked),
            (true, _) => Some(SceneFormat::Legacy),
            (false, true) => Some(SceneFormat::Baked),
            _ => None,
        }
    }

    /// `[Environment] Renderer` from terrain.inibin ("legacy" when absent, as the game assumes).
    pub fn renderer_setting(&self) -> Option<String> {
        let f = self.file("terrain.inibin")?;
        let Doc::Inibin(ini) = &f.doc else { return None };
        let key = crate::formats::inibin::hash_key(TERRAIN_RENDERER_KEY.0, TERRAIN_RENDERER_KEY.1);
        match ini.get(key) {
            Some(crate::formats::inibin::Value::Str(s)) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn mob(&self) -> Option<&MapObjects> {
        match &self.file("Scene/MapObjects.mob")?.doc {
            Doc::Mob(m) => Some(m),
            _ => None,
        }
    }

    pub fn mob_mut(&mut self) -> Option<&mut MapObjects> {
        let f = self.file_mut("Scene/MapObjects.mob")?;
        f.dirty = true;
        match &mut f.doc {
            Doc::Mob(m) => Some(m),
            _ => None,
        }
    }

    pub fn dsc_entries(&self) -> Vec<DscEntry> {
        match self.file("Scene/room.dsc").map(|f| &f.doc) {
            Some(Doc::Text(t)) => parse_dsc(t),
            _ => Vec::new(),
        }
    }

    pub fn set_dsc_entries(&mut self, entries: &[DscEntry]) {
        let text = write_dsc(entries);
        match self.file_mut("Scene/room.dsc") {
            Some(f) => {
                f.doc = Doc::Text(text);
                f.dirty = true;
            }
            None => {
                self.add_or_replace(MapFile::generated("Scene/room.dsc", crate::formats::bin::latin1_encode(&text)));
            }
        }
    }

    pub fn navgrid(&self) -> Option<&NavGrid> {
        match &self.file("AIPath.aimesh_ngrid")?.doc {
            Doc::NavGrid(g) => Some(g),
            _ => None,
        }
    }

    pub fn navgrid_mut(&mut self) -> Option<&mut NavGrid> {
        let f = self.file_mut("AIPath.aimesh_ngrid")?;
        match &mut f.doc {
            Doc::NavGrid(g) => {
                f.dirty = true;
                Some(g)
            }
            _ => None,
        }
    }

    pub fn text(&self, path: &str) -> Option<&str> {
        match &self.file(path)?.doc {
            Doc::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn set_text(&mut self, path: &str, text: String) {
        match self.file_mut(path) {
            Some(f) => {
                f.doc = Doc::Text(text);
                f.dirty = true;
            }
            None => {
                self.add_or_replace(MapFile::generated(path, crate::formats::bin::latin1_encode(&text)));
            }
        }
    }

    /// Writes the whole map into `dest`. It is assembled in a sibling folder first, so a failure
    /// leaves any existing `dest` untouched; an existing `dest` is replaced only when `replace`.
    pub fn export(&self, dest: &Path, replace: bool) -> Result<ExportReport> {
        if dest.exists() && !replace {
            bail!("{} already exists", dest.display());
        }
        let encoded: Vec<(String, Vec<u8>)> = self
            .files
            .par_iter()
            .map(|f| Ok((f.path.clone(), f.bytes()?)))
            .collect::<Result<_>>()?;
        let parent = dest.parent().context("the export folder has no parent")?;
        std::fs::create_dir_all(parent)?;
        let stage = parent.join(format!(".{}.export-tmp", dest.file_name().unwrap().to_string_lossy()));
        if stage.exists() {
            std::fs::remove_dir_all(&stage)?;
        }
        let mut report = ExportReport::default();
        for (rel, data) in &encoded {
            let target = stage.join(rel);
            std::fs::create_dir_all(target.parent().unwrap())?;
            std::fs::write(&target, data).with_context(|| format!("writing {rel}"))?;
            report.files += 1;
            report.bytes += data.len() as u64;
        }
        report.rewritten = self.files.iter().filter(|f| f.dirty).map(|f| f.path.clone()).collect();
        if dest.exists() {
            let old = parent.join(format!(".{}.export-old", dest.file_name().unwrap().to_string_lossy()));
            if old.exists() {
                std::fs::remove_dir_all(&old)?;
            }
            std::fs::rename(dest, &old).with_context(|| format!("moving the old {} aside", dest.display()))?;
            std::fs::rename(&stage, dest)?;
            std::fs::remove_dir_all(&old)?;
        } else {
            std::fs::rename(&stage, dest)?;
        }
        Ok(report)
    }
}

#[derive(Default, Debug)]
pub struct ExportReport {
    pub files: usize,
    pub bytes: u64,
    pub rewritten: Vec<String>,
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out)?;
        } else {
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}
