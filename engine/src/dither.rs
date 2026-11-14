//! Ordered dither. A threshold matrix rather than noise, so a pattern
//! textured with it lines up from one cell to the next instead of speckling.

/// 8x8 ordered (Bayer) dither, values `(m + 0.5)/64`.
///
/// ORDERED, not random. This is the single most visible way to get the look
/// wrong: an ordered matrix makes a pattern line up from one row of cells to
/// the next, so it reads as structure rather than as speckle.
pub static DITHER: [f32; 64] = build_dither();

const fn build_dither() -> [f32; 64] {
    // Recursive Bayer construction, unrolled for const evaluation.
    let mut m = [[0u32; 8]; 8];
    let mut n = 1usize;
    while n < 8 {
        let mut y = 0;
        while y < n {
            let mut x = 0;
            while x < n {
                let v = 4 * m[y][x];
                m[y][x] = v;
                m[y][x + n] = v + 2;
                m[y + n][x] = v + 3;
                m[y + n][x + n] = v + 1;
                x += 1;
            }
            y += 1;
        }
        n *= 2;
    }
    let mut out = [0.0f32; 64];
    let mut y = 0;
    while y < 8 {
        let mut x = 0;
        while x < 8 {
            out[y * 8 + x] = (m[y][x] as f32 + 0.5) / 64.0;
            x += 1;
        }
        y += 1;
    }
    out
}
