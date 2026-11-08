//! Character meshes (`*.skn`, magic 0x00112233, versions 0-4), read in their bind pose for
//! display: positions, normals, UVs and triangle indices.

use super::bin::{Reader, fixed_str};
use anyhow::{Result, bail};

pub struct Skn {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub submeshes: Vec<String>,
    /// Per vertex: four skeleton influence indices and their weights.
    pub bones: Vec<[u8; 4]>,
    pub weights: Vec<[f32; 4]>,
}

impl Skn {
    pub fn parse(d: &[u8]) -> Result<Self> {
        let mut r = Reader::new(d);
        if r.u32()? != 0x00112233 {
            bail!("not an SKN mesh");
        }
        let major = r.u16()?;
        let _minor = r.u16()?;
        let mut submeshes = Vec::new();
        let mut vertex_size = 52usize;
        if major > 0 {
            let n = r.u32()? as usize;
            if n > 1000 {
                bail!("SKN has {n} submeshes");
            }
            for _ in 0..n {
                submeshes.push(fixed_str(r.bytes(64)?));
                r.skip(16)?;
            }
            if major == 4 {
                r.u32()?;
            }
        }
        let ni = r.u32()? as usize;
        let nv = r.u32()? as usize;
        if major == 4 {
            vertex_size = r.u32()? as usize;
            r.u32()?;
            r.skip(40)?;
        }
        if vertex_size < 52 {
            bail!("SKN vertex size {vertex_size}");
        }
        let mut indices = Vec::with_capacity(ni);
        for _ in 0..ni {
            indices.push(r.u16()? as u32);
        }
        let (mut positions, mut normals, mut uvs) = (Vec::with_capacity(nv), Vec::with_capacity(nv), Vec::with_capacity(nv));
        let (mut bones, mut weights) = (Vec::with_capacity(nv), Vec::with_capacity(nv));
        for _ in 0..nv {
            let v = r.bytes(vertex_size)?;
            let f = |o: usize| f32::from_le_bytes(v[o..o + 4].try_into().unwrap());
            positions.push([f(0), f(4), f(8)]);
            normals.push([f(32), f(36), f(40)]);
            uvs.push([f(44), f(48)]);
            bones.push([v[12], v[13], v[14], v[15]]);
            weights.push([f(16), f(20), f(24), f(28)]);
        }
        indices.retain(|&i| (i as usize) < nv);
        indices.truncate(indices.len() / 3 * 3);
        Ok(Self { positions, normals, uvs, indices, submeshes, bones, weights })
    }
}
