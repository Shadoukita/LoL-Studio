//! `AIPath.aimesh`: the legacy AI triangle mesh ("r3d2aims" version 2), shipped with Map1 next to
//! the navigation grid. Each triangle is 3 points plus 3 neighbour indices (0xffff = none).

use super::bin::Reader;
use anyhow::{Result, bail};

#[derive(Clone, Debug)]
pub struct AiMesh {
    pub version: i32,
    pub triangles: Vec<[[f32; 3]; 3]>,
    pub neighbours: Vec<[u16; 3]>,
    raw: Vec<u8>,
}

impl AiMesh {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        if r.bytes(8)? != b"r3d2aims" {
            bail!("not an AI mesh");
        }
        let version = r.i32()?;
        let n = r.i32()? as usize;
        r.skip(8)?;
        let mut triangles = Vec::with_capacity(n);
        let mut neighbours = Vec::with_capacity(n);
        for _ in 0..n {
            triangles.push([r.vec3()?, r.vec3()?, r.vec3()?]);
            neighbours.push([r.u16()?, r.u16()?, r.u16()?]);
        }
        if r.remaining() != 0 {
            bail!("{} unexpected bytes at the end of the AI mesh", r.remaining());
        }
        Ok(Self { version, triangles, neighbours, raw: data.to_vec() })
    }

    pub fn write(&self) -> Vec<u8> {
        self.raw.clone()
    }

    /// Two triangles per walkable cell, joined along shared edges.
    pub fn from_walkable(walk: &[bool], heights: &[f32], nx: usize, ny: usize, cell: f32, origin: [f32; 2]) -> Result<Vec<u8>> {
        const NONE: u16 = 0xffff;
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
        for j in 0..ny {
            for i in 0..nx {
                let k = j * nx + i;
                if !walk[k] {
                    continue;
                }
                let h = heights[k];
                let (ax, az) = (origin[0] + i as f32 * cell, origin[1] + j as f32 * cell);
                let p00 = [ax, h, az];
                let p10 = [ax + cell, h, az];
                let p01 = [ax, h, az + cell];
                let p11 = [ax + cell, h, az + cell];
                tris.push([p00, p01, p11]);
                tris.push([p00, p11, p10]);
            }
        }
        if tris.len() >= NONE as usize {
            bail!("too many walkable cells for the 16-bit AI mesh ({} triangles); the game can run without AIPath.aimesh", tris.len());
        }
        let key = |a: [f32; 3], b: [f32; 3]| {
            let (s, t) = if (a[0], a[1], a[2]) <= (b[0], b[1], b[2]) { (a, b) } else { (b, a) };
            (s.map(f32::to_bits), t.map(f32::to_bits))
        };
        let mut edges: std::collections::HashMap<_, Vec<(usize, usize)>> = Default::default();
        for (t, tri) in tris.iter().enumerate() {
            for e in 0..3 {
                edges.entry(key(tri[e], tri[(e + 1) % 3])).or_default().push((t, e));
            }
        }
        let mut nbr = vec![[NONE; 3]; tris.len()];
        for owners in edges.values() {
            if let [(t1, e1), (t2, e2)] = owners[..] {
                nbr[t1][e1] = t2 as u16;
                nbr[t2][e2] = t1 as u16;
            }
        }
        let mut out = Vec::with_capacity(24 + tris.len() * 42);
        out.extend_from_slice(b"r3d2aims");
        out.extend_from_slice(&2i32.to_le_bytes());
        out.extend_from_slice(&(tris.len() as i32).to_le_bytes());
        out.extend_from_slice(&[0u8; 8]);
        for (t, tri) in tris.iter().enumerate() {
            for p in tri {
                for c in p {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
            for n in nbr[t] {
                out.extend_from_slice(&n.to_le_bytes());
            }
        }
        Ok(out)
    }
}
