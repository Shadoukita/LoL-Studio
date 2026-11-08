//! `Scene/room.nvr`: the legacy environment mesh (`SimpleEnvironment::Asset::LoadNVR`, v9.1).
//!
//! Header "NVR\0", version (major 9, minor 1), counts; then 0xbac-byte materials, vertex buffers,
//! index buffers, 100-byte meshes and 40-byte culling nodes. The renderer walks the nodes from the
//! last one (the root); each mesh draws two primitives: [0] full vertices, [1] positions only.

use super::bin::{Reader, Writer, fixed_str, get_f32, put_f32};
use anyhow::{Result, bail};

pub const MATERIAL_BYTES: usize = 0xbac;
pub const MESH_BYTES: usize = 100;
pub const NODE_BYTES: usize = 40;
pub const CHANNELS: usize = 8;
pub const INDEX16: i32 = 101;
pub const INDEX32: i32 = 102;

pub mod material_type {
    pub const DEFAULT: i32 = 0;
    pub const DECAL: i32 = 1;
    pub const WALL_OF_GRASS: i32 = 2;
    pub const FOUR_BLEND: i32 = 3;
    pub const ANTI_BRUSH: i32 = 4;
    pub fn label(t: i32) -> &'static str {
        match t {
            DEFAULT => "Default",
            DECAL => "Decal",
            WALL_OF_GRASS => "Wall of grass",
            FOUR_BLEND => "Four blend",
            ANTI_BRUSH => "Anti brush",
            _ => "Unknown",
        }
    }
}

pub mod material_flags {
    pub const GROUND: u32 = 1;
    pub const NO_SHADOW: u32 = 2;
    pub const VERTALPHA: u32 = 4;
    pub const LIGHTMAPPED: u32 = 8;
    pub const DUAL_VTXCOLOR: u32 = 16;
    pub const BACKGROUND: u32 = 32;
    pub const BK_WITH_FOG: u32 = 64;
    pub const ALL: [(u32, &str); 7] = [
        (GROUND, "Ground"),
        (NO_SHADOW, "No shadow"),
        (VERTALPHA, "Vertex alpha"),
        (LIGHTMAPPED, "Lightmapped"),
        (DUAL_VTXCOLOR, "Dual vertex color"),
        (BACKGROUND, "Background"),
        (BK_WITH_FOG, "Background with fog"),
    ];
}

#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub color: [f32; 4],
    pub texture: String,
    pub transform: [f32; 16],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    pub kind: i32,
    pub flags: u32,
    pub channels: Vec<Channel>,
    raw: Option<(Box<Material>, Vec<u8>)>,
}

impl Material {
    /// A material laid out like Map1's: channel 0 holds the diffuse texture, colours are 1/255.
    pub fn new(name: &str, texture: &str, kind: i32, flags: u32) -> Self {
        const C: f32 = 1.0 / 255.0;
        let mut channels = vec![
            Channel { color: [C, C, C, C], texture: texture.into(), transform: [0.0; 16] },
            Channel { color: [0.0, 0.0, 0.0, C], texture: String::new(), transform: [0.0; 16] },
        ];
        while channels.len() < CHANNELS {
            channels.push(Channel { color: [0.0; 4], texture: String::new(), transform: [0.0; 16] });
        }
        Self { name: name.into(), kind, flags, channels, raw: None }
    }

    fn same_as(&self, o: &Material) -> bool {
        self.name == o.name && self.kind == o.kind && self.flags == o.flags && self.channels == o.channels
    }

    /// Vertex size of primitive 0 (`Asset::SelectVertexType`).
    pub fn vertex_size(&self) -> usize {
        let table = [0x28, 0x24, 0x24, 0x2c, 0x24];
        if (self.kind as u32 | (self.flags & material_flags::DUAL_VTXCOLOR)) != 0 {
            table.get(self.kind as usize).copied().unwrap_or(0x24)
        } else {
            0x24
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Primitive {
    pub vertex_buffer: u32,
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub index_buffer: u32,
    pub first_index: u32,
    pub index_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub quality: i32,
    pub flags: u32,
    pub sphere: [f32; 4],
    pub bbox: [f32; 6],
    pub material: u32,
    pub prims: [Primitive; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub bbox: [f32; 6],
    pub first_mesh: u32,
    pub mesh_count: u32,
    pub first_child: i32,
    pub child_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IndexBuffer {
    pub format: i32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Nvr {
    pub major: u16,
    pub minor: u16,
    pub materials: Vec<Material>,
    pub vertex_buffers: Vec<Vec<u8>>,
    pub index_buffers: Vec<IndexBuffer>,
    pub meshes: Vec<Mesh>,
    pub nodes: Vec<Node>,
}

/// Plain triangle geometry used for preview and for building new meshes.
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Second UV set (four-blend materials sample their blend mask with it).
    pub uvs1: Vec<[f32; 2]>,
    pub colors: Vec<u32>,
    pub indices: Vec<u32>,
}

impl Nvr {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        if r.bytes(4)? != b"NVR\0" {
            bail!("not a room.nvr file");
        }
        let major = r.u16()?;
        let minor = r.u16()?;
        if (major, minor) != (9, 1) && (major, minor) != (8, 1) {
            bail!("room.nvr version {major}.{minor}; the game loads 8.1 and 9.1");
        }
        if (major, minor) == (8, 1) {
            bail!("room.nvr version 8.1 (old material layout) can be viewed by the game but not edited here");
        }
        let counts: Vec<usize> = (0..5).map(|_| r.i32().map(|v| v as usize)).collect::<Result<_>>()?;
        let mut materials = Vec::with_capacity(counts[0]);
        for _ in 0..counts[0] {
            let raw = r.bytes(MATERIAL_BYTES)?;
            let mut m = Reader::new(raw);
            let name = fixed_str(m.bytes(260)?);
            let kind = m.i32()?;
            let flags = m.u32()?;
            let mut channels = Vec::with_capacity(CHANNELS);
            for _ in 0..CHANNELS {
                let color = m.f32s()?;
                let texture = fixed_str(m.bytes(260)?);
                let transform = m.f32s()?;
                channels.push(Channel { color, texture, transform });
            }
            let mut mat = Material { name, kind, flags, channels, raw: None };
            mat.raw = Some((Box::new(mat.clone()), raw.to_vec()));
            materials.push(mat);
        }
        let mut vertex_buffers = Vec::with_capacity(counts[1]);
        for _ in 0..counts[1] {
            let n = r.u32()? as usize;
            vertex_buffers.push(r.bytes(n)?.to_vec());
        }
        let mut index_buffers = Vec::with_capacity(counts[2]);
        for _ in 0..counts[2] {
            let n = r.u32()? as usize;
            let format = r.i32()?;
            index_buffers.push(IndexBuffer { format, data: r.bytes(n)?.to_vec() });
        }
        let mut meshes = Vec::with_capacity(counts[3]);
        for _ in 0..counts[3] {
            let quality = r.i32()?;
            let flags = r.u32()?;
            let sphere = r.f32s()?;
            let bbox = r.f32s()?;
            let material = r.u32()?;
            let mut prims = [Primitive::default(); 2];
            for p in &mut prims {
                *p = Primitive {
                    vertex_buffer: r.u32()?,
                    first_vertex: r.u32()?,
                    vertex_count: r.u32()?,
                    index_buffer: r.u32()?,
                    first_index: r.u32()?,
                    index_count: r.u32()?,
                };
            }
            meshes.push(Mesh { quality, flags, sphere, bbox, material, prims });
        }
        let mut nodes = Vec::with_capacity(counts[4]);
        for _ in 0..counts[4] {
            nodes.push(Node {
                bbox: r.f32s()?,
                first_mesh: r.u32()?,
                mesh_count: r.u32()?,
                first_child: r.i32()?,
                child_count: r.u32()?,
            });
        }
        if r.remaining() != 0 {
            bail!("{} unexpected bytes at the end of room.nvr", r.remaining());
        }
        let nvr = Self { major, minor, materials, vertex_buffers, index_buffers, meshes, nodes };
        nvr.validate()?;
        Ok(nvr)
    }

    pub fn validate(&self) -> Result<()> {
        for (i, m) in self.meshes.iter().enumerate() {
            if m.material as usize >= self.materials.len() {
                bail!("mesh {i} uses material {} of {}", m.material, self.materials.len());
            }
            for p in &m.prims {
                if p.vertex_buffer as usize >= self.vertex_buffers.len() || p.index_buffer as usize >= self.index_buffers.len() {
                    bail!("mesh {i} references a missing buffer");
                }
            }
        }
        for (i, n) in self.nodes.iter().enumerate() {
            if (n.first_mesh + n.mesh_count) as usize > self.meshes.len() {
                bail!("node {i} covers meshes past the end");
            }
            if n.child_count > 0 && (n.first_child < 0 || n.first_child as usize + n.child_count as usize > self.nodes.len()) {
                bail!("node {i} has invalid children");
            }
        }
        Ok(())
    }

    pub fn write(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut size = 28 + self.materials.len() * MATERIAL_BYTES + self.meshes.len() * MESH_BYTES + self.nodes.len() * NODE_BYTES;
        size += self.vertex_buffers.iter().map(|b| 4 + b.len()).sum::<usize>();
        size += self.index_buffers.iter().map(|b| 8 + b.data.len()).sum::<usize>();
        let mut w = Writer::with_capacity(size);
        w.bytes(b"NVR\0");
        w.u16(self.major);
        w.u16(self.minor);
        for c in [self.materials.len(), self.vertex_buffers.len(), self.index_buffers.len(), self.meshes.len(), self.nodes.len()] {
            w.i32(c as i32);
        }
        for m in &self.materials {
            match &m.raw {
                Some((orig, raw)) if m.same_as(orig) => w.bytes(raw),
                _ => {
                    w.fixed_str(&m.name, 260)?;
                    w.i32(m.kind);
                    w.u32(m.flags);
                    for c in 0..CHANNELS {
                        let ch = m.channels.get(c).cloned().unwrap_or(Channel { color: [0.0; 4], texture: String::new(), transform: [0.0; 16] });
                        w.f32s(&ch.color);
                        w.fixed_str(&ch.texture, 260)?;
                        w.f32s(&ch.transform);
                    }
                }
            }
        }
        for b in &self.vertex_buffers {
            w.u32(b.len() as u32);
            w.bytes(b);
        }
        for b in &self.index_buffers {
            w.u32(b.data.len() as u32);
            w.i32(b.format);
            w.bytes(&b.data);
        }
        for m in &self.meshes {
            w.i32(m.quality);
            w.u32(m.flags);
            w.f32s(&m.sphere);
            w.f32s(&m.bbox);
            w.u32(m.material);
            for p in &m.prims {
                for v in [p.vertex_buffer, p.first_vertex, p.vertex_count, p.index_buffer, p.first_index, p.index_count] {
                    w.u32(v);
                }
            }
        }
        for n in &self.nodes {
            w.f32s(&n.bbox);
            w.u32(n.first_mesh);
            w.u32(n.mesh_count);
            w.i32(n.first_child);
            w.u32(n.child_count);
        }
        debug_assert_eq!(w.data.len(), size);
        Ok(w.data)
    }

    fn index(&self, buffer: u32, i: u32) -> u32 {
        let b = &self.index_buffers[buffer as usize];
        if b.format == INDEX32 {
            super::bin::get_u32(&b.data, i as usize * 4)
        } else {
            super::bin::get_u16(&b.data, i as usize * 2) as u32
        }
    }

    /// Geometry of a mesh's full-vertex primitive, with indices local to the returned vertices.
    pub fn mesh_geometry(&self, mesh: usize) -> Geometry {
        let m = &self.meshes[mesh];
        let p = m.prims[0];
        let stride = self.materials[m.material as usize].vertex_size();
        let vb = &self.vertex_buffers[p.vertex_buffer as usize];
        let mut g = Geometry::default();
        let color_at = match stride {
            0x2c => 0x28,
            _ => 0x20,
        };
        for v in 0..p.vertex_count {
            let o = (p.first_vertex + v) as usize * stride;
            if o + stride > vb.len() {
                break;
            }
            g.positions.push([get_f32(vb, o), get_f32(vb, o + 4), get_f32(vb, o + 8)]);
            g.normals.push([get_f32(vb, o + 12), get_f32(vb, o + 16), get_f32(vb, o + 20)]);
            g.uvs.push([get_f32(vb, o + 24), get_f32(vb, o + 28)]);
            g.colors.push(super::bin::get_u32(vb, o + color_at));
        }
        // Index values are absolute within the buffer; Map1 meshes may or may not be rebased.
        let mut raw: Vec<u32> = (0..p.index_count).map(|i| self.index(p.index_buffer, p.first_index + i)).collect();
        let min = raw.iter().copied().min().unwrap_or(0);
        if min >= p.first_vertex {
            for x in &mut raw {
                *x -= p.first_vertex;
            }
        }
        g.indices = raw.into_iter().filter(|&i| (i as usize) < g.positions.len()).collect();
        g.indices.truncate(g.indices.len() / 3 * 3);
        g
    }

    /// Moves a mesh's vertices (both primitives) by `delta`, and updates its bounds and the nodes.
    pub fn translate_mesh(&mut self, mesh: usize, delta: [f32; 3]) {
        let m = self.meshes[mesh].clone();
        for (k, p) in m.prims.iter().enumerate() {
            let stride = if k == 0 { self.materials[m.material as usize].vertex_size() } else { 12 };
            let vb = &mut self.vertex_buffers[p.vertex_buffer as usize];
            for v in 0..p.vertex_count {
                let o = (p.first_vertex + v) as usize * stride;
                for a in 0..3 {
                    let x = get_f32(vb, o + 4 * a) + delta[a];
                    put_f32(vb, o + 4 * a, x);
                }
            }
        }
        let mm = &mut self.meshes[mesh];
        for a in 0..3 {
            mm.sphere[a] += delta[a];
            mm.bbox[a] += delta[a];
            mm.bbox[a + 3] += delta[a];
        }
        self.refit_nodes();
    }

    /// Recomputes every node's box from the meshes it covers (keeps the tree shape).
    pub fn refit_nodes(&mut self) {
        for n in &mut self.nodes {
            let mut b = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
            for i in n.first_mesh..n.first_mesh + n.mesh_count {
                let mb = self.meshes[i as usize].bbox;
                for a in 0..3 {
                    b[a] = b[a].min(mb[a]);
                    b[a + 3] = b[a + 3].max(mb[a + 3]);
                }
            }
            if n.mesh_count > 0 {
                n.bbox = b;
            }
        }
    }

    /// Adds geometry as new meshes using `material`. The meshes get their own buffers, a leaf node,
    /// and a new root whose children are the old root and that leaf, so existing culling is kept.
    pub fn append_geometry(&mut self, material: u32, geo: &Geometry, quality: i32) -> Result<usize> {
        if material as usize >= self.materials.len() {
            bail!("no material {material}");
        }
        let stride = self.materials[material as usize].vertex_size();
        let first_new = self.meshes.len();
        for chunk in split_geometry(geo, 65000) {
            let nv = chunk.positions.len();
            let mut full = vec![0u8; nv * stride];
            let mut pos = vec![0u8; nv * 12];
            for i in 0..nv {
                let o = i * stride;
                for a in 0..3 {
                    put_f32(&mut full, o + 4 * a, chunk.positions[i][a]);
                    put_f32(&mut full, o + 12 + 4 * a, chunk.normals.get(i).map_or(if a == 1 { 1.0 } else { 0.0 }, |n| n[a]));
                    put_f32(&mut pos, i * 12 + 4 * a, chunk.positions[i][a]);
                }
                let uv = chunk.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
                put_f32(&mut full, o + 24, uv[0]);
                put_f32(&mut full, o + 28, uv[1]);
                let color = chunk.colors.get(i).copied().unwrap_or(0xffffffff);
                let color_at = if stride == 0x2c { 0x28 } else { 0x20 };
                full[o + color_at..o + color_at + 4].copy_from_slice(&color.to_le_bytes());
                if stride == 0x28 {
                    full[o + 0x24..o + 0x28].copy_from_slice(&color.to_le_bytes());
                }
            }
            let ib: Vec<u8> = chunk.indices.iter().flat_map(|&i| (i as u16).to_le_bytes()).collect();
            let vb_full = self.vertex_buffers.len() as u32;
            self.vertex_buffers.push(full);
            let vb_pos = self.vertex_buffers.len() as u32;
            self.vertex_buffers.push(pos);
            let ib_full = self.index_buffers.len() as u32;
            self.index_buffers.push(IndexBuffer { format: INDEX16, data: ib.clone() });
            let ib_pos = self.index_buffers.len() as u32;
            self.index_buffers.push(IndexBuffer { format: INDEX16, data: ib });
            let (bbox, sphere) = bounds(&chunk.positions);
            let ni = chunk.indices.len() as u32;
            self.meshes.push(Mesh {
                quality,
                flags: 0,
                sphere,
                bbox,
                material,
                prims: [
                    Primitive { vertex_buffer: vb_full, first_vertex: 0, vertex_count: nv as u32, index_buffer: ib_full, first_index: 0, index_count: ni },
                    Primitive { vertex_buffer: vb_pos, first_vertex: 0, vertex_count: nv as u32, index_buffer: ib_pos, first_index: 0, index_count: ni },
                ],
            });
        }
        let added = self.meshes.len() - first_new;
        if added == 0 {
            return Ok(0);
        }
        let leaf_box = union_boxes(self.meshes[first_new..].iter().map(|m| m.bbox));
        let total = self.meshes.len() as u32;
        if self.nodes.is_empty() {
            self.nodes.push(Node { bbox: leaf_box, first_mesh: 0, mesh_count: total, first_child: -1, child_count: 0 });
            return Ok(added);
        }
        let old_root = self.nodes.len() - 1;
        let root_box = union_boxes([self.nodes[old_root].bbox, leaf_box].into_iter());
        self.nodes.push(Node { bbox: leaf_box, first_mesh: first_new as u32, mesh_count: added as u32, first_child: -1, child_count: 0 });
        self.nodes.push(Node { bbox: root_box, first_mesh: 0, mesh_count: total, first_child: old_root as i32, child_count: 2 });
        Ok(added)
    }

    pub fn add_material(&mut self, m: Material) -> u32 {
        if let Some(i) = self.materials.iter().position(|x| x.name.eq_ignore_ascii_case(&m.name)) {
            return i as u32;
        }
        self.materials.push(m);
        (self.materials.len() - 1) as u32
    }

    /// An empty 9.1 file.
    pub fn empty() -> Self {
        Self { major: 9, minor: 1, materials: Vec::new(), vertex_buffers: Vec::new(), index_buffers: Vec::new(), meshes: Vec::new(), nodes: Vec::new() }
    }

    /// Builds a complete NVR from parts, like the shipped maps: geometry is cut
    /// into tiles of `chunk` world units, ordered along a Morton curve, packed into shared buffers
    /// per material, and covered by a quadtree of nodes.
    pub fn build(materials: Vec<Material>, parts: &[(u32, Geometry)], chunk: f32) -> Result<Self> {
        let mut tiles: Vec<(u32, Geometry)> = Vec::new();
        for (mat, g) in parts {
            let mut buckets: std::collections::BTreeMap<(i64, i64), Vec<u32>> = Default::default();
            for t in g.indices.chunks_exact(3) {
                let c = |a: usize| (g.positions[t[0] as usize][a] + g.positions[t[1] as usize][a] + g.positions[t[2] as usize][a]) / 3.0;
                let key = ((c(0) / chunk).floor() as i64, (c(2) / chunk).floor() as i64);
                buckets.entry(key).or_default().extend_from_slice(t);
            }
            for (_, idx) in buckets {
                let sub = subset(g, &idx);
                for piece in split_geometry(&sub, 65000) {
                    tiles.push((*mat, piece));
                }
            }
        }
        tiles.sort_by_key(|(_, g)| morton(&g.positions));
        let mut nvr = Nvr::empty();
        nvr.materials = materials;
        struct Buf {
            full: Vec<u8>,
            idx: Vec<u8>,
            vc: u32,
            ic: u32,
        }
        let mut full_bufs: std::collections::BTreeMap<u32, Vec<Buf>> = Default::default();
        let mut pos_bufs: Vec<Buf> = Vec::new();
        let mut records: Vec<(Mesh, [(bool, u32, usize, u32, u32, u32); 2], u32)> = Vec::new();
        const MAXV: u32 = 65000;
        for (mat, t) in &tiles {
            let stride = nvr.materials[*mat as usize].vertex_size();
            let nv = t.positions.len() as u32;
            let ni = t.indices.len() as u32;
            let mut prim_refs = [(false, 0u32, 0usize, 0u32, 0u32, 0u32); 2];
            for (k, is_pos) in [(0usize, false), (1usize, true)] {
                let list = if is_pos { &mut pos_bufs } else { full_bufs.entry(*mat).or_default() };
                if list.last().is_none_or(|b| b.vc + nv > MAXV) {
                    list.push(Buf { full: Vec::new(), idx: Vec::new(), vc: 0, ic: 0 });
                }
                let slot = list.len() - 1;
                let b = list.last_mut().unwrap();
                let fv = b.vc;
                let fi = b.ic;
                for i in 0..nv as usize {
                    let p = t.positions[i];
                    for a in 0..3 {
                        b.full.extend_from_slice(&p[a].to_le_bytes());
                    }
                    if !is_pos {
                        let n = t.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
                        for a in 0..3 {
                            b.full.extend_from_slice(&n[a].to_le_bytes());
                        }
                        let uv = t.uvs.get(i).copied().unwrap_or([0.0; 2]);
                        b.full.extend_from_slice(&uv[0].to_le_bytes());
                        b.full.extend_from_slice(&uv[1].to_le_bytes());
                        let c = t.colors.get(i).copied().unwrap_or(0);
                        match stride {
                            0x2c => {
                                let uv1 = t.uvs1.get(i).copied().unwrap_or(uv);
                                b.full.extend_from_slice(&uv1[0].to_le_bytes());
                                b.full.extend_from_slice(&uv1[1].to_le_bytes());
                                b.full.extend_from_slice(&c.to_le_bytes());
                            }
                            0x28 => {
                                b.full.extend_from_slice(&c.to_le_bytes());
                                b.full.extend_from_slice(&c.to_le_bytes());
                            }
                            _ => b.full.extend_from_slice(&c.to_le_bytes()),
                        }
                    }
                }
                for &i in &t.indices {
                    b.idx.extend_from_slice(&((i + fv) as u16).to_le_bytes());
                }
                b.vc += nv;
                b.ic += ni;
                prim_refs[k] = (is_pos, *mat, slot, fv, nv, fi);
            }
            let (bbox, sphere) = bounds(&t.positions);
            records.push((Mesh { quality: -100, flags: 0, sphere, bbox, material: *mat, prims: [Primitive::default(); 2] }, prim_refs, ni));
        }
        // Buffers: full-vertex buffers by material, then position buffers.
        let mut ids: std::collections::BTreeMap<(bool, u32, usize), u32> = Default::default();
        for (mat, list) in full_bufs {
            for (slot, b) in list.into_iter().enumerate() {
                ids.insert((false, mat, slot), nvr.vertex_buffers.len() as u32);
                nvr.vertex_buffers.push(b.full);
                nvr.index_buffers.push(IndexBuffer { format: INDEX16, data: b.idx });
            }
        }
        for (slot, b) in pos_bufs.into_iter().enumerate() {
            ids.insert((true, 0, slot), nvr.vertex_buffers.len() as u32);
            nvr.vertex_buffers.push(b.full);
            nvr.index_buffers.push(IndexBuffer { format: INDEX16, data: b.idx });
        }
        for (mut mesh, refs, ni) in records {
            for k in 0..2 {
                let (is_pos, mat, slot, fv, nv, fi) = refs[k];
                let id = ids[&(is_pos, if is_pos { 0 } else { mat }, slot)];
                mesh.prims[k] = Primitive { vertex_buffer: id, first_vertex: fv, vertex_count: nv, index_buffer: id, first_index: fi, index_count: ni };
            }
            nvr.meshes.push(mesh);
        }
        nvr.nodes = quadtree(&nvr.meshes, 8);
        nvr.validate()?;
        Ok(nvr)
    }
}

/// The node layout of the shipped maps: children split each range in four, root last.
pub fn quadtree(meshes: &[Mesh], leaf: usize) -> Vec<Node> {
    fn boxed(meshes: &[Mesh], a: usize, b: usize) -> [f32; 6] {
        union_boxes(meshes[a..b].iter().map(|m| m.bbox))
    }
    fn emit(meshes: &[Mesh], nodes: &mut Vec<Node>, rec: &mut Node, a: usize, b: usize, leaf: usize) {
        if b - a <= leaf {
            return;
        }
        let k = b - a;
        let cuts: Vec<usize> = (0..5).map(|i| a + k * i / 4).collect();
        let kids: Vec<(usize, usize)> = (0..4).filter(|&i| cuts[i + 1] > cuts[i]).map(|i| (cuts[i], cuts[i + 1])).collect();
        let mut recs: Vec<Node> = kids
            .iter()
            .map(|&(x, y)| Node { bbox: boxed(meshes, x, y), first_mesh: x as u32, mesh_count: (y - x) as u32, first_child: -1, child_count: 0 })
            .collect();
        for (r, &(x, y)) in recs.iter_mut().zip(&kids) {
            emit(meshes, nodes, r, x, y, leaf);
        }
        rec.first_child = nodes.len() as i32;
        rec.child_count = recs.len() as u32;
        nodes.extend(recs);
    }
    let mut nodes = Vec::new();
    let mut root = Node { bbox: boxed(meshes, 0, meshes.len()), first_mesh: 0, mesh_count: meshes.len() as u32, first_child: -1, child_count: 0 };
    emit(meshes, &mut nodes, &mut root, 0, meshes.len(), leaf);
    nodes.push(root);
    nodes
}

pub fn union_boxes(it: impl Iterator<Item = [f32; 6]>) -> [f32; 6] {
    let mut b = [f32::MAX, f32::MAX, f32::MAX, -f32::MAX, -f32::MAX, -f32::MAX];
    for m in it {
        for a in 0..3 {
            b[a] = b[a].min(m[a]);
            b[a + 3] = b[a + 3].max(m[a + 3]);
        }
    }
    b
}

pub fn bounds(p: &[[f32; 3]]) -> ([f32; 6], [f32; 4]) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [-f32::MAX; 3];
    for v in p {
        for a in 0..3 {
            lo[a] = lo[a].min(v[a]);
            hi[a] = hi[a].max(v[a]);
        }
    }
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let r = p
        .iter()
        .map(|v| ((v[0] - c[0]).powi(2) + (v[1] - c[1]).powi(2) + (v[2] - c[2]).powi(2)).sqrt())
        .fold(0.0f32, f32::max);
    ([lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]], [c[0], c[1], c[2], r])
}

fn morton(p: &[[f32; 3]]) -> u64 {
    let n = p.len().max(1) as f32;
    let cx = p.iter().map(|v| v[0]).sum::<f32>() / n;
    let cz = p.iter().map(|v| v[2]).sum::<f32>() / n;
    let qx = ((cx + 32768.0) / 4.0).clamp(0.0, 65535.0) as u64;
    let qz = ((cz + 32768.0) / 4.0).clamp(0.0, 65535.0) as u64;
    let spread = |mut v: u64| {
        v &= 0xffff;
        v = (v | (v << 8)) & 0x00ff00ff;
        v = (v | (v << 4)) & 0x0f0f0f0f;
        v = (v | (v << 2)) & 0x33333333;
        v = (v | (v << 1)) & 0x55555555;
        v
    };
    spread(qx) | (spread(qz) << 1)
}

/// The vertices used by `indices` (a triangle list into `g`), re-indexed.
pub fn subset(g: &Geometry, indices: &[u32]) -> Geometry {
    let mut map = std::collections::HashMap::new();
    let mut out = Geometry::default();
    for &i in indices {
        let k = *map.entry(i).or_insert_with(|| {
            let i = i as usize;
            out.positions.push(g.positions[i]);
            if let Some(n) = g.normals.get(i) {
                out.normals.push(*n);
            }
            if let Some(u) = g.uvs.get(i) {
                out.uvs.push(*u);
            }
            if let Some(u) = g.uvs1.get(i) {
                out.uvs1.push(*u);
            }
            if let Some(c) = g.colors.get(i) {
                out.colors.push(*c);
            }
            (out.positions.len() - 1) as u32
        });
        out.indices.push(k);
    }
    out
}

/// Splits a triangle list into pieces of at most `max_vertices` vertices (16-bit indices).
pub fn split_geometry(g: &Geometry, max_vertices: usize) -> Vec<Geometry> {
    if g.positions.len() <= max_vertices {
        return vec![g.clone()];
    }
    let mut out = Vec::new();
    let mut cur: Vec<u32> = Vec::new();
    let mut used = std::collections::HashSet::new();
    for t in g.indices.chunks_exact(3) {
        let new = t.iter().filter(|i| !used.contains(*i)).count();
        if used.len() + new > max_vertices {
            out.push(subset(g, &cur));
            cur.clear();
            used.clear();
        }
        cur.extend_from_slice(t);
        used.extend(t.iter().copied());
    }
    if !cur.is_empty() {
        out.push(subset(g, &cur));
    }
    out
}

impl Nvr {
    /// Drops the remembered raw material records, so `write` encodes every field (used by tests).
    pub fn forget_raw(&mut self) {
        for m in &mut self.materials {
            m.raw = None;
        }
    }
}
