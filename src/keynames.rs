//! Names for inibin key hashes. The game only stores hashes of "Section*Key"; names come from the
//! settings the engine reads and from the map's own .ini files.

use crate::formats::inibin::hash_key;
use std::collections::HashMap;

static BUILTIN: &str = include_str!("data/keynames.txt");

#[derive(Default)]
pub struct KeyNames {
    names: HashMap<u32, String>,
}

impl KeyNames {
    pub fn new() -> Self {
        let mut k = Self::default();
        for line in BUILTIN.lines() {
            if let Some((s, key)) = line.split_once('*') {
                k.add(s.trim(), key.trim());
            }
        }
        k
    }

    pub fn add(&mut self, section: &str, key: &str) {
        self.names.entry(hash_key(section, key)).or_insert_with(|| format!("{section} / {key}"));
    }

    /// Learns the section/key pairs of a text ini/cfg file.
    pub fn learn_ini(&mut self, text: &str) {
        let mut section = String::new();
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with('[') {
                if let Some(end) = l.find(']') {
                    section = l[1..end].to_string();
                }
            } else if let Some((k, _)) = l.split_once('=') {
                let k = k.trim();
                if !k.is_empty() && !k.starts_with(';') && !section.is_empty() {
                    self.add(&section, k);
                }
            }
        }
    }

    pub fn name(&self, hash: u32) -> Option<&str> {
        self.names.get(&hash).map(|s| s.as_str())
    }
}
