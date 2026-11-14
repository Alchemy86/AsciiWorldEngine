//! Colour: a hue-saturation-lightness constructor, and the shaded-cell
//! background — the colour of the part of a cell its glyph does not cover.

use crate::ramp::INK;

/// `hsl(h deg, s%, l%)` -> 8-bit RGB.
///
/// Colour is *always* a floor plus a range: `hsl(h, s, base + range*b)`.
/// Brightness never multiplies a colour toward black — that is why distant
/// surfaces stay vivid instead of fading to grey.
#[inline]
pub fn hsl(h: f32, s: f32, l: f32) -> [u8; 3] {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = (s.clamp(0.0, 100.0)) / 100.0;
    let l = (l.clamp(0.0, 100.0)) / 100.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [
        (((r + m) * 255.0) + 0.5) as u8,
        (((g + m) * 255.0) + 0.5) as u8,
        (((b + m) * 255.0) + 0.5) as u8,
    ]
}

// --- shaded cells --------------------------------------------------------
/// How much of a surface's own colour survives into the gap behind its glyph.
///
/// Not one: a cell whose gap is as bright as its ink is a flat block of colour
/// with a letter hidden in it, and the ramp — the thing that carries most of
/// an ASCII picture's shading — stops meaning anything. A third is enough to close the
/// black gaps while leaving the ink clearly the brighter half.
///
/// Held in 1024ths, because this runs on every cell of every frame and a
/// background does not need floating point to be a third as bright as its ink.
pub const SHADE_LEVEL_Q: u32 = 348; // 0.34

/// How much a DENSE glyph darkens its own background, as a percentage.
///
/// A `@` leaves almost no gap to fill, so what little there is should sit well
/// under the ink; a `.` leaves the whole cell, so its background is nearly all
/// of what the eye gets and has to carry the surface. Without this the ramp
/// compresses from about 10:1 to about 3:1 and the picture goes flat; with it a
/// solid cell still reads around two and a half times a near-empty one.
pub const SHADE_INK_BIAS_PCT: u32 = 45;

/// A background is slightly greyer than the ink in front of it, which is what
/// keeps a wall of lit panes from turning into a wall of solid colour.
pub const SHADE_DESAT_PCT: u32 = 22;

/// **What each glyph's background is scaled by**, in 1024ths: the level, taken
/// down by how much of its cell that glyph already covers. One table lookup on
/// the draw path instead of the arithmetic above, because `put` runs it for
/// every cell of every shaded frame.
static SHADE_K: [u16; 96] = {
    let mut t = [0u16; 96];
    let mut i = 0;
    while i < 96 {
        let ink = INK[i] as u32;
        t[i] = (SHADE_LEVEL_Q * (100 - SHADE_INK_BIAS_PCT * ink / 100) / 100) as u16;
        i += 1;
    }
    t
};

/// The scale a glyph's own background is taken at. `shade` is the whole of it;
/// this is exposed so a caller drawing many cells of one glyph can hoist it.
#[inline]
pub fn shade_k(ch: u8) -> u32 {
    SHADE_K[if ch < 32 || ch >= 128 { 0 } else { (ch - 32) as usize }] as u32
}

/// **The colour of the gaps around a glyph**, given the surface colour that
/// glyph was drawn in.
///
/// This is the whole of the shaded look in one function: the cell keeps the
/// colour the renderer already chose for it, and the part of the cell the glyph
/// does not cover gets a dimmer, slightly greyer version of the same colour
/// instead of black. Hue is very nearly preserved — the same reason `dim_rgb`
/// scales rather than blends — so a cell reads as one surface at one colour,
/// not as ink of one colour over a wash of another.
///
/// The result is quantised to multiples of four. A background is a dim colour
/// and four counts of 255 is invisible there, but it turns a smooth gradient
/// into runs of equal cells, and the ANSI writer emits one escape per run.
#[inline]
pub fn shade(c: [u8; 3], ch: u8) -> [u8; 3] {
    shade_at(c, shade_k(ch))
}

#[inline]
pub fn shade_at(c: [u8; 3], k: u32) -> [u8; 3] {
    let m = (c[0] as u32 + c[1] as u32 + c[2] as u32) / 3;
    let one = |v: u8| -> u8 {
        let v = (v as u32 * (100 - SHADE_DESAT_PCT) + m * SHADE_DESAT_PCT) / 100;
        ((((v * k) >> 10) as u8) & !3) as u8
    };
    [one(c[0]), one(c[1]), one(c[2])]
}
