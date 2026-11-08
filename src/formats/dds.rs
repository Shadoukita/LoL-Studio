//! DDS textures: decoding (DXT1/3/5, 32/24-bit) for previews, and writing A8R8G8B8 textures,
//! a format the game's texture registry loads (`DDSF_A8R8G8B8`).

use anyhow::{Result, bail};

#[derive(Clone)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// RGBA8
    pub pixels: Vec<u8>,
}

pub struct DdsInfo {
    pub width: usize,
    pub height: usize,
    pub mipmaps: usize,
    pub format: String,
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

pub fn info(d: &[u8]) -> Result<DdsInfo> {
    if d.len() < 128 || &d[0..4] != b"DDS " {
        bail!("not a DDS file");
    }
    let height = u32_at(d, 12) as usize;
    let width = u32_at(d, 16) as usize;
    let mipmaps = (u32_at(d, 28) as usize).max(1);
    let pf_flags = u32_at(d, 80);
    let fourcc = &d[84..88];
    let format = if pf_flags & 4 != 0 {
        String::from_utf8_lossy(fourcc).to_string()
    } else {
        let bits = u32_at(d, 88);
        let amask = u32_at(d, 104);
        format!("{}-bit {}", bits, if amask != 0 { "RGBA" } else { "RGB" })
    };
    Ok(DdsInfo { width, height, mipmaps, format })
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8]
}

fn color_block(b: &[u8], out: &mut [[u8; 4]; 16], dxt1: bool) {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (a, bb) = (rgb565(c0), rgb565(c1));
    let mix = |x: u8, y: u8, wx: u32, wy: u32| ((x as u32 * wx + y as u32 * wy) / (wx + wy)) as u8;
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [a[0], a[1], a[2], 255];
    pal[1] = [bb[0], bb[1], bb[2], 255];
    if c0 > c1 || !dxt1 {
        pal[2] = [mix(a[0], bb[0], 2, 1), mix(a[1], bb[1], 2, 1), mix(a[2], bb[2], 2, 1), 255];
        pal[3] = [mix(a[0], bb[0], 1, 2), mix(a[1], bb[1], 1, 2), mix(a[2], bb[2], 1, 2), 255];
    } else {
        pal[2] = [mix(a[0], bb[0], 1, 1), mix(a[1], bb[1], 1, 1), mix(a[2], bb[2], 1, 1), 255];
        pal[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    for i in 0..16 {
        out[i] = pal[((bits >> (2 * i)) & 3) as usize];
    }
}

/// Decodes the top mip level. `max_side` downsamples large textures for previews.
pub fn decode(d: &[u8], max_side: usize) -> Result<Image> {
    let inf = info(d)?;
    let (w, h) = (inf.width, inf.height);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        bail!("DDS has an invalid size {w}x{h}");
    }
    let data = &d[128..];
    let pf_flags = u32_at(d, 80);
    let mut px = vec![0u8; w * h * 4];
    if pf_flags & 4 != 0 {
        let fourcc = &d[84..88];
        let (block_bytes, kind) = match fourcc {
            b"DXT1" => (8, 1),
            b"DXT3" => (16, 3),
            b"DXT5" => (16, 5),
            _ => bail!("DDS compression {} is not supported for preview", String::from_utf8_lossy(fourcc)),
        };
        let bw = w.div_ceil(4);
        let bh = h.div_ceil(4);
        if data.len() < bw * bh * block_bytes {
            bail!("DDS data is truncated");
        }
        let mut block = [[0u8; 4]; 16];
        for by in 0..bh {
            for bx in 0..bw {
                let b = &data[(by * bw + bx) * block_bytes..][..block_bytes];
                match kind {
                    1 => color_block(b, &mut block, true),
                    3 => {
                        color_block(&b[8..], &mut block, false);
                        for i in 0..16 {
                            let a = (b[i / 2] >> ((i % 2) * 4)) & 15;
                            block[i][3] = a * 17;
                        }
                    }
                    _ => {
                        color_block(&b[8..], &mut block, false);
                        let (a0, a1) = (b[0] as u32, b[1] as u32);
                        let mut pal = [0u32; 8];
                        pal[0] = a0;
                        pal[1] = a1;
                        if a0 > a1 {
                            for k in 1..7 {
                                pal[k + 1] = ((7 - k as u32) * a0 + k as u32 * a1) / 7;
                            }
                        } else {
                            for k in 1..5 {
                                pal[k + 1] = ((5 - k as u32) * a0 + k as u32 * a1) / 5;
                            }
                            pal[6] = 0;
                            pal[7] = 255;
                        }
                        let bits = u64::from_le_bytes([b[2], b[3], b[4], b[5], b[6], b[7], 0, 0]);
                        for i in 0..16 {
                            block[i][3] = pal[((bits >> (3 * i)) & 7) as usize] as u8;
                        }
                    }
                }
                for i in 0..16 {
                    let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                    if x < w && y < h {
                        px[(y * w + x) * 4..][..4].copy_from_slice(&block[i]);
                    }
                }
            }
        }
    } else {
        let bits = u32_at(d, 88) as usize;
        let masks = [u32_at(d, 92), u32_at(d, 96), u32_at(d, 100), u32_at(d, 104)];
        let bpp = bits / 8;
        if !(bpp == 3 || bpp == 4) || data.len() < w * h * bpp {
            bail!("DDS {bits}-bit pixel format is not supported for preview");
        }
        let chan = |v: u32, m: u32| -> u8 {
            if m == 0 {
                return 255;
            }
            let shift = m.trailing_zeros();
            let max = m >> shift;
            (((v & m) >> shift) * 255 / max) as u8
        };
        for i in 0..w * h {
            let s = &data[i * bpp..];
            let v = if bpp == 4 { u32::from_le_bytes([s[0], s[1], s[2], s[3]]) } else { u32::from_le_bytes([s[0], s[1], s[2], 0]) };
            px[i * 4] = chan(v, masks[0]);
            px[i * 4 + 1] = chan(v, masks[1]);
            px[i * 4 + 2] = chan(v, masks[2]);
            px[i * 4 + 3] = if masks[3] != 0 { chan(v, masks[3]) } else { 255 };
        }
    }
    let mut img = Image { width: w, height: h, pixels: px };
    while img.width > max_side.max(1) || img.height > max_side.max(1) {
        img = half(&img);
    }
    Ok(img)
}

pub fn half(img: &Image) -> Image {
    let (w, h) = ((img.width / 2).max(1), (img.height / 2).max(1));
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            for c in 0..4 {
                let mut s = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(img.width - 1);
                    let sy = (y * 2 + dy).min(img.height - 1);
                    s += img.pixels[(sy * img.width + sx) * 4 + c] as u32;
                }
                px[(y * w + x) * 4 + c] = (s / 4) as u8;
            }
        }
    }
    Image { width: w, height: h, pixels: px }
}

/// Average colour of a texture (used to tint the map preview).
pub fn average_color(d: &[u8]) -> Option<[u8; 3]> {
    let img = decode(d, 64).ok()?;
    let mut s = [0u64; 3];
    let n = (img.width * img.height) as u64;
    for p in img.pixels.chunks_exact(4) {
        for c in 0..3 {
            s[c] += p[c] as u64;
        }
    }
    Some([(s[0] / n) as u8, (s[1] / n) as u8, (s[2] / n) as u8])
}

/// Writes an uncompressed A8R8G8B8 DDS with a full mip chain.
pub fn encode_argb(img: &Image, mipmaps: bool) -> Vec<u8> {
    let mut levels = vec![Image { width: img.width, height: img.height, pixels: img.pixels.clone() }];
    if mipmaps {
        while levels.last().unwrap().width > 1 || levels.last().unwrap().height > 1 {
            let next = half(levels.last().unwrap());
            levels.push(next);
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"DDS ");
    let mut hdr = [0u32; 31];
    hdr[0] = 124;
    hdr[1] = 0x1 | 0x2 | 0x4 | 0x1000 | 0x8 | if mipmaps { 0x20000 } else { 0 };
    hdr[2] = img.height as u32;
    hdr[3] = img.width as u32;
    hdr[4] = (img.width * 4) as u32;
    hdr[6] = levels.len() as u32;
    // pixel format at dword 18
    hdr[18] = 32;
    hdr[19] = 0x41; // RGB | ALPHAPIXELS
    hdr[21] = 32;
    hdr[22] = 0x00ff0000;
    hdr[23] = 0x0000ff00;
    hdr[24] = 0x000000ff;
    hdr[25] = 0xff000000;
    hdr[26] = 0x1000 | if mipmaps { 0x400008 } else { 0 };
    for v in hdr {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for l in &levels {
        for p in l.pixels.chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out
}
