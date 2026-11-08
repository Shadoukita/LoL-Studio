//! Where the game files are. Nothing from the game ships with the project: people copy their own
//! `DATA` and `LEVELS` folders into `game/` (next to the program or the project), or point
//! `LMS_GAME_DIR` at a folder holding them. An older layout, an `Example Maps` folder with the
//! map folders and `DATA` side by side, is found too.

use std::path::{Path, PathBuf};

fn is_game_dir(p: &Path) -> bool {
    p.join("DATA").is_dir() || p.join("LEVELS").is_dir()
}

/// Folders to look in: the program's folder and its parents, then the working folder and its parents.
fn bases() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut up = |start: Option<PathBuf>| {
        let mut d = start;
        while let Some(p) = d {
            out.push(p.clone());
            d = p.parent().map(|x| x.to_path_buf());
        }
    };
    up(std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())));
    up(std::env::current_dir().ok());
    out
}

/// The folder holding the game's `DATA` and `LEVELS` (or the older `Example Maps` folder).
pub fn game_root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LMS_GAME_DIR").map(PathBuf::from).filter(|p| p.is_dir()) {
        return Some(p);
    }
    let bases = bases();
    bases.iter().map(|b| b.join("game")).find(|p| is_game_dir(p)).or_else(|| bases.iter().map(|b| b.join("Example Maps")).find(|p| p.is_dir()))
}

/// The folder holding the map folders (`Map1`, `map11`, collections such as `Season1/Map8`).
pub fn levels_dir() -> Option<PathBuf> {
    let root = game_root()?;
    let lv = root.join("LEVELS");
    Some(if lv.is_dir() { lv } else { root })
}

/// The game's `DATA` folder (characters, particles, HUD).
pub fn data_dir() -> Option<PathBuf> {
    game_root().map(|r| r.join("DATA")).filter(|d| d.is_dir())
}

/// A map folder by name (e.g. "Map1", "Season1/Map8"); the path even if it does not exist.
pub fn map_dir(name: &str) -> PathBuf {
    levels_dir().unwrap_or_else(|| PathBuf::from("game").join("LEVELS")).join(name)
}
