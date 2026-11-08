//! Mesh operations backing the Model Studio (a Blender-like editor for
//! League of Legends custom assets).
//!
//! Conventions:
//! * Y is up; League props stand on the Y = 0 plane.
//! * Triangles are counter-clockwise when seen from the front: the geometric
//!   face normal is `(b - a) x (c - a)` (right-handed).
//! * [`Mat4`] is column-major (`m[col * 4 + row]`), column vectors (`M * v`).
//! * [`trs`] builds `T * Ry * Rx * Rz * S` (Euler degrees, right-handed:
//!   +90 deg about Y maps +X to -Z).
//! * UV v grows downward (DirectX / League convention).
//! * Every function tolerates empty meshes, short attribute arrays and
//!   out-of-range indices (invalid triangles are skipped or dropped).

use std::collections::{HashMap, HashSet};
use std::f32::consts::PI;

/// Simple editable triangle mesh.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Per-vertex RGBA 0..1 (vertex paint / tint); may be empty = white.
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>, // triangles
}

/// Column-major 4x4 matrix (`m[col * 4 + row]`).
pub type Mat4 = [f32; 16];

/// The identity matrix.
pub const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Where [`recenter`] puts the mesh origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Pivot {
    /// Centre X/Z, min Y = 0 (League props stand on the ground plane).
    BottomCenter,
    /// Bounding-box centre at the origin.
    Center,
    /// Keep the current origin (no-op).
    Origin,
}

// ---------------------------------------------------------------- vectors

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
/// Normalized vector, or zero when the input is (near) zero.
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = length(a);
    if l > 1e-20 && l.is_finite() { mul(a, 1.0 / l) } else { [0.0; 3] }
}
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

// ---------------------------------------------------------------- matrices

/// `a * b` (apply `b` first, then `a`).
pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [0.0; 16];
    for c in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + row] * b[c * 4 + k];
            }
            r[c * 4 + row] = s;
        }
    }
    r
}

/// General 4x4 inverse; `None` when singular.
pub fn mat_invert(m: &Mat4) -> Option<Mat4> {
    let mut inv = [0.0f32; 16];
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
        + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
        - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
        + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
        - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
        - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
        + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
        - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
        + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
        + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
        - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
        + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
        - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
        - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
        + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
        - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
        + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
    let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    let id = 1.0 / det;
    for v in &mut inv {
        *v *= id;
    }
    Some(inv)
}

/// Translate * Ry * Rx * Rz * Scale, with rotations in degrees.
pub fn trs(translate: [f32; 3], rotate_deg: [f32; 3], scale: [f32; 3]) -> Mat4 {
    let (sx, cx) = rotate_deg[0].to_radians().sin_cos();
    let (sy, cy) = rotate_deg[1].to_radians().sin_cos();
    let (sz, cz) = rotate_deg[2].to_radians().sin_cos();
    let rx: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, cx, sx, 0.0, 0.0, -sx, cx, 0.0, 0.0, 0.0, 0.0, 1.0];
    let ry: Mat4 = [cy, 0.0, -sy, 0.0, 0.0, 1.0, 0.0, 0.0, sy, 0.0, cy, 0.0, 0.0, 0.0, 0.0, 1.0];
    let rz: Mat4 = [cz, sz, 0.0, 0.0, -sz, cz, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let mut r = mat_mul(&mat_mul(&ry, &rx), &rz);
    for c in 0..3 {
        for row in 0..3 {
            r[c * 4 + row] *= scale[c];
        }
    }
    r[12] = translate[0];
    r[13] = translate[1];
    r[14] = translate[2];
    r
}

/// Transform a point (w = 1, no perspective divide).
pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// Transform a point to homogeneous clip space `[x, y, z, w]`.
pub fn transform_point4(m: &Mat4, p: [f32; 3]) -> [f32; 4] {
    let t = transform_point(m, p);
    [t[0], t[1], t[2], m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15]]
}

fn det3(m: &Mat4) -> f32 {
    m[0] * (m[5] * m[10] - m[9] * m[6]) - m[4] * (m[1] * m[10] - m[9] * m[2])
        + m[8] * (m[1] * m[6] - m[5] * m[2])
}

/// Row-major 3x3 normal matrix (inverse-transpose of the upper 3x3).
fn normal_matrix(m: &Mat4) -> [[f32; 3]; 3] {
    let mut r = [[0.0; 3]; 3];
    match mat_invert(m) {
        Some(inv) => {
            for (row, rr) in r.iter_mut().enumerate() {
                for (c, v) in rr.iter_mut().enumerate() {
                    *v = inv[row * 4 + c];
                }
            }
        }
        None => {
            for (row, rr) in r.iter_mut().enumerate() {
                for (c, v) in rr.iter_mut().enumerate() {
                    *v = m[c * 4 + row];
                }
            }
        }
    }
    r
}

fn apply3(nm: &[[f32; 3]; 3], n: [f32; 3]) -> [f32; 3] {
    [dot(nm[0], n), dot(nm[1], n), dot(nm[2], n)]
}

fn flip_winding(m: &mut Mesh) {
    for t in m.indices.chunks_exact_mut(3) {
        t.swap(1, 2);
    }
}

/// Transform the whole mesh. Normals use the inverse-transpose and are
/// renormalized; winding is flipped when the transform mirrors (det < 0).
pub fn transform(mesh: &mut Mesh, m: &Mat4) {
    for p in &mut mesh.positions {
        *p = transform_point(m, *p);
    }
    let nm = normal_matrix(m);
    for n in &mut mesh.normals {
        *n = normalize(apply3(&nm, *n));
    }
    if det3(m) < 0.0 {
        flip_winding(mesh);
    }
}

/// Transform only the given vertices (positions and normals; winding untouched).
pub fn transform_selected(mesh: &mut Mesh, verts: &[u32], m: &Mat4) {
    let nm = normal_matrix(m);
    let set: HashSet<u32> = verts.iter().copied().collect();
    for v in set {
        let v = v as usize;
        if let Some(p) = mesh.positions.get_mut(v) {
            *p = transform_point(m, *p);
        }
        if let Some(n) = mesh.normals.get_mut(v) {
            *n = normalize(apply3(&nm, *n));
        }
    }
}

// ---------------------------------------------------------------- bounds / pivot

/// Axis-aligned bounds `(min, max)`; zeros for an empty mesh.
pub fn bounds(m: &Mesh) -> ([f32; 3], [f32; 3]) {
    if m.positions.is_empty() {
        return ([0.0; 3], [0.0; 3]);
    }
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for p in &m.positions {
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
    }
    (mn, mx)
}

/// Bounding-box centre.
pub fn center(m: &Mesh) -> [f32; 3] {
    let (mn, mx) = bounds(m);
    mul(add(mn, mx), 0.5)
}

/// Move the mesh so its pivot matches `mode`; returns the applied offset.
pub fn recenter(m: &mut Mesh, mode: Pivot) -> [f32; 3] {
    let (mn, mx) = bounds(m);
    let off = match mode {
        Pivot::BottomCenter => [-(mn[0] + mx[0]) * 0.5, -mn[1], -(mn[2] + mx[2]) * 0.5],
        Pivot::Center => mul(add(mn, mx), -0.5),
        Pivot::Origin => [0.0; 3],
    };
    for p in &mut m.positions {
        *p = add(*p, off);
    }
    off
}

// ---------------------------------------------------------------- internals

#[derive(Clone, Copy)]
struct V {
    p: [f32; 3],
    n: [f32; 3],
    t: [f32; 2],
    c: [f32; 4],
}

fn getv(m: &Mesh, i: usize) -> V {
    V {
        p: m.positions[i],
        n: m.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
        t: m.uvs.get(i).copied().unwrap_or([0.0; 2]),
        c: m.colors.get(i).copied().unwrap_or([1.0; 4]),
    }
}

/// Push a vertex; the mesh must have padded attributes.
fn pushv(m: &mut Mesh, v: V, colors: bool) -> u32 {
    let i = m.positions.len() as u32;
    m.positions.push(v.p);
    m.normals.push(v.n);
    m.uvs.push(v.t);
    if colors {
        m.colors.push(v.c);
    }
    i
}

fn lerpv(a: V, b: V, t: f32) -> V {
    V {
        p: lerp3(a.p, b.p, t),
        n: {
            let n = normalize(lerp3(a.n, b.n, t));
            if n == [0.0; 3] { a.n } else { n }
        },
        t: [a.t[0] + (b.t[0] - a.t[0]) * t, a.t[1] + (b.t[1] - a.t[1]) * t],
        c: [
            a.c[0] + (b.c[0] - a.c[0]) * t,
            a.c[1] + (b.c[1] - a.c[1]) * t,
            a.c[2] + (b.c[2] - a.c[2]) * t,
            a.c[3] + (b.c[3] - a.c[3]) * t,
        ],
    }
}

/// Resize normals/uvs (and colors, if present) to match positions.
fn pad_attrs(m: &mut Mesh) {
    let n = m.positions.len();
    m.normals.resize(n, [0.0, 1.0, 0.0]);
    m.uvs.resize(n, [0.0; 2]);
    if !m.colors.is_empty() {
        m.colors.resize(n, [1.0; 4]);
    }
}

/// Drop triangles that reference missing vertices (and a trailing partial one).
fn drop_invalid(m: &mut Mesh) {
    let n = m.positions.len();
    let mut idx = Vec::with_capacity(m.indices.len());
    for t in m.indices.chunks_exact(3) {
        if t.iter().all(|&i| (i as usize) < n) {
            idx.extend_from_slice(t);
        }
    }
    m.indices = idx;
}

/// Pad attribute arrays to the vertex count and drop invalid triangles.
pub fn sanitize(m: &mut Mesh) {
    pad_attrs(m);
    drop_invalid(m);
}

fn tri_valid(m: &Mesh, f: usize) -> Option<[usize; 3]> {
    let t = m.indices.get(f * 3..f * 3 + 3)?;
    let n = m.positions.len();
    let t = [t[0] as usize, t[1] as usize, t[2] as usize];
    if t.iter().all(|&i| i < n) { Some(t) } else { None }
}

fn face_cross(m: &Mesh, t: [usize; 3]) -> [f32; 3] {
    let a = m.positions[t[0]];
    cross(sub(m.positions[t[1]], a), sub(m.positions[t[2]], a))
}

fn pos_key(p: [f32; 3], eps: f32) -> (i64, i64, i64) {
    (
        (p[0] / eps).round() as i64,
        (p[1] / eps).round() as i64,
        (p[2] / eps).round() as i64,
    )
}

/// Group id (a representative vertex index) per vertex, by quantized position.
fn pos_groups(m: &Mesh, eps: f32) -> Vec<usize> {
    let mut map: HashMap<(i64, i64, i64), usize> = HashMap::new();
    m.positions
        .iter()
        .enumerate()
        .map(|(i, &p)| *map.entry(pos_key(p, eps)).or_insert(i))
        .collect()
}

struct Uf(Vec<usize>);
impl Uf {
    fn new(n: usize) -> Self {
        Uf((0..n).collect())
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }
}

/// Union-find over vertices linked by triangles and shared positions.
fn link_components(m: &Mesh) -> Uf {
    let n = m.positions.len();
    let mut uf = Uf::new(n);
    for t in m.indices.chunks_exact(3) {
        if t.iter().all(|&i| (i as usize) < n) {
            uf.union(t[0] as usize, t[1] as usize);
            uf.union(t[0] as usize, t[2] as usize);
        }
    }
    for (i, g) in pos_groups(m, 1e-4).into_iter().enumerate() {
        uf.union(i, g);
    }
    uf
}

/// Swap winding of triangles whose geometric normal disagrees with the
/// summed vertex normals.
fn orient_to_normals(m: &mut Mesh) {
    pad_attrs(m);
    drop_invalid(m);
    for f in 0..m.indices.len() / 3 {
        let t = [
            m.indices[f * 3] as usize,
            m.indices[f * 3 + 1] as usize,
            m.indices[f * 3 + 2] as usize,
        ];
        let g = face_cross(m, t);
        let s = add(add(m.normals[t[0]], m.normals[t[1]]), m.normals[t[2]]);
        if dot(g, s) < 0.0 {
            m.indices.swap(f * 3 + 1, f * 3 + 2);
        }
    }
}

/// Drop triangles with repeated indices or (near) zero area.
fn drop_degenerate(m: &mut Mesh, area_eps: f32) {
    drop_invalid(m);
    let mut idx = Vec::with_capacity(m.indices.len());
    for t in m.indices.chunks_exact(3) {
        let tu = [t[0] as usize, t[1] as usize, t[2] as usize];
        if t[0] == t[1] || t[1] == t[2] || t[0] == t[2] {
            continue;
        }
        if length(face_cross(m, tu)) <= area_eps {
            continue;
        }
        idx.extend_from_slice(t);
    }
    m.indices = idx;
}

// ---------------------------------------------------------------- normals

/// Recompute vertex normals from faces.
/// `None`: fully smooth (angle weighted), averaging over vertices sharing a position (1e-4),
/// vertices are kept. `Some(angle)`: faces whose normals differ by more than
/// `angle` degrees are split (vertices duplicated) so those edges stay sharp.
pub fn recompute_normals(m: &mut Mesh, smooth_angle_deg: Option<f32>) {
    sanitize(m);
    let n = m.positions.len();
    if n == 0 {
        return;
    }
    let g = pos_groups(m, 1e-4);
    let nf = m.indices.len() / 3;
    let tris: Vec<[usize; 3]> = (0..nf).filter_map(|f| tri_valid(m, f)).collect();
    let fnr: Vec<[f32; 3]> = tris.iter().map(|&t| face_cross(m, t)).collect();
    match smooth_angle_deg {
        None => {
            let mut acc = vec![[0.0f32; 3]; n];
            // angle-weighted unit face normals (symmetric regardless of diagonals)
            for (t, f) in tris.iter().zip(&fnr) {
                let un = normalize(*f);
                for k in 0..3 {
                    let v = t[k];
                    let e1 = normalize(sub(m.positions[t[(k + 1) % 3]], m.positions[v]));
                    let e2 = normalize(sub(m.positions[t[(k + 2) % 3]], m.positions[v]));
                    let ang = dot(e1, e2).clamp(-1.0, 1.0).acos();
                    acc[g[v]] = add(acc[g[v]], mul(un, ang));
                }
            }
            for v in 0..n {
                let s = normalize(acc[g[v]]);
                if s != [0.0; 3] {
                    m.normals[v] = s;
                }
            }
        }
        Some(angle) => {
            let cos_t = angle.clamp(0.0, 180.0).to_radians().cos() - 1e-6;
            let unit: Vec<[f32; 3]> = fnr.iter().map(|&f| normalize(f)).collect();
            let mut gf: Vec<Vec<usize>> = vec![Vec::new(); n];
            for (fi, t) in tris.iter().enumerate() {
                for &v in t {
                    if gf[g[v]].last() != Some(&fi) {
                        gf[g[v]].push(fi);
                    }
                }
            }
            let colors = !m.colors.is_empty();
            let mut variants: Vec<Vec<([f32; 3], u32)>> = vec![Vec::new(); n];
            for (fi, t) in tris.iter().enumerate() {
                for (k, &v) in t.iter().enumerate() {
                    let mut s = [0.0; 3];
                    for &of in &gf[g[v]] {
                        if dot(unit[of], unit[fi]) >= cos_t {
                            s = add(s, fnr[of]);
                        }
                    }
                    let mut nn = normalize(s);
                    if nn == [0.0; 3] {
                        nn = unit[fi];
                    }
                    let found = variants[v].iter().find(|(e, _)| dot(*e, nn) > 0.9999).map(|x| x.1);
                    let idx = match found {
                        Some(i) => i,
                        None if variants[v].is_empty() => {
                            m.normals[v] = nn;
                            variants[v].push((nn, v as u32));
                            v as u32
                        }
                        None => {
                            let mut vv = getv(m, v);
                            vv.n = nn;
                            let i = pushv(m, vv, colors);
                            variants[v].push((nn, i));
                            i
                        }
                    };
                    m.indices[fi * 3 + k] = idx;
                }
            }
        }
    }
}

/// Unshare every vertex and assign face normals (faceted look).
pub fn flat_shade(m: &mut Mesh) {
    sanitize(m);
    let colors = !m.colors.is_empty();
    let mut out = Mesh::default();
    for f in 0..m.indices.len() / 3 {
        let Some(t) = tri_valid(m, f) else { continue };
        let nn = normalize(face_cross(m, t));
        for &v in &t {
            let mut vv = getv(m, v);
            if nn != [0.0; 3] {
                vv.n = nn;
            }
            pushv(&mut out, vv, colors);
        }
    }
    out.indices = (0..out.positions.len() as u32).collect();
    *m = out;
}

/// Flip normals and triangle winding.
pub fn flip(m: &mut Mesh) {
    for n in &mut m.normals {
        *n = mul(*n, -1.0);
    }
    flip_winding(m);
}

// ---------------------------------------------------------------- topology

/// Remove unused vertices (and invalid triangles); returns old->new remap.
fn compact_remap(m: &mut Mesh) -> Vec<Option<u32>> {
    sanitize(m);
    let n = m.positions.len();
    let mut used = vec![false; n];
    for &i in &m.indices {
        used[i as usize] = true;
    }
    let mut remap = vec![None; n];
    let colors = !m.colors.is_empty();
    let mut out = Mesh::default();
    for i in 0..n {
        if used[i] {
            remap[i] = Some(pushv(&mut out, getv(m, i), colors));
        }
    }
    out.indices = m.indices.iter().map(|&i| remap[i as usize].unwrap()).collect();
    *m = out;
    remap
}

/// Drop vertices not referenced by any (valid) triangle.
pub fn compact(m: &mut Mesh) {
    compact_remap(m);
}

/// Merge vertices whose position and uv are within `epsilon`; degenerate
/// triangles are dropped and the mesh compacted. Returns removed vertex count.
pub fn weld(m: &mut Mesh, epsilon: f32) -> usize {
    sanitize(m);
    let n = m.positions.len();
    let eps = if epsilon > 0.0 && epsilon.is_finite() { epsilon } else { 1e-7 };
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    let mut remap = vec![0u32; n];
    for i in 0..n {
        let p = m.positions[i];
        let uv = m.uvs[i];
        let key = (
            (p[0] / eps).floor() as i64,
            (p[1] / eps).floor() as i64,
            (p[2] / eps).floor() as i64,
        );
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(list) = grid.get(&(key.0 + dx, key.1 + dy, key.2 + dz)) {
                        for &j in list {
                            let q = m.positions[j as usize];
                            let quv = m.uvs[j as usize];
                            if (0..3).all(|k| (p[k] - q[k]).abs() <= eps)
                                && (uv[0] - quv[0]).abs() <= eps
                                && (uv[1] - quv[1]).abs() <= eps
                            {
                                found = Some(j);
                                break 'search;
                            }
                        }
                    }
                }
            }
        }
        remap[i] = match found {
            Some(j) => j,
            None => {
                grid.entry(key).or_default().push(i as u32);
                i as u32
            }
        };
    }
    let mut idx = Vec::with_capacity(m.indices.len());
    for t in m.indices.chunks_exact(3) {
        let r = [remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]];
        if r[0] != r[1] && r[1] != r[2] && r[0] != r[2] {
            idx.extend_from_slice(&r);
        }
    }
    m.indices = idx;
    compact(m);
    n - m.positions.len()
}

/// Split into connected components (linked by triangles or shared positions).
pub fn split_loose(m: &Mesh) -> Vec<Mesh> {
    let mut uf = link_components(m);
    let mut groups: Vec<(usize, Vec<u32>)> = Vec::new();
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for f in 0..m.indices.len() / 3 {
        let Some(t) = tri_valid(m, f) else { continue };
        let r = uf.find(t[0]);
        let s = *slot.entry(r).or_insert_with(|| {
            groups.push((r, Vec::new()));
            groups.len() - 1
        });
        groups[s].1.push(f as u32);
    }
    groups.into_iter().map(|(_, faces)| extract_faces(m, &faces)).collect()
}

/// Concatenate two meshes (attributes padded; colors kept if either has them).
pub fn merge(a: &Mesh, b: &Mesh) -> Mesh {
    let mut a2 = a.clone();
    let mut b2 = b.clone();
    sanitize(&mut a2);
    sanitize(&mut b2);
    let colors = !a2.colors.is_empty() || !b2.colors.is_empty();
    if colors {
        a2.colors.resize(a2.positions.len(), [1.0; 4]);
        b2.colors.resize(b2.positions.len(), [1.0; 4]);
    }
    let off = a2.positions.len() as u32;
    a2.positions.extend_from_slice(&b2.positions);
    a2.normals.extend_from_slice(&b2.normals);
    a2.uvs.extend_from_slice(&b2.uvs);
    a2.colors.extend_from_slice(&b2.colors);
    a2.indices.extend(b2.indices.iter().map(|&i| i + off));
    a2
}

/// Remove the given faces (triangle ids) and drop now-unused vertices.
pub fn remove_faces(m: &mut Mesh, faces: &[u32]) {
    let sel: HashSet<u32> = faces.iter().copied().collect();
    let mut idx = Vec::with_capacity(m.indices.len());
    for (f, t) in m.indices.chunks_exact(3).enumerate() {
        if !sel.contains(&(f as u32)) {
            idx.extend_from_slice(t);
        }
    }
    m.indices = idx;
    compact(m);
}

/// Copy the given faces into a new compacted mesh.
pub fn extract_faces(m: &Mesh, faces: &[u32]) -> Mesh {
    let mut out = Mesh { indices: Vec::new(), ..m.clone() };
    let mut seen = HashSet::new();
    for &f in faces {
        if seen.insert(f) {
            if let Some(t) = tri_valid(m, f as usize) {
                out.indices.extend(t.iter().map(|&i| i as u32));
            }
        }
    }
    compact(&mut out);
    out
}

/// Sorted ids of faces that reference any of `verts`.
pub fn faces_using(m: &Mesh, verts: &[u32]) -> Vec<u32> {
    let set: HashSet<u32> = verts.iter().copied().collect();
    m.indices
        .chunks_exact(3)
        .enumerate()
        .filter(|(_, t)| t.iter().any(|i| set.contains(i)))
        .map(|(f, _)| f as u32)
        .collect()
}

/// "Select linked": all vertices connected to `seed` (through triangles or
/// shared positions), sorted.
pub fn linked_vertices(m: &Mesh, seed: &[u32]) -> Vec<u32> {
    let n = m.positions.len();
    let mut uf = link_components(m);
    let roots: HashSet<usize> =
        seed.iter().filter(|&&s| (s as usize) < n).map(|&s| uf.find(s as usize)).collect();
    if roots.is_empty() {
        return Vec::new();
    }
    (0..n).filter(|&i| roots.contains(&uf.find(i))).map(|i| i as u32).collect()
}

/// Vertices whose projection lies inside `rect_ndc = [x0, y0, x1, y1]`
/// (any corner order) with clip w > 0.
pub fn select_by_box(m: &Mesh, mvp: &Mat4, rect_ndc: [f32; 4]) -> Vec<u32> {
    let (x0, x1) = (rect_ndc[0].min(rect_ndc[2]), rect_ndc[0].max(rect_ndc[2]));
    let (y0, y1) = (rect_ndc[1].min(rect_ndc[3]), rect_ndc[1].max(rect_ndc[3]));
    let mut out = Vec::new();
    for (i, &p) in m.positions.iter().enumerate() {
        let c = transform_point4(mvp, p);
        if c[3] <= 0.0 {
            continue;
        }
        let (x, y) = (c[0] / c[3], c[1] / c[3]);
        if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
            out.push(i as u32);
        }
    }
    out
}

/// Closest vertex (in NDC distance) to `ndc` within `radius_ndc`, w > 0.
pub fn pick_vertex(m: &Mesh, mvp: &Mat4, ndc: [f32; 2], radius_ndc: f32) -> Option<u32> {
    let mut best: Option<(u32, f32, f32)> = None;
    let r2 = radius_ndc * radius_ndc;
    for (i, &p) in m.positions.iter().enumerate() {
        let c = transform_point4(mvp, p);
        if c[3] <= 0.0 {
            continue;
        }
        let (dx, dy) = (c[0] / c[3] - ndc[0], c[1] / c[3] - ndc[1]);
        let d2 = dx * dx + dy * dy;
        let depth = c[2] / c[3];
        if d2 <= r2 {
            let better = match best {
                None => true,
                Some((_, bd, bz)) => d2 < bd - 1e-12 || ((d2 - bd).abs() <= 1e-12 && depth < bz),
            };
            if better {
                best = Some((i as u32, d2, depth));
            }
        }
    }
    best.map(|b| b.0)
}

/// Nearest face hit by the ray (two-sided Moller-Trumbore): `(face, t)`.
pub fn pick_face(m: &Mesh, ray_origin: [f32; 3], ray_dir: [f32; 3]) -> Option<(u32, f32)> {
    let mut best: Option<(u32, f32)> = None;
    for f in 0..m.indices.len() / 3 {
        let Some(t) = tri_valid(m, f) else { continue };
        let (a, b, c) = (m.positions[t[0]], m.positions[t[1]], m.positions[t[2]]);
        let e1 = sub(b, a);
        let e2 = sub(c, a);
        let p = cross(ray_dir, e2);
        let det = dot(e1, p);
        if det.abs() < 1e-12 {
            continue;
        }
        let inv = 1.0 / det;
        let s = sub(ray_origin, a);
        let u = dot(s, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = cross(s, e1);
        let v = dot(ray_dir, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let tt = dot(e2, q) * inv;
        if tt > 1e-6 && best.is_none_or(|b| tt < b.1) {
            best = Some((f as u32, tt));
        }
    }
    best
}

// ---------------------------------------------------------------- editing

/// Mirror across the plane `axis = 0` (0 X, 1 Y, 2 Z); winding is fixed.
pub fn mirror(m: &mut Mesh, axis: usize) {
    if axis > 2 {
        return;
    }
    for p in &mut m.positions {
        p[axis] = -p[axis];
    }
    for n in &mut m.normals {
        n[axis] = -n[axis];
    }
    flip_winding(m);
}

/// Keep the +axis half, mirror it onto the -axis half and weld the seam.
pub fn symmetrize(m: &mut Mesh, axis: usize) {
    if axis > 2 {
        return;
    }
    sanitize(m);
    let neg: Vec<u32> = (0..m.indices.len() / 3)
        .filter(|&f| {
            let t = tri_valid(m, f).unwrap();
            (m.positions[t[0]][axis] + m.positions[t[1]][axis] + m.positions[t[2]][axis]) < -1e-6
        })
        .map(|f| f as u32)
        .collect();
    remove_faces(m, &neg);
    for p in &mut m.positions {
        if p[axis].abs() < 1e-4 {
            p[axis] = 0.0;
        }
    }
    let n0 = m.positions.len();
    let mut half = m.clone();
    mirror(&mut half, axis);
    let mut out = merge(m, &half);
    let key = |p: [f32; 3], t: [f32; 2]| {
        let k = pos_key(p, 1e-5);
        (k.0, k.1, k.2, (t[0] / 1e-5).round() as i64, (t[1] / 1e-5).round() as i64)
    };
    let mut on_plane = HashMap::new();
    for i in 0..n0 {
        if out.positions[i][axis] == 0.0 {
            on_plane.entry(key(out.positions[i], out.uvs[i])).or_insert(i as u32);
            let mut nn = out.normals[i];
            nn[axis] = 0.0;
            let nn = normalize(nn);
            if nn != [0.0; 3] {
                out.normals[i] = nn;
            }
        }
    }
    let remap: Vec<u32> = (0..out.positions.len())
        .map(|i| {
            if i >= n0 && out.positions[i][axis] == 0.0 {
                if let Some(&j) = on_plane.get(&key(out.positions[i], out.uvs[i])) {
                    return j;
                }
            }
            i as u32
        })
        .collect();
    for i in out.indices.iter_mut() {
        *i = remap[*i as usize];
    }
    drop_degenerate(&mut out, -1.0);
    compact(&mut out);
    *m = out;
}

/// Extrude faces along their average normal by `distance`, adding side walls
/// on the selection boundary. Returns the (post-compaction) ids of the moved
/// cap vertices.
pub fn extrude_faces(m: &mut Mesh, faces: &[u32], distance: f32) -> Vec<u32> {
    pad_attrs(m);
    let nf = m.indices.len() / 3;
    let mut sel: Vec<usize> = faces
        .iter()
        .map(|&f| f as usize)
        .filter(|&f| f < nf && tri_valid(m, f).is_some())
        .collect();
    sel.sort_unstable();
    sel.dedup();
    if sel.is_empty() {
        return Vec::new();
    }
    let colors = !m.colors.is_empty();
    let mut avg = [0.0; 3];
    let mut navg = [0.0; 3];
    for &f in &sel {
        let t = tri_valid(m, f).unwrap();
        avg = add(avg, face_cross(m, t));
        for &v in &t {
            navg = add(navg, m.normals[v]);
        }
    }
    let mut dir = normalize(avg);
    if dir == [0.0; 3] {
        dir = normalize(navg);
    }
    if dir == [0.0; 3] {
        dir = [0.0, 1.0, 0.0];
    }
    let g = pos_groups(m, 1e-4);
    let ekey = |a: usize, b: usize| (g[a].min(g[b]), g[a].max(g[b]));
    let mut edge_count: HashMap<(usize, usize), u32> = HashMap::new();
    for &f in &sel {
        let t = tri_valid(m, f).unwrap();
        for k in 0..3 {
            *edge_count.entry(ekey(t[k], t[(k + 1) % 3])).or_default() += 1;
        }
    }
    let shift = mul(dir, distance);
    let mut dup: HashMap<usize, u32> = HashMap::new();
    let mut cap = Vec::new();
    let mut walls = Vec::new();
    for &f in &sel {
        let t = tri_valid(m, f).unwrap();
        for k in 0..3 {
            let v = t[k];
            let nv = match dup.get(&v) {
                Some(&x) => x,
                None => {
                    let mut vv = getv(m, v);
                    vv.p = add(vv.p, shift);
                    let id = pushv(m, vv, colors);
                    dup.insert(v, id);
                    cap.push(id);
                    id
                }
            };
            m.indices[f * 3 + k] = nv;
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if g[a] != g[b] && edge_count[&ekey(a, b)] == 1 {
                walls.push((a, b));
            }
        }
    }
    if distance.abs() > 1e-9 {
        for (a, b) in walls {
            let (a2, b2) = (dup[&a] as usize, dup[&b] as usize);
            let (pa, pb, pb2) = (m.positions[a], m.positions[b], m.positions[b2]);
            let wn = normalize(cross(sub(pb, pa), sub(pb2, pa)));
            let el = length(sub(pb, pa));
            let h = distance.abs();
            let mut va = getv(m, a);
            let mut vb = getv(m, b);
            let mut vb2 = getv(m, b2);
            let mut va2 = getv(m, a2);
            for (v, uv) in [
                (&mut va, [0.0, h]),
                (&mut vb, [el, h]),
                (&mut vb2, [el, 0.0]),
                (&mut va2, [0.0, 0.0]),
            ] {
                v.n = wn;
                v.t = uv;
            }
            let ia = pushv(m, va, colors);
            let ib = pushv(m, vb, colors);
            let ib2 = pushv(m, vb2, colors);
            let ia2 = pushv(m, va2, colors);
            m.indices.extend_from_slice(&[ia, ib, ib2, ia, ib2, ia2]);
        }
    }
    let remap = compact_remap(m);
    cap.into_iter().filter_map(|i| remap.get(i as usize).copied().flatten()).collect()
}

/// Split each triangle into 4; edge midpoints are shared per edge and
/// interpolate position, normal, uv and colour.
pub fn subdivide(m: &mut Mesh) {
    sanitize(m);
    let colors = !m.colors.is_empty();
    let old = std::mem::take(&mut m.indices);
    let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
    let mut idx = Vec::with_capacity(old.len() * 4);
    for t in old.chunks_exact(3) {
        let mut e = [0u32; 3];
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let key = (a.min(b), a.max(b));
            e[k] = match mid.get(&key) {
                Some(&x) => x,
                None => {
                    let v = lerpv(getv(m, a as usize), getv(m, b as usize), 0.5);
                    let id = pushv(m, v, colors);
                    mid.insert(key, id);
                    id
                }
            };
        }
        let (a, b, c) = (t[0], t[1], t[2]);
        let (ab, bc, ca) = (e[0], e[1], e[2]);
        idx.extend_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca]);
    }
    m.indices = idx;
}

fn cluster(m: &Mesh, res: u32, mn: [f32; 3], ext: f32) -> (Vec<u32>, usize, Vec<[u32; 3]>) {
    let cell = ext / res as f32;
    let mut map: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let ids: Vec<u32> = m
        .positions
        .iter()
        .map(|&p| {
            let k = (
                ((p[0] - mn[0]) / cell).floor().min(res as f32 - 1.0) as i64,
                ((p[1] - mn[1]) / cell).floor().min(res as f32 - 1.0) as i64,
                ((p[2] - mn[2]) / cell).floor().min(res as f32 - 1.0) as i64,
            );
            let next = map.len() as u32;
            *map.entry(k).or_insert(next)
        })
        .collect();
    let mut seen = HashSet::new();
    let mut tris = Vec::new();
    for t in m.indices.chunks_exact(3) {
        let r = [ids[t[0] as usize], ids[t[1] as usize], ids[t[2] as usize]];
        if r[0] == r[1] || r[1] == r[2] || r[0] == r[2] {
            continue;
        }
        let rot = if r[0] < r[1] && r[0] < r[2] {
            r
        } else if r[1] < r[2] {
            [r[1], r[2], r[0]]
        } else {
            [r[2], r[0], r[1]]
        };
        if seen.insert(rot) {
            tris.push(r);
        }
    }
    (ids, map.len(), tris)
}

/// Vertex-clustering decimation aiming at `target_ratio` of the triangle
/// count. Cluster positions are averaged; uv/colour come from the cluster's
/// first (representative) vertex; degenerate triangles are dropped.
pub fn decimate(m: &mut Mesh, target_ratio: f32) {
    sanitize(m);
    let tri0 = m.indices.len() / 3;
    if tri0 == 0 || !(target_ratio < 1.0) {
        return;
    }
    let target = ((tri0 as f32 * target_ratio.max(0.0)).round() as usize).max(1);
    let (mn, mx) = bounds(m);
    let ext = (0..3).map(|k| mx[k] - mn[k]).fold(0.0f32, f32::max);
    if ext <= 0.0 || !ext.is_finite() {
        return;
    }
    let count = |r: u32| cluster(m, r, mn, ext).2.len();
    let (mut lo, mut hi) = (1u32, 2048u32);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if count(mid) >= target { hi = mid } else { lo = mid + 1 }
    }
    let mut res = lo;
    if lo > 1 {
        let (a, b) = (count(lo - 1), count(lo));
        if target.abs_diff(a) < target.abs_diff(b) {
            res = lo - 1;
        }
    }
    let (ids, ncl, tris) = cluster(m, res, mn, ext);
    if tris.len() >= tri0 {
        return;
    }
    let colors = !m.colors.is_empty();
    let mut sum = vec![[0.0f32; 3]; ncl];
    let mut nsum = vec![[0.0f32; 3]; ncl];
    let mut cnt = vec![0u32; ncl];
    let mut rep = vec![usize::MAX; ncl];
    for (i, &c) in ids.iter().enumerate() {
        let c = c as usize;
        sum[c] = add(sum[c], m.positions[i]);
        nsum[c] = add(nsum[c], m.normals[i]);
        cnt[c] += 1;
        if rep[c] == usize::MAX {
            rep[c] = i;
        }
    }
    let mut out = Mesh::default();
    for c in 0..ncl {
        let mut v = getv(m, rep[c]);
        v.p = mul(sum[c], 1.0 / cnt[c] as f32);
        let nn = normalize(nsum[c]);
        if nn != [0.0; 3] {
            v.n = nn;
        }
        pushv(&mut out, v, colors);
    }
    out.indices = tris.into_iter().flatten().collect();
    compact(&mut out);
    *m = out;
}

/// Fan-triangulate a polygon given as vertex ids: (p0, pi, pi+1).
pub fn triangulate_fan(poly: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    for i in 1..poly.len().saturating_sub(1) {
        out.extend_from_slice(&[poly[0], poly[i], poly[i + 1]]);
    }
    out
}

// ---------------------------------------------------------------- UVs

/// Planar projection seen from the +axis (sign = 1) or -axis (sign = -1),
/// unmirrored, v down.
fn planar_uv(p: [f32; 3], axis: usize, sign: f32, scale: f32) -> [f32; 2] {
    match axis {
        0 => [-p[2] * sign * scale, -p[1] * scale],
        1 => [p[0] * scale, p[2] * sign * scale],
        _ => [p[0] * sign * scale, -p[1] * scale],
    }
}

/// Planar UV projection along `axis` (0 X, 1 Y, 2 Z), as seen from +axis.
pub fn uv_planar(m: &mut Mesh, axis: usize, scale: f32) {
    let axis = axis.min(2);
    let uvs = m.positions.iter().map(|&p| planar_uv(p, axis, 1.0, scale)).collect();
    m.uvs = uvs;
}

/// Box (triplanar-by-face-normal) projection; vertices shared by faces with
/// different dominant axes are duplicated.
pub fn uv_box(m: &mut Mesh, scale: f32) {
    sanitize(m);
    let colors = !m.colors.is_empty();
    let n = m.positions.len();
    let mut variant: HashMap<(usize, usize), u32> = HashMap::new();
    let mut taken = vec![false; n];
    for f in 0..m.indices.len() / 3 {
        let t = tri_valid(m, f).unwrap();
        let fnm = face_cross(m, t);
        let a = fnm.map(f32::abs);
        let axis = if a[0] >= a[1] && a[0] >= a[2] { 0 } else if a[1] >= a[2] { 1 } else { 2 };
        let sign = if fnm[axis] < 0.0 { -1.0 } else { 1.0 };
        let code = axis * 2 + (sign < 0.0) as usize;
        for k in 0..3 {
            let v = t[k];
            let id = match variant.get(&(v, code)) {
                Some(&i) => i,
                None => {
                    let uv = planar_uv(m.positions[v], axis, sign, scale);
                    let i = if !taken[v] {
                        taken[v] = true;
                        m.uvs[v] = uv;
                        v as u32
                    } else {
                        let mut vv = getv(m, v);
                        vv.t = uv;
                        pushv(m, vv, colors)
                    };
                    variant.insert((v, code), i);
                    i
                }
            };
            m.indices[f * 3 + k] = id;
        }
    }
}

/// Scale and rotate UVs about (0.5, 0.5), then add `offset`.
pub fn uv_transform(m: &mut Mesh, offset: [f32; 2], scale: [f32; 2], rotate_deg: f32) {
    let (s, c) = rotate_deg.to_radians().sin_cos();
    for uv in &mut m.uvs {
        let x = (uv[0] - 0.5) * scale[0];
        let y = (uv[1] - 0.5) * scale[1];
        *uv = [x * c - y * s + 0.5 + offset[0], x * s + y * c + 0.5 + offset[1]];
    }
}

// ---------------------------------------------------------------- colour

/// Blend `color` into the given vertices by `strength` (0..1).
pub fn paint_vertices(m: &mut Mesh, verts: &[u32], color: [f32; 4], strength: f32) {
    let n = m.positions.len();
    m.colors.resize(n, [1.0; 4]);
    let s = strength.clamp(0.0, 1.0);
    for &v in verts {
        if let Some(c) = m.colors.get_mut(v as usize) {
            for k in 0..4 {
                c[k] += (color[k] - c[k]) * s;
            }
        }
    }
}

/// Set every vertex colour.
pub fn fill_color(m: &mut Mesh, color: [f32; 4]) {
    m.colors = vec![color; m.positions.len()];
}

// ---------------------------------------------------------------- primitives

/// Grid face centred at `c`, spanning +-u (right) and +-v (up), normal `n`.
fn grid_face(m: &mut Mesh, c: [f32; 3], u: [f32; 3], v: [f32; 3], n: [f32; 3], segs: u32) {
    let s = segs.max(1);
    let base = m.positions.len() as u32;
    for j in 0..=s {
        for i in 0..=s {
            let (fu, fv) = (i as f32 / s as f32, j as f32 / s as f32);
            let p = add(add(c, mul(u, fu * 2.0 - 1.0)), mul(v, fv * 2.0 - 1.0));
            pushv(m, V { p, n, t: [fu, 1.0 - fv], c: [1.0; 4] }, false);
        }
    }
    let w = s + 1;
    for j in 0..s {
        for i in 0..s {
            let bl = base + j * w + i;
            let (br, tl) = (bl + 1, bl + w);
            let tr = tl + 1;
            m.indices.extend_from_slice(&[bl, br, tr, bl, tr, tl]);
        }
    }
}

fn add_quad(m: &mut Mesh, p: [[f32; 3]; 4], n: [f32; 3]) {
    let b = m.positions.len() as u32;
    for (q, t) in p.iter().zip([[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]) {
        pushv(m, V { p: *q, n, t, c: [1.0; 4] }, false);
    }
    m.indices.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
}

fn add_tri(m: &mut Mesh, p: [[f32; 3]; 3], n: [f32; 3], t: [[f32; 2]; 3]) {
    let b = m.positions.len() as u32;
    for (q, uv) in p.iter().zip(t) {
        pushv(m, V { p: *q, n, t: uv, c: [1.0; 4] }, false);
    }
    m.indices.extend_from_slice(&[b, b + 1, b + 2]);
}

fn finish(mut m: Mesh) -> Mesh {
    orient_to_normals(&mut m);
    recenter(&mut m, Pivot::BottomCenter);
    m
}

/// Box of the given size (24 vertices, 12 triangles).
pub fn cube(size: [f32; 3]) -> Mesh {
    let h = mul(size, 0.5);
    let mut m = Mesh::default();
    let (x, y, z) = ([h[0], 0.0, 0.0], [0.0, h[1], 0.0], [0.0, 0.0, h[2]]);
    let neg = |a: [f32; 3]| mul(a, -1.0);
    grid_face(&mut m, x, neg(z), y, [1.0, 0.0, 0.0], 1);
    grid_face(&mut m, neg(x), z, y, [-1.0, 0.0, 0.0], 1);
    grid_face(&mut m, y, x, neg(z), [0.0, 1.0, 0.0], 1);
    grid_face(&mut m, neg(y), x, z, [0.0, -1.0, 0.0], 1);
    grid_face(&mut m, z, x, y, [0.0, 0.0, 1.0], 1);
    grid_face(&mut m, neg(z), neg(x), y, [0.0, 0.0, -1.0], 1);
    finish(m)
}

fn disc(m: &mut Mesh, radius: f32, y: f32, segments: u32, up: bool) {
    let n = if up { [0.0, 1.0, 0.0] } else { [0.0, -1.0, 0.0] };
    let c = pushv(m, V { p: [0.0, y, 0.0], n, t: [0.5, 0.5], c: [1.0; 4] }, false);
    for i in 0..segments {
        let a = 2.0 * PI * i as f32 / segments as f32;
        let (s, co) = a.sin_cos();
        pushv(
            m,
            V { p: [radius * co, y, radius * s], n, t: [0.5 + 0.5 * co, 0.5 + 0.5 * s], c: [1.0; 4] },
            false,
        );
    }
    for i in 0..segments {
        m.indices.extend_from_slice(&[c, c + 1 + i, c + 1 + (i + 1) % segments]);
    }
}

/// Capped cylinder along Y.
pub fn cylinder(radius: f32, height: f32, segments: u32) -> Mesh {
    let seg = segments.max(3);
    let mut m = Mesh::default();
    for i in 0..=seg {
        let u = i as f32 / seg as f32;
        let (s, c) = (2.0 * PI * u).sin_cos();
        let n = [c, 0.0, s];
        pushv(&mut m, V { p: [radius * c, 0.0, radius * s], n, t: [u, 1.0], c: [1.0; 4] }, false);
        pushv(&mut m, V { p: [radius * c, height, radius * s], n, t: [u, 0.0], c: [1.0; 4] }, false);
    }
    for i in 0..seg {
        let (b0, t0, b1, t1) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
        m.indices.extend_from_slice(&[b0, b1, t1, b0, t1, t0]);
    }
    disc(&mut m, radius, 0.0, seg, false);
    disc(&mut m, radius, height, seg, true);
    finish(m)
}

/// Cone along Y with its apex at the top.
pub fn cone(radius: f32, height: f32, segments: u32) -> Mesh {
    let seg = segments.max(3);
    let mut m = Mesh::default();
    let side_n = |a: f32| {
        let (s, c) = a.sin_cos();
        let n = normalize([height * c, radius, height * s]);
        if n == [0.0; 3] { [0.0, 1.0, 0.0] } else { n }
    };
    for i in 0..=seg {
        let u = i as f32 / seg as f32;
        let a = 2.0 * PI * u;
        let (s, c) = a.sin_cos();
        pushv(&mut m, V { p: [radius * c, 0.0, radius * s], n: side_n(a), t: [u, 1.0], c: [1.0; 4] }, false);
    }
    let apex0 = m.positions.len() as u32;
    for i in 0..seg {
        let u = (i as f32 + 0.5) / seg as f32;
        pushv(&mut m, V { p: [0.0, height, 0.0], n: side_n(2.0 * PI * u), t: [u, 0.0], c: [1.0; 4] }, false);
    }
    for i in 0..seg {
        m.indices.extend_from_slice(&[i, i + 1, apex0 + i]);
    }
    disc(&mut m, radius, 0.0, seg, false);
    finish(m)
}

/// UV sphere (`rings` latitude bands, `segments` longitude slices).
pub fn sphere(radius: f32, rings: u32, segments: u32) -> Mesh {
    let (rings, seg) = (rings.max(2), segments.max(3));
    let mut m = Mesh::default();
    for j in 0..=rings {
        let v = j as f32 / rings as f32;
        let (sp, cp) = (PI * v).sin_cos();
        for i in 0..=seg {
            let u = i as f32 / seg as f32;
            let (st, ct) = (2.0 * PI * u).sin_cos();
            let n = [sp * ct, cp, sp * st];
            pushv(&mut m, V { p: mul(n, radius), n, t: [u, v], c: [1.0; 4] }, false);
        }
    }
    let w = seg + 1;
    for j in 0..rings {
        for i in 0..seg {
            let a = j * w + i;
            let (b, c, d) = (a + 1, a + w + 1, a + w);
            m.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    drop_degenerate(&mut m, radius.abs() * radius.abs() * 1e-9);
    compact(&mut m);
    finish(m)
}

/// Flat plane on Y = 0 facing +Y; `subdiv` extra cuts per side.
pub fn plane(size: [f32; 2], subdiv: u32) -> Mesh {
    let mut m = Mesh::default();
    grid_face(
        &mut m,
        [0.0; 3],
        [size[0] * 0.5, 0.0, 0.0],
        [0.0, 0.0, -size[1] * 0.5],
        [0.0, 1.0, 0.0],
        subdiv.saturating_add(1),
    );
    finish(m)
}

/// Torus lying in the XZ plane.
pub fn torus(r_major: f32, r_minor: f32, seg_major: u32, seg_minor: u32) -> Mesh {
    let (sm, sn) = (seg_major.max(3), seg_minor.max(3));
    let mut m = Mesh::default();
    for j in 0..=sm {
        let u = j as f32 / sm as f32;
        let (st, ct) = (2.0 * PI * u).sin_cos();
        for i in 0..=sn {
            let v = i as f32 / sn as f32;
            let (sp, cp) = (2.0 * PI * v).sin_cos();
            let n = [cp * ct, sp, cp * st];
            let p = add([r_major * ct, 0.0, r_major * st], mul(n, r_minor));
            pushv(&mut m, V { p, n, t: [u, v], c: [1.0; 4] }, false);
        }
    }
    let w = sn + 1;
    for j in 0..sm {
        for i in 0..sn {
            let a = j * w + i;
            let (b, c, d) = (a + 1, a + w + 1, a + w);
            m.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    finish(m)
}

/// Wedge: full height at -Z, sloping down to the ground at +Z.
pub fn ramp(size: [f32; 3]) -> Mesh {
    let (hw, h, hd) = (size[0] * 0.5, size[1], size[2] * 0.5);
    let fl = [-hw, 0.0, hd];
    let fr = [hw, 0.0, hd];
    let bl = [-hw, 0.0, -hd];
    let br = [hw, 0.0, -hd];
    let tl = [-hw, h, -hd];
    let tr = [hw, h, -hd];
    let mut m = Mesh::default();
    add_quad(&mut m, [fl, fr, br, bl], [0.0, -1.0, 0.0]);
    add_quad(&mut m, [br, bl, tl, tr], [0.0, 0.0, -1.0]);
    let sn = normalize([0.0, size[2], h]);
    add_quad(&mut m, [fl, fr, tr, tl], if sn == [0.0; 3] { [0.0, 1.0, 0.0] } else { sn });
    add_tri(&mut m, [fl, bl, tl], [-1.0, 0.0, 0.0], [[1.0, 1.0], [0.0, 1.0], [0.0, 0.0]]);
    add_tri(&mut m, [fr, br, tr], [1.0, 0.0, 0.0], [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0]]);
    finish(m)
}

fn hash3(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f)
        ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

fn value_noise(p: [f32; 3], seed: u32) -> f32 {
    let f = p.map(f32::floor);
    let i = f.map(|v| v as i32);
    let t = [p[0] - f[0], p[1] - f[1], p[2] - f[2]].map(|x| x * x * (3.0 - 2.0 * x));
    let mut acc = 0.0;
    for dz in 0..2 {
        for dy in 0..2 {
            for dx in 0..2 {
                let w = (if dx == 1 { t[0] } else { 1.0 - t[0] })
                    * (if dy == 1 { t[1] } else { 1.0 - t[1] })
                    * (if dz == 1 { t[2] } else { 1.0 - t[2] });
                acc += w * hash3(i[0] + dx, i[1] + dy, i[2] + dz, seed);
            }
        }
    }
    acc
}

/// Noisy, slightly squashed icosphere with a flat base and semi-faceted
/// normals (good for League-style rocks). Deterministic per `seed`.
pub fn rock(radius: f32, seed: u32) -> Mesh {
    let t = (1.0 + 5.0f32.sqrt()) * 0.5;
    let pts = [
        [-1.0, t, 0.0], [1.0, t, 0.0], [-1.0, -t, 0.0], [1.0, -t, 0.0],
        [0.0, -1.0, t], [0.0, 1.0, t], [0.0, -1.0, -t], [0.0, 1.0, -t],
        [t, 0.0, -1.0], [t, 0.0, 1.0], [-t, 0.0, -1.0], [-t, 0.0, 1.0],
    ];
    let faces: [u32; 60] = [
        0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7,
        1, 8, 3, 9, 4, 3, 4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9,
        8, 1,
    ];
    let mut m = Mesh::default();
    for p in pts {
        let n = normalize(p);
        pushv(&mut m, V { p: n, n, t: [0.0; 2], c: [1.0; 4] }, false);
    }
    m.indices = faces.to_vec();
    orient_to_normals(&mut m);
    subdivide(&mut m);
    subdivide(&mut m);
    let off = [
        hash3(1, 2, 3, seed) * 100.0,
        hash3(4, 5, 6, seed) * 100.0,
        hash3(7, 8, 9, seed) * 100.0,
    ];
    for p in &mut m.positions {
        let d = normalize(*p);
        let q = add(mul(d, 1.3), off);
        let q2 = add(mul(d, 2.9), off);
        let f = 0.65 * value_noise(q, seed) + 0.35 * value_noise(q2, seed ^ 0x5bd1_e995);
        let mut r = mul(d, radius * (0.7 + 0.6 * f));
        r[1] *= 0.75;
        r[1] = r[1].max(-0.45 * radius);
        *p = r;
    }
    recompute_normals(&mut m, Some(40.0));
    if radius.abs() > 1e-9 {
        uv_box(&mut m, 1.0 / radius);
    }
    recenter(&mut m, Pivot::BottomCenter);
    m
}

// ---------------------------------------------------------------- stats

/// `(vertex count, valid triangle count)`.
pub fn stats(m: &Mesh) -> (usize, usize) {
    let tris = (0..m.indices.len() / 3).filter(|&f| tri_valid(m, f).is_some()).count();
    (m.positions.len(), tris)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, e: f32) -> bool {
        (a - b).abs() <= e
    }
    fn close3(a: [f32; 3], b: [f32; 3], e: f32) -> bool {
        (0..3).all(|k| close(a[k], b[k], e))
    }

    /// Every triangle's geometric normal agrees with its vertex normals and
    /// (for convex shapes) points away from the mesh centre.
    fn check_outward(m: &Mesh, convex: bool) {
        let mut c = [0.0; 3];
        for p in &m.positions {
            c = add(c, *p);
        }
        let c = mul(c, 1.0 / m.positions.len().max(1) as f32);
        for f in 0..m.indices.len() / 3 {
            let t = tri_valid(m, f).expect("valid tri");
            let g = face_cross(m, t);
            let cen = mul(add(add(m.positions[t[0]], m.positions[t[1]]), m.positions[t[2]]), 1.0 / 3.0);
            for &v in &t {
                assert!(dot(g, m.normals[v]) > 0.0, "face {f} disagrees with normal");
            }
            if convex {
                assert!(dot(g, sub(cen, c)) > 0.0, "face {f} points inward");
            }
        }
    }

    #[test]
    fn invert_round_trip() {
        let m = trs([1.0, 2.0, 3.0], [30.0, 45.0, 60.0], [2.0, 3.0, 4.0]);
        let inv = mat_invert(&m).unwrap();
        let id = mat_mul(&m, &inv);
        for k in 0..16 {
            assert!(close(id[k], IDENTITY[k], 1e-5), "{id:?}");
        }
        assert!(mat_invert(&[0.0; 16]).is_none());
    }

    #[test]
    fn rotation_order_and_axes() {
        let ry = trs([0.0; 3], [0.0, 90.0, 0.0], [1.0; 3]);
        assert!(close3(transform_point(&ry, [1.0, 0.0, 0.0]), [0.0, 0.0, -1.0], 1e-6));
        let rx = trs([0.0; 3], [90.0, 0.0, 0.0], [1.0; 3]);
        assert!(close3(transform_point(&rx, [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0], 1e-6));
        // R = Ry * Rx * Rz
        let r = trs([0.0; 3], [20.0, 30.0, 40.0], [1.0; 3]);
        let manual = mat_mul(
            &mat_mul(&trs([0.0; 3], [0.0, 30.0, 0.0], [1.0; 3]), &trs([0.0; 3], [20.0, 0.0, 0.0], [1.0; 3])),
            &trs([0.0; 3], [0.0, 0.0, 40.0], [1.0; 3]),
        );
        for k in 0..16 {
            assert!(close(r[k], manual[k], 1e-6));
        }
    }

    #[test]
    fn transform_round_trip() {
        let orig = cube([1.0, 2.0, 3.0]);
        let mut m = orig.clone();
        let t = trs([5.0, -2.0, 1.0], [10.0, 70.0, -30.0], [2.0, 0.5, 1.5]);
        transform(&mut m, &t);
        transform(&mut m, &mat_invert(&t).unwrap());
        for i in 0..orig.positions.len() {
            assert!(close3(m.positions[i], orig.positions[i], 1e-4));
            assert!(close3(m.normals[i], orig.normals[i], 1e-4));
        }
        assert_eq!(m.indices, orig.indices);
    }

    #[test]
    fn negative_scale_flips_winding() {
        let mut m = cube([1.0; 3]);
        transform(&mut m, &trs([0.0; 3], [0.0; 3], [-1.0, 1.0, 1.0]));
        check_outward(&m, true);
        let mut m2 = cube([1.0; 3]);
        mirror(&mut m2, 2);
        check_outward(&m2, true);
    }

    #[test]
    fn transform_selected_moves_only_selection() {
        let mut m = cube([1.0; 3]);
        let before = m.clone();
        transform_selected(&mut m, &[0, 0, 999], &trs([0.0, 5.0, 0.0], [0.0; 3], [1.0; 3]));
        assert!(close(m.positions[0][1], before.positions[0][1] + 5.0, 1e-6));
        assert_eq!(m.positions[1], before.positions[1]);
    }

    #[test]
    fn recenter_modes() {
        let mut m = cube([2.0, 2.0, 2.0]);
        transform(&mut m, &trs([3.0, 4.0, 5.0], [0.0; 3], [1.0; 3]));
        let off = recenter(&mut m, Pivot::BottomCenter);
        assert!(close3(off, [-3.0, -4.0, -5.0], 1e-5));
        let (mn, mx) = bounds(&m);
        assert!(close(mn[1], 0.0, 1e-6) && close(mn[0] + mx[0], 0.0, 1e-6) && close(mn[2] + mx[2], 0.0, 1e-6));
        recenter(&mut m, Pivot::Center);
        assert!(close3(center(&m), [0.0; 3], 1e-6));
        assert_eq!(recenter(&mut m, Pivot::Origin), [0.0; 3]);
    }

    #[test]
    fn cube_normals_outward() {
        let m = cube([1.0, 2.0, 3.0]);
        assert_eq!(stats(&m), (24, 12));
        check_outward(&m, true);
    }

    #[test]
    fn primitives_valid() {
        let prims = [
            ("cube", cube([1.0; 3]), true),
            ("cylinder", cylinder(1.0, 2.0, 16), true),
            ("cone", cone(1.0, 2.0, 16), true),
            ("sphere", sphere(1.0, 8, 12), true),
            ("plane", plane([2.0, 3.0], 3), false),
            ("torus", torus(2.0, 0.5, 16, 8), false),
            ("ramp", ramp([1.0, 1.0, 2.0]), true),
        ];
        for (name, m, convex) in prims {
            let (v, t) = stats(&m);
            println!("{name}: {v} verts, {t} tris");
            assert!(t > 0 && m.indices.len() == t * 3);
            assert_eq!(m.normals.len(), v);
            assert_eq!(m.uvs.len(), v);
            for n in &m.normals {
                assert!(close(length(*n), 1.0, 1e-4), "{name} normal not unit");
            }
            let (mn, _) = bounds(&m);
            assert!(close(mn[1], 0.0, 1e-5), "{name} not on ground");
            check_outward(&m, convex);
        }
        assert_eq!(stats(&plane([1.0, 1.0], 3)).1, 32);
    }

    #[test]
    fn rock_is_deterministic_and_sane() {
        let a = rock(1.5, 7);
        let b = rock(1.5, 7);
        let c = rock(1.5, 8);
        assert_eq!(a, b);
        assert_ne!(a, c);
        let (v, t) = stats(&a);
        println!("rock: {v} verts, {t} tris");
        assert_eq!(t, 320);
        assert!(close(bounds(&a).0[1], 0.0, 1e-5));
        // geometric normals agree with vertex normals for the vast majority
        let mut agree = 0;
        for f in 0..t {
            let tr = tri_valid(&a, f).unwrap();
            let g = face_cross(&a, tr);
            if tr.iter().all(|&v| dot(g, a.normals[v]) > 0.0) {
                agree += 1;
            }
        }
        assert!(agree as f32 > t as f32 * 0.95, "{agree}/{t}");
        assert!(a.uvs.iter().all(|u| u[0].is_finite() && u[1].is_finite()));
    }

    #[test]
    fn subdivide_counts() {
        let mut m = cube([1.0; 3]);
        subdivide(&mut m);
        assert_eq!(stats(&m).1, 48);
        // each face: 4 corners + 5 unique edge midpoints
        assert_eq!(stats(&m).0, 24 + 6 * 5);
        check_outward(&m, true);
        let mut c = cube([1.0; 3]);
        fill_color(&mut c, [1.0, 0.0, 0.0, 1.0]);
        subdivide(&mut c);
        assert_eq!(c.colors.len(), c.positions.len());
    }

    #[test]
    fn extrude_adds_geometry() {
        let mut p = plane([1.0, 1.0], 0);
        let cap = extrude_faces(&mut p, &[0, 1], 1.0);
        assert_eq!(cap.len(), 4);
        assert_eq!(stats(&p).1, 2 + 4 * 2);
        for &v in &cap {
            assert!(close(p.positions[v as usize][1], 1.0, 1e-5));
        }
        // walls face outward from the extruded column
        let c = [0.0, 0.5, 0.0];
        for f in 2..stats(&p).1 {
            let t = tri_valid(&p, f).unwrap();
            let cen = mul(add(add(p.positions[t[0]], p.positions[t[1]]), p.positions[t[2]]), 1.0 / 3.0);
            assert!(dot(face_cross(&p, t), sub(cen, c)) > 0.0);
        }
        let mut m = cube([1.0; 3]);
        let top: Vec<u32> = (0..12u32)
            .filter(|&f| {
                let t = tri_valid(&m, f as usize).unwrap();
                normalize(face_cross(&m, t))[1] > 0.9
            })
            .collect();
        assert_eq!(top.len(), 2);
        extrude_faces(&mut m, &top, 0.5);
        assert_eq!(stats(&m).1, 20);
        assert!(close(bounds(&m).1[1], 1.5, 1e-5));
        assert!(extrude_faces(&mut m, &[999], 1.0).is_empty());
    }

    #[test]
    fn split_loose_two_cubes() {
        let a = cube([1.0; 3]);
        let mut b = cube([1.0; 3]);
        transform(&mut b, &trs([5.0, 0.0, 0.0], [0.0; 3], [1.0; 3]));
        let m = merge(&a, &b);
        assert_eq!(stats(&m), (48, 24));
        let parts = split_loose(&m);
        assert_eq!(parts.len(), 2);
        for p in &parts {
            assert_eq!(stats(p), (24, 12));
        }
        assert_eq!(linked_vertices(&m, &[0]).len(), 24);
        assert_eq!(linked_vertices(&m, &[0, 30]).len(), 48);
        assert!(linked_vertices(&m, &[9999]).is_empty());
    }

    #[test]
    fn weld_reduces_duplicates() {
        let a = cube([1.0; 3]);
        let mut single = a.clone();
        weld(&mut single, 1e-5);
        let k = stats(&single).0;
        assert!(k < 24); // corners shared by faces with equal uv merge too
        let mut m = merge(&a, &a);
        let removed = weld(&mut m, 1e-5);
        assert_eq!(removed, 48 - k);
        assert_eq!(stats(&m).0, k);
        let mut s = sphere(1.0, 6, 8);
        let v0 = stats(&s).0;
        flat_shade(&mut s);
        assert_eq!(stats(&s).0, stats(&s).1 * 3);
        weld(&mut s, 1e-5);
        assert_eq!(stats(&s).0, v0);
    }

    #[test]
    fn recompute_normals_modes() {
        let mut m = cube([2.0; 3]);
        recompute_normals(&mut m, None);
        let d = 1.0 / 3.0f32.sqrt();
        for (p, n) in m.positions.iter().zip(&m.normals) {
            let c = [p[0], p[1] - 1.0, p[2]];
            assert!(close3(*n, [c[0].signum() * d, c[1].signum() * d, c[2].signum() * d], 1e-4));
        }
        // split by angle restores flat faces
        recompute_normals(&mut m, Some(30.0));
        assert_eq!(stats(&m).0, 24);
        check_outward(&m, true);
        for n in &m.normals {
            assert!(n.iter().filter(|x| x.abs() > 0.99).count() == 1);
        }
        // welded cube split by angle duplicates vertices
        let mut w = cube([1.0; 3]);
        w.uvs = vec![[0.0; 2]; 24];
        weld(&mut w, 1e-4);
        assert_eq!(stats(&w).0, 8);
        recompute_normals(&mut w, Some(30.0));
        assert_eq!(stats(&w).0, 24);
        check_outward(&w, true);
        let mut f = cube([1.0; 3]);
        flip(&mut f);
        flip(&mut f);
        check_outward(&f, true);
    }

    #[test]
    fn decimate_reduces() {
        let mut s = sphere(1.0, 16, 24);
        subdivide(&mut s);
        let t0 = stats(&s).1;
        decimate(&mut s, 0.25);
        let t1 = stats(&s).1;
        println!("decimate: {t0} -> {t1}");
        assert!(t1 > 0 && t1 < t0 / 2, "{t0} -> {t1}");
        assert!(s.indices.iter().all(|&i| (i as usize) < s.positions.len()));
    }

    #[test]
    fn pick_face_hits_cube() {
        let m = cube([2.0; 3]); // spans y 0..2
        let (f, t) = pick_face(&m, [0.2, 1.1, 5.0], [0.0, 0.0, -1.0]).unwrap();
        assert!(close(t, 4.0, 1e-5));
        let tr = tri_valid(&m, f as usize).unwrap();
        assert!(normalize(face_cross(&m, tr))[2] > 0.99);
        assert!(pick_face(&m, [5.0, 5.0, 5.0], [0.0, 0.0, -1.0]).is_none());
        assert!(pick_face(&m, [0.0, 1.0, 5.0], [0.0, 0.0, 1.0]).is_none());
    }

    #[test]
    fn select_and_pick_vertex() {
        let m = cube([2.0; 3]); // x -1..1, y 0..2
        let mut mvp = IDENTITY;
        mvp[0] = 0.5;
        mvp[5] = 0.5;
        mvp[10] = 0.1;
        let sel = select_by_box(&m, &mvp, [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(sel.len(), 12);
        for &v in &sel {
            assert!(m.positions[v as usize][0] > 0.0);
        }
        let mut behind = mvp;
        behind[15] = -1.0;
        assert!(select_by_box(&m, &behind, [-1.0, -1.0, 1.0, 1.0]).is_empty());
        let v = pick_vertex(&m, &mvp, [0.49, 0.99], 0.05).unwrap();
        assert!(close3(m.positions[v as usize], [1.0, 2.0, m.positions[v as usize][2]], 1e-6));
        assert!(pick_vertex(&m, &mvp, [0.0, 0.25], 0.05).is_none());
    }

    #[test]
    fn faces_remove_extract() {
        let mut m = cube([1.0; 3]);
        assert_eq!(faces_using(&m, &[0]).len(), 2);
        let e = extract_faces(&m, &[0, 1, 1, 500]);
        assert_eq!(stats(&e), (4, 2));
        remove_faces(&mut m, &[0, 1]);
        assert_eq!(stats(&m), (20, 10));
        assert_eq!(triangulate_fan(&[0, 1, 2, 3, 4]), vec![0, 1, 2, 0, 2, 3, 0, 3, 4]);
        assert!(triangulate_fan(&[0, 1]).is_empty());
    }

    #[test]
    fn symmetrize_mirrors_half() {
        let mut m = plane([2.0, 2.0], 1); // 2x2 cells split at x = 0
        transform_selected(&mut m, &[0], &trs([-5.0, 0.0, 0.0], [0.0; 3], [1.0; 3]));
        symmetrize(&mut m, 0);
        let (mn, mx) = bounds(&m);
        assert!(close(mn[0], -1.0, 1e-5) && close(mx[0], 1.0, 1e-5));
        assert_eq!(stats(&m).1, 8);
        for f in 0..8 {
            let t = tri_valid(&m, f).unwrap();
            assert!(face_cross(&m, t)[1] > 0.0);
        }
    }

    #[test]
    fn uv_ops() {
        let mut m = cube([1.0; 3]);
        uv_planar(&mut m, 1, 2.0);
        assert!(m.uvs.iter().all(|u| u[0].abs() <= 1.0 + 1e-6));
        let mut s = sphere(1.0, 8, 12);
        let v0 = stats(&s).0;
        let t0 = stats(&s).1;
        uv_box(&mut s, 1.0);
        assert!(stats(&s).0 >= v0);
        assert_eq!(stats(&s).1, t0);
        assert!(s.uvs.iter().all(|u| u[0].is_finite() && u[1].is_finite()));
        let mut c = cube([1.0; 3]);
        let before = c.uvs.clone();
        uv_transform(&mut c, [0.0; 2], [1.0; 2], 0.0);
        for (a, b) in c.uvs.iter().zip(&before) {
            assert!(close(a[0], b[0], 1e-6) && close(a[1], b[1], 1e-6));
        }
        uv_transform(&mut c, [0.25, 0.0], [1.0; 2], 90.0);
        // (0,0) -> rotated about centre to (1,0), then offset
        let i = before.iter().position(|u| *u == [0.0, 0.0]).unwrap();
        assert!(close(c.uvs[i][0], 1.25, 1e-5) && close(c.uvs[i][1], 0.0, 1e-5));
    }

    #[test]
    fn colours() {
        let mut m = cube([1.0; 3]);
        paint_vertices(&mut m, &[0, 77], [1.0, 0.0, 0.0, 1.0], 0.5);
        assert_eq!(m.colors.len(), 24);
        assert_eq!(m.colors[0], [1.0, 0.5, 0.5, 1.0]);
        assert_eq!(m.colors[1], [1.0; 4]);
        fill_color(&mut m, [0.0, 0.0, 1.0, 1.0]);
        assert!(m.colors.iter().all(|c| *c == [0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn robust_to_empty_and_bad_indices() {
        let id = IDENTITY;
        for m in [
            Mesh::default(),
            Mesh { positions: vec![[0.0; 3], [1.0, 0.0, 0.0]], indices: vec![0, 1, 5, 0, 1], ..Default::default() },
        ] {
            transform(&mut m.clone(), &id);
            transform_selected(&mut m.clone(), &[0, 9], &id);
            let _ = (bounds(&m), center(&m), stats(&m));
            recenter(&mut m.clone(), Pivot::BottomCenter);
            recompute_normals(&mut m.clone(), None);
            recompute_normals(&mut m.clone(), Some(30.0));
            flat_shade(&mut m.clone());
            flip(&mut m.clone());
            weld(&mut m.clone(), 1e-4);
            let _ = split_loose(&m);
            let _ = merge(&m, &m);
            remove_faces(&mut m.clone(), &[0, 7]);
            compact(&mut m.clone());
            let _ = extract_faces(&m, &[0, 3]);
            let _ = faces_using(&m, &[0]);
            let _ = linked_vertices(&m, &[0, 50]);
            let _ = select_by_box(&m, &id, [-1.0, -1.0, 1.0, 1.0]);
            let _ = pick_vertex(&m, &id, [0.0, 0.0], 0.1);
            let _ = pick_face(&m, [0.0; 3], [0.0, 0.0, 1.0]);
            mirror(&mut m.clone(), 0);
            mirror(&mut m.clone(), 7);
            symmetrize(&mut m.clone(), 0);
            assert!(extrude_faces(&mut m.clone(), &[0], 1.0).is_empty());
            subdivide(&mut m.clone());
            decimate(&mut m.clone(), 0.5);
            uv_planar(&mut m.clone(), 5, 1.0);
            uv_box(&mut m.clone(), 1.0);
            uv_transform(&mut m.clone(), [0.0; 2], [1.0; 2], 45.0);
            paint_vertices(&mut m.clone(), &[0, 9], [1.0; 4], 1.0);
            fill_color(&mut m.clone(), [1.0; 4]);
            let mut c = m.clone();
            compact(&mut c);
            assert_eq!(stats(&c), (0, 0));
        }
        let _ = (cylinder(1.0, 1.0, 0), cone(0.0, 0.0, 0), sphere(0.0, 0, 0), torus(1.0, 0.2, 0, 0));
        let _ = (plane([0.0; 2], 0), ramp([0.0; 3]), cube([0.0; 3]), rock(0.0, 0));
    }
}
