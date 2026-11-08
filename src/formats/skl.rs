//! Character skeletons (`*.skl`): the current format (format token 0x22FD4FC3) and the legacy
//! "r3d2sklt" versions 1 and 2.

use super::bin::{Reader, fixed_str};
use anyhow::{Result, bail};

#[derive(Clone, Debug)]
pub struct Joint {
    pub name: String,
    pub hash: u32,
    pub parent: i32,
    pub local_t: [f32; 3],
    pub local_s: [f32; 3],
    pub local_r: [f32; 4],
    /// Inverse bind pose (model space → joint space).
    pub inv_bind: Mat4,
}

pub type Mat4 = [f32; 16];

#[derive(Clone, Debug)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
    /// Skin vertex bone index → joint index.
    pub influences: Vec<u16>,
}

/// The hash animations use to name joints (ELF hash of the lowercase name).
pub fn elf_hash(name: &str) -> u32 {
    let mut h: u32 = 0;
    for c in name.bytes() {
        h = (h << 4).wrapping_add(c.to_ascii_lowercase() as u32);
        let g = h & 0xf000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

pub fn identity() -> Mat4 {
    [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
}

/// Column-major product a * b.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [0.0; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

/// Translation · rotation · scale.
pub fn compose(t: [f32; 3], r: [f32; 4], s: [f32; 3]) -> Mat4 {
    let [x, y, z, w] = r;
    let (xx, yy, zz, xy, xz, yz, wx, wy, wz) = (x * x, y * y, z * z, x * y, x * z, y * z, w * x, w * y, w * z);
    [
        (1.0 - 2.0 * (yy + zz)) * s[0],
        (2.0 * (xy + wz)) * s[0],
        (2.0 * (xz - wy)) * s[0],
        0.0,
        (2.0 * (xy - wz)) * s[1],
        (1.0 - 2.0 * (xx + zz)) * s[1],
        (2.0 * (yz + wx)) * s[1],
        0.0,
        (2.0 * (xz + wy)) * s[2],
        (2.0 * (yz - wx)) * s[2],
        (1.0 - 2.0 * (xx + yy)) * s[2],
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

/// Inverse of an affine matrix.
pub fn invert_affine(m: &Mat4) -> Mat4 {
    let a = [[m[0], m[4], m[8]], [m[1], m[5], m[9]], [m[2], m[6], m[10]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    let d = if det.abs() < 1e-12 { 1.0 } else { 1.0 / det };
    let inv = [
        [(a[1][1] * a[2][2] - a[1][2] * a[2][1]) * d, (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * d, (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * d],
        [(a[1][2] * a[2][0] - a[1][0] * a[2][2]) * d, (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * d, (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * d],
        [(a[1][0] * a[2][1] - a[1][1] * a[2][0]) * d, (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * d, (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * d],
    ];
    let t = [m[12], m[13], m[14]];
    let it = [
        -(inv[0][0] * t[0] + inv[0][1] * t[1] + inv[0][2] * t[2]),
        -(inv[1][0] * t[0] + inv[1][1] * t[1] + inv[1][2] * t[2]),
        -(inv[2][0] * t[0] + inv[2][1] * t[1] + inv[2][2] * t[2]),
    ];
    [inv[0][0], inv[1][0], inv[2][0], 0.0, inv[0][1], inv[1][1], inv[2][1], 0.0, inv[0][2], inv[1][2], inv[2][2], 0.0, it[0], it[1], it[2], 1.0]
}

/// Rotation quaternion of an orthonormal-ish 3x3 (column-major in m).
fn quat_from(m: &Mat4) -> [f32; 4] {
    let (m00, m11, m22) = (m[0], m[5], m[10]);
    let tr = m00 + m11 + m22;
    let q = if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        [(m[6] - m[9]) / s, (m[8] - m[2]) / s, (m[1] - m[4]) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [0.25 * s, (m[4] + m[1]) / s, (m[8] + m[2]) / s, (m[6] - m[9]) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m[4] + m[1]) / s, 0.25 * s, (m[9] + m[6]) / s, (m[8] - m[2]) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m[8] + m[2]) / s, (m[9] + m[6]) / s, 0.25 * s, (m[1] - m[4]) / s]
    };
    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-9);
    q.map(|v| v / l)
}

impl Skeleton {
    pub fn parse(d: &[u8]) -> Result<Self> {
        if d.len() >= 8 && &d[0..8] == b"r3d2sklt" {
            return Self::parse_legacy(d);
        }
        let mut r = Reader::new(d);
        let _size = r.u32()?;
        if r.u32()? != 0x22fd4fc3 {
            bail!("not a skeleton");
        }
        let _version = r.u32()?;
        let _flags = r.u16()?;
        let count = r.u16()? as usize;
        let n_infl = r.u32()? as usize;
        let joints_at = r.i32()? as usize;
        let _joint_index_at = r.i32()?;
        let infl_at = r.i32()? as usize;
        let mut joints = Vec::with_capacity(count);
        for i in 0..count {
            let base = joints_at + i * 100;
            let mut j = Reader::new(&d[base..]);
            let _flags = j.u16()?;
            let _id = j.i16()?;
            let parent = j.i16()? as i32;
            j.u16()?;
            let hash = j.u32()?;
            let _radius = j.f32()?;
            let local_t = j.vec3()?;
            let local_s = j.vec3()?;
            let local_r: [f32; 4] = j.f32s()?;
            let ib_t = j.vec3()?;
            let ib_s = j.vec3()?;
            let ib_r: [f32; 4] = j.f32s()?;
            let name_off = j.i32()?;
            let name_at = (base + 96) as i64 + name_off as i64;
            let name = if name_at > 0 && (name_at as usize) < d.len() { fixed_str(&d[name_at as usize..(name_at as usize + 64).min(d.len())]) } else { String::new() };
            joints.push(Joint { name, hash, parent, local_t, local_s, local_r, inv_bind: compose(ib_t, ib_r, ib_s) });
        }
        let mut influences = Vec::with_capacity(n_infl);
        let mut ir = Reader::new(&d[infl_at..]);
        for _ in 0..n_infl {
            influences.push(ir.u16()?);
        }
        Ok(Self { joints, influences })
    }

    /// Legacy skeletons store each joint's model-space transform.
    fn parse_legacy(d: &[u8]) -> Result<Self> {
        let mut r = Reader::new(d);
        r.skip(8)?;
        let version = r.u32()?;
        let _id = r.u32()?;
        let count = r.u32()? as usize;
        let mut globals = Vec::with_capacity(count);
        let mut joints = Vec::with_capacity(count);
        for _ in 0..count {
            let name = fixed_str(r.bytes(32)?);
            let parent = r.i32()?;
            let _scale = r.f32()?;
            let m: [f32; 12] = r.f32s()?;
            // Rows of a 3x4 (rotation | translation).
            let g = [m[0], m[4], m[8], 0.0, m[1], m[5], m[9], 0.0, m[2], m[6], m[10], 0.0, m[3], m[7], m[11], 1.0];
            globals.push(g);
            joints.push(Joint { hash: elf_hash(&name), name, parent, local_t: [0.0; 3], local_s: [1.0; 3], local_r: [0.0, 0.0, 0.0, 1.0], inv_bind: invert_affine(&g) });
        }
        for i in 0..count {
            let p = joints[i].parent;
            let local = if p >= 0 && (p as usize) < count { mul(&invert_affine(&globals[p as usize]), &globals[i]) } else { globals[i] };
            joints[i].local_t = [local[12], local[13], local[14]];
            joints[i].local_r = quat_from(&local);
        }
        let influences = if version >= 2 {
            let n = r.u32()? as usize;
            (0..n).map(|_| r.u32().map(|v| v as u16)).collect::<Result<_>>()?
        } else {
            (0..count as u16).collect()
        };
        Ok(Self { joints, influences })
    }
}
