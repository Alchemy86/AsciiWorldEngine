//! An RGB picture in floating point, read from a JPEG or a binary PPM, and
//! sampled anywhere in it with bilinear filtering.

pub struct Image {
    pub w: usize,
    pub h: usize,
    /// Row-major, 0..1 per channel.
    pub px: Vec<[f32; 3]>,
}

impl Image {
    pub fn from_rgb8(w: usize, h: usize, rgb: &[u8]) -> Image {
        let px = rgb
            .chunks_exact(3)
            .map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0])
            .collect();
        Image { w, h, px }
    }

    /// A baseline JPEG (`FF D8`) or a binary PPM (`P6`), told apart by their
    /// first bytes rather than the file name.
    pub fn load(path: &str) -> Result<Image, String> {
        let d = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        if d.starts_with(&[0xFF, 0xD8]) {
            crate::jpeg::decode(&d).map_err(|e| format!("{path}: {e}"))
        } else if d.starts_with(b"P6") {
            ppm(&d).map_err(|e| format!("{path}: {e}"))
        } else {
            Err(format!("{path}: not a JPEG or a binary PPM (P6)"))
        }
    }

    /// Box-average down so neither side exceeds `max_w` x `max_h`. Sampling a
    /// big picture straight onto a small grid would alias brush strokes into
    /// noise; averaging first keeps each cell the colour of its own patch.
    pub fn shrink(&self, max_w: usize, max_h: usize) -> Image {
        let f = (self.w as f32 / max_w as f32).max(self.h as f32 / max_h as f32).max(1.0);
        let (w, h) = (((self.w as f32 / f) as usize).max(1), ((self.h as f32 / f) as usize).max(1));
        let mut px = vec![[0.0f32; 3]; w * h];
        for y in 0..h {
            let y0 = y * self.h / h;
            let y1 = ((y + 1) * self.h / h).max(y0 + 1);
            for x in 0..w {
                let x0 = x * self.w / w;
                let x1 = ((x + 1) * self.w / w).max(x0 + 1);
                let mut s = [0.0f32; 3];
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let p = self.px[sy * self.w + sx];
                        s[0] += p[0];
                        s[1] += p[1];
                        s[2] += p[2];
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as f32;
                px[y * w + x] = [s[0] / n, s[1] / n, s[2] / n];
            }
        }
        Image { w, h, px }
    }

    /// The colour at `(u, v)` in 0..1 across and down, bilinear, clamped at
    /// the edges.
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> [f32; 3] {
        let fx = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let p = |x: usize, y: usize| self.px[y * self.w + x];
        let (a, b, c, d) = (p(x0, y0), p(x1, y0), p(x0, y1), p(x1, y1));
        let mut o = [0.0f32; 3];
        for i in 0..3 {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bot = c[i] + (d[i] - c[i]) * tx;
            o[i] = top + (bot - top) * ty;
        }
        o
    }
}

/// Rec. 709 luma, on the 0..1 channels as they are (no linearising: this is
/// about how bright a character should look, not about light).
#[inline]
pub fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn ppm(d: &[u8]) -> Result<Image, String> {
    let mut fields = Vec::new();
    let mut i = 2;
    while fields.len() < 3 {
        while i < d.len() && d[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < d.len() && d[i] == b'#' {
            while i < d.len() && d[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let s = i;
        while i < d.len() && d[i].is_ascii_digit() {
            i += 1;
        }
        let n: usize = std::str::from_utf8(&d[s..i])
            .ok()
            .and_then(|t| t.parse().ok())
            .ok_or("bad PPM header")?;
        fields.push(n);
    }
    let (w, h, max) = (fields[0], fields[1], fields[2]);
    if max != 255 {
        return Err("only 8-bit PPM is supported".into());
    }
    let body = d.get(i + 1..i + 1 + w * h * 3).ok_or("truncated PPM")?;
    Ok(Image::from_rgb8(w, h, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ppm_reads_back_the_pixels_it_was_given() {
        let mut d = b"P6\n# a comment\n2 1\n255\n".to_vec();
        d.extend_from_slice(&[255, 0, 0, 0, 0, 255]);
        let p = ppm(&d).unwrap();
        assert_eq!((p.w, p.h), (2, 1));
        assert_eq!(p.px[0], [1.0, 0.0, 0.0]);
        assert_eq!(p.px[1], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn shrinking_averages_and_sampling_interpolates() {
        let img = Image::from_rgb8(2, 2, &[0, 0, 0, 255, 255, 255, 0, 0, 0, 255, 255, 255]);
        let s = img.shrink(1, 1);
        assert!((s.px[0][0] - 0.5).abs() < 1e-6);
        let mid = img.sample(0.5, 0.5);
        assert!((mid[0] - 0.5).abs() < 1e-6);
    }
}
