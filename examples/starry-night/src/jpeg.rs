//! A baseline JPEG decoder, just big enough for one painting.
//!
//! Huffman-coded, 8-bit, sequential DCT (SOF0/SOF1), any chroma subsampling,
//! restart markers, interleaved or single-component scans. Not progressive and
//! not arithmetic-coded: those fail with an error that says so. Chroma is
//! upsampled by replication, which is all a picture about to be averaged down
//! to terminal cells needs.

use crate::image::Image;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27,
    20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58,
    59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone, Default)]
struct Huffman {
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    values: Vec<u8>,
}

impl Huffman {
    fn new(counts: &[u8; 16], values: Vec<u8>) -> Huffman {
        let mut h = Huffman { maxcode: [-1; 18], values, ..Default::default() };
        let mut code = 0i32;
        let mut k = 0i32;
        for l in 1..=16 {
            let n = counts[l - 1] as i32;
            if n > 0 {
                h.valptr[l] = k;
                h.mincode[l] = code;
                code += n;
                k += n;
                h.maxcode[l] = code - 1;
            }
            code <<= 1;
        }
        h.maxcode[17] = i32::MAX;
        h
    }
}

#[derive(Clone)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    td: usize,
    ta: usize,
    /// Blocks across and down this component's plane, padded to whole MCUs.
    bw: usize,
    bh: usize,
    plane: Vec<u8>,
    pred: i32,
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u32,
    n: u32,
}

impl<'a> Bits<'a> {
    fn byte(&mut self) -> u32 {
        if self.pos >= self.data.len() {
            return 0;
        }
        let b = self.data[self.pos];
        if b == 0xFF {
            let next = self.data.get(self.pos + 1).copied().unwrap_or(0);
            if next == 0x00 {
                self.pos += 2;
                return 0xFF;
            }
            // A marker: the scan's data has run out. Pad with zeros and leave
            // the marker where it is for whoever reads next.
            return 0;
        }
        self.pos += 1;
        b as u32
    }

    fn bit(&mut self) -> u32 {
        if self.n == 0 {
            self.acc = self.byte();
            self.n = 8;
        }
        self.n -= 1;
        (self.acc >> self.n) & 1
    }

    fn receive(&mut self, s: u32) -> i32 {
        let mut v = 0i32;
        for _ in 0..s {
            v = (v << 1) | self.bit() as i32;
        }
        v
    }

    fn decode(&mut self, h: &Huffman) -> Result<u8, String> {
        let mut code = self.bit() as i32;
        let mut l = 1usize;
        while code > h.maxcode[l] {
            code = (code << 1) | self.bit() as i32;
            l += 1;
            if l > 16 {
                return Err("corrupt Huffman code".into());
            }
        }
        let i = (h.valptr[l] + code - h.mincode[l]) as usize;
        h.values.get(i).copied().ok_or_else(|| "Huffman value out of range".to_string())
    }

    fn restart(&mut self) {
        self.n = 0;
        if self.pos + 1 < self.data.len()
            && self.data[self.pos] == 0xFF
            && (0xD0..=0xD7).contains(&self.data[self.pos + 1])
        {
            self.pos += 2;
        }
    }
}

fn extend(v: i32, s: u32) -> i32 {
    if s == 0 {
        0
    } else if v < 1 << (s - 1) {
        v - (1 << s) + 1
    } else {
        v
    }
}

/// `cos((2x + 1) u pi / 16) * c(u) / 2`, the separable IDCT's whole table.
fn idct_table() -> [[f32; 8]; 8] {
    let mut t = [[0.0f32; 8]; 8];
    for (x, row) in t.iter_mut().enumerate() {
        for (u, v) in row.iter_mut().enumerate() {
            let c = if u == 0 { std::f32::consts::FRAC_1_SQRT_2 } else { 1.0 };
            *v = c * 0.5 * (((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI) / 16.0).cos();
        }
    }
    t
}

fn idct(coef: &[i32; 64], t: &[[f32; 8]; 8], out: &mut [u8; 64]) {
    let mut tmp = [0.0f32; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0.0;
            for u in 0..8 {
                s += t[x][u] * coef[y * 8 + u] as f32;
            }
            tmp[y * 8 + x] = s;
        }
    }
    for x in 0..8 {
        for y in 0..8 {
            let mut s = 0.0;
            for v in 0..8 {
                s += t[y][v] * tmp[v * 8 + x];
            }
            out[y * 8 + x] = (s + 128.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn be16(d: &[u8], i: usize) -> Result<usize, String> {
    if i + 1 >= d.len() {
        return Err("truncated JPEG".into());
    }
    Ok(((d[i] as usize) << 8) | d[i + 1] as usize)
}

/// Decode a baseline JPEG to RGB.
pub fn decode(d: &[u8]) -> Result<Image, String> {
    if d.len() < 4 || d[0] != 0xFF || d[1] != 0xD8 {
        return Err("not a JPEG".into());
    }
    let mut qt = [[0u16; 64]; 4];
    let mut dc: [Huffman; 4] = Default::default();
    let mut ac: [Huffman; 4] = Default::default();
    let mut comps: Vec<Component> = Vec::new();
    let (mut width, mut height) = (0usize, 0usize);
    let (mut hmax, mut vmax) = (1usize, 1usize);
    let mut restart = 0usize;
    let table = idct_table();
    let mut i = 2usize;

    while i + 3 < d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = d[i + 1];
        if m == 0xFF || m == 0x00 || (0xD0..=0xD7).contains(&m) || m == 0x01 {
            i += if m == 0xFF { 1 } else { 2 };
            continue;
        }
        if m == 0xD9 {
            break;
        }
        let len = be16(d, i + 2)?;
        let seg = d.get(i + 4..i + 2 + len).ok_or("truncated JPEG segment")?;
        match m {
            0xDB => {
                let mut p = 0;
                while p < seg.len() {
                    let pq = seg[p] >> 4;
                    let tq = (seg[p] & 15) as usize & 3;
                    p += 1;
                    for k in 0..64 {
                        qt[tq][k] = if pq == 0 {
                            p += 1;
                            seg[p - 1] as u16
                        } else {
                            p += 2;
                            ((seg[p - 2] as u16) << 8) | seg[p - 1] as u16
                        };
                    }
                }
            }
            0xC0 | 0xC1 => {
                height = be16(seg, 1)?;
                width = be16(seg, 3)?;
                let n = seg[5] as usize;
                for c in 0..n {
                    let b = &seg[6 + c * 3..9 + c * 3];
                    comps.push(Component {
                        id: b[0],
                        h: (b[1] >> 4).max(1) as usize,
                        v: (b[1] & 15).max(1) as usize,
                        tq: (b[2] & 3) as usize,
                        td: 0,
                        ta: 0,
                        bw: 0,
                        bh: 0,
                        plane: Vec::new(),
                        pred: 0,
                    });
                }
                hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
                vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
                let mcux = width.div_ceil(8 * hmax);
                let mcuy = height.div_ceil(8 * vmax);
                for c in comps.iter_mut() {
                    c.bw = mcux * c.h;
                    c.bh = mcuy * c.v;
                    c.plane = vec![0; c.bw * 8 * c.bh * 8];
                }
            }
            0xC2 | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(format!(
                    "unsupported JPEG (SOF{:X}: progressive, lossless or arithmetic); \
                     convert it to a baseline JPEG or a PPM first",
                    m - 0xC0
                ));
            }
            0xC4 => {
                let mut p = 0;
                while p + 17 <= seg.len() {
                    let class = seg[p] >> 4;
                    let id = (seg[p] & 15) as usize & 3;
                    let mut counts = [0u8; 16];
                    counts.copy_from_slice(&seg[p + 1..p + 17]);
                    let total: usize = counts.iter().map(|&c| c as usize).sum();
                    let values = seg.get(p + 17..p + 17 + total).ok_or("truncated DHT")?.to_vec();
                    let h = Huffman::new(&counts, values);
                    if class == 0 {
                        dc[id] = h;
                    } else {
                        ac[id] = h;
                    }
                    p += 17 + total;
                }
            }
            0xDD => restart = be16(seg, 0)?,
            0xDA => {
                if comps.is_empty() {
                    return Err("scan before frame header".into());
                }
                let n = seg[0] as usize;
                let mut members = Vec::with_capacity(n);
                for k in 0..n {
                    let id = seg[1 + k * 2];
                    let t = seg[2 + k * 2];
                    let ci = comps.iter().position(|c| c.id == id).ok_or("scan names an unknown component")?;
                    comps[ci].td = (t >> 4) as usize & 3;
                    comps[ci].ta = (t & 15) as usize & 3;
                    comps[ci].pred = 0;
                    members.push(ci);
                }
                let start = i + 2 + len;
                let mut bits = Bits { data: d, pos: start, acc: 0, n: 0 };
                scan(&mut comps, &members, &qt, &dc, &ac, &table, &mut bits, restart, width, height, hmax, vmax)?;
                let mut p = bits.pos;
                while p + 1 < d.len() && !(d[p] == 0xFF && d[p + 1] != 0 && !(0xD0..=0xD7).contains(&d[p + 1])) {
                    p += 1;
                }
                i = p;
                continue;
            }
            _ => {}
        }
        i += 2 + len;
    }

    if comps.is_empty() || width == 0 || height == 0 {
        return Err("no image in JPEG".into());
    }
    let mut rgb = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let sample = |c: &Component| -> f32 {
                let sx = x * c.h / hmax;
                let sy = y * c.v / vmax;
                c.plane[sy * c.bw * 8 + sx] as f32
            };
            let o = (y * width + x) * 3;
            if comps.len() >= 3 {
                let (yy, cb, cr) = (sample(&comps[0]), sample(&comps[1]) - 128.0, sample(&comps[2]) - 128.0);
                rgb[o] = (yy + 1.402 * cr).round().clamp(0.0, 255.0) as u8;
                rgb[o + 1] = (yy - 0.344136 * cb - 0.714136 * cr).round().clamp(0.0, 255.0) as u8;
                rgb[o + 2] = (yy + 1.772 * cb).round().clamp(0.0, 255.0) as u8;
            } else {
                let g = sample(&comps[0]) as u8;
                rgb[o] = g;
                rgb[o + 1] = g;
                rgb[o + 2] = g;
            }
        }
    }
    Ok(Image::from_rgb8(width, height, &rgb))
}

#[allow(clippy::too_many_arguments)]
fn scan(
    comps: &mut [Component],
    members: &[usize],
    qt: &[[u16; 64]; 4],
    dc: &[Huffman; 4],
    ac: &[Huffman; 4],
    table: &[[f32; 8]; 8],
    bits: &mut Bits,
    restart: usize,
    width: usize,
    height: usize,
    hmax: usize,
    vmax: usize,
) -> Result<(), String> {
    let block = |c: &mut Component, bits: &mut Bits, bx: usize, by: usize| -> Result<(), String> {
        let mut coef = [0i32; 64];
        let q = &qt[c.tq];
        let s = bits.decode(&dc[c.td])? as u32;
        let diff = extend(bits.receive(s), s);
        c.pred += diff;
        coef[0] = c.pred * q[0] as i32;
        let mut k = 1usize;
        while k < 64 {
            let rs = bits.decode(&ac[c.ta])?;
            let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
            if s == 0 {
                if r == 15 {
                    k += 16;
                    continue;
                }
                break;
            }
            k += r;
            if k > 63 {
                break;
            }
            coef[ZIGZAG[k]] = extend(bits.receive(s), s) * q[k] as i32;
            k += 1;
        }
        let mut px = [0u8; 64];
        idct(&coef, table, &mut px);
        let stride = c.bw * 8;
        for y in 0..8 {
            let row = (by * 8 + y) * stride + bx * 8;
            c.plane[row..row + 8].copy_from_slice(&px[y * 8..y * 8 + 8]);
        }
        Ok(())
    };

    let reset = |comps: &mut [Component], bits: &mut Bits| {
        bits.restart();
        for c in comps.iter_mut() {
            c.pred = 0;
        }
    };

    if members.len() == 1 {
        let ci = members[0];
        let c = &comps[ci];
        let w = (width * c.h).div_ceil(hmax).div_ceil(8);
        let h = (height * c.v).div_ceil(vmax).div_ceil(8);
        let mut n = 0usize;
        for by in 0..h {
            for bx in 0..w {
                if restart > 0 && n > 0 && n % restart == 0 {
                    reset(comps, bits);
                }
                block(&mut comps[ci], bits, bx, by)?;
                n += 1;
            }
        }
        return Ok(());
    }

    let mcux = width.div_ceil(8 * hmax);
    let mcuy = height.div_ceil(8 * vmax);
    let mut n = 0usize;
    for my in 0..mcuy {
        for mx in 0..mcux {
            if restart > 0 && n > 0 && n % restart == 0 {
                reset(comps, bits);
            }
            for &ci in members {
                let (h, v) = (comps[ci].h, comps[ci].v);
                for yy in 0..v {
                    for xx in 0..h {
                        block(&mut comps[ci], bits, mx * h + xx, my * v + yy)?;
                    }
                }
            }
            n += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a real JPEG named by `STARRY_JPEG` and write it as a PPM to
    /// `STARRY_PPM_OUT`, so the decoder can be compared pixel for pixel with an
    /// independent one. `cargo test -p starry-night -- --ignored`.
    #[test]
    #[ignore]
    fn decode_a_real_file_for_comparison() {
        let src = std::env::var("STARRY_JPEG").expect("set STARRY_JPEG");
        let out = std::env::var("STARRY_PPM_OUT").expect("set STARRY_PPM_OUT");
        let img = decode(&std::fs::read(src).unwrap()).unwrap();
        let mut d = format!("P6\n{} {}\n255\n", img.w, img.h).into_bytes();
        for p in &img.px {
            d.extend(p.iter().map(|v| (v * 255.0).round() as u8));
        }
        std::fs::write(out, d).unwrap();
    }

    #[test]
    fn a_non_jpeg_and_a_progressive_jpeg_are_refused_by_name() {
        assert!(decode(b"GIF89a").is_err());
        let sof2 = [0xFF, 0xD8, 0xFF, 0xC2, 0x00, 0x0B, 8, 0, 1, 0, 1, 1, 1, 0x11, 0, 0xFF, 0xD9];
        assert!(decode(&sof2).err().unwrap_or_default().contains("progressive"));
    }
}
