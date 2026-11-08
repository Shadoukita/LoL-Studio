//! Static object meshes: `*.SCB` (binary, `Mesh::LoadBinShared`) and `*.sco` (text,
//! `Mesh::LoadAsciiShared`). Turrets, the nexus, inhibitors, shops and the lake are such objects,
//! listed in `room.dsc`; the game takes their position from the mesh's central point.

use super::bin::{Reader, Writer, fixed_str};
use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq)]
pub struct ScbFace {
    pub indices: [u32; 3],
    pub material: String,
    pub material_field: Vec<u8>,
    pub uv: [[f32; 2]; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scb {
    pub version: u32,
    pub name: String,
    name_field: Vec<u8>,
    pub flags: u32,
    pub bbox_origin: [f32; 3],
    pub bbox_size: [f32; 3],
    pub has_vertex_colors: i32,
    pub vertices: Vec<[f32; 3]>,
    pub vertex_colors: Vec<[u8; 4]>,
    pub central: [f32; 3],
    pub faces: Vec<ScbFace>,
    pub face_colors: Vec<[u8; 9]>,
}

pub const FLAG_HAS_VCP: u32 = 1;
pub const FLAG_LOCAL_ORIGIN: u32 = 2;

impl Scb {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        if r.bytes(8)? != b"r3d2Mesh" {
            bail!("not an SCB mesh");
        }
        let version = r.u32()?;
        if !(0x20001..=0x20003).contains(&version) {
            bail!("SCB version {version:#x}; the game reads 2.1 to 2.3");
        }
        let name_field = r.bytes(128)?.to_vec();
        let nv = r.i32()? as usize;
        let nf = r.i32()? as usize;
        let flags = r.u32()?;
        let (mut bbox_origin, mut bbox_size) = ([0.0; 3], [0.0; 3]);
        if version >= 0x20002 {
            bbox_origin = r.vec3()?;
            bbox_size = r.vec3()?;
        }
        let has_vertex_colors = if version >= 0x20003 { r.i32()? } else { 0 };
        let mut vertices = Vec::with_capacity(nv);
        for _ in 0..nv {
            vertices.push(r.vec3()?);
        }
        let mut vertex_colors = Vec::new();
        if has_vertex_colors != 0 {
            for _ in 0..nv {
                vertex_colors.push(r.bytes(4)?.try_into().unwrap());
            }
        }
        let central = r.vec3()?;
        let mut faces = Vec::with_capacity(nf);
        for _ in 0..nf {
            let indices = [r.u32()?, r.u32()?, r.u32()?];
            let mf = r.bytes(64)?.to_vec();
            let tx: [f32; 3] = r.f32s()?;
            let ty: [f32; 3] = r.f32s()?;
            faces.push(ScbFace { indices, material: fixed_str(&mf), material_field: mf, uv: [[tx[0], ty[0]], [tx[1], ty[1]], [tx[2], ty[2]]] });
        }
        let mut face_colors = Vec::new();
        if flags & FLAG_HAS_VCP != 0 {
            for _ in 0..nf {
                face_colors.push(r.bytes(9)?.try_into().unwrap());
            }
        }
        if r.remaining() != 0 {
            bail!("{} unexpected bytes at the end of the SCB", r.remaining());
        }
        Ok(Self { version, name: fixed_str(&name_field), name_field, flags, bbox_origin, bbox_size, has_vertex_colors, vertices, vertex_colors, central, faces, face_colors })
    }

    pub fn write(&self) -> Result<Vec<u8>> {
        let mut w = Writer::default();
        w.bytes(b"r3d2Mesh");
        w.u32(self.version);
        if fixed_str(&self.name_field) == self.name {
            w.bytes(&self.name_field);
        } else {
            w.fixed_str(&self.name, 128)?;
        }
        w.i32(self.vertices.len() as i32);
        w.i32(self.faces.len() as i32);
        w.u32(self.flags);
        if self.version >= 0x20002 {
            w.f32s(&self.bbox_origin);
            w.f32s(&self.bbox_size);
        }
        if self.version >= 0x20003 {
            w.i32(self.has_vertex_colors);
        }
        for v in &self.vertices {
            w.f32s(v);
        }
        if self.has_vertex_colors != 0 {
            for c in &self.vertex_colors {
                w.bytes(c);
            }
        }
        w.f32s(&self.central);
        for f in &self.faces {
            for i in f.indices {
                w.u32(i);
            }
            if fixed_str(&f.material_field) == f.material {
                w.bytes(&f.material_field);
            } else {
                w.fixed_str(&f.material, 64)?;
            }
            w.f32s(&[f.uv[0][0], f.uv[1][0], f.uv[2][0]]);
            w.f32s(&[f.uv[0][1], f.uv[1][1], f.uv[2][1]]);
        }
        if self.flags & FLAG_HAS_VCP != 0 {
            for c in &self.face_colors {
                w.bytes(c);
            }
        }
        Ok(w.data)
    }

    pub fn translate(&mut self, d: [f32; 3]) {
        self.transform(|p| [p[0] + d[0], p[1] + d[1], p[2] + d[2]]);
    }

    /// Applies `f` to every vertex and the central point, and refits the bounding box.
    pub fn transform(&mut self, f: impl Fn([f32; 3]) -> [f32; 3]) {
        for v in &mut self.vertices {
            *v = f(*v);
        }
        self.central = f(self.central);
        if !self.vertices.is_empty() {
            let (b, _) = super::nvr::bounds(&self.vertices);
            self.bbox_origin = [b[0], b[1], b[2]];
            self.bbox_size = [b[3] - b[0], b[4] - b[1], b[5] - b[2]];
        }
    }

    pub fn rename(&mut self, name: &str) {
        self.name = name.to_string();
    }
}

/// A text mesh kept line by line (the game's files mix CRLF and LF endings), so editing the
/// central point or the vertices leaves every other byte as it was.
#[derive(Clone, Debug, PartialEq)]
pub struct Sco {
    lines: Vec<(String, String)>,
    name_line: usize,
    central_line: usize,
    first_vertex_line: usize,
    pub name: String,
    pub central: [f32; 3],
    pub pivot: Option<[f32; 3]>,
    pub vertices: Vec<[f32; 3]>,
    pub face_count: usize,
}

fn split_lines(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (line, end, next) = match rest.find('\n') {
            Some(p) if p > 0 && rest.as_bytes()[p - 1] == b'\r' => (&rest[..p - 1], "\r\n", &rest[p + 1..]),
            Some(p) => (&rest[..p], "\n", &rest[p + 1..]),
            None => (rest, "", ""),
        };
        out.push((line.to_string(), end.to_string()));
        rest = next;
    }
    out
}

fn floats(s: &str) -> Vec<f32> {
    s.split_whitespace().filter_map(|t| t.parse().ok()).collect()
}

impl Sco {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let text = super::bin::latin1_decode(data);
        let lines = split_lines(&text);
        let begin = lines
            .iter()
            .position(|(l, _)| l.trim_start().to_ascii_lowercase().starts_with("[objectbegin]"))
            .ok_or_else(|| anyhow::anyhow!("no [ObjectBegin] in the SCO"))?;
        let get = |i: usize| lines.get(i).map(|(l, _)| l.as_str()).ok_or_else(|| anyhow::anyhow!("SCO ends early"));
        let name_line = begin + 1;
        let name_text = get(name_line)?;
        let mut it = name_text.split_whitespace();
        if !it.next().unwrap_or("").eq_ignore_ascii_case("Name=") {
            bail!("SCO has no Name= line");
        }
        let name = it.next().unwrap_or("").to_string();
        let central_line = begin + 2;
        let c = floats(get(central_line)?.split_once('=').map_or("", |x| x.1));
        if c.len() < 3 {
            bail!("SCO has no CentralPoint");
        }
        let mut i = begin + 3;
        let mut pivot = None;
        if get(i)?.trim_start().starts_with("PivotPoint=") {
            let p = floats(get(i)?.split_once('=').unwrap().1);
            if p.len() >= 3 {
                pivot = Some([p[0], p[1], p[2]]);
            }
            i += 1;
        }
        let mut has_colors = false;
        if get(i)?.trim_start().starts_with("VertexColors=") {
            has_colors = get(i)?.split_once('=').unwrap().1.trim().parse::<i32>().unwrap_or(0) != 0;
            i += 1;
        }
        let nv: usize = get(i)?.split_once('=').map_or("", |x| x.1).trim().parse().map_err(|_| anyhow::anyhow!("SCO has no Verts= count"))?;
        let first_vertex_line = i + 1;
        let mut vertices = Vec::with_capacity(nv);
        for k in 0..nv {
            let v = floats(get(first_vertex_line + k)?);
            if v.len() < 3 {
                bail!("SCO vertex {k} is not three numbers");
            }
            vertices.push([v[0], v[1], v[2]]);
        }
        let mut after = first_vertex_line + nv;
        if has_colors {
            after += nv;
        }
        let face_count = lines
            .get(after)
            .and_then(|(l, _)| l.split_once('=').and_then(|x| x.1.trim().parse().ok()))
            .unwrap_or(0);
        Ok(Self { lines, name_line, central_line, first_vertex_line, name, central: [c[0], c[1], c[2]], pivot, vertices, face_count })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut s = String::new();
        for (l, e) in &self.lines {
            s.push_str(l);
            s.push_str(e);
        }
        super::bin::latin1_encode(&s)
    }

    pub fn translate(&mut self, d: [f32; 3]) {
        self.transform(|p| [p[0] + d[0], p[1] + d[1], p[2] + d[2]]);
    }

    /// Applies `f` to every vertex, the central point and the pivot, rewriting only those lines.
    pub fn transform(&mut self, f: impl Fn([f32; 3]) -> [f32; 3]) {
        self.central = f(self.central);
        if let Some(p) = &mut self.pivot {
            *p = f(*p);
        }
        self.lines[self.central_line].0 = format!("CentralPoint= {:.4} {:.4} {:.4}", self.central[0], self.central[1], self.central[2]);
        if let Some(p) = self.pivot {
            self.lines[self.central_line + 1].0 = format!("PivotPoint= {:.4} {:.4} {:.4}", p[0], p[1], p[2]);
        }
        for (k, v) in self.vertices.iter_mut().enumerate() {
            *v = f(*v);
            self.lines[self.first_vertex_line + k].0 = format!("{:.4} {:.4} {:.4}", v[0], v[1], v[2]);
        }
    }

    pub fn rename(&mut self, name: &str) {
        self.name = name.to_string();
        self.lines[self.name_line].0 = format!("Name= {name}");
    }
}

impl Scb {
    /// A new static mesh (version 2.3, no colours). Faces: indices, material, UVs.
    pub fn new(name: &str, vertices: Vec<[f32; 3]>, faces: Vec<([u32; 3], String, [[f32; 2]; 3])>) -> Self {
        let (b, _) = super::nvr::bounds(&vertices);
        let n = vertices.len().max(1) as f32;
        let central = [0, 1, 2].map(|a| vertices.iter().map(|v| v[a]).sum::<f32>() / n);
        Self {
            version: 0x20003,
            name: name.into(),
            name_field: Vec::new(),
            flags: 0,
            bbox_origin: [b[0], b[1], b[2]],
            bbox_size: [b[3] - b[0], b[4] - b[1], b[5] - b[2]],
            has_vertex_colors: 0,
            vertices,
            vertex_colors: Vec::new(),
            central,
            faces: faces.into_iter().map(|(indices, material, uv)| ScbFace { indices, material_field: Vec::new(), material, uv }).collect(),
            face_colors: Vec::new(),
        }
    }

    /// The same mesh as an .sco text file, in the layout of the game's files.
    pub fn to_sco(&self) -> Vec<u8> {
        let mut s = format!("[ObjectBegin]\r\nName= {}\r\nCentralPoint= {:.4} {:.4} {:.4}\r\nVerts= {}\r\n", self.name, self.central[0], self.central[1], self.central[2], self.vertices.len());
        for v in &self.vertices {
            s += &format!("{:.4} {:.4} {:.4}\r\n", v[0], v[1], v[2]);
        }
        s += &format!("Faces= {}\r\n", self.faces.len());
        for f in &self.faces {
            s += &format!("3\t{:4} {:4} {:4}\t{:20}\t{:.12} {:.12} {:.12} {:.12} {:.12} {:.12}\r\n", f.indices[0], f.indices[1], f.indices[2], f.material, f.uv[0][0], f.uv[0][1], f.uv[1][0], f.uv[1][1], f.uv[2][0], f.uv[2][1]);
        }
        s += "[ObjectEnd]\r\n";
        super::bin::latin1_encode(&s)
    }
}
