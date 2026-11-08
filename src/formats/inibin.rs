//! Binary ini files (`*.inibin`, `*.cfgbin`, version 2). The game reads `Foo.ini` settings from
//! `Foo.inibin`: each value is keyed by a hash of "Section*Key" and grouped into typed sets.

use super::bin::{Reader, Writer};
use anyhow::{Result, bail};

/// The key hash: `h = lower(c) + 65599 * h` over "Section*Key".
pub fn hash_key(section: &str, key: &str) -> u32 {
    let mut h: u32 = 0;
    for c in section.bytes().chain(std::iter::once(b'*')).chain(key.bytes()) {
        h = (c.to_ascii_lowercase() as u32).wrapping_add(h.wrapping_mul(65599));
    }
    h
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    I32 = 0,
    F32 = 1,
    U8Scaled = 2,
    I16 = 3,
    U8 = 4,
    Bool = 5,
    U8x3 = 6,
    F32x3 = 7,
    U8x2 = 8,
    F32x2 = 9,
    U8x4 = 10,
    F32x4 = 11,
    Str = 12,
    I64 = 13,
}

impl ValueKind {
    pub const ALL: [ValueKind; 14] = [
        ValueKind::I32,
        ValueKind::F32,
        ValueKind::U8Scaled,
        ValueKind::I16,
        ValueKind::U8,
        ValueKind::Bool,
        ValueKind::U8x3,
        ValueKind::F32x3,
        ValueKind::U8x2,
        ValueKind::F32x2,
        ValueKind::U8x4,
        ValueKind::F32x4,
        ValueKind::Str,
        ValueKind::I64,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ValueKind::I32 => "int32",
            ValueKind::F32 => "float",
            ValueKind::U8Scaled => "byte/10",
            ValueKind::I16 => "int16",
            ValueKind::U8 => "byte",
            ValueKind::Bool => "bool",
            ValueKind::U8x3 => "3 bytes/10",
            ValueKind::F32x3 => "3 floats",
            ValueKind::U8x2 => "2 bytes/10",
            ValueKind::F32x2 => "2 floats",
            ValueKind::U8x4 => "4 bytes/10",
            ValueKind::F32x4 => "4 floats",
            ValueKind::Str => "string",
            ValueKind::I64 => "int64",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    I32(i32),
    F32(f32),
    U8Scaled(u8),
    I16(i16),
    U8(u8),
    Bool(bool),
    U8x3([u8; 3]),
    F32x3([f32; 3]),
    U8x2([u8; 2]),
    F32x2([f32; 2]),
    U8x4([u8; 4]),
    F32x4([f32; 4]),
    Str(String),
    I64(i64),
}

impl Value {
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::I32(_) => ValueKind::I32,
            Value::F32(_) => ValueKind::F32,
            Value::U8Scaled(_) => ValueKind::U8Scaled,
            Value::I16(_) => ValueKind::I16,
            Value::U8(_) => ValueKind::U8,
            Value::Bool(_) => ValueKind::Bool,
            Value::U8x3(_) => ValueKind::U8x3,
            Value::F32x3(_) => ValueKind::F32x3,
            Value::U8x2(_) => ValueKind::U8x2,
            Value::F32x2(_) => ValueKind::F32x2,
            Value::U8x4(_) => ValueKind::U8x4,
            Value::F32x4(_) => ValueKind::F32x4,
            Value::Str(_) => ValueKind::Str,
            Value::I64(_) => ValueKind::I64,
        }
    }

    pub fn default_of(kind: ValueKind) -> Value {
        match kind {
            ValueKind::I32 => Value::I32(0),
            ValueKind::F32 => Value::F32(0.0),
            ValueKind::U8Scaled => Value::U8Scaled(0),
            ValueKind::I16 => Value::I16(0),
            ValueKind::U8 => Value::U8(0),
            ValueKind::Bool => Value::Bool(false),
            ValueKind::U8x3 => Value::U8x3([0; 3]),
            ValueKind::F32x3 => Value::F32x3([0.0; 3]),
            ValueKind::U8x2 => Value::U8x2([0; 2]),
            ValueKind::F32x2 => Value::F32x2([0.0; 2]),
            ValueKind::U8x4 => Value::U8x4([0; 4]),
            ValueKind::F32x4 => Value::F32x4([0.0; 4]),
            ValueKind::Str => Value::Str(String::new()),
            ValueKind::I64 => Value::I64(0),
        }
    }

    /// Text form used by the inspector; `parse_text` reads it back.
    pub fn to_text(&self) -> String {
        fn join<T: ToString>(v: &[T]) -> String {
            v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" ")
        }
        match self {
            Value::I32(v) => v.to_string(),
            Value::F32(v) => v.to_string(),
            Value::U8Scaled(v) => format!("{}", *v as f32 / 10.0),
            Value::I16(v) => v.to_string(),
            Value::U8(v) => v.to_string(),
            Value::Bool(v) => if *v { "1".into() } else { "0".into() },
            Value::U8x3(v) => join(&v.map(|x| x as f32 / 10.0)),
            Value::F32x3(v) => join(v),
            Value::U8x2(v) => join(&v.map(|x| x as f32 / 10.0)),
            Value::F32x2(v) => join(v),
            Value::U8x4(v) => join(&v.map(|x| x as f32 / 10.0)),
            Value::F32x4(v) => join(v),
            Value::Str(s) => s.clone(),
            Value::I64(v) => v.to_string(),
        }
    }

    pub fn parse_text(kind: ValueKind, text: &str) -> Result<Value> {
        let nums = || -> Result<Vec<f32>> {
            text.split_whitespace().map(|t| t.parse::<f32>().map_err(|e| anyhow::anyhow!("{t}: {e}"))).collect()
        };
        let scaled = |x: f32| -> u8 { (x * 10.0).round().clamp(0.0, 255.0) as u8 };
        let fixed = |n: usize| -> Result<Vec<f32>> {
            let v = nums()?;
            if v.len() != n {
                bail!("expected {n} numbers");
            }
            Ok(v)
        };
        let t = text.trim();
        Ok(match kind {
            ValueKind::I32 => Value::I32(t.parse()?),
            ValueKind::F32 => Value::F32(t.parse()?),
            ValueKind::U8Scaled => Value::U8Scaled(scaled(t.parse()?)),
            ValueKind::I16 => Value::I16(t.parse()?),
            ValueKind::U8 => Value::U8(t.parse()?),
            ValueKind::Bool => Value::Bool(matches!(t, "1" | "true" | "yes" | "on")),
            ValueKind::U8x3 => {
                let v = fixed(3)?;
                Value::U8x3([scaled(v[0]), scaled(v[1]), scaled(v[2])])
            }
            ValueKind::F32x3 => {
                let v = fixed(3)?;
                Value::F32x3([v[0], v[1], v[2]])
            }
            ValueKind::U8x2 => {
                let v = fixed(2)?;
                Value::U8x2([scaled(v[0]), scaled(v[1])])
            }
            ValueKind::F32x2 => {
                let v = fixed(2)?;
                Value::F32x2([v[0], v[1]])
            }
            ValueKind::U8x4 => {
                let v = fixed(4)?;
                Value::U8x4([scaled(v[0]), scaled(v[1]), scaled(v[2]), scaled(v[3])])
            }
            ValueKind::F32x4 => {
                let v = fixed(4)?;
                Value::F32x4([v[0], v[1], v[2], v[3]])
            }
            ValueKind::Str => Value::Str(text.to_string()),
            ValueKind::I64 => Value::I64(t.parse()?),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: u32,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Inibin {
    pub entries: Vec<Entry>,
    /// The original bytes, written back while the entries are unchanged.
    original: Option<(Vec<Entry>, Vec<u8>)>,
    /// The original string table and each string key's offset in it. Edited files keep this
    /// table (new strings are appended), so unchanged strings stay where they were.
    original_strings: Option<(Vec<u8>, std::collections::HashMap<u32, (usize, String)>)>,
}

impl Inibin {
    pub fn new() -> Self {
        Self { entries: Vec::new(), original: None, original_strings: None }
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let version = r.u8()?;
        if version != 2 {
            bail!("inibin version {version} is not supported (only version 2)");
        }
        let string_table_len = r.u16()? as usize;
        let flags = r.u16()?;
        let mut entries = Vec::new();
        let mut string_offsets: Vec<(usize, u16)> = Vec::new();
        for kind in ValueKind::ALL {
            let bit = kind as u16;
            if bit >= 16 || flags & (1 << bit) == 0 {
                continue;
            }
            let n = r.u16()? as usize;
            let mut keys = Vec::with_capacity(n);
            for _ in 0..n {
                keys.push(r.u32()?);
            }
            if kind == ValueKind::Bool {
                let packed = r.bytes(n.div_ceil(8))?;
                for (i, key) in keys.into_iter().enumerate() {
                    entries.push(Entry { key, value: Value::Bool(packed[i / 8] >> (i % 8) & 1 != 0) });
                }
                continue;
            }
            for key in keys {
                let value = match kind {
                    ValueKind::I32 => Value::I32(r.i32()?),
                    ValueKind::F32 => Value::F32(r.f32()?),
                    ValueKind::U8Scaled => Value::U8Scaled(r.u8()?),
                    ValueKind::I16 => Value::I16(r.i16()?),
                    ValueKind::U8 => Value::U8(r.u8()?),
                    ValueKind::U8x3 => Value::U8x3([r.u8()?, r.u8()?, r.u8()?]),
                    ValueKind::F32x3 => Value::F32x3(r.f32s()?),
                    ValueKind::U8x2 => Value::U8x2([r.u8()?, r.u8()?]),
                    ValueKind::F32x2 => Value::F32x2(r.f32s()?),
                    ValueKind::U8x4 => Value::U8x4([r.u8()?, r.u8()?, r.u8()?, r.u8()?]),
                    ValueKind::F32x4 => Value::F32x4(r.f32s()?),
                    ValueKind::Str => {
                        string_offsets.push((entries.len(), r.u16()?));
                        Value::Str(String::new())
                    }
                    ValueKind::I64 => Value::I64(r.i64()?),
                    ValueKind::Bool => unreachable!(),
                };
                entries.push(Entry { key, value });
            }
        }
        if flags & 0xc000 != 0 {
            bail!("inibin uses unknown value sets (flags {flags:#06x})");
        }
        let table = r.bytes(string_table_len)?;
        let mut offsets = std::collections::HashMap::new();
        for (idx, off) in string_offsets {
            let off = off as usize;
            if off >= table.len() {
                bail!("inibin string offset {off} is outside its {}-byte table", table.len());
            }
            let end = table[off..].iter().position(|&b| b == 0).map_or(table.len(), |p| off + p);
            let text = super::bin::latin1_decode(&table[off..end]);
            offsets.insert(entries[idx].key, (off, text.clone()));
            entries[idx].value = Value::Str(text);
        }
        if r.remaining() != 0 {
            bail!("{} unexpected bytes after the inibin string table", r.remaining());
        }
        Ok(Self { original: Some((entries.clone(), data.to_vec())), original_strings: Some((table.to_vec(), offsets)), entries })
    }

    pub fn write(&self) -> Result<Vec<u8>> {
        if let Some((entries, bytes)) = &self.original {
            if *entries == self.entries {
                return Ok(bytes.clone());
            }
        }
        // Each set holds its keys in ascending order, as in the game's files.
        let mut flags = 0u16;
        let mut body = Writer::default();
        let (mut table, kept) = self.original_strings.clone().unwrap_or_default();
        let mut string_at: std::collections::HashMap<String, usize> = Default::default();
        for kind in ValueKind::ALL {
            let mut set: Vec<&Entry> = self.entries.iter().filter(|e| e.value.kind() == kind).collect();
            if set.is_empty() {
                continue;
            }
            set.sort_by_key(|e| e.key);
            for pair in set.windows(2) {
                if pair[0].key == pair[1].key {
                    bail!("key {:#010x} appears twice with type {}", pair[0].key, kind.label());
                }
            }
            if set.len() > u16::MAX as usize {
                bail!("too many {} values", kind.label());
            }
            flags |= 1 << kind as u16;
            body.u16(set.len() as u16);
            for e in &set {
                body.u32(e.key);
            }
            if kind == ValueKind::Bool {
                let mut packed = vec![0u8; set.len().div_ceil(8)];
                for (i, e) in set.iter().enumerate() {
                    if e.value == Value::Bool(true) {
                        packed[i / 8] |= 1 << (i % 8);
                    }
                }
                body.bytes(&packed);
                continue;
            }
            for e in &set {
                match &e.value {
                    Value::I32(v) => body.i32(*v),
                    Value::F32(v) => body.f32(*v),
                    Value::U8Scaled(v) | Value::U8(v) => body.u8(*v),
                    Value::I16(v) => body.i16(*v),
                    Value::U8x3(v) => body.bytes(v),
                    Value::F32x3(v) => body.f32s(v),
                    Value::U8x2(v) => body.bytes(v),
                    Value::F32x2(v) => body.f32s(v),
                    Value::U8x4(v) => body.bytes(v),
                    Value::F32x4(v) => body.f32s(v),
                    Value::Str(s) => {
                        // Unchanged strings keep their original offset; new ones are stored once.
                        let at = match (kept.get(&e.key), string_at.get(s)) {
                            (Some((at, old)), _) if old == s => *at,
                            (_, Some(&at)) => at,
                            _ => {
                                let at = table.len();
                                table.extend(super::bin::latin1_encode(s));
                                table.push(0);
                                string_at.insert(s.clone(), at);
                                at
                            }
                        };
                        if at > u16::MAX as usize {
                            bail!("inibin string table is over 64 KiB");
                        }
                        body.u16(at as u16);
                    }
                    Value::I64(v) => body.i64(*v),
                    Value::Bool(_) => unreachable!(),
                }
            }
        }
        if table.len() > u16::MAX as usize {
            bail!("inibin string table is over 64 KiB");
        }
        let mut w = Writer::default();
        w.u8(2);
        w.u16(table.len() as u16);
        w.u16(flags);
        w.bytes(&body.data);
        w.bytes(&table);
        Ok(w.data)
    }

    pub fn get(&self, key: u32) -> Option<&Value> {
        self.entries.iter().find(|e| e.key == key).map(|e| &e.value)
    }

    pub fn set(&mut self, key: u32, value: Value) {
        match self.entries.iter_mut().find(|e| e.key == key) {
            Some(e) => e.value = value,
            None => self.entries.push(Entry { key, value }),
        }
    }
}

impl Default for Inibin {
    fn default() -> Self {
        Self::new()
    }
}

impl Inibin {
    /// Drops the remembered original bytes, so `write` encodes from the entries (used by tests).
    pub fn forget_original(&mut self) {
        self.original = None;
    }
}
