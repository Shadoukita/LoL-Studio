//! `Scene/room.wgeo`: baked environment geometry (`BakedEnvironmentAsset::Load`, versions 3-5).
//!
//! Header "WGEO", version, mesh count, total triangles; per mesh a 260-byte texture name (v4+: a
//! 64-byte material name), bounding sphere and box, counts, 20-byte vertices (position, UV) and
//! indices (16-bit up to 65536 indices). Then a bucketed triangle list the game uses for ground
//! height and picking (`SceneGraph::BucketedTriangleList`).

use super::bin::{Reader, Writer, fixed_str};
use super::nvr::bounds;
use anyhow::{Result, bail};

pub const MAGIC: u32 = 0x4f454757;

#[derive(Clone, Debug, PartialEq)]
pub struct WgeoMesh {
    pub texture: String,
    pub material: String,
    /// Original fixed-size name fields, kept so unchanged names round-trip exactly.
    texture_field: Option<Vec<u8>>,
    material_field: Option<Vec<u8>>,
    pub sphere: [f32; 4],
    pub bbox: [f32; 6],
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl WgeoMesh {
    pub fn new(texture: &str, material: &str, positions: Vec<[f32; 3]>, uvs: Vec<[f32; 2]>, indices: Vec<u32>) -> Self {
        let (bbox, sphere) = bounds(&positions);
        Self { texture: texture.into(), material: material.into(), texture_field: None, material_field: None, sphere, bbox, positions, uvs, indices }
    }

    pub fn recompute_bounds(&mut self) {
        let (bbox, sphere) = bounds(&self.positions);
        self.bbox = bbox;
        self.sphere = sphere;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct BucketHeader {
    pub min_x: f32,
    pub min_z: f32,
    pub max_x: f32,
    pub max_z: f32,
    pub max_out_stick_x: f32,
    pub max_out_stick_z: f32,
    pub bucket_size_x: f32,
    pub bucket_size_z: f32,
    pub buckets_per_side: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Bucket {
    pub out_stick_x: f32,
    pub out_stick_z: f32,
    pub index_start: u32,
    pub vertex_start: u32,
    pub inside_triangles: u16,
    pub outside_triangles: u16,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Buckets {
    pub header: BucketHeader,
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<u16>,
    pub buckets: Vec<Bucket>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wgeo {
    pub version: u32,
    pub meshes: Vec<WgeoMesh>,
    pub buckets: Buckets,
    /// The header's triangle total as stored (the game does not use it).
    pub total_triangles: u32,
}

impl Wgeo {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        if r.u32()? != MAGIC {
            bail!("not a room.wgeo file");
        }
        let version = r.u32()?;
        if !(3..=5).contains(&version) {
            bail!("WGEO version {version}; the game loads versions 3 to 5");
        }
        let count = r.u32()? as usize;
        let total_triangles = r.u32()?;
        let mut meshes = Vec::with_capacity(count);
        for _ in 0..count {
            let tf = r.bytes(260)?.to_vec();
            let mf = if version >= 4 { Some(r.bytes(64)?.to_vec()) } else { None };
            let sphere = r.f32s()?;
            let bbox = r.f32s()?;
            let nv = r.u32()? as usize;
            let ni = r.u32()? as usize;
            let mut positions = Vec::with_capacity(nv);
            let mut uvs = Vec::with_capacity(nv);
            for _ in 0..nv {
                positions.push(r.vec3()?);
                uvs.push(r.f32s()?);
            }
            let mut indices = Vec::with_capacity(ni);
            if ni <= 0x10000 {
                for _ in 0..ni {
                    indices.push(r.u16()? as u32);
                }
            } else {
                for _ in 0..ni {
                    indices.push(r.u32()?);
                }
            }
            meshes.push(WgeoMesh {
                texture: fixed_str(&tf),
                material: mf.as_deref().map(fixed_str).unwrap_or_default(),
                texture_field: Some(tf),
                material_field: mf,
                sphere,
                bbox,
                positions,
                uvs,
                indices,
            });
        }
        let header = BucketHeader {
            min_x: r.f32()?,
            min_z: r.f32()?,
            max_x: r.f32()?,
            max_z: r.f32()?,
            max_out_stick_x: r.f32()?,
            max_out_stick_z: r.f32()?,
            bucket_size_x: r.f32()?,
            bucket_size_z: r.f32()?,
            buckets_per_side: r.u32()?,
        };
        let nv = r.u32()? as usize;
        let ni = r.u32()? as usize;
        let mut buckets = Buckets { header, ..Default::default() };
        for _ in 0..nv {
            buckets.vertices.push(r.vec3()?);
        }
        for _ in 0..ni {
            buckets.indices.push(r.u16()?);
        }
        let n = header.buckets_per_side as usize;
        for _ in 0..n * n {
            buckets.buckets.push(Bucket {
                out_stick_x: r.f32()?,
                out_stick_z: r.f32()?,
                index_start: r.u32()?,
                vertex_start: r.u32()?,
                inside_triangles: r.u16()?,
                outside_triangles: r.u16()?,
            });
        }
        if r.remaining() != 0 {
            bail!("{} unexpected bytes at the end of room.wgeo", r.remaining());
        }
        Ok(Self { version, meshes, buckets, total_triangles })
    }

    pub fn write(&self) -> Result<Vec<u8>> {
        let mut w = Writer::default();
        w.u32(MAGIC);
        w.u32(self.version);
        w.u32(self.meshes.len() as u32);
        w.u32(self.total_triangles);
        for m in &self.meshes {
            match &m.texture_field {
                Some(f) if fixed_str(f) == m.texture => w.bytes(f),
                _ => w.fixed_str(&m.texture, 260)?,
            }
            if self.version >= 4 {
                match &m.material_field {
                    Some(f) if fixed_str(f) == m.material => w.bytes(f),
                    _ => w.fixed_str(&m.material, 64)?,
                }
            }
            w.f32s(&m.sphere);
            w.f32s(&m.bbox);
            if m.uvs.len() != m.positions.len() {
                bail!("WGEO mesh \"{}\" has {} positions but {} UVs", m.texture, m.positions.len(), m.uvs.len());
            }
            w.u32(m.positions.len() as u32);
            w.u32(m.indices.len() as u32);
            for (p, uv) in m.positions.iter().zip(&m.uvs) {
                w.f32s(p);
                w.f32s(uv);
            }
            if m.indices.len() <= 0x10000 {
                for &i in &m.indices {
                    if i > 0xffff {
                        bail!("WGEO mesh \"{}\" has 16-bit indices above 65535", m.texture);
                    }
                    w.u16(i as u16);
                }
            } else {
                for &i in &m.indices {
                    w.u32(i);
                }
            }
        }
        let h = &self.buckets.header;
        w.f32s(&[h.min_x, h.min_z, h.max_x, h.max_z, h.max_out_stick_x, h.max_out_stick_z, h.bucket_size_x, h.bucket_size_z]);
        w.u32(h.buckets_per_side);
        w.u32(self.buckets.vertices.len() as u32);
        w.u32(self.buckets.indices.len() as u32);
        for v in &self.buckets.vertices {
            w.f32s(v);
        }
        for &i in &self.buckets.indices {
            w.u16(i);
        }
        for b in &self.buckets.buckets {
            w.f32(b.out_stick_x);
            w.f32(b.out_stick_z);
            w.u32(b.index_start);
            w.u32(b.vertex_start);
            w.u16(b.inside_triangles);
            w.u16(b.outside_triangles);
        }
        Ok(w.data)
    }

    /// The ground triangles stored in the buckets, as a plain triangle list.
    pub fn bucket_triangles(&self) -> Vec<[f32; 3]> {
        let b = &self.buckets;
        let mut out = Vec::with_capacity(b.indices.len());
        for k in &b.buckets {
            let n = (k.inside_triangles as usize + k.outside_triangles as usize) * 3;
            for i in 0..n {
                let Some(&idx) = b.indices.get(k.index_start as usize + i) else { break };
                if let Some(v) = b.vertices.get(k.vertex_start as usize + idx as usize) {
                    out.push(*v);
                }
            }
        }
        out
    }

    pub fn triangle_count(&self) -> u32 {
        self.meshes.iter().map(|m| m.indices.len() as u32 / 3).sum()
    }

    /// Rebuilds the bucketed triangle list from walkable geometry. `ground` picks the meshes whose
    /// triangles the game should use for ground height (the original tools bake the ground layer).
    pub fn rebuild_buckets(&mut self, ground: impl Fn(&WgeoMesh) -> bool, buckets_per_side: u32) {
        let mut tris: Vec<[f32; 3]> = Vec::new();
        for m in self.meshes.iter().filter(|m| ground(m)) {
            for t in m.indices.chunks_exact(3) {
                for &i in t {
                    tris.push(m.positions[i as usize]);
                }
            }
        }
        self.buckets = build_buckets(&tris, buckets_per_side);
        self.total_triangles = self.triangle_count();
    }
}

/// A bucket's edges. Rebuilt buckets match the shipped map11 file in triangle assignment, vertices
/// and indices; out-stick margins can differ in the last digits (the offline tool's float order).
fn edges(i: usize, size: f32, min: f32) -> (f32, f32) {
    let lo = (i as f64 * size as f64 + min as f64) as f32;
    (lo, lo + size)
}

/// Port of `BucketedTriangleList::BuildFromTriangles` (vertices are a plain triangle list).
pub fn build_buckets(vert: &[[f32; 3]], n: u32) -> Buckets {
    let n_us = n as usize;
    let tri_count = vert.len() / 3;
    let mut tri_box: Vec<([f32; 3], [f32; 3])> = Vec::with_capacity(tri_count);
    let mut lo = [f32::MAX; 3];
    let mut hi = [-f32::MAX; 3];
    for t in 0..tri_count {
        let mut a = [f32::MAX; 3];
        let mut b = [-f32::MAX; 3];
        for v in &vert[t * 3..t * 3 + 3] {
            for k in 0..3 {
                a[k] = a[k].min(v[k]);
                b[k] = b[k].max(v[k]);
            }
        }
        // The original grows the overall box by each triangle's minimum corner only.
        for k in 0..3 {
            lo[k] = lo[k].min(a[k]);
            hi[k] = hi[k].max(a[k]);
        }
        tri_box.push((a, b));
    }
    let (min_x, min_z, max_x, max_z) = if tri_count > 0 { (lo[0] - 10.0, lo[2] - 10.0, hi[0] + 10.0, hi[2] + 10.0) } else { (-10.0, -10.0, 10.0, 10.0) };
    let bsx = (max_x - min_x) / n as f32;
    let bsz = (max_z - min_z) / n as f32;
    let mut lists: Vec<Vec<usize>> = vec![Vec::new(); n_us * n_us];
    for (t, (a, _)) in tri_box.iter().enumerate() {
        let bx = (((a[0] - min_x) / bsx).floor() as i64).clamp(0, n as i64 - 1) as usize;
        let bz = (((a[2] - min_z) / bsz).floor() as i64).clamp(0, n as i64 - 1) as usize;
        lists[bz * n_us + bx].push(t);
    }
    let mut out = Buckets::default();
    let mut max_out = [0.0f32; 2];
    for z in 0..n_us {
        let (zmin, zmax) = edges(z, bsz, min_z);
        for x in 0..n_us {
            let (xmin, xmax) = edges(x, bsx, min_x);
            let mut bucket = Bucket::default();
            let mut inside = Vec::new();
            let mut outside = Vec::new();
            for &t in &lists[z * n_us + x] {
                let m = tri_box[t].1;
                let (mx, mz) = (m[0], m[2]);
                if mx >= xmin && mx <= xmax && mz >= zmin && mz <= zmax {
                    inside.push(t);
                } else {
                    outside.push(t);
                    let ox = mx - xmax;
                    let oz = mz - zmax;
                    max_out[0] = max_out[0].max(ox);
                    max_out[1] = max_out[1].max(oz);
                    bucket.out_stick_x = bucket.out_stick_x.max(ox);
                    bucket.out_stick_z = bucket.out_stick_z.max(oz);
                }
            }
            bucket.index_start = out.indices.len() as u32;
            bucket.vertex_start = out.vertices.len() as u32;
            bucket.inside_triangles = inside.len() as u16;
            bucket.outside_triangles = outside.len() as u16;
            let mut local: Vec<[f32; 3]> = Vec::new();
            for &t in inside.iter().chain(outside.iter()) {
                for v in &vert[t * 3..t * 3 + 3] {
                    let idx = match local.iter().position(|p| p == v) {
                        Some(i) => i,
                        None => {
                            local.push(*v);
                            local.len() - 1
                        }
                    };
                    out.indices.push(idx as u16);
                }
            }
            out.vertices.extend(local);
            out.buckets.push(bucket);
        }
    }
    out.header = BucketHeader {
        min_x,
        min_z,
        max_x,
        max_z,
        max_out_stick_x: max_out[0],
        max_out_stick_z: max_out[1],
        bucket_size_x: bsx,
        bucket_size_z: bsz,
        buckets_per_side: n,
    };
    out
}
