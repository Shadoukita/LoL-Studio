//! Compiled level scripts (`*.luaobj`, Lua 5.1 bytecode). They are copied as they are; this reader
//! only lists their string constants so the editor can show which object names a script expects.

use super::bin::Reader;
use anyhow::{Result, bail};

pub struct LuaInfo {
    pub strings: Vec<String>,
    pub functions: usize,
}

struct Header {
    int_size: usize,
    size_t: usize,
    instr: usize,
    number: usize,
}

fn read_int(r: &mut Reader, n: usize) -> Result<u64> {
    Ok(match n {
        4 => r.u32()? as u64,
        8 => {
            let lo = r.u32()? as u64;
            let hi = r.u32()? as u64;
            lo | (hi << 32)
        }
        _ => bail!("unsupported integer size {n}"),
    })
}

fn read_string(r: &mut Reader, h: &Header) -> Result<String> {
    let n = read_int(r, h.size_t)? as usize;
    if n == 0 {
        return Ok(String::new());
    }
    let b = r.bytes(n)?;
    Ok(super::bin::latin1_decode(&b[..n - 1]))
}

fn function(r: &mut Reader, h: &Header, info: &mut LuaInfo) -> Result<()> {
    info.functions += 1;
    read_string(r, h)?;
    read_int(r, h.int_size)?;
    read_int(r, h.int_size)?;
    r.skip(4)?;
    let code = read_int(r, h.int_size)? as usize;
    r.skip(code * h.instr)?;
    let consts = read_int(r, h.int_size)? as usize;
    for _ in 0..consts {
        match r.u8()? {
            0 => {}
            1 => r.skip(1)?,
            3 => r.skip(h.number)?,
            4 => {
                let s = read_string(r, h)?;
                info.strings.push(s);
            }
            t => bail!("unknown Lua constant type {t}"),
        }
    }
    let protos = read_int(r, h.int_size)? as usize;
    for _ in 0..protos {
        function(r, h, info)?;
    }
    let lines = read_int(r, h.int_size)? as usize;
    r.skip(lines * h.int_size)?;
    let locals = read_int(r, h.int_size)? as usize;
    for _ in 0..locals {
        read_string(r, h)?;
        read_int(r, h.int_size)?;
        read_int(r, h.int_size)?;
    }
    let upvalues = read_int(r, h.int_size)? as usize;
    for _ in 0..upvalues {
        read_string(r, h)?;
    }
    Ok(())
}

pub fn inspect(data: &[u8]) -> Result<LuaInfo> {
    let mut r = Reader::new(data);
    if r.bytes(4)? != b"\x1bLua" || r.u8()? != 0x51 {
        bail!("not Lua 5.1 bytecode");
    }
    r.u8()?;
    if r.u8()? != 1 {
        bail!("big-endian Lua bytecode is not supported");
    }
    let h = Header { int_size: r.u8()? as usize, size_t: r.u8()? as usize, instr: r.u8()? as usize, number: r.u8()? as usize };
    r.u8()?;
    let mut info = LuaInfo { strings: Vec::new(), functions: 0 };
    function(&mut r, &h, &mut info)?;
    Ok(info)
}

/// A value passed to a function call in compiled Lua (straight-line code only).
#[derive(Clone, Debug, PartialEq)]
pub enum LuaArg {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    /// A global variable's name (e.g. TEAM_ORDER).
    Global(String),
    Unknown,
}

impl LuaArg {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            LuaArg::Str(s) | LuaArg::Global(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_num(&self) -> Option<f64> {
        if let LuaArg::Num(n) = self { Some(*n) } else { None }
    }
}

struct Proto {
    code: Vec<u32>,
    k: Vec<LuaArg>,
    protos: Vec<Proto>,
}

fn proto(r: &mut Reader, h: &Header) -> Result<Proto> {
    read_string(r, h)?;
    read_int(r, h.int_size)?;
    read_int(r, h.int_size)?;
    r.skip(4)?;
    let n = read_int(r, h.int_size)? as usize;
    let mut code = Vec::with_capacity(n);
    for _ in 0..n {
        code.push(read_int(r, h.instr)? as u32);
    }
    let nk = read_int(r, h.int_size)? as usize;
    let mut k = Vec::with_capacity(nk);
    for _ in 0..nk {
        k.push(match r.u8()? {
            0 => LuaArg::Nil,
            1 => LuaArg::Bool(r.u8()? != 0),
            3 => {
                let b = r.bytes(h.number)?;
                LuaArg::Num(if h.number == 8 { f64::from_le_bytes(b.try_into().unwrap()) } else { f32::from_le_bytes(b[..4].try_into().unwrap()) as f64 })
            }
            4 => LuaArg::Str(read_string(r, h)?),
            t => bail!("unknown Lua constant type {t}"),
        });
    }
    let np = read_int(r, h.int_size)? as usize;
    let mut protos = Vec::with_capacity(np);
    for _ in 0..np {
        protos.push(proto(r, h)?);
    }
    let lines = read_int(r, h.int_size)? as usize;
    r.skip(lines * h.int_size)?;
    let locals = read_int(r, h.int_size)? as usize;
    for _ in 0..locals {
        read_string(r, h)?;
        read_int(r, h.int_size)?;
        read_int(r, h.int_size)?;
    }
    let ups = read_int(r, h.int_size)? as usize;
    for _ in 0..ups {
        read_string(r, h)?;
    }
    Ok(Proto { code, k, protos })
}

fn scan(p: &Proto, func: &str, out: &mut Vec<Vec<LuaArg>>) {
    let mut reg: Vec<LuaArg> = vec![LuaArg::Unknown; 256];
    let rk = |reg: &Vec<LuaArg>, x: usize| if x >= 256 { p.k.get(x - 256).cloned().unwrap_or(LuaArg::Unknown) } else { reg[x].clone() };
    for &i in &p.code {
        let op = i & 63;
        let a = ((i >> 6) & 255) as usize;
        let b = ((i >> 23) & 511) as usize;
        let bx = (i >> 14) as usize;
        match op {
            0 => reg[a] = reg[b].clone(),                                       // MOVE
            1 => reg[a] = p.k.get(bx).cloned().unwrap_or(LuaArg::Unknown),       // LOADK
            2 => reg[a] = LuaArg::Bool(b != 0),                                  // LOADBOOL
            3 => (a..=b.min(255)).for_each(|r| reg[r] = LuaArg::Nil),           // LOADNIL
            5 => reg[a] = match p.k.get(bx) { Some(LuaArg::Str(s)) => LuaArg::Global(s.clone()), _ => LuaArg::Unknown }, // GETGLOBAL
            18 => reg[a] = match rk(&reg, b) { LuaArg::Num(n) => LuaArg::Num(-n), _ => LuaArg::Unknown }, // UNM
            28 => {
                // CALL
                if reg[a] == LuaArg::Global(func.to_string()) && b >= 1 {
                    out.push((a + 1..a + b).map(|r| reg[r.min(255)].clone()).collect());
                }
                reg[a] = LuaArg::Unknown;
            }
            _ => {
                if op <= 37 && !matches!(op, 7 | 8 | 9 | 22 | 23 | 24 | 25 | 26 | 30 | 31 | 32 | 33 | 34 | 35) {
                    reg[a] = LuaArg::Unknown;
                }
            }
        }
    }
    for q in &p.protos {
        scan(q, func, out);
    }
}

/// Arguments of every call to the global function `func` (e.g. CreateChildTurret) in straight-line
/// code of any function in the chunk. Values computed at run time come back as `Unknown`.
pub fn call_args(data: &[u8], func: &str) -> Result<Vec<Vec<LuaArg>>> {
    let mut r = Reader::new(data);
    if r.bytes(4)? != b"\x1bLua" || r.u8()? != 0x51 {
        bail!("not Lua 5.1 bytecode");
    }
    r.u8()?;
    if r.u8()? != 1 {
        bail!("big-endian Lua bytecode is not supported");
    }
    let h = Header { int_size: r.u8()? as usize, size_t: r.u8()? as usize, instr: r.u8()? as usize, number: r.u8()? as usize };
    r.u8()?;
    let main = proto(&mut r, &h)?;
    let mut out = Vec::new();
    scan(&main, func, &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn map1_turrets() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../Example Maps/Map1/Scripts/CreateLevelProps.luaobj");
        let Ok(b) = std::fs::read(&p) else { return };
        let calls = super::call_args(&b, "CreateChildTurret").unwrap();
        assert_eq!(calls.len(), 24);
        assert_eq!(calls[0][0].as_str(), Some("Turret_T1_R_03"));
        assert_eq!(calls[0][2].as_str(), Some("TEAM_ORDER"));
        assert_eq!(calls[0][3].as_num(), Some(3.0));
    }
}
