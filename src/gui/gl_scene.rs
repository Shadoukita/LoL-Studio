//! OpenGL renderer for a `RenderScene`, drawn inside egui (3D perspective and 2D top-down).
//! The fragment shader reproduces the client's environment shading (LIT_PS, FOUR_BLEND,
//! BAKED_ENV_PS) without shadows, clouds and fog of war.

use eframe::glow::{self, HasContext};
use lol_map_studio::render::{Batch, Blend, RenderScene, Shading, Sun};
use std::sync::{Arc, Mutex};

pub type Mat4 = [f32; 16];

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut r = [0.0; 16];
    for c in 0..4 {
        for row in 0..4 {
            r[c * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[c * 4 + k]).sum();
        }
    }
    r
}

pub fn transform(m: &Mat4, p: [f32; 3]) -> [f32; 4] {
    let v = [p[0], p[1], p[2], 1.0];
    let mut c = [0.0f32; 4];
    for (row, out) in c.iter_mut().enumerate() {
        *out = (0..4).map(|k| m[k * 4 + row] * v[k]).sum();
    }
    c
}

const VS: &str = r#"
in vec3 a_pos;
in vec3 a_normal;
in vec2 a_uv0;
in vec2 a_uv1;
in vec4 a_color;
uniform mat4 u_mvp;
out vec3 v_normal;
out vec2 v_uv0;
out vec2 v_uv1;
out vec4 v_color;
out vec3 v_world;
void main() {
    v_normal = a_normal;
    v_uv0 = a_uv0;
    v_uv1 = a_uv1;
    v_color = a_color;
    v_world = a_pos;
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
"#;

const FS: &str = r#"
in vec3 v_normal;
in vec2 v_uv0;
in vec2 v_uv1;
in vec4 v_color;
in vec3 v_world;
uniform int u_shading;
uniform int u_alpha_test;
uniform sampler2D t0;
uniform sampler2D t1;
uniform sampler2D t2;
uniform sampler2D t3;
uniform sampler2D t4;
uniform int u_has4;
uniform vec3 u_sun_dir;
uniform vec3 u_sun_color;
uniform vec3 u_ambient;
uniform float u_time;
uniform vec4 u_tint;
// Ghosts: 1 = ignore the texture's alpha, 2 = use it only as a cutout.
uniform int u_ghost;
// The map's picture settings (Gamma.inibin): gamma, brightness, contrast.
uniform vec3 u_grade;
// Height fog (terrain.inibin Fog*): colour + opacity, and the heights it spans.
uniform vec4 u_fog;
uniform vec2 u_fog_h;
out vec4 o_color;
void main() {
    vec4 c;
    vec3 n = normalize(v_normal);
    float intensity = clamp(-dot(normalize(u_sun_dir), n), 0.0, 1.0);
    vec3 lighting = intensity * u_sun_color * 2.0 + v_color.rgb * 4.0 + u_ambient;
    if (u_shading == 1) {
        vec4 m = texture(t4, v_uv1);
        vec3 t = texture(t0, v_uv0).rgb;
        t = mix(t, texture(t1, v_uv0).rgb, m.r);
        t = mix(t, texture(t2, v_uv0).rgb, m.g);
        t = mix(t, texture(t3, v_uv0).rgb, m.b);
        c = vec4(clamp(lighting * t, 0.0, 1.0), 1.0);
    } else if (u_shading == 2) {
        c = texture(t0, v_uv0);
    } else if (u_shading == 3) {
        c = texture(t0, v_uv0);
    } else if (u_shading == 5) {
        // Particles: texture times the particle colour, unlit.
        c = texture(t0, v_uv0) * v_color;
    } else if (u_shading == 4) {
        vec2 w = v_world.xz * 0.004;
        float r = sin(w.x * 1.7 + w.y * 0.6 + u_time * 0.7) + sin(w.x * -0.8 + w.y * 1.9 - u_time * 0.5) + 0.5 * sin((w.x + w.y) * 3.1 + u_time * 1.1);
        float ripple = 0.5 + 0.18 * r;
        vec3 deep = vec3(0.10, 0.27, 0.30);
        vec3 shallow = vec3(0.26, 0.48, 0.50);
        vec3 light = mix(vec3(0.85), u_sun_color * 1.3 + u_ambient * 0.25, 0.5);
        c = vec4(mix(deep, shallow, ripple) * light * 1.4, 0.55);
    } else {
        vec4 t = texture(t0, v_uv0);
        float em = u_has4 == 1 ? texture(t4, v_uv0).g : 0.0;
        c = vec4(clamp(lighting * t.rgb * (1.0 - em) + em * t.rgb, 0.0, 1.0), t.a);
    }
    if (u_alpha_test == 1 && c.a < 0.5) discard;
    if (u_ghost == 2 && c.a < 0.3) discard;
    if (u_ghost != 0) c.a = 1.0;
    c *= u_tint;
    if (u_shading != 5) {
        if (u_fog.a > 0.0) {
            float f = clamp((u_fog_h.y - v_world.y) / max(u_fog_h.y - u_fog_h.x, 1.0), 0.0, 1.0);
            c.rgb = mix(c.rgb, u_fog.rgb, f * u_fog.a);
        }
        c.rgb = pow(clamp(c.rgb, 0.0, 1.0), vec3(1.0 / max(u_grade.x, 0.05)));
        c.rgb = clamp(((c.rgb - 0.5) * u_grade.z + 0.5) * u_grade.y, 0.0, 1.0);
    }
    o_color = c;
}
"#;

struct GpuBatch {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ibo: glow::Buffer,
    count: i32,
    shading: Shading,
    blend: Blend,
    textures: [Option<usize>; 5],
    clamp: bool,
    center: [f32; 3],
}

/// The Play-mode champion: skinned vertices rebuilt every frame.
pub struct CharacterDraw {
    pub vertices: Vec<lol_map_studio::render::RVertex>,
    pub indices: Vec<u32>,
    pub alpha: f32,
}

/// A mesh drawn for one frame on top of the scene: placement ghosts, animated monsters,
/// selection previews. `texture` names a texture added with `add_texture`.
pub struct DynMesh {
    pub texture: Option<String>,
    pub vertices: Vec<lol_map_studio::render::RVertex>,
    pub indices: Vec<u32>,
    /// Colour multiplier; alpha below 1 draws see-through.
    pub tint: [f32; 4],
    /// For see-through meshes: the texture's alpha cuts holes (leaves) instead of being ignored.
    pub cutout: bool,
    /// Particle quads: unlit, vertex colour, no depth writes, drawn last.
    pub particle: bool,
    /// Particles: added to the colour behind (fire, glows) instead of blended over it.
    pub additive: bool,
}

/// How the map's picture and fog settings look in the preview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub gamma: f32,
    pub brightness: f32,
    pub contrast: f32,
    /// Colour (0..1) and opacity; opacity 0 = no fog.
    pub fog: [f32; 4],
    pub fog_heights: [f32; 2],
}

impl Default for Look {
    fn default() -> Self {
        Self { gamma: 1.0, brightness: 1.0, contrast: 1.0, fog: [0.0; 4], fog_heights: [-200.0, 100.0] }
    }
}

pub struct GlScene {
    pub look: Look,
    pub character: Option<CharacterDraw>,
    /// Per-frame meshes (cleared by the app at the start of every frame).
    pub extras: Vec<DynMesh>,
    dyn_tex: std::collections::HashMap<String, glow::Texture>,
    dyn_tex_pending: Vec<(String, usize, usize, Vec<u8>)>,
    /// Textures to delete on the GL thread.
    dyn_tex_dead: Vec<glow::Texture>,
    dyn_gpu: Option<(glow::VertexArray, glow::Buffer, glow::Buffer)>,
    char_texture_pending: Option<(usize, usize, Vec<u8>)>,
    char_gpu: Option<(glow::VertexArray, glow::Buffer, glow::Buffer, usize)>,
    char_tex: Option<glow::Texture>,
    char_indices_uploaded: bool,
    program: Option<glow::Program>,
    white: Option<glow::Texture>,
    textures: Vec<glow::Texture>,
    batches: Vec<GpuBatch>,
    pending: Option<Arc<RenderScene>>,
    pub sun: Sun,
    pub error: Option<String>,
    pub start: std::time::Instant,
}

pub type Shared = Arc<Mutex<GlScene>>;

pub fn new_shared() -> Shared {
    Arc::new(Mutex::new(GlScene { look: Look::default(), character: None, extras: Vec::new(), dyn_tex: Default::default(), dyn_tex_pending: Vec::new(), dyn_tex_dead: Vec::new(), dyn_gpu: None, char_texture_pending: None, char_gpu: None, char_tex: None, char_indices_uploaded: false, program: None, white: None, textures: Vec::new(), batches: Vec::new(), pending: None, sun: Sun::default(), error: None, start: std::time::Instant::now() }))
}

impl GlScene {
    pub fn set(&mut self, s: Arc<RenderScene>) {
        self.sun = s.sun;
        self.pending = Some(s);
    }

    pub fn set_character_texture(&mut self, w: usize, h: usize, rgba: Vec<u8>) {
        self.char_texture_pending = Some((w, h, rgba));
        self.char_indices_uploaded = false;
    }

    /// Whether a texture for dynamic meshes is uploaded or queued.
    pub fn has_texture(&self, key: &str) -> bool {
        self.dyn_tex.contains_key(key) || self.dyn_tex_pending.iter().any(|t| t.0 == key)
    }

    /// Drops dynamic textures whose key starts with a prefix (they are reloaded on next use).
    pub fn forget_textures(&mut self, prefix: &str) {
        let gone: Vec<String> = self.dyn_tex.keys().filter(|k| k.starts_with(prefix)).cloned().collect();
        for k in gone {
            if let Some(t) = self.dyn_tex.remove(&k) {
                self.dyn_tex_dead.push(t);
            }
        }
        self.dyn_tex_pending.retain(|t| !t.0.starts_with(prefix));
    }

    pub fn add_texture(&mut self, key: &str, w: usize, h: usize, rgba: Vec<u8>) {
        if !self.has_texture(key) {
            self.dyn_tex_pending.push((key.to_string(), w, h, rgba));
        }
    }

    pub fn has_geometry(&self) -> bool {
        !self.batches.is_empty() || self.pending.as_ref().is_some_and(|p| !p.batches.is_empty())
    }

    unsafe fn init(&mut self, gl: &glow::Context) -> Result<(), String> {
        unsafe {
            let header = if gl.version().is_embedded { "#version 300 es\nprecision mediump float;\n" } else { "#version 330 core\n" };
            let program = gl.create_program()?;
            let mut shaders = Vec::new();
            for (kind, src) in [(glow::VERTEX_SHADER, VS), (glow::FRAGMENT_SHADER, FS)] {
                let s = gl.create_shader(kind)?;
                gl.shader_source(s, &format!("{header}{src}"));
                gl.compile_shader(s);
                if !gl.get_shader_compile_status(s) {
                    return Err(gl.get_shader_info_log(s));
                }
                gl.attach_shader(program, s);
                shaders.push(s);
            }
            for (i, n) in ["a_pos", "a_normal", "a_uv0", "a_uv1", "a_color"].iter().enumerate() {
                gl.bind_attrib_location(program, i as u32, n);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program));
            }
            for s in shaders {
                gl.detach_shader(program, s);
                gl.delete_shader(s);
            }
            self.program = Some(program);
            let white = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(white));
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, 1, 1, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&[255, 255, 255, 255])));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            self.white = Some(white);
            Ok(())
        }
    }

    unsafe fn upload(&mut self, gl: &glow::Context, s: &RenderScene) {
        unsafe {
            for t in self.textures.drain(..) {
                gl.delete_texture(t);
            }
            for b in self.batches.drain(..) {
                gl.delete_vertex_array(b.vao);
                gl.delete_buffer(b.vbo);
                gl.delete_buffer(b.ibo);
            }
            for t in &s.textures {
                let Ok(tex) = gl.create_texture() else { continue };
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                if t.width > 0 {
                    gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, t.width as i32, t.height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&t.rgba)));
                } else {
                    gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, 1, 1, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&[200, 200, 200, 255])));
                }
                gl.generate_mipmap(glow::TEXTURE_2D);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                self.textures.push(tex);
            }
            for b in &s.batches {
                if let Some(g) = Self::upload_batch(gl, b) {
                    self.batches.push(g);
                }
            }
        }
    }

    unsafe fn paint_character(&mut self, gl: &glow::Context, program: glow::Program) {
        unsafe {
            if let Some((w, h, px)) = self.char_texture_pending.take() {
                if let Some(t) = self.char_tex.take() {
                    gl.delete_texture(t);
                }
                if let Ok(tex) = gl.create_texture() {
                    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                    gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&px)));
                    gl.generate_mipmap(glow::TEXTURE_2D);
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                    self.char_tex = Some(tex);
                }
            }
            let Some(c) = &self.character else { return };
            if c.vertices.is_empty() {
                return;
            }
            if self.char_gpu.is_none() {
                let (Ok(vao), Ok(vbo), Ok(ibo)) = (gl.create_vertex_array(), gl.create_buffer(), gl.create_buffer()) else { return };
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                let stride = std::mem::size_of::<lol_map_studio::render::RVertex>() as i32;
                for (loc, size, off) in [(0u32, 3, 0), (1, 3, 12), (2, 2, 24), (3, 2, 32), (4, 4, 40)] {
                    gl.enable_vertex_attrib_array(loc);
                    gl.vertex_attrib_pointer_f32(loc, size, glow::FLOAT, false, stride, off);
                }
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
                gl.bind_vertex_array(None);
                self.char_gpu = Some((vao, vbo, ibo, 0));
                self.char_indices_uploaded = false;
            }
            let (vao, vbo, ibo, _) = self.char_gpu.unwrap();
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&c.vertices), glow::STREAM_DRAW);
            if !self.char_indices_uploaded {
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
                gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytemuck::cast_slice(&c.indices), glow::STATIC_DRAW);
                self.char_indices_uploaded = true;
            }
            let loc = |n: &str| gl.get_uniform_location(program, n);
            gl.uniform_1_i32(loc("u_shading").as_ref(), 0);
            gl.uniform_1_i32(loc("u_has4").as_ref(), 0);
            gl.uniform_1_i32(loc("u_alpha_test").as_ref(), (c.alpha >= 1.0) as i32);
            gl.uniform_4_f32(loc("u_tint").as_ref(), 1.0, 1.0, 1.0, c.alpha);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, self.char_tex.or(self.white));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
            if c.alpha < 1.0 {
                gl.enable(glow::BLEND);
                gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                gl.depth_mask(false);
            } else {
                gl.disable(glow::BLEND);
                gl.depth_mask(true);
            }
            gl.draw_elements(glow::TRIANGLES, c.indices.len() as i32, glow::UNSIGNED_INT, 0);
            gl.bind_vertex_array(None);
            gl.uniform_4_f32(loc("u_tint").as_ref(), 1.0, 1.0, 1.0, 1.0);
        }
    }

    unsafe fn paint_extras(&mut self, gl: &glow::Context, program: glow::Program) {
        unsafe {
            for t in self.dyn_tex_dead.drain(..) {
                gl.delete_texture(t);
            }
            for (key, w, h, px) in std::mem::take(&mut self.dyn_tex_pending) {
                let Ok(tex) = gl.create_texture() else { continue };
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w.max(1) as i32, h.max(1) as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&px)));
                gl.generate_mipmap(glow::TEXTURE_2D);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                self.dyn_tex.insert(key, tex);
            }
            if self.extras.is_empty() {
                return;
            }
            if self.dyn_gpu.is_none() {
                let (Ok(vao), Ok(vbo), Ok(ibo)) = (gl.create_vertex_array(), gl.create_buffer(), gl.create_buffer()) else { return };
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                let stride = std::mem::size_of::<lol_map_studio::render::RVertex>() as i32;
                for (loc, size, off) in [(0u32, 3, 0), (1, 3, 12), (2, 2, 24), (3, 2, 32), (4, 4, 40)] {
                    gl.enable_vertex_attrib_array(loc);
                    gl.vertex_attrib_pointer_f32(loc, size, glow::FLOAT, false, stride, off);
                }
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
                gl.bind_vertex_array(None);
                self.dyn_gpu = Some((vao, vbo, ibo));
            }
            let (vao, vbo, ibo) = self.dyn_gpu.unwrap();
            let loc = |n: &str| gl.get_uniform_location(program, n);
            gl.uniform_1_i32(loc("u_shading").as_ref(), 0);
            gl.uniform_1_i32(loc("u_has4").as_ref(), 0);
            gl.active_texture(glow::TEXTURE0);
            // Solid meshes first, then see-through ones.
            let mut order: Vec<usize> = (0..self.extras.len()).collect();
            order.sort_by_key(|&i| (self.extras[i].particle, self.extras[i].tint[3] < 1.0));
            for i in order {
                let m = &self.extras[i];
                if m.indices.is_empty() {
                    continue;
                }
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&m.vertices), glow::STREAM_DRAW);
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
                gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytemuck::cast_slice(&m.indices), glow::STREAM_DRAW);
                let solid = m.tint[3] >= 1.0 && !m.particle;
                gl.uniform_1_i32(loc("u_shading").as_ref(), if m.particle { 5 } else { 0 });
                gl.uniform_1_i32(loc("u_alpha_test").as_ref(), solid as i32);
                gl.uniform_1_i32(loc("u_ghost").as_ref(), if solid || m.particle { 0 } else if m.cutout { 2 } else { 1 });
                gl.uniform_4_f32(loc("u_tint").as_ref(), m.tint[0], m.tint[1], m.tint[2], m.tint[3]);
                gl.bind_texture(glow::TEXTURE_2D, m.texture.as_ref().and_then(|k| self.dyn_tex.get(k).copied()).or(self.white));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
                if m.particle {
                    gl.enable(glow::BLEND);
                    if m.additive {
                        gl.blend_func(glow::SRC_ALPHA, glow::ONE);
                    } else {
                        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                    }
                    gl.depth_mask(false);
                } else if solid {
                    gl.disable(glow::BLEND);
                    gl.depth_mask(true);
                } else {
                    gl.enable(glow::BLEND);
                    gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                    gl.depth_mask(false);
                }
                gl.draw_elements(glow::TRIANGLES, m.indices.len() as i32, glow::UNSIGNED_INT, 0);
            }
            gl.bind_vertex_array(None);
            gl.uniform_4_f32(loc("u_tint").as_ref(), 1.0, 1.0, 1.0, 1.0);
            gl.uniform_1_i32(loc("u_ghost").as_ref(), 0);
            gl.depth_mask(true);
        }
    }

    unsafe fn upload_batch(gl: &glow::Context, b: &Batch) -> Option<GpuBatch> {
        unsafe {
            let vao = gl.create_vertex_array().ok()?;
            let vbo = gl.create_buffer().ok()?;
            let ibo = gl.create_buffer().ok()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&b.vertices), glow::STATIC_DRAW);
            let stride = std::mem::size_of::<lol_map_studio::render::RVertex>() as i32;
            for (loc, size, off) in [(0u32, 3, 0), (1, 3, 12), (2, 2, 24), (3, 2, 32), (4, 4, 40)] {
                gl.enable_vertex_attrib_array(loc);
                gl.vertex_attrib_pointer_f32(loc, size, glow::FLOAT, false, stride, off);
            }
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
            gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytemuck::cast_slice(&b.indices), glow::STATIC_DRAW);
            gl.bind_vertex_array(None);
            let c = [(b.bounds[0] + b.bounds[3]) / 2.0, (b.bounds[1] + b.bounds[4]) / 2.0, (b.bounds[2] + b.bounds[5]) / 2.0];
            Some(GpuBatch { vao, vbo, ibo, count: b.indices.len() as i32, shading: b.shading, blend: b.blend, textures: b.textures, clamp: b.clamp, center: c })
        }
    }

    /// Draws into the current egui viewport. `eye` orders blended batches back to front.
    pub unsafe fn paint(&mut self, gl: &glow::Context, info: &eframe::egui::PaintCallbackInfo, mvp: &Mat4, eye: [f32; 3], clear: [f32; 4]) {
        unsafe {
            if self.program.is_none() && self.error.is_none() {
                if let Err(e) = self.init(gl) {
                    self.error = Some(format!("Renderer unavailable: {e}"));
                }
            }
            let Some(program) = self.program else { return };
            if let Some(s) = self.pending.take() {
                self.upload(gl, &s);
            }
            let vp = info.viewport_in_pixels();
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
            gl.viewport(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
            gl.clear_color(clear[0], clear[1], clear[2], clear[3]);
            gl.clear_depth_f32(1.0);
            gl.depth_mask(true);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.disable(glow::CULL_FACE);
            gl.use_program(Some(program));
            let loc = |n: &str| gl.get_uniform_location(program, n);
            gl.uniform_matrix_4_f32_slice(loc("u_mvp").as_ref(), false, mvp);
            gl.uniform_3_f32_slice(loc("u_sun_dir").as_ref(), &self.sun.dir);
            gl.uniform_3_f32_slice(loc("u_sun_color").as_ref(), &self.sun.color);
            gl.uniform_3_f32_slice(loc("u_ambient").as_ref(), &self.sun.ambient);
            gl.uniform_1_f32(loc("u_time").as_ref(), self.start.elapsed().as_secs_f32());
            gl.uniform_4_f32(loc("u_tint").as_ref(), 1.0, 1.0, 1.0, 1.0);
            gl.uniform_1_i32(loc("u_ghost").as_ref(), 0);
            let lk = self.look;
            gl.uniform_3_f32(loc("u_grade").as_ref(), lk.gamma, lk.brightness, lk.contrast);
            gl.uniform_4_f32(loc("u_fog").as_ref(), lk.fog[0], lk.fog[1], lk.fog[2], lk.fog[3]);
            gl.uniform_2_f32(loc("u_fog_h").as_ref(), lk.fog_heights[0], lk.fog_heights[1]);
            for (i, n) in ["t0", "t1", "t2", "t3", "t4"].iter().enumerate() {
                gl.uniform_1_i32(loc(n).as_ref(), i as i32);
            }
            let (u_shading, u_alpha, u_has4) = (loc("u_shading"), loc("u_alpha_test"), loc("u_has4"));
            let mut order: Vec<usize> = (0..self.batches.len()).collect();
            let dist = |b: &GpuBatch| (b.center[0] - eye[0]).powi(2) + (b.center[1] - eye[1]).powi(2) + (b.center[2] - eye[2]).powi(2);
            order.sort_by(|&a, &b| {
                let (x, y) = (&self.batches[a], &self.batches[b]);
                let tx = matches!(x.blend, Blend::Blend | Blend::Premultiplied);
                let ty = matches!(y.blend, Blend::Blend | Blend::Premultiplied);
                tx.cmp(&ty).then_with(|| if tx { dist(y).partial_cmp(&dist(x)).unwrap_or(std::cmp::Ordering::Equal) } else { std::cmp::Ordering::Equal })
            });
            for i in order {
                let b = &self.batches[i];
                match b.blend {
                    Blend::Opaque | Blend::AlphaTest => {
                        gl.disable(glow::BLEND);
                        gl.depth_mask(true);
                    }
                    Blend::Blend => {
                        gl.enable(glow::BLEND);
                        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                        gl.depth_mask(false);
                    }
                    Blend::Premultiplied => {
                        gl.enable(glow::BLEND);
                        gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
                        gl.depth_mask(false);
                    }
                }
                gl.uniform_1_i32(u_shading.as_ref(), b.shading as i32);
                gl.uniform_1_i32(u_alpha.as_ref(), (b.blend == Blend::AlphaTest) as i32);
                gl.uniform_1_i32(u_has4.as_ref(), (b.textures[4].is_some() && b.shading == Shading::Lit) as i32);
                for (slot, t) in b.textures.iter().enumerate() {
                    gl.active_texture(glow::TEXTURE0 + slot as u32);
                    let tex = t.and_then(|i| self.textures.get(i).copied()).or(self.white);
                    gl.bind_texture(glow::TEXTURE_2D, tex);
                    let wrap = if b.clamp || (slot == 4 && b.shading == Shading::FourBlend) { glow::CLAMP_TO_EDGE } else { glow::REPEAT };
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap as i32);
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap as i32);
                }
                gl.bind_vertex_array(Some(b.vao));
                gl.draw_elements(glow::TRIANGLES, b.count, glow::UNSIGNED_INT, 0);
            }
            gl.bind_vertex_array(None);
            self.paint_extras(gl, program);
            self.paint_character(gl, program);
            gl.active_texture(glow::TEXTURE0);
            gl.depth_mask(true);
            gl.disable(glow::DEPTH_TEST);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }
    }
}
