//! Character animations (`*.anm`, "r3d2anmd"): version 4 (vector and quaternion palettes with
//! per-frame indices) and the legacy version 3 (a rotation and translation per frame).

use super::bin::{Reader, fixed_str};
use super::skl::elf_hash;
use anyhow::{Result, bail};
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub t: Vec<[f32; 3]>,
    pub s: Vec<[f32; 3]>,
    pub r: Vec<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct Animation {
    pub frame_time: f32,
    pub frames: usize,
    pub tracks: HashMap<u32, Track>,
}

impl Animation {
    pub fn duration(&self) -> f32 {
        self.frame_time * self.frames.max(1) as f32
    }

    pub fn parse(d: &[u8]) -> Result<Self> {
        let mut r = Reader::new(d);
        if r.bytes(8)? != b"r3d2anmd" {
            bail!("not an animation");
        }
        let version = r.u32()?;
        match version {
            4 => {
                let _size = r.u32()?;
                let _token = r.u32()?;
                let _v = r.u32()?;
                let _flags = r.u32()?;
                let tracks_n = r.i32()? as usize;
                let frames = r.i32()? as usize;
                let frame_time = r.f32()?;
                let _tracks_at = r.i32()?;
                let _name_at = r.i32()?;
                let _time_at = r.i32()?;
                let vec_at = r.i32()? as usize + 12;
                let quat_at = r.i32()? as usize + 12;
                let frames_at = r.i32()? as usize + 12;
                let n_vec = (quat_at - vec_at) / 12;
                let n_quat = (frames_at - quat_at) / 16;
                let f = |o: usize| f32::from_le_bytes(d[o..o + 4].try_into().unwrap());
                let vecs: Vec<[f32; 3]> = (0..n_vec).map(|i| [f(vec_at + i * 12), f(vec_at + i * 12 + 4), f(vec_at + i * 12 + 8)]).collect();
                let quats: Vec<[f32; 4]> = (0..n_quat).map(|i| [f(quat_at + i * 16), f(quat_at + i * 16 + 4), f(quat_at + i * 16 + 8), f(quat_at + i * 16 + 12)]).collect();
                let mut tracks: HashMap<u32, Track> = HashMap::new();
                let mut fr = Reader::new(&d[frames_at..]);
                for _ in 0..frames * tracks_n {
                    let hash = fr.u32()?;
                    let (ti, si, ri) = (fr.u16()? as usize, fr.u16()? as usize, fr.u16()? as usize);
                    fr.u16()?;
                    let tr = tracks.entry(hash).or_default();
                    tr.t.push(vecs.get(ti).copied().unwrap_or([0.0; 3]));
                    tr.s.push(vecs.get(si).copied().unwrap_or([1.0; 3]));
                    tr.r.push(quats.get(ri).copied().unwrap_or([0.0, 0.0, 0.0, 1.0]));
                }
                Ok(Self { frame_time: if frame_time > 0.0 { frame_time } else { 1.0 / 30.0 }, frames, tracks })
            }
            3 | 2 | 1 => {
                let _skeleton = r.u32()?;
                let bones = r.u32()? as usize;
                let frames = r.u32()? as usize;
                let fps = r.u32()?.max(1);
                let mut tracks = HashMap::new();
                for _ in 0..bones {
                    let name = fixed_str(r.bytes(32)?);
                    r.u32()?;
                    let mut tr = Track::default();
                    for _ in 0..frames {
                        let q: [f32; 4] = r.f32s()?;
                        let t = r.vec3()?;
                        tr.r.push(q);
                        tr.t.push(t);
                        tr.s.push([1.0; 3]);
                    }
                    tracks.insert(elf_hash(&name), tr);
                }
                Ok(Self { frame_time: 1.0 / fps as f32, frames, tracks })
            }
            v => bail!("animation version {v} is not supported"),
        }
    }
}

pub fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let mut b = b;
    if d < 0.0 {
        b = b.map(|v| -v);
        d = -d;
    }
    let q = if d > 0.9995 {
        [0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * t)
    } else {
        let th = d.acos();
        let s = th.sin();
        let (wa, wb) = (((1.0 - t) * th).sin() / s, (t * th).sin() / s);
        [0, 1, 2, 3].map(|i| a[i] * wa + b[i] * wb)
    };
    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
    q.map(|v| v / l)
}
