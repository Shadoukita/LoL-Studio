//! Auto dressing: painted areas are traced into smooth
//! outlines; models are placed by walking each outline at exact steps (so they follow the drawn
//! shape), and the inside is filled on a jittered grid kept a margin away from the edge.

use crate::layout_build::hash01;

/// One model choice: library key, weight, size range (diameter in units).
#[derive(Clone, Debug)]
pub struct Item {
    pub key: String,
    pub weight: f32,
    pub size: [f32; 2],
}

/// Models along an outline.
#[derive(Clone, Debug)]
pub struct EdgeRule {
    pub items: Vec<Item>,
    /// Step along the outline, as a fraction of each model's size (0.5 = half overlapping).
    pub spacing: f32,
    /// How far inside the area each model sits, as a fraction of its size.
    pub inset: f32,
    /// Turn models along the outline (else a random turn).
    pub align: bool,
}

/// Models filling an area.
#[derive(Clone, Debug)]
pub struct FillRule {
    pub items: Vec<Item>,
    /// Grid step in units.
    pub step: f32,
    /// Distance kept from the edge, as a fraction of each model's size.
    pub margin: f32,
    /// Chance a grid spot gets a model.
    pub chance: f32,
}

/// A placed model: key, position, turn (degrees), size (diameter in units), along-edge.
#[derive(Clone, Debug)]
pub struct Placed {
    pub key: String,
    pub at: [f32; 2],
    pub yaw: f32,
    pub size: f32,
    pub edge: bool,
    /// The point on the outline it was placed from (= `at` for fills).
    pub on: [f32; 2],
}

fn pick(items: &[Item], r: f32) -> &Item {
    let total: f32 = items.iter().map(|i| i.weight.max(0.0)).sum::<f32>().max(1e-6);
    let mut t = 0.0;
    for i in items {
        t += i.weight.max(0.0) / total;
        if r < t {
            return i;
        }
    }
    &items[items.len() - 1]
}

/// Smooth closed outlines (counter-clockwise around the area in x/z) of the cells where `mask` is
/// set. `cell` is the cell size; outlines are in world units.
pub fn outlines(mask: &[bool], nx: usize, ny: usize, cell: f32) -> Vec<Vec<[f32; 2]>> {
    if nx < 2 || ny < 2 {
        return Vec::new();
    }
    // A softened field so the traced line is smooth rather than stair-stepped.
    let at = |i: i64, j: i64| -> f32 {
        if i < 0 || j < 0 || i as usize >= nx || j as usize >= ny {
            0.0
        } else if mask[j as usize * nx + i as usize] {
            1.0
        } else {
            0.0
        }
    };
    let mut f = vec![0.0f32; nx * ny];
    for j in 0..ny as i64 {
        for i in 0..nx as i64 {
            let mut s = 0.0;
            for dj in -1..=1 {
                for di in -1..=1 {
                    let w = if di == 0 && dj == 0 { 4.0 } else if di == 0 || dj == 0 { 2.0 } else { 1.0 };
                    s += w * at(i + di, j + dj);
                }
            }
            f[j as usize * nx + i as usize] = s / 16.0;
        }
    }
    let fv = |i: usize, j: usize| f[j * nx + i];
    // Marching squares over cell centres; segments keyed by edge id so they link up exactly.
    // Edge ids: horizontal edge (i,j)-(i+1,j) = 2*(j*nx+i), vertical (i,j)-(i,j+1) = 2*(j*nx+i)+1.
    let pos = |e: usize| -> [f32; 2] {
        let (k, v) = (e / 2, e % 2 == 1);
        let (i, j) = (k % nx, k / nx);
        let (a, b) = if v { (fv(i, j), fv(i, j + 1)) } else { (fv(i, j), fv(i + 1, j)) };
        let t = if (b - a).abs() < 1e-6 { 0.5 } else { ((0.5 - a) / (b - a)).clamp(0.0, 1.0) };
        let (x, z) = if v { (i as f32, j as f32 + t) } else { (i as f32 + t, j as f32) };
        [(x + 0.5) * cell, (z + 0.5) * cell]
    };
    let mut next: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let c = [fv(i, j) > 0.5, fv(i + 1, j) > 0.5, fv(i + 1, j + 1) > 0.5, fv(i, j + 1) > 0.5];
            let code = c[0] as u8 | (c[1] as u8) << 1 | (c[2] as u8) << 2 | (c[3] as u8) << 3;
            // Edges of this square: bottom, right, top, left.
            let eb = 2 * (j * nx + i);
            let er = 2 * (j * nx + i + 1) + 1;
            let et = 2 * ((j + 1) * nx + i);
            let el = 2 * (j * nx + i) + 1;
            // Listed with the inside on the right; stored reversed so outlines run counter-clockwise.
            let segs: &[(usize, usize)] = match code {
                1 => &[(el, eb)],
                2 => &[(eb, er)],
                3 => &[(el, er)],
                4 => &[(er, et)],
                5 => &[(el, et), (er, eb)],
                6 => &[(eb, et)],
                7 => &[(el, et)],
                8 => &[(et, el)],
                9 => &[(et, eb)],
                10 => &[(et, er), (eb, el)],
                11 => &[(et, er)],
                12 => &[(er, el)],
                13 => &[(er, eb)],
                14 => &[(eb, el)],
                _ => &[],
            };
            for &(a, b) in segs {
                next.insert(b, a);
            }
        }
    }
    let mut out = Vec::new();
    let mut keys: Vec<usize> = next.keys().copied().collect();
    keys.sort_unstable();
    let mut used = std::collections::HashSet::new();
    for start in keys {
        if used.contains(&start) {
            continue;
        }
        let mut loop_pts = Vec::new();
        let mut e = start;
        while used.insert(e) {
            loop_pts.push(pos(e));
            match next.get(&e) {
                Some(&n) => e = n,
                None => break,
            }
        }
        if loop_pts.len() >= 4 {
            out.push(smooth(&loop_pts, 2));
        }
    }
    out
}

/// Chaikin corner cutting on a closed polyline.
fn smooth(p: &[[f32; 2]], rounds: usize) -> Vec<[f32; 2]> {
    let mut p = p.to_vec();
    for _ in 0..rounds {
        let n = p.len();
        let mut q = Vec::with_capacity(n * 2);
        for k in 0..n {
            let (a, b) = (p[k], p[(k + 1) % n]);
            q.push([a[0] * 0.75 + b[0] * 0.25, a[1] * 0.75 + b[1] * 0.25]);
            q.push([a[0] * 0.25 + b[0] * 0.75, a[1] * 0.25 + b[1] * 0.75]);
        }
        p = q;
    }
    p
}

/// Signed area (positive = counter-clockwise in x/z).
pub fn area(p: &[[f32; 2]]) -> f32 {
    let n = p.len();
    (0..n).map(|k| p[k][0] * p[(k + 1) % n][1] - p[(k + 1) % n][0] * p[k][1]).sum::<f32>() * 0.5
}

/// Walks a closed outline (`closed`) or an open line at exact steps and places models on it.
/// `inward` = which side is inside (1 = left of the walking direction). Models are offset inside by
/// `inset × size`. `seed` keeps the result stable while the rest of the map is edited.
pub fn along(line: &[[f32; 2]], closed: bool, rule: &EdgeRule, seed: u32, inward: f32, out: &mut Vec<Placed>) {
    if rule.items.is_empty() || line.len() < 2 {
        return;
    }
    let n = line.len();
    let segs = if closed { n } else { n - 1 };
    let lens: Vec<f32> = (0..segs).map(|k| {
        let (a, b) = (line[k], line[(k + 1) % n]);
        (b[0] - a[0]).hypot(b[1] - a[1])
    }).collect();
    let total: f32 = lens.iter().sum();
    let (mut s, mut seg, mut seg_start) = (if closed { 60.0 * hash01(seed, 0, 1) } else { 0.0 }, 0usize, 0.0f32);
    while s <= total + 1e-3 {
        while seg + 1 < segs && s > seg_start + lens[seg] {
            seg_start += lens[seg];
            seg += 1;
        }
        // Choices keyed by where the spot is, so extending a wall keeps the rest of it as it was.
        let (a0, b0) = (line[seg], line[(seg + 1) % n]);
        let t0 = if lens[seg] > 1e-6 { ((s - seg_start) / lens[seg]).clamp(0.0, 1.0) } else { 0.0 };
        let (qx, qz) = (((a0[0] + (b0[0] - a0[0]) * t0) / 60.0).round() as i32 as u32, ((a0[1] + (b0[1] - a0[1]) * t0) / 60.0).round() as i32 as u32);
        let (seed, k) = (qx.wrapping_mul(73_856_093) ^ qz.wrapping_mul(19_349_663), 1u32);
        let it = pick(&rule.items, hash01(seed, k, 2));
        let size = it.size[0] + (it.size[1] - it.size[0]) * hash01(seed, k, 3);
        let (a, b) = (line[seg], line[(seg + 1) % n]);
        let t = if lens[seg] > 1e-6 { ((s - seg_start) / lens[seg]).clamp(0.0, 1.0) } else { 0.0 };
        let d = if lens[seg] > 1e-6 { [(b[0] - a[0]) / lens[seg], (b[1] - a[1]) / lens[seg]] } else { [1.0, 0.0] };
        // Left of the walking direction.
        let nrm = [-d[1] * inward, d[0] * inward];
        let off = rule.inset * size;
        let at = [a[0] + (b[0] - a[0]) * t + nrm[0] * off, a[1] + (b[1] - a[1]) * t + nrm[1] * off];
        let yaw = if rule.align {
            d[1].atan2(d[0]).to_degrees() + if hash01(seed, k, 4) < 0.5 { 0.0 } else { 180.0 } + (hash01(seed, k, 5) - 0.5) * 12.0
        } else {
            360.0 * hash01(seed, k, 4)
        };
        let on = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
        out.push(Placed { key: it.key.clone(), at, yaw, size, edge: true, on });
        s += (size * rule.spacing).max(20.0);
    }
}

/// Exact-step points along an open polyline (for drawn rows): position and direction (degrees).
pub fn resample(line: &[[f32; 2]], step: f32) -> Vec<([f32; 2], f32)> {
    let mut out = Vec::new();
    if line.len() < 2 || step <= 1.0 {
        return out;
    }
    let mut carry = 0.0f32;
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len < 1e-4 {
            continue;
        }
        let d = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let mut s = carry;
        while s <= len {
            out.push(([a[0] + d[0] * s, a[1] + d[1] * s], d[1].atan2(d[0]).to_degrees()));
            s += step;
        }
        carry = s - len;
    }
    out
}

/// Fills a box on a jittered grid; `inside(x, z)` gives the distance inside the area (negative
/// outside). A spot is used when it is at least `margin × size` inside.
pub fn fill(bbox: [f32; 4], rule: &FillRule, seed: u32, inside: &dyn Fn(f32, f32) -> f32, out: &mut Vec<Placed>) {
    if rule.items.is_empty() || rule.step < 10.0 {
        return;
    }
    let st = rule.step;
    let (cols, rows) = (((bbox[2] - bbox[0]) / st) as u32 + 1, ((bbox[3] - bbox[1]) / st) as u32 + 1);
    for r in 0..rows {
        for c in 0..cols {
            let a = seed.wrapping_mul(4096).wrapping_add(c);
            if hash01(a, r, 5) >= rule.chance {
                continue;
            }
            let x = bbox[0] + (c as f32 + 0.5 + 0.8 * (hash01(a, r, 1) - 0.5)) * st;
            let z = bbox[1] + (r as f32 + 0.5 + 0.8 * (hash01(a, r, 2) - 0.5)) * st;
            let it = pick(&rule.items, hash01(a, r, 3));
            let size = it.size[0] + (it.size[1] - it.size[0]) * hash01(a, r, 4);
            if inside(x, z) < rule.margin * size {
                continue;
            }
            out.push(Placed { key: it.key.clone(), at: [x, z], yaw: 360.0 * hash01(a, r, 6), size, edge: false, on: [x, z] });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_outline_is_ccw_and_follows_edge() {
        let (nx, ny) = (40, 40);
        let mut m = vec![false; nx * ny];
        for j in 10..30 {
            for i in 10..30 {
                m[j * nx + i] = true;
            }
        }
        let o = outlines(&m, nx, ny, 25.0);
        assert_eq!(o.len(), 1);
        assert!(area(&o[0]) > 0.0);
        // Edge of the area lies at cell 10 / 30 boundaries (250 / 750 units).
        for p in &o[0] {
            let d = [(p[0] - 250.0).abs(), (p[0] - 750.0).abs(), (p[1] - 250.0).abs(), (p[1] - 750.0).abs()].into_iter().fold(f32::MAX, f32::min);
            assert!(d < 40.0, "{p:?}");
        }
        let rule = EdgeRule { items: vec![Item { key: "r".into(), weight: 1.0, size: [100.0, 100.0] }], spacing: 1.0, inset: 0.3, align: true };
        let mut out = Vec::new();
        along(&o[0], true, &rule, 1, 1.0, &mut out);
        // ~2000 units of outline / 100 per step.
        assert!((17..=22).contains(&out.len()), "{}", out.len());
        // Inset puts them inside the square.
        for p in &out {
            assert!(p.at[0] > 250.0 && p.at[0] < 750.0 && p.at[1] > 250.0 && p.at[1] < 750.0, "{:?}", p.at);
        }
    }

    #[test]
    fn resample_exact_steps() {
        let pts = resample(&[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]], 50.0);
        assert_eq!(pts.len(), 5);
        assert!((pts[3].0[1] - 50.0).abs() < 1e-3 && (pts[3].1 - 90.0).abs() < 1e-3);
    }
}
